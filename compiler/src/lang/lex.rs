//! The lexer (Language Ch. 0 §1).

use trit_core::{Bt, Literal, Trit, literal};

/// A source position, for diagnostics.
pub type Line = u32;

/// One token.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tok {
    /// An identifier.
    Ident(String),
    /// An integer literal, in any of the three radices.
    Int(Bt),
    /// A `trit` literal: `-1t`, `0t`, `1t`.
    TritLit(Trit),
    /// A character literal, as its Unicode scalar value (Ch. 5 §1.4).
    CharLit(i128),
    /// A string literal, as its characters (Ch. 5 §1.4). Held decoded, since
    /// text in this language is characters and the file's UTF-8 is a fact
    /// about the file.
    StrLit(Vec<i128>),
    /// A keyword (§1.3), interned.
    Kw(&'static str),
    /// A lifetime, `'a` (Ch. 3 §3.2), without its quote.
    Lifetime(String),
    /// An operator or punctuation mark (§1.5), interned.
    Op(&'static str),
    /// End of input.
    Eof,
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Ident(n) => write!(f, "`{n}`"),
            Tok::Int(v) => write!(f, "`{v}`"),
            Tok::TritLit(t) => write!(f, "`{}t`", t.to_i8()),
            Tok::StrLit(cs) => {
                let text: String = cs
                    .iter()
                    .filter_map(|c| char::from_u32(*c as u32))
                    .collect();
                write!(f, "`\"{text}\"`")
            }
            Tok::CharLit(v) => match char::from_u32(*v as u32) {
                Some(c) => write!(f, "`'{c}'`"),
                None => write!(f, "`'\\u{{{v:X}}}'`"),
            },
            Tok::Kw(k) => write!(f, "`{k}`"),
            Tok::Lifetime(l) => write!(f, "`'{l}`"),
            Tok::Op(o) => write!(f, "`{o}`"),
            Tok::Eof => f.write_str("end of input"),
        }
    }
}

/// A lexical or syntactic error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    /// 1-based line number.
    pub line: Line,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for SyntaxError {}

/// The keywords of §1.3.
pub const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "dyn", "else", "enum", "false", "fn", "for", "if", "impl",
    "in", "let", "loop", "match", "mut", "return", "self", "Self", "struct", "trait", "true",
    "type", "where", "while",
];

/// Reserved by §1.3 and claimed by Ch. 4, which is written but not
/// implemented (Ch. 4 Appendix C). The diagnostic says so, because "wait for
/// a chapter that does not exist" and "wait for that chapter to be built" are
/// different pieces of news.
pub const CHAPTER_4: &[&str] = &[];

/// Reserved for chapters that do not exist yet (§1.3). Using one is an error
/// that names the reason, rather than a mysterious parse failure.
pub const RESERVED: &[&str] = &[
    "crate", "mod", "move", "pub", "ref", "static", "union", "unsafe", "use",
];

/// Operators and punctuation, longest first so that maximal munch is simply
/// the order of this table.
const OPERATORS: &[&str] = &[
    "<<=", ">>=", "<=>", "..=", "==", "!=", "<=", ">=", "&&", "||", "->", "=>", "::", "+=", "-=",
    "*=", "/=", "%=", "<<", ">>", "(", ")", "[", "]", "{", "}", ",", ";", ":", ".", "#", "=", "<",
    ">", "+", "-", "*", "/", "%", "!", "&", "|", "@", "_", "?",
];

/// Tokenize a source file.
pub fn lex(src: &str) -> Result<Vec<(Tok, Line)>, SyntaxError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line: Line = 1;

    let err = |line: Line, m: String| SyntaxError { line, message: m };

    while i < chars.len() {
        let c = chars[i];

        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Comments are whitespace (§1.2). Block comments nest.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let start = line;
            let mut depth = 1;
            i += 2;
            while i < chars.len() && depth > 0 {
                if chars[i] == '\n' {
                    line += 1;
                }
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return Err(err(start, "unterminated block comment".into()));
            }
            continue;
        }

        // A lifetime: `'` followed by a name (Ch. 3 §3.2) — but `'a'` is a
        // character literal, and the two are told apart by what follows the
        // name. A lifetime is never closed by a quote.
        if c == '\''
            && chars
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
            && !closes_a_char(&chars, i)
        {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            out.push((Tok::Lifetime(chars[start..j].iter().collect()), line));
            i = j;
            continue;
        }

        // A character literal (Ch. 5 §1.4): one scalar value, in one word.
        if c == '\'' {
            let (v, next) = char_literal(&chars, i, line)?;
            out.push((Tok::CharLit(v), line));
            i = next;
            continue;
        }

        // A string literal (Ch. 5 §1.4): a sequence of scalar values, whose
        // storage is static and whose type is `&'static str`.
        if c == '"' {
            let (cs, next) = string_literal(&chars, i, line)?;
            out.push((Tok::StrLit(cs), line));
            i = next;
            continue;
        }

        // A numeric literal, or a trit literal — which shares the `0t`
        // spelling and is resolved by maximal munch in `trit-core`.
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            match literal::parse_literal(&text) {
                Ok(Literal::Int { value, .. }) => out.push((Tok::Int(value), line)),
                Ok(Literal::Trit(t)) => out.push((Tok::TritLit(t), line)),
                Err(e) => return Err(err(line, e.to_string())),
            }
            continue;
        }

        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if text == "_" {
                out.push((Tok::Op("_"), line));
            } else if let Some(k) = KEYWORDS.iter().find(|k| **k == text) {
                out.push((Tok::Kw(k), line));
            } else if CHAPTER_4.contains(&text.as_str()) {
                return Err(err(
                    line,
                    format!(
                        "`{text}` belongs to Ch. 4, which is specified but not implemented \
                         (Ch. 4 Appendix C)"
                    ),
                ));
            } else if RESERVED.contains(&text.as_str()) {
                return Err(err(
                    line,
                    format!(
                        "`{text}` is reserved for a chapter that is not written yet (Ch. 0 §1.3)"
                    ),
                ));
            } else {
                out.push((Tok::Ident(text), line));
            }
            continue;
        }

        let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
        match OPERATORS.iter().find(|op| rest.starts_with(**op)) {
            Some(op) => {
                // `^` and `~` are reserved, and the diagnostic names the
                // methods that replace them (§2.5).
                out.push((Tok::Op(op), line));
                i += op.chars().count();
            }
            None if c == '^' || c == '~' => {
                return Err(err(
                    line,
                    format!(
                        "`{c}` is reserved: the trit-wise operations are the methods \
                         `tmin`, `tmax`, `tmul` and `tneg` (Ch. 0 §2.5)"
                    ),
                ));
            }
            None => return Err(err(line, format!("unexpected character `{c}`"))),
        }
    }

    out.push((Tok::Eof, line));
    Ok(out)
}

/// One string literal, returning its characters and where it ends.
///
/// A newline inside one is an error rather than a continuation: an unclosed
/// quote would otherwise swallow the rest of the file and report its error
/// somewhere unrelated.
fn string_literal(
    chars: &[char],
    at: usize,
    line: Line,
) -> Result<(Vec<i128>, usize), SyntaxError> {
    let mut out = Vec::new();
    let mut i = at + 1;
    loop {
        match chars.get(i) {
            None | Some('\n') => {
                return Err(SyntaxError {
                    line,
                    message: "unterminated string literal".into(),
                });
            }
            Some('"') => return Ok((out, i + 1)),
            Some('\\') => {
                // The escapes are the character literal's, and are read by
                // the same code so that the two cannot drift apart.
                let (v, next) = escape(chars, i, line)?;
                out.push(v);
                i = next;
            }
            Some(&c) => {
                out.push(c as i128);
                i += 1;
            }
        }
    }
}

/// Whether the `'` at `i` opens a character literal rather than a lifetime.
///
/// A lifetime is `'` and a name; a character literal is `'` and one character
/// and `'`. They overlap on the first two characters and are told apart by the
/// third — `'a'` closes and `'a` does not — which is the same rule Rust uses
/// and for the same reason.
fn closes_a_char(chars: &[char], i: usize) -> bool {
    chars.get(i + 2) == Some(&'\'')
}

/// One character literal, returning its scalar value and where it ends.
fn char_literal(chars: &[char], at: usize, line: Line) -> Result<(i128, usize), SyntaxError> {
    let bad = |m: &str| SyntaxError {
        line,
        message: format!("in a character literal: {m}"),
    };
    let mut i = at + 1;
    let value = match chars.get(i) {
        None => return Err(bad("the file ends")),
        Some('\'') => return Err(bad("it is empty")),
        Some('\\') => {
            let (v, next) = escape(chars, i, line)?;
            i = next;
            v
        }
        Some(&c) => {
            i += 1;
            c as i128
        }
    };
    if chars.get(i) != Some(&'\'') {
        return Err(bad("it holds more than one character, or is not closed"));
    }
    Ok((scalar_value(value, line)?, i + 1))
}

/// One escape sequence, beginning at the backslash at `at`.
///
/// Shared by both literal forms so that the two cannot drift apart.
fn escape(chars: &[char], at: usize, line: Line) -> Result<(i128, usize), SyntaxError> {
    let bad = |m: &str| SyntaxError {
        line,
        message: format!("in an escape: {m}"),
    };
    let mut i = at + 1;
    let Some(&e) = chars.get(i) else {
        return Err(bad("the file ends after `\\`"));
    };
    i += 1;
    let value = match e {
        'n' => 10,
        'r' => 13,
        't' => 9,
        '\\' => 92,
        '\'' => 39,
        '"' => 34,
        '0' => 0,
        'u' => {
            // `\u{…}`, hexadecimal — it names a code point in an external
            // standard, which writes them one way (Ch. 5 §1.4).
            if chars.get(i) != Some(&'{') {
                return Err(bad("`\\u` is written `\\u{…}`"));
            }
            i += 1;
            let start = i;
            while chars.get(i).is_some_and(|c| c.is_ascii_hexdigit()) {
                i += 1;
            }
            if i == start || i - start > 6 {
                return Err(bad("`\\u{…}` takes one to six hexadecimal digits"));
            }
            if chars.get(i) != Some(&'}') {
                return Err(bad("`\\u{…}` is not closed"));
            }
            let text: String = chars[start..i].iter().collect();
            i += 1;
            i128::from_str_radix(&text, 16).map_err(|_| bad("that is not a number"))?
        }
        'x' => {
            return Err(bad(
                "there is no `\\x` escape: it names a byte, and text here is characters. \
                 Write `\\u{…}` for a scalar value, or a `[t9]` for bytes (Ch. 5 §1.4)",
            ));
        }
        other => return Err(bad(&format!("`\\{other}` is not an escape"))),
    };
    Ok((scalar_value(value, line)?, i))
}

/// Ch. 5 §1.2: a character is a scalar value, which excludes the surrogates.
fn scalar_value(v: i128, line: Line) -> Result<i128, SyntaxError> {
    if !(0..=0x10FFFF).contains(&v) || (0xD800..=0xDFFF).contains(&v) {
        return Err(SyntaxError {
            line,
            message: format!("U+{v:04X} is not a Unicode scalar value (Ch. 5 §1.2)"),
        });
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn comments_are_whitespace_and_block_comments_nest() {
        assert_eq!(
            kinds("1 // two\n /* three /* four */ five */ 6"),
            vec![
                Tok::Int(Bt::from_i128(1)),
                Tok::Int(Bt::from_i128(6)),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn the_three_radices_and_trit_literals() {
        assert_eq!(
            kinds("6 0t1T0 0hDDE 1t -1t 0t"),
            vec![
                Tok::Int(Bt::from_i128(6)),
                Tok::Int(Bt::from_i128(6)),
                Tok::Int(Bt::from_i128(1)),
                Tok::TritLit(Trit::Pos),
                Tok::Op("-"),
                Tok::TritLit(Trit::Pos),
                Tok::TritLit(Trit::Zero),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn maximal_munch_on_operators() {
        assert_eq!(
            kinds("<=> <= < <<= << ="),
            vec![
                Tok::Op("<=>"),
                Tok::Op("<="),
                Tok::Op("<"),
                Tok::Op("<<="),
                Tok::Op("<<"),
                Tok::Op("="),
                Tok::Eof,
            ]
        );
    }

    #[test]
    fn the_deferred_and_reserved_are_diagnosed_by_name() {
        // A string literal waits on the storage a `&str` needs (Ch. 5 §1.3);
        // a character literal does not, and lexes.
        // Both literal forms lex now (Ch. 5 §1.4).
        assert_eq!(lex("'a'").unwrap()[0].0, Tok::CharLit(97));
        assert_eq!(lex("\"hi\"").unwrap()[0].0, Tok::StrLit(vec![104, 105]));

        assert!(lex("mod").unwrap_err().message.contains("reserved"));
        assert!(lex("a ^ b").unwrap_err().message.contains("tmul"));
    }
}
