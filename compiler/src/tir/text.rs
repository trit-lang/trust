//! The TIR textual format (TIR §8): lexer, parser, and canonical printer.
//!
//! The textual form is the canonical serialization — there is no binary
//! encoding in draft 0.1 — so this module is also the medium the test suite
//! and differential testing work in. Printing then re-parsing any module must
//! reproduce it exactly; `tests/tir.rs` holds that to it.

use super::ir::*;
use std::fmt::Write as _;
use trit_core::{Bt, FaultCode, Flavor, Literal, Radix, Trit, literal};

// ---------------------------------------------------------------- diagnostics

/// A parse failure, with the line it was found on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseError {
    /// 1-based line number.
    pub line: u32,
    /// What went wrong.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

type PResult<T> = Result<T, ParseError>;

// ---------------------------------------------------------------------- lexer

#[derive(Clone, PartialEq, Eq, Debug)]
enum Tok {
    /// A bare word: keyword, mnemonic (`add.wrap`), type (`t27`), or fault code.
    Word(String),
    /// `@name`.
    Sym(String),
    /// `%name`.
    Val(String),
    /// `^name`.
    Label(String),
    /// A numeric literal in any of the three radices, sign included.
    Num(String),
    /// A quoted string.
    Str(String),
    /// One of `( ) [ ] { } , : =`.
    Punct(char),
    /// `->`.
    Arrow,
    Eof,
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Word(w) => write!(f, "`{w}`"),
            Tok::Sym(s) => write!(f, "`@{s}`"),
            Tok::Val(v) => write!(f, "`%{v}`"),
            Tok::Label(l) => write!(f, "`^{l}`"),
            Tok::Num(n) => write!(f, "`{n}`"),
            Tok::Str(s) => write!(f, "`\"{s}\"`"),
            Tok::Punct(c) => write!(f, "`{c}`"),
            Tok::Arrow => f.write_str("`->`"),
            Tok::Eof => f.write_str("end of input"),
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

fn lex(src: &str) -> PResult<Vec<(Tok, u32)>> {
    let mut out = Vec::new();
    let mut line = 1u32;
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            // `;` starts a comment, as in every listing in the spec.
            ';' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '@' | '%' | '^' => {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && is_word_char(chars[j]) {
                    j += 1;
                }
                if j == start {
                    return Err(ParseError {
                        line,
                        message: format!("`{c}` must be followed by a name"),
                    });
                }
                let name: String = chars[start..j].iter().collect();
                out.push((
                    match c {
                        '@' => Tok::Sym(name),
                        '%' => Tok::Val(name),
                        _ => Tok::Label(name),
                    },
                    line,
                ));
                i = j;
            }
            '"' => {
                let mut j = i + 1;
                let mut s = String::new();
                while j < chars.len() && chars[j] != '"' {
                    if chars[j] == '\n' {
                        return Err(ParseError {
                            line,
                            message: "unterminated string".into(),
                        });
                    }
                    s.push(chars[j]);
                    j += 1;
                }
                if j >= chars.len() {
                    return Err(ParseError {
                        line,
                        message: "unterminated string".into(),
                    });
                }
                out.push((Tok::Str(s), line));
                i = j + 1;
            }
            '-' if chars.get(i + 1) == Some(&'>') => {
                out.push((Tok::Arrow, line));
                i += 2;
            }
            '-' | '0'..='9' => {
                let mut j = i + 1;
                while j < chars.len() && is_word_char(chars[j]) {
                    j += 1;
                }
                let text: String = chars[i..j].iter().collect();
                if text == "-" {
                    return Err(ParseError {
                        line,
                        message: "stray `-`".into(),
                    });
                }
                out.push((Tok::Num(text), line));
                i = j;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut j = i;
                while j < chars.len() && is_word_char(chars[j]) {
                    j += 1;
                }
                out.push((Tok::Word(chars[i..j].iter().collect()), line));
                i = j;
            }
            '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':' | '=' => {
                out.push((Tok::Punct(c), line));
                i += 1;
            }
            other => {
                return Err(ParseError {
                    line,
                    message: format!("unexpected character `{other}`"),
                });
            }
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

// --------------------------------------------------------------------- parser

struct Parser {
    toks: Vec<(Tok, u32)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].1
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err(ParseError {
            line: self.line(),
            message: msg.into(),
        })
    }

    fn expect_punct(&mut self, c: char) -> PResult<()> {
        if *self.peek() == Tok::Punct(c) {
            self.bump();
            Ok(())
        } else {
            self.err(format!("expected `{c}`, found {}", self.peek()))
        }
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if *self.peek() == Tok::Punct(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, w: &str) -> PResult<()> {
        match self.peek() {
            Tok::Word(x) if x == w => {
                self.bump();
                Ok(())
            }
            other => self.err(format!("expected `{w}`, found {other}")),
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if matches!(self.peek(), Tok::Word(x) if x == w) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_value(&mut self) -> PResult<String> {
        match self.bump() {
            Tok::Val(v) => Ok(v),
            other => {
                self.pos -= 1;
                self.err(format!("expected an SSA value, found {other}"))
            }
        }
    }

    fn expect_uint(&mut self) -> PResult<u32> {
        let line = self.line();
        match self.bump() {
            Tok::Num(n) => n.parse::<u32>().map_err(|_| ParseError {
                line,
                message: format!("expected a non-negative integer, found `{n}`"),
            }),
            other => {
                self.pos -= 1;
                self.err(format!("expected a number, found {other}"))
            }
        }
    }

    /// `tN` or `ptr`.
    fn parse_type(&mut self) -> PResult<Type> {
        let line = self.line();
        match self.bump() {
            Tok::Word(w) => match parse_type_word(&w) {
                Some(t) => Ok(t),
                None => Err(ParseError {
                    line,
                    message: format!("`{w}` is not a TIR type"),
                }),
            },
            other => {
                self.pos -= 1;
                self.err(format!("expected a type, found {other}"))
            }
        }
    }

    fn peek_type(&self) -> Option<Type> {
        match self.peek() {
            Tok::Word(w) => parse_type_word(w),
            _ => None,
        }
    }

    /// `%name` or `const tN <literal>`.
    fn parse_operand(&mut self) -> PResult<Operand> {
        match self.peek().clone() {
            Tok::Val(v) => {
                self.bump();
                Ok(Operand::Value(v))
            }
            Tok::Sym(g) => {
                self.bump();
                Ok(Operand::Global(g))
            }
            Tok::Word(w) if w == "const" => {
                self.bump();
                let ty = self.parse_type()?;
                let line = self.line();
                let value = self.parse_literal_value()?;
                if let Type::Int(n) = ty
                    && !value.fits_width(n)
                {
                    return Err(ParseError {
                        line,
                        message: format!("constant {value} does not fit in {ty}"),
                    });
                }
                Ok(Operand::Const(ty, value))
            }
            other => self.err(format!("expected an operand, found {other}")),
        }
    }

    fn parse_literal_value(&mut self) -> PResult<Bt> {
        let line = self.line();
        match self.bump() {
            Tok::Num(n) => match literal::parse_literal(&n) {
                Ok(Literal::Int { value, .. }) => Ok(value),
                Ok(Literal::Trit(t)) => Ok(Bt::from(t)),
                Err(e) => Err(ParseError {
                    line,
                    message: e.to_string(),
                }),
            },
            other => {
                self.pos -= 1;
                self.err(format!("expected a literal, found {other}"))
            }
        }
    }

    /// A comma-separated operand list until `)`.
    fn parse_arg_list(&mut self) -> PResult<Vec<Operand>> {
        self.expect_punct('(')?;
        let mut args = Vec::new();
        if !self.eat_punct(')') {
            loop {
                args.push(self.parse_operand()?);
                if self.eat_punct(')') {
                    break;
                }
                self.expect_punct(',')?;
            }
        }
        Ok(args)
    }

    /// `^label` with an optional argument list.
    fn parse_target(&mut self) -> PResult<Target> {
        let label = match self.bump() {
            Tok::Label(l) => l,
            other => {
                self.pos -= 1;
                return self.err(format!("expected a block label, found {other}"));
            }
        };
        let args = if *self.peek() == Tok::Punct('(') {
            self.parse_arg_list()?
        } else {
            Vec::new()
        };
        Ok(Target { label, args })
    }

    /// `(%a: t27, %b: t9)` — a parameter list.
    fn parse_params(&mut self) -> PResult<Vec<(String, Type)>> {
        self.expect_punct('(')?;
        let mut params = Vec::new();
        if !self.eat_punct(')') {
            loop {
                let name = self.expect_value()?;
                self.expect_punct(':')?;
                let ty = self.parse_type()?;
                params.push((name, ty));
                if self.eat_punct(')') {
                    break;
                }
                self.expect_punct(',')?;
            }
        }
        Ok(params)
    }
}

fn parse_type_word(w: &str) -> Option<Type> {
    if w == "ptr" {
        return Some(Type::Ptr);
    }
    let n = w.strip_prefix('t')?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse::<u32>().ok().filter(|&n| n >= 1).map(Type::Int)
}

/// Parse a TIR module from its textual form.
pub fn parse_module(src: &str) -> PResult<Module> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
    };

    // Header: `tir 0.1 target "name"` (TIR §8).
    p.expect_word("tir")?;
    let line = p.line();
    let version = match p.bump() {
        Tok::Num(n) => n,
        other => {
            return Err(ParseError {
                line,
                message: format!("expected a version stamp, found {other}"),
            });
        }
    };
    if version != TIR_VERSION {
        return Err(ParseError {
            line,
            message: format!(
                "TIR version `{version}` is not `{TIR_VERSION}`; \
                 mismatched versions are rejected outright (TIR §8)"
            ),
        });
    }
    p.expect_word("target")?;
    let target = match p.bump() {
        Tok::Str(s) => s,
        other => {
            p.pos -= 1;
            return p.err(format!("expected a target name in quotes, found {other}"));
        }
    };

    let mut module = Module {
        version,
        target,
        ..Module::default()
    };

    loop {
        match p.peek().clone() {
            Tok::Eof => break,
            Tok::Word(w) if w == "global" => module.globals.push(parse_global(&mut p)?),
            Tok::Word(w) if w == "fn" => match parse_function(&mut p)? {
                Ok(f) => module.funcs.push(f),
                Err(sig) => module.decls.push(sig),
            },
            other => return p.err(format!("expected `fn` or `global`, found {other}")),
        }
    }
    Ok(module)
}

fn parse_global(p: &mut Parser) -> PResult<Global> {
    p.expect_word("global")?;
    let name = match p.bump() {
        Tok::Sym(s) => s,
        other => {
            p.pos -= 1;
            return p.err(format!("expected `@name`, found {other}"));
        }
    };
    p.expect_punct(':')?;
    p.expect_word("tryte")?;
    p.expect_punct('[')?;
    let trytes = p.expect_uint()?;
    p.expect_punct(']')?;

    let init = if p.eat_punct('=') {
        if p.eat_word("zeroinit") {
            None
        } else {
            p.expect_punct('[')?;
            let mut vals = Vec::new();
            if !p.eat_punct(']') {
                loop {
                    vals.push(p.parse_literal_value()?);
                    if p.eat_punct(']') {
                        break;
                    }
                    p.expect_punct(',')?;
                }
            }
            if vals.len() as u32 != trytes {
                return p.err(format!(
                    "initializer has {} trytes but `@{name}` is tryte[{trytes}]",
                    vals.len()
                ));
            }
            Some(vals)
        }
    } else {
        None
    };
    Ok(Global { name, trytes, init })
}

/// Returns `Ok(function)` for a definition, `Err(signature)` for a
/// declaration — a signature with no `{ … }` body.
fn parse_function(p: &mut Parser) -> PResult<Result<Function, Signature>> {
    p.expect_word("fn")?;
    let name = match p.bump() {
        Tok::Sym(s) => s,
        other => {
            p.pos -= 1;
            return p.err(format!("expected `@name`, found {other}"));
        }
    };
    let params = p.parse_params()?;
    let ret = if *p.peek() == Tok::Arrow {
        p.bump();
        Some(p.parse_type()?)
    } else {
        None
    };
    let sig = Signature { name, params, ret };

    if !p.eat_punct('{') {
        return Ok(Err(sig));
    }
    let mut blocks = Vec::new();
    while !p.eat_punct('}') {
        if *p.peek() == Tok::Eof {
            return p.err("unterminated function body");
        }
        blocks.push(parse_block(p)?);
    }
    if blocks.is_empty() {
        return p.err(format!("function `@{}` has no entry block", sig.name));
    }
    Ok(Ok(Function { sig, blocks }))
}

fn parse_block(p: &mut Parser) -> PResult<Block> {
    let label = match p.bump() {
        Tok::Label(l) => l,
        other => {
            p.pos -= 1;
            return p.err(format!("expected a block label, found {other}"));
        }
    };
    let params = if *p.peek() == Tok::Punct('(') {
        p.parse_params()?
    } else {
        Vec::new()
    };
    p.expect_punct(':')?;

    let mut insts = Vec::new();
    loop {
        if let Some(term) = parse_terminator(p)? {
            return Ok(Block {
                label,
                params,
                insts,
                term,
            });
        }
        match p.peek() {
            Tok::Punct('}') | Tok::Label(_) => {
                return p.err(format!("block `^{label}` has no terminator"));
            }
            Tok::Eof => return p.err(format!("block `^{label}` has no terminator")),
            _ => insts.push(parse_inst(p)?),
        }
    }
}

fn parse_terminator(p: &mut Parser) -> PResult<Option<Terminator>> {
    let Tok::Word(w) = p.peek().clone() else {
        return Ok(None);
    };
    match w.as_str() {
        "br3" => {
            p.bump();
            let t = p.parse_operand()?;
            p.expect_punct(',')?;
            let neg = p.parse_target()?;
            p.expect_punct(',')?;
            let zero = p.parse_target()?;
            p.expect_punct(',')?;
            let pos = p.parse_target()?;
            Ok(Some(Terminator::Br3 { t, neg, zero, pos }))
        }
        // Sugar for the two-way case: a `br3` whose −1 and 0 arms coincide
        // (TIR §3.6). `br2 %t, ^then, ^else` takes the `then` arm on +1.
        "br2" => {
            p.bump();
            let t = p.parse_operand()?;
            p.expect_punct(',')?;
            let then = p.parse_target()?;
            p.expect_punct(',')?;
            let els = p.parse_target()?;
            Ok(Some(Terminator::Br3 {
                t,
                neg: els.clone(),
                zero: els,
                pos: then,
            }))
        }
        "br" => {
            p.bump();
            Ok(Some(Terminator::Br(p.parse_target()?)))
        }
        "ret" => {
            p.bump();
            let v = match p.peek() {
                Tok::Val(_) | Tok::Word(_) if !matches!(p.peek(), Tok::Word(w) if is_block_end(w)) => {
                    Some(p.parse_operand()?)
                }
                _ => None,
            };
            Ok(Some(Terminator::Ret(v)))
        }
        "trap" => {
            p.bump();
            let line = p.line();
            match p.bump() {
                Tok::Word(code) => match FaultCode::from_name(&code) {
                    Some(c) => Ok(Some(Terminator::Trap(c))),
                    None => Err(ParseError {
                        line,
                        message: format!("`{code}` is not a fault code"),
                    }),
                },
                other => Err(ParseError {
                    line,
                    message: format!("expected a fault code, found {other}"),
                }),
            }
        }
        "unreachable" => {
            p.bump();
            Ok(Some(Terminator::Unreachable))
        }
        _ => Ok(None),
    }
}

fn is_block_end(w: &str) -> bool {
    matches!(w, "fn" | "global")
}

fn parse_inst(p: &mut Parser) -> PResult<Inst> {
    // Optional result list: `%a = ` or `%a, %b = ` (the `.flag` form).
    let mut results = Vec::new();
    if matches!(p.peek(), Tok::Val(_)) {
        let save = p.pos;
        let mut names = Vec::new();
        loop {
            match p.bump() {
                Tok::Val(v) => names.push(v),
                _ => {
                    p.pos = save;
                    names.clear();
                    break;
                }
            }
            if p.eat_punct(',') {
                continue;
            }
            if p.eat_punct('=') {
                results = names;
            } else {
                p.pos = save;
            }
            break;
        }
    }

    let line = p.line();
    let Tok::Word(mnemonic) = p.bump() else {
        p.pos -= 1;
        return p.err(format!("expected an instruction, found {}", p.peek()));
    };

    let (stem, flavor) = match mnemonic.split_once('.') {
        Some((stem, fl)) => match Flavor::from_name(fl) {
            Some(f) => (stem.to_string(), Some(f)),
            None => {
                return Err(ParseError {
                    line,
                    message: format!("`{fl}` is not an overflow flavor"),
                });
            }
        },
        None => (mnemonic.clone(), None),
    };

    let kind = match stem.as_str() {
        "add" | "sub" | "mul" | "shl" => {
            let op = match stem.as_str() {
                "add" => FlavoredOp::Add,
                "sub" => FlavoredOp::Sub,
                "mul" => FlavoredOp::Mul,
                _ => FlavoredOp::Shl,
            };
            let flavor = flavor.ok_or_else(|| ParseError {
                line,
                message: format!(
                    "`{stem}` needs an overflow flavor: `{stem}.wrap`, `{stem}.trap` or `{stem}.flag`"
                ),
            })?;
            let (ty, a, b) = parse_typed_binary(p)?;
            InstKind::Flavored {
                op,
                flavor,
                ty,
                a,
                b,
            }
        }
        "div" | "rem" | "shr" | "tmin" | "tmax" | "tmul" => {
            reject_flavor(&stem, flavor, line)?;
            let op = match stem.as_str() {
                "div" => PlainOp::Div,
                "rem" => PlainOp::Rem,
                "shr" => PlainOp::Shr,
                "tmin" => PlainOp::TMin,
                "tmax" => PlainOp::TMax,
                _ => PlainOp::TMul,
            };
            let (ty, a, b) = parse_typed_binary(p)?;
            InstKind::Plain { op, ty, a, b }
        }
        // `tneg` is an alias of `neg`; one canonical form survives parsing.
        "neg" | "tneg" => {
            reject_flavor(&stem, flavor, line)?;
            let explicit = p.peek_type();
            if explicit.is_some() {
                p.bump();
            }
            let a = p.parse_operand()?;
            let ty = infer_type(explicit, &[&a], p, "neg")?;
            InstKind::Neg { ty, a }
        }
        "cmp" => {
            reject_flavor(&stem, flavor, line)?;
            let (ty, a, b) = parse_typed_binary(p)?;
            InstKind::Cmp { ty, a, b }
        }
        "select3" => {
            reject_flavor(&stem, flavor, line)?;
            let t = p.parse_operand()?;
            p.expect_punct(',')?;
            let explicit = p.peek_type();
            if explicit.is_some() {
                p.bump();
            }
            let neg = p.parse_operand()?;
            p.expect_punct(',')?;
            let zero = p.parse_operand()?;
            p.expect_punct(',')?;
            let pos = p.parse_operand()?;
            let ty = infer_type(explicit, &[&neg, &zero, &pos], p, "select3")?;
            InstKind::Select3 {
                t,
                ty,
                neg,
                zero,
                pos,
            }
        }
        "slot" => {
            reject_flavor(&stem, flavor, line)?;
            p.expect_word("tryte")?;
            p.expect_punct('[')?;
            let trytes = p.expect_uint()?;
            p.expect_punct(']')?;
            InstKind::Slot { trytes }
        }
        "load" => {
            reject_flavor(&stem, flavor, line)?;
            let ty = p.parse_type()?;
            let p_op = p.parse_operand()?;
            InstKind::Load { ty, p: p_op }
        }
        "store" => {
            reject_flavor(&stem, flavor, line)?;
            let ty = p.parse_type()?;
            let v = p.parse_operand()?;
            p.expect_punct(',')?;
            let p_op = p.parse_operand()?;
            InstKind::Store { ty, v, p: p_op }
        }
        "offset" => {
            reject_flavor(&stem, flavor, line)?;
            let p_op = p.parse_operand()?;
            p.expect_punct(',')?;
            let d = p.parse_operand()?;
            InstKind::Offset { p: p_op, d }
        }
        "widen" | "trunc" => {
            reject_flavor(&stem, flavor, line)?;
            let from = p.parse_type()?;
            let a = p.parse_operand()?;
            if *p.peek() != Tok::Arrow {
                return p.err(format!("expected `->` after the `{stem}` operand"));
            }
            p.bump();
            let to = p.parse_type()?;
            if stem == "widen" {
                InstKind::Widen { from, a, to }
            } else {
                InstKind::Trunc { from, a, to }
            }
        }
        "call" => {
            reject_flavor(&stem, flavor, line)?;
            let callee = match p.bump() {
                Tok::Sym(s) => s,
                other => {
                    p.pos -= 1;
                    return p.err(format!(
                        "expected `@callee` (indirect calls are reserved), found {other}"
                    ));
                }
            };
            let args = p.parse_arg_list()?;
            let ret = if *p.peek() == Tok::Arrow {
                p.bump();
                Some(p.parse_type()?)
            } else {
                None
            };
            InstKind::Call { callee, args, ret }
        }
        other => {
            return Err(ParseError {
                line,
                message: format!("unknown instruction `{other}`"),
            });
        }
    };
    Ok(Inst { results, kind })
}

fn reject_flavor(stem: &str, flavor: Option<Flavor>, line: u32) -> PResult<()> {
    match flavor {
        None => Ok(()),
        Some(f) => Err(ParseError {
            line,
            message: format!(
                "`{stem}` takes no overflow flavor, but `{}` was given",
                f.suffix()
            ),
        }),
    }
}

/// `tN %a, %b`, with the type omissible when a constant operand carries it.
fn parse_typed_binary(p: &mut Parser) -> PResult<(Type, Operand, Operand)> {
    let explicit = p.peek_type();
    if explicit.is_some() {
        p.bump();
    }
    let a = p.parse_operand()?;
    p.expect_punct(',')?;
    let b = p.parse_operand()?;
    let ty = infer_type(explicit, &[&a, &b], p, "instruction")?;
    Ok((ty, a, b))
}

fn infer_type(
    explicit: Option<Type>,
    operands: &[&Operand],
    p: &Parser,
    what: &str,
) -> PResult<Type> {
    if let Some(t) = explicit {
        return Ok(t);
    }
    operands
        .iter()
        .find_map(|o| match o {
            Operand::Const(t, _) => Some(*t),
            Operand::Value(_) | Operand::Global(_) => None,
        })
        .ok_or(())
        .or_else(|()| {
            p.err(format!(
                "`{what}` needs an explicit operand type: no constant operand supplies one"
            ))
        })
}

// -------------------------------------------------------------------- printer

/// The canonical serialization of a module.
pub fn print_module(m: &Module) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "tir {} target \"{}\"", m.version, m.target);

    for g in &m.globals {
        s.push('\n');
        let _ = write!(s, "global @{} : tryte[{}] = ", g.name, g.trytes);
        match &g.init {
            None => s.push_str("zeroinit\n"),
            Some(vals) => {
                let items: Vec<String> = vals.iter().map(|v| v.to_decimal()).collect();
                let _ = writeln!(s, "[{}]", items.join(", "));
            }
        }
    }

    for d in &m.decls {
        s.push('\n');
        let _ = writeln!(s, "{}", print_signature(d));
    }

    for f in &m.funcs {
        s.push('\n');
        let _ = writeln!(s, "{} {{", print_signature(&f.sig));
        for b in &f.blocks {
            let _ = write!(s, "^{}", b.label);
            if !b.params.is_empty() {
                let ps: Vec<String> = b.params.iter().map(|(n, t)| format!("%{n}: {t}")).collect();
                let _ = write!(s, "({})", ps.join(", "));
            }
            s.push_str(":\n");
            for i in &b.insts {
                let _ = writeln!(s, "    {}", print_inst(i));
            }
            let _ = writeln!(s, "    {}", print_terminator(&b.term));
        }
        s.push_str("}\n");
    }
    s
}

fn print_signature(sig: &Signature) -> String {
    let ps: Vec<String> = sig
        .params
        .iter()
        .map(|(n, t)| format!("%{n}: {t}"))
        .collect();
    let mut s = format!("fn @{}({})", sig.name, ps.join(", "));
    if let Some(r) = sig.ret {
        let _ = write!(s, " -> {r}");
    }
    s
}

/// Print an operand. Constants keep their own `const tN` prefix, so an
/// operand is always self-describing.
pub fn print_operand(o: &Operand) -> String {
    match o {
        Operand::Value(v) => format!("%{v}"),
        Operand::Const(t, v) => format!("const {t} {}", literal::render(v, Radix::Dec)),
        Operand::Global(g) => format!("@{g}"),
    }
}

fn print_results(results: &[String]) -> String {
    if results.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = results.iter().map(|r| format!("%{r}")).collect();
        format!("{} = ", names.join(", "))
    }
}

fn print_inst(i: &Inst) -> String {
    let lhs = print_results(&i.results);
    let body = match &i.kind {
        InstKind::Flavored {
            op,
            flavor,
            ty,
            a,
            b,
        } => format!(
            "{}{} {ty} {}, {}",
            op.name(),
            flavor.suffix(),
            print_operand(a),
            print_operand(b)
        ),
        InstKind::Plain { op, ty, a, b } => format!(
            "{} {ty} {}, {}",
            op.name(),
            print_operand(a),
            print_operand(b)
        ),
        InstKind::Neg { ty, a } => format!("neg {ty} {}", print_operand(a)),
        InstKind::Cmp { ty, a, b } => {
            format!("cmp {ty} {}, {}", print_operand(a), print_operand(b))
        }
        InstKind::Select3 {
            t,
            ty,
            neg,
            zero,
            pos,
        } => format!(
            "select3 {}, {ty} {}, {}, {}",
            print_operand(t),
            print_operand(neg),
            print_operand(zero),
            print_operand(pos)
        ),
        InstKind::Slot { trytes } => format!("slot tryte[{trytes}]"),
        InstKind::Load { ty, p } => format!("load {ty} {}", print_operand(p)),
        InstKind::Store { ty, v, p } => {
            format!("store {ty} {}, {}", print_operand(v), print_operand(p))
        }
        InstKind::Offset { p, d } => {
            format!("offset {}, {}", print_operand(p), print_operand(d))
        }
        InstKind::Widen { from, a, to } => {
            format!("widen {from} {} -> {to}", print_operand(a))
        }
        InstKind::Trunc { from, a, to } => {
            format!("trunc {from} {} -> {to}", print_operand(a))
        }
        InstKind::Call { callee, args, ret } => {
            let args: Vec<String> = args.iter().map(print_operand).collect();
            let mut s = format!("call @{callee}({})", args.join(", "));
            if let Some(r) = ret {
                let _ = write!(s, " -> {r}");
            }
            s
        }
    };
    format!("{lhs}{body}")
}

fn print_target(t: &Target) -> String {
    if t.args.is_empty() {
        format!("^{}", t.label)
    } else {
        let args: Vec<String> = t.args.iter().map(print_operand).collect();
        format!("^{}({})", t.label, args.join(", "))
    }
}

fn print_terminator(t: &Terminator) -> String {
    match t {
        // The two-way case prints as `br2` sugar (TIR §3.6).
        Terminator::Br3 { t, neg, zero, pos } if neg == zero => format!(
            "br2 {}, {}, {}",
            print_operand(t),
            print_target(pos),
            print_target(neg)
        ),
        Terminator::Br3 { t, neg, zero, pos } => format!(
            "br3 {}, {}, {}, {}",
            print_operand(t),
            print_target(neg),
            print_target(zero),
            print_target(pos)
        ),
        Terminator::Br(d) => format!("br {}", print_target(d)),
        Terminator::Ret(None) => "ret".to_string(),
        Terminator::Ret(Some(v)) => format!("ret {}", print_operand(v)),
        Terminator::Trap(c) => format!("trap {c}"),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

/// The trit a `t1` operand denotes, for printing diagnostics.
pub fn trit_of(v: &Bt) -> Option<Trit> {
    v.to_i128()
        .and_then(|n| i8::try_from(n).ok())
        .and_then(Trit::from_i8)
}
