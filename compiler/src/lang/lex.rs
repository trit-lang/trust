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
    "as", "break", "const", "continue", "else", "enum", "false", "fn", "if", "let", "loop",
    "match", "mut", "return", "self", "struct", "true", "while",
];

/// Reserved for chapters that do not exist yet (§1.3). Using one is an error
/// that names the reason, rather than a mysterious parse failure.
pub const RESERVED: &[&str] = &[
    "crate", "dyn", "for", "impl", "in", "mod", "move", "pub", "ref", "Self", "static", "trait",
    "type", "union", "unsafe", "use", "where",
];

/// Operators and punctuation, longest first so that maximal munch is simply
/// the order of this table.
const OPERATORS: &[&str] = &[
    "<<=", ">>=", "<=>", "..=", "==", "!=", "<=", ">=", "&&", "||", "->", "=>", "::", "+=", "-=",
    "*=", "/=", "%=", "<<", ">>", "(", ")", "[", "]", "{", "}", ",", ";", ":", ".", "#", "=", "<",
    ">", "+", "-", "*", "/", "%", "!", "&", "|", "@", "_",
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

        // A lifetime: `'` followed by a name (Ch. 3 §3.2). A `'` followed by
        // anything else would be a character literal, which §1.4 does not
        // have.
        if c == '\''
            && chars
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
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

        // §1.4 has no string or character literals, and says why.
        if c == '"' || c == '\'' {
            return Err(err(
                line,
                "there are no string or character literals in draft 0.1: text encoding is \
                 deferred to the library chapter (Ch. 0 §1.4). Write the code units as an \
                 array of t9"
                    .into(),
            ));
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
        assert!(
            lex("\"hi\"")
                .unwrap_err()
                .message
                .contains("library chapter")
        );
        assert!(lex("for").unwrap_err().message.contains("reserved"));
        assert!(lex("a ^ b").unwrap_err().message.contains("tmul"));
    }
}
