//! The TRISC-27 assembler (`spec/isa/assembly-0.1.md`).
//!
//! Two passes, as §3.3 requires: the first assigns an address to every
//! statement and defines every label, the second evaluates operand
//! expressions — which may refer forward — and emits trytes.
//!
//! It lives in the `tritium` crate rather than in one of its own for two
//! reasons. It shares [`crate::inst`] with the machine, so the encoder an
//! assembler needs and the decoder a disassembler needs cannot drift apart;
//! and Naming §2 deliberately leaves the assembler unnamed, a rule worth
//! keeping until the naming document assigns one.
//!
//! # Reading of one thing the specifications leave open
//!
//! A branch or jump operand is written as a **target address**, normally a
//! label, and the assembler computes the displacement in words and checks its
//! range (TRISC-27 §4.5). The alternative — writing the displacement itself —
//! would make every branch depend on its own position, which is what labels
//! exist to avoid.

use crate::inst::{AluOp, Inst, Reg, Width};
use crate::word;
use std::collections::BTreeMap;
use trit_core::{Bt, FaultCode, Flavor, Literal, literal};

/// An assembly-time error. These are not faults (AM §4): a fault is a runtime
/// halt of a machine.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AsmError {
    /// 1-based line number.
    pub line: u32,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for AsmError {}

type R<T> = Result<T, AsmError>;

fn err<T>(line: u32, message: impl Into<String>) -> R<T> {
    Err(AsmError {
        line,
        message: message.into(),
    })
}

// --------------------------------------------------------------------- lexer

#[derive(Clone, PartialEq, Eq, Debug)]
enum Tok {
    /// A bare word: mnemonic, directive, register or symbol.
    Ident(String),
    /// A numeric literal in any of the three radices.
    Num(Bt),
    /// A quoted string. Only `.trits` gives one a meaning, and it is
    /// interpreted there rather than here — so a reserved directive that
    /// happens to take a string is diagnosed as reserved, not as a bad trit.
    Str(String),
    /// One of `: , ( ) + - * / % $`.
    Punct(char),
    /// `<<`.
    Shl,
    /// `>>`.
    Shr,
}

fn tokenize(text: &str, line: u32) -> R<Vec<Tok>> {
    // `;` starts a comment, as in every listing in the specification (§1.2).
    let text = text.split(';').next().unwrap_or("");
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,

            '<' | '>' => {
                if chars.get(i + 1) == Some(&c) {
                    out.push(if c == '<' { Tok::Shl } else { Tok::Shr });
                    i += 2;
                } else {
                    return err(
                        line,
                        format!("`{c}` alone is not an operator; did you mean `{c}{c}`?"),
                    );
                }
            }

            '&' | '|' | '^' | '~' | '!' => {
                // Reserved so that Language Ch. 1 §4's open question about
                // repurposing `& | ^` stays open (§2.4).
                return err(
                    line,
                    format!("`{c}` is reserved; use the named forms tmin, tmax, tmul, tneg"),
                );
            }

            '"' => {
                let mut j = i + 1;
                let mut text = String::new();
                while j < chars.len() && chars[j] != '"' {
                    text.push(chars[j]);
                    j += 1;
                }
                if j >= chars.len() {
                    return err(line, "unterminated string");
                }
                out.push(Tok::Str(text));
                i = j + 1;
            }

            // A number, or a `-` used as an operator. `-` followed by a digit
            // is handled by the expression parser as unary minus, so the
            // lexer only needs to recognize digits here.
            '0'..='9' => {
                let mut j = i;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                let text: String = chars[i..j].iter().collect();
                match literal::parse_literal(&text) {
                    Ok(Literal::Int { value, .. }) => out.push(Tok::Num(value)),
                    Ok(Literal::Trit(t)) => out.push(Tok::Num(Bt::from(t))),
                    Err(e) => return err(line, e.to_string()),
                }
                i = j;
            }

            c if c.is_ascii_alphabetic() || c == '_' || c == '.' => {
                let mut j = i;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '.')
                {
                    j += 1;
                }
                out.push(Tok::Ident(chars[i..j].iter().collect()));
                i = j;
            }

            ':' | ',' | '(' | ')' | '+' | '-' | '*' | '/' | '%' | '$' => {
                out.push(Tok::Punct(c));
                i += 1;
            }

            other => return err(line, format!("unexpected character `{other}`")),
        }
    }
    Ok(out)
}

// -------------------------------------------------------------- expressions

/// Everything an expression may refer to.
struct Symbols {
    values: BTreeMap<String, Bt>,
}

struct Expr<'a> {
    toks: &'a [Tok],
    pos: usize,
    line: u32,
    /// The address of the statement being assembled — the `$` of §4.5.
    here: i128,
    syms: &'a Symbols,
}

impl Expr<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `a << b`, `a >> b` — the loosest level.
    fn parse(&mut self) -> R<Bt> {
        let mut lhs = self.additive()?;
        loop {
            let shl = self.eat(&Tok::Shl);
            if !shl && !self.eat(&Tok::Shr) {
                return Ok(lhs);
            }
            let rhs = self.additive()?;
            let k = self.shift_amount(&rhs)?;
            lhs = if shl { lhs.shl(k) } else { lhs.shr(k) };
        }
    }

    fn shift_amount(&self, v: &Bt) -> R<u32> {
        match v.to_i128().and_then(|n| u32::try_from(n).ok()) {
            Some(k) => Ok(k),
            None => err(self.line, format!("shift amount {v} is negative")),
        }
    }

    fn additive(&mut self) -> R<Bt> {
        let mut lhs = self.multiplicative()?;
        loop {
            if self.eat(&Tok::Punct('+')) {
                lhs = lhs.add(&self.multiplicative()?);
            } else if self.eat(&Tok::Punct('-')) {
                lhs = lhs.sub(&self.multiplicative()?);
            } else {
                return Ok(lhs);
            }
        }
    }

    fn multiplicative(&mut self) -> R<Bt> {
        let mut lhs = self.unary()?;
        loop {
            let (mul, div, rem) = (
                self.eat(&Tok::Punct('*')),
                self.peek() == Some(&Tok::Punct('/')),
                self.peek() == Some(&Tok::Punct('%')),
            );
            if mul {
                lhs = lhs.mul(&self.unary()?);
                continue;
            }
            if !div && !rem {
                return Ok(lhs);
            }
            self.pos += 1;
            let rhs = self.unary()?;
            // One division, and it is the AM's (§4.3).
            let Some((q, r)) = lhs.divrem(&rhs) else {
                return err(self.line, "division by zero");
            };
            lhs = if div { q } else { r };
        }
    }

    fn unary(&mut self) -> R<Bt> {
        if self.eat(&Tok::Punct('-')) {
            return Ok(self.unary()?.neg());
        }
        if self.eat(&Tok::Punct('+')) {
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> R<Bt> {
        match self.peek().cloned() {
            Some(Tok::Num(v)) => {
                self.pos += 1;
                Ok(v)
            }
            Some(Tok::Punct('$')) => {
                self.pos += 1;
                Ok(Bt::from_i128(self.here))
            }
            Some(Tok::Punct('(')) => {
                self.pos += 1;
                let v = self.parse()?;
                if !self.eat(&Tok::Punct(')')) {
                    return err(self.line, "expected `)`");
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                if self.peek() == Some(&Tok::Punct('(')) {
                    return self.call(&name);
                }
                match self.syms.values.get(&name) {
                    Some(v) => Ok(v.clone()),
                    None => err(self.line, format!("`{name}` is not defined")),
                }
            }
            other => err(
                self.line,
                match other {
                    None => "expected an expression".to_string(),
                    Some(t) => format!("expected an expression, found {t:?}"),
                },
            ),
        }
    }

    /// The named operations of §4.3 — named rather than spelled with symbols
    /// so that the language's `& | ^` question stays open.
    fn call(&mut self, name: &str) -> R<Bt> {
        self.pos += 1; // '('
        let mut args = Vec::new();
        if !self.eat(&Tok::Punct(')')) {
            loop {
                args.push(self.parse()?);
                if self.eat(&Tok::Punct(')')) {
                    break;
                }
                if !self.eat(&Tok::Punct(',')) {
                    return err(self.line, "expected `,` or `)`");
                }
            }
        }
        let want = |n: usize| -> R<()> {
            if args.len() == n {
                Ok(())
            } else {
                err(
                    self.line,
                    format!("`{name}` takes {n} argument(s), {} given", args.len()),
                )
            }
        };
        match name {
            "tneg" => {
                want(1)?;
                Ok(args[0].neg())
            }
            "sign" => {
                want(1)?;
                Ok(Bt::from(args[0].sign()))
            }
            "tmin" | "tmax" | "tmul" => {
                want(2)?;
                let n = args[0].trit_len().max(args[1].trit_len());
                let f = |a: i8, b: i8| match name {
                    "tmin" => a.min(b),
                    "tmax" => a.max(b),
                    _ => a * b,
                };
                Ok(Bt::from_trits_lsb((0..n).map(|i| {
                    f(args[0].trit(i).to_i8(), args[1].trit(i).to_i8())
                })))
            }
            "cmp" => {
                want(2)?;
                Ok(Bt::from(args[0].cmp3(&args[1])))
            }
            "wrap" => {
                want(2)?;
                match args[1].to_i128().and_then(|n| u32::try_from(n).ok()) {
                    Some(n) if n >= 1 => Ok(args[0].wrap_to(n)),
                    _ => err(self.line, "`wrap` needs a positive width"),
                }
            }
            other => err(self.line, format!("`{other}` is not a function")),
        }
    }
}

// -------------------------------------------------------------- statements

/// One assembled statement, before its operands are resolved.
struct Stmt {
    line: u32,
    addr: i128,
    kind: Kind,
}

enum Kind {
    /// Emit trytes from expressions, one tryte each.
    Trytes(Vec<Vec<Tok>>),
    /// Emit words from expressions, three trytes each.
    Words(Vec<Vec<Tok>>),
    /// Emit a packed trit string.
    Trits(Vec<i8>),
    /// Emit `n` zero trytes (padding, `.zero`, `.align`, `.org`).
    Pad(i128),
    /// Emit `n` copies of a tryte.
    Fill(i128, Vec<Tok>),
    /// One or two instruction words.
    Inst(InstForm, u32),
    /// Nothing — a label-only line, `.equ`, `.global`.
    Nothing,
}

/// An instruction with unresolved operand expressions.
enum InstForm {
    /// Fully determined already.
    Fixed(Inst),
    /// An operation whose immediate is an expression.
    Imm {
        build: fn(Reg, Reg, i128) -> Inst,
        rd: Reg,
        rs1: Reg,
        expr: Vec<Tok>,
        /// Trits available to the immediate.
        width: u32,
    },
    /// A branch: three target addresses.
    Br3 { rs1: Reg, targets: [Vec<Tok>; 3] },
    /// A jump: one target address.
    Jal { rd: Reg, target: Vec<Tok> },
    /// `li`/`la`: one or two words, materializing a value.
    Load { rd: Reg, expr: Vec<Tok>, words: u32 },
}

/// Assemble a source file into an image.
pub fn assemble(src: &str) -> Result<Vec<i16>, Vec<AsmError>> {
    match assemble_inner(src) {
        Ok(image) => Ok(image),
        Err(e) => Err(vec![e]),
    }
}

fn assemble_inner(src: &str) -> R<Vec<i16>> {
    let mut syms = Symbols {
        values: BTreeMap::new(),
    };
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut loc: i128 = 0;

    // ---- pass one: addresses, labels and every size-determining value.
    for (n, raw) in src.lines().enumerate() {
        let line = n as u32 + 1;
        let toks = tokenize(raw, line)?;
        let mut i = 0;

        // Any number of labels may precede one statement (§1.3).
        while let (Some(Tok::Ident(name)), Some(Tok::Punct(':'))) = (toks.get(i), toks.get(i + 1)) {
            if Reg::from_name(name).is_some() {
                return err(line, format!("`{name}` is a register name (§7)"));
            }
            if syms
                .values
                .insert(name.clone(), Bt::from_i128(loc))
                .is_some()
            {
                return err(line, format!("`{name}` is defined more than once"));
            }
            i += 2;
        }
        if i >= toks.len() {
            continue;
        }

        let rest = &toks[i..];
        let stmt = parse_statement(rest, line, loc, &mut syms)?;
        loc += size_of(&stmt.kind);
        stmts.push(stmt);
    }

    // ---- pass two: evaluate operands and emit.
    let mut image: Vec<i16> = Vec::new();
    for stmt in &stmts {
        let at = image.len() as i128;
        if at < stmt.addr {
            image.resize(stmt.addr as usize, 0);
        }
        emit(stmt, &syms, &mut image)?;
    }
    Ok(image)
}

fn size_of(kind: &Kind) -> i128 {
    match kind {
        Kind::Trytes(v) => v.len() as i128,
        Kind::Words(v) => v.len() as i128 * 3,
        Kind::Trits(t) => (t.len() as i128).div_euclid(9) + i128::from(t.len() % 9 != 0),
        Kind::Pad(n) => *n,
        Kind::Fill(n, _) => *n,
        Kind::Inst(_, words) => *words as i128 * 3,
        Kind::Nothing => 0,
    }
}

/// Evaluate an expression that must be resolvable now — sizes depend on it.
fn eval(toks: &[Tok], line: u32, here: i128, syms: &Symbols) -> R<Bt> {
    let mut e = Expr {
        toks,
        pos: 0,
        line,
        here,
        syms,
    };
    let v = e.parse()?;
    if e.pos != toks.len() {
        return err(line, "trailing tokens after expression");
    }
    Ok(v)
}

fn eval_i128(toks: &[Tok], line: u32, here: i128, syms: &Symbols) -> R<i128> {
    let v = eval(toks, line, here, syms)?;
    v.to_i128()
        .ok_or(())
        .or_else(|()| err(line, format!("{v} is too large")))
}

/// Split a comma-separated operand list into expression token runs.
fn split_commas(toks: &[Tok]) -> Vec<Vec<Tok>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0;
    for t in toks {
        match t {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth -= 1,
            Tok::Punct(',') if depth == 0 => {
                out.push(Vec::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("non-empty").push(t.clone());
    }
    if out.len() == 1 && out[0].is_empty() {
        out.clear();
    }
    out
}

fn parse_statement(toks: &[Tok], line: u32, loc: i128, syms: &mut Symbols) -> R<Stmt> {
    let Tok::Ident(head) = &toks[0] else {
        return err(line, "expected a directive or an instruction");
    };
    let args = &toks[1..];
    let kind = if let Some(name) = head.strip_prefix('.') {
        directive(name, args, line, loc, syms)?
    } else {
        instruction(head, args, line, syms)?
    };
    Ok(Stmt {
        line,
        addr: loc,
        kind,
    })
}

// -------------------------------------------------------------- directives

fn directive(name: &str, args: &[Tok], line: u32, loc: i128, syms: &mut Symbols) -> R<Kind> {
    let parts = split_commas(args);
    match name {
        "tryte" => Ok(Kind::Trytes(parts)),
        "word" => Ok(Kind::Words(parts)),

        "trits" => match args {
            [Tok::Str(text)] => Ok(Kind::Trits(parse_trit_string(text, line)?)),
            _ => err(line, "`.trits` takes one trit string"),
        },

        "zero" => {
            let n = eval_i128(&parts[0], line, loc, syms)?;
            if n < 0 {
                return err(line, "`.zero` needs a non-negative count");
            }
            Ok(Kind::Pad(n))
        }

        "fill" => {
            if parts.len() != 2 {
                return err(line, "`.fill` takes a count and a value");
            }
            let n = eval_i128(&parts[0], line, loc, syms)?;
            if n < 0 {
                return err(line, "`.fill` needs a non-negative count");
            }
            Ok(Kind::Fill(n, parts[1].clone()))
        }

        // Alignment is a power of three (Composites Ch. 2 §1).
        "align" => {
            let a = eval_i128(&parts[0], line, loc, syms)?;
            if a < 1 || !is_power_of_three(a) {
                return err(line, format!("alignment {a} is not a power of three"));
            }
            let pad = (a - loc.rem_euclid(a)).rem_euclid(a);
            Ok(Kind::Pad(pad))
        }

        "org" => {
            let a = eval_i128(&parts[0], line, loc, syms)?;
            if a < loc {
                return err(
                    line,
                    format!(
                        "`.org {a}` moves backwards from {loc}; an assembler does not overwrite"
                    ),
                );
            }
            Ok(Kind::Pad(a - loc))
        }

        "equ" => {
            if parts.len() != 2 {
                return err(line, "`.equ` takes a name and an expression");
            }
            let [Tok::Ident(name)] = &parts[0][..] else {
                return err(line, "`.equ` needs a name");
            };
            if Reg::from_name(name).is_some() {
                return err(line, format!("`{name}` is a register name (§7)"));
            }
            // Not forward-resolvable, by §3.2 — which keeps constant
            // definitions free of ordering surprises.
            let v = eval(&parts[1], line, loc, syms)?;
            if syms.values.insert(name.clone(), v).is_some() {
                return err(line, format!("`{name}` is defined more than once"));
            }
            Ok(Kind::Nothing)
        }

        // Exported symbols are meaningful only once there is an object
        // format; accepted and recorded as a no-op (§3.4, §8).
        "global" => Ok(Kind::Nothing),

        "section" | "extern" | "string" | "ascii" | "macro" | "endm" | "if" | "else" | "endif"
        | "include" => err(line, format!("`.{name}` is reserved (§5.5)")),

        "byte" | "half" | "quad" => err(
            line,
            format!("`.{name}` is a binary-world unit; use `.tryte` or `.word` (§5.5)"),
        ),

        other => err(line, format!("`.{other}` is not a directive")),
    }
}

/// A trit string, written most significant trit first (§2.3), returned least
/// significant first.
fn parse_trit_string(text: &str, line: u32) -> R<Vec<i8>> {
    let mut trits = Vec::new();
    for c in text.chars() {
        match c {
            '_' => {}
            '1' => trits.push(1),
            '0' => trits.push(0),
            'T' | 't' => trits.push(-1),
            other => return err(line, format!("`{other}` is not a trit; use T, 0 or 1")),
        }
    }
    trits.reverse();
    Ok(trits)
}

fn is_power_of_three(mut a: i128) -> bool {
    if a < 1 {
        return false;
    }
    while a % 3 == 0 {
        a /= 3;
    }
    a == 1
}

// ------------------------------------------------------------ instructions

fn reg(tok: Option<&Tok>, line: u32) -> R<Reg> {
    match tok {
        Some(Tok::Ident(name)) => Reg::from_name(name)
            .ok_or(())
            .or_else(|()| err(line, format!("`{name}` is not a register"))),
        _ => err(line, "expected a register"),
    }
}

/// `imm(rs1)` — the one addressing mode (§7, item 4).
fn split_offset(toks: &[Tok], line: u32) -> R<(Vec<Tok>, Reg)> {
    let open = toks
        .iter()
        .rposition(|t| *t == Tok::Punct('('))
        .ok_or(())
        .or_else(|()| err::<usize>(line, "expected `imm(reg)`"))?;
    if toks.last() != Some(&Tok::Punct(')')) {
        return err(line, "expected `)`");
    }
    let base = reg(toks.get(open + 1), line)?;
    if toks.len() != open + 3 {
        return err(line, "expected one register inside `( )`");
    }
    let expr = toks[..open].to_vec();
    if expr.is_empty() {
        return err(line, "expected a displacement before `(`");
    }
    Ok((expr, base))
}

fn alu_op(stem: &str) -> Option<(AluOp, bool)> {
    let table = [
        ("add", AluOp::Add),
        ("sub", AluOp::Sub),
        ("mul", AluOp::Mul),
        ("mulh", AluOp::MulH),
        ("div", AluOp::Div),
        ("rem", AluOp::Rem),
        ("shl", AluOp::Shl),
        ("shr", AluOp::Shr),
        ("tmin", AluOp::TMin),
        ("tmax", AluOp::TMax),
        ("tmul", AluOp::TMul),
        ("cmp", AluOp::Cmp),
    ];
    for (name, op) in table {
        if stem == name {
            return Some((op, false));
        }
        if stem == format!("{name}i") {
            return Some((op, true));
        }
    }
    (stem == "wrap").then_some((AluOp::Wrap, true))
}

fn instruction(head: &str, args: &[Tok], line: u32, syms: &Symbols) -> R<Kind> {
    let (stem, suffix) = match head.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (head, None),
    };
    let parts = split_commas(args);
    let nargs = parts.len();

    let flavor = match suffix {
        None => Flavor::Wrap,
        Some(f) => match Flavor::from_name(f) {
            Some(fl) => fl,
            None if stem == "ld" || stem == "st" => Flavor::Wrap,
            None => return err(line, format!("`{f}` is not an overflow flavor")),
        },
    };

    let want = |n: usize| -> R<()> {
        if nargs == n {
            Ok(())
        } else {
            err(
                line,
                format!("`{head}` takes {n} operand(s), {nargs} given"),
            )
        }
    };
    let one = |p: &Vec<Tok>| -> R<Reg> {
        reg(p.first(), line).and_then(|r| {
            if p.len() == 1 {
                Ok(r)
            } else {
                err(line, "expected a single register")
            }
        })
    };

    // --- arithmetic
    if let Some((op, immediate)) = alu_op(stem) {
        if suffix.is_some() && !op.is_flavored() {
            return err(line, format!("`{}` takes no flavor", op.name()));
        }
        if immediate {
            want(3)?;
            if flavor == Flavor::Flag {
                return err(line, "the flag flavor is reserved in the immediate form");
            }
            return Ok(Kind::Inst(
                InstForm::Imm {
                    build: build_alui(op, flavor),
                    rd: one(&parts[0])?,
                    rs1: one(&parts[1])?,
                    expr: parts[2].clone(),
                    width: 14,
                },
                1,
            ));
        }
        // The flag form names a second destination.
        let expected = if flavor == Flavor::Flag { 4 } else { 3 };
        want(expected)?;
        return Ok(Kind::Inst(
            InstForm::Fixed(Inst::Alu {
                op,
                flavor,
                rd: one(&parts[0])?,
                rs1: one(&parts[1])?,
                rs2: one(&parts[2])?,
                rc: if flavor == Flavor::Flag {
                    one(&parts[3])?
                } else {
                    Reg::ZERO
                },
            }),
            1,
        ));
    }

    // --- memory
    if stem == "ld" || stem == "st" {
        let width = match suffix {
            Some("word") => Width::Word,
            Some("tryte") => Width::Tryte,
            _ => return err(line, format!("`{stem}` needs `.word` or `.tryte`")),
        };
        want(2)?;
        let r0 = one(&parts[0])?;
        let (expr, base) = split_offset(&parts[1], line)?;
        let build: fn(Reg, Reg, i128) -> Inst = if stem == "ld" {
            match width {
                Width::Word => |rd, rs1, imm| Inst::Load {
                    width: Width::Word,
                    rd,
                    rs1,
                    imm,
                },
                Width::Tryte => |rd, rs1, imm| Inst::Load {
                    width: Width::Tryte,
                    rd,
                    rs1,
                    imm,
                },
            }
        } else {
            match width {
                Width::Word => |rs2, rs1, imm| Inst::Store {
                    width: Width::Word,
                    rs2,
                    rs1,
                    imm,
                },
                Width::Tryte => |rs2, rs1, imm| Inst::Store {
                    width: Width::Tryte,
                    rs2,
                    rs1,
                    imm,
                },
            }
        };
        return Ok(Kind::Inst(
            InstForm::Imm {
                build,
                rd: r0,
                rs1: base,
                expr,
                width: 14,
            },
            1,
        ));
    }

    match stem {
        "br3" => {
            want(4)?;
            Ok(Kind::Inst(
                InstForm::Br3 {
                    rs1: one(&parts[0])?,
                    targets: [parts[1].clone(), parts[2].clone(), parts[3].clone()],
                },
                1,
            ))
        }

        // `br2 rs, then, else` — the two-way case, mirroring TIR's printer.
        "br2" => {
            want(3)?;
            Ok(Kind::Inst(
                InstForm::Br3 {
                    rs1: one(&parts[0])?,
                    targets: [parts[2].clone(), parts[2].clone(), parts[1].clone()],
                },
                1,
            ))
        }

        "jal" => {
            want(2)?;
            Ok(Kind::Inst(
                InstForm::Jal {
                    rd: one(&parts[0])?,
                    target: parts[1].clone(),
                },
                1,
            ))
        }

        "j" => {
            want(1)?;
            Ok(Kind::Inst(
                InstForm::Jal {
                    rd: Reg::ZERO,
                    target: parts[0].clone(),
                },
                1,
            ))
        }

        "call" => {
            want(1)?;
            Ok(Kind::Inst(
                InstForm::Jal {
                    rd: Reg::from_name("ra").expect("ra"),
                    target: parts[0].clone(),
                },
                1,
            ))
        }

        "ret" => {
            want(0)?;
            Ok(Kind::Inst(
                InstForm::Fixed(Inst::Jalr {
                    rd: Reg::ZERO,
                    rs1: Reg::from_name("ra").expect("ra"),
                    imm: 0,
                }),
                1,
            ))
        }

        "jalr" => {
            want(2)?;
            let rd = one(&parts[0])?;
            let (expr, base) = split_offset(&parts[1], line)?;
            Ok(Kind::Inst(
                InstForm::Imm {
                    build: |rd, rs1, imm| Inst::Jalr { rd, rs1, imm },
                    rd,
                    rs1: base,
                    expr,
                    width: 14,
                },
                1,
            ))
        }

        "lui" => {
            want(2)?;
            Ok(Kind::Inst(
                InstForm::Imm {
                    build: |rd, _, imm| Inst::Lui { rd, imm },
                    rd: one(&parts[0])?,
                    rs1: Reg::ZERO,
                    expr: parts[1].clone(),
                    width: 13,
                },
                1,
            ))
        }

        "sel3" => {
            want(5)?;
            Ok(Kind::Inst(
                InstForm::Fixed(Inst::Sel3 {
                    rd: one(&parts[0])?,
                    rt: one(&parts[1])?,
                    rn: one(&parts[2])?,
                    rz: one(&parts[3])?,
                    rp: one(&parts[4])?,
                }),
                1,
            ))
        }

        "halt" => {
            want(1)?;
            Ok(Kind::Inst(
                InstForm::Fixed(Inst::Halt {
                    rs1: one(&parts[0])?,
                }),
                1,
            ))
        }

        "trap" => {
            want(1)?;
            let [Tok::Ident(name)] = &parts[0][..] else {
                return err(line, "`trap` takes a fault code");
            };
            let code = FaultCode::from_name(name)
                .ok_or(())
                .or_else(|()| err(line, format!("`{name}` is not a fault code")))?;
            Ok(Kind::Inst(InstForm::Fixed(Inst::Trap { code }), 1))
        }

        // --- pseudo-instructions (§7.1)
        "nop" => {
            want(0)?;
            Ok(Kind::Inst(
                InstForm::Fixed(Inst::Alu {
                    op: AluOp::Add,
                    flavor: Flavor::Wrap,
                    rd: Reg::ZERO,
                    rs1: Reg::ZERO,
                    rs2: Reg::ZERO,
                    rc: Reg::ZERO,
                }),
                1,
            ))
        }

        "mv" | "neg" => {
            want(2)?;
            let (rd, rs) = (one(&parts[0])?, one(&parts[1])?);
            let (op, rs1, rs2) = if stem == "mv" {
                (AluOp::Add, rs, Reg::ZERO)
            } else {
                (AluOp::Sub, Reg::ZERO, rs)
            };
            Ok(Kind::Inst(
                InstForm::Fixed(Inst::Alu {
                    op,
                    flavor: Flavor::Wrap,
                    rd,
                    rs1,
                    rs2,
                    rc: Reg::ZERO,
                }),
                1,
            ))
        }

        "li" | "la" => {
            want(2)?;
            let rd = one(&parts[0])?;
            let expr = parts[1].clone();
            // `la` of a forward-referenced label always takes the two-word
            // form, so a statement's size never depends on a value pass one
            // cannot see (§7.1). `li` of a constant expression may shrink.
            let words = if stem == "la" {
                2
            } else {
                match eval_i128(&expr, line, 0, syms) {
                    Ok(v) if word::fits(v, 14) => 1,
                    _ => 2,
                }
            };
            Ok(Kind::Inst(InstForm::Load { rd, expr, words }, words))
        }

        other => err(line, format!("`{other}` is not an instruction")),
    }
}

fn build_alui(op: AluOp, flavor: Flavor) -> fn(Reg, Reg, i128) -> Inst {
    // A function pointer cannot capture, so the flavor and operation are
    // recovered from a small table.
    macro_rules! pick {
        ($($o:ident),*) => {
            match (op, flavor) {
                $(
                    (AluOp::$o, Flavor::Wrap) => |rd, rs1, imm| Inst::AluI {
                        op: AluOp::$o, flavor: Flavor::Wrap, rd, rs1, imm },
                    (AluOp::$o, Flavor::Trap) => |rd, rs1, imm| Inst::AluI {
                        op: AluOp::$o, flavor: Flavor::Trap, rd, rs1, imm },
                    (AluOp::$o, Flavor::Flag) => |rd, rs1, imm| Inst::AluI {
                        op: AluOp::$o, flavor: Flavor::Flag, rd, rs1, imm },
                )*
            }
        };
    }
    pick!(
        Add, Sub, Mul, MulH, Div, Rem, Shl, Shr, TMin, TMax, TMul, Cmp, Wrap
    )
}

// -------------------------------------------------------------------- emit

fn emit(stmt: &Stmt, syms: &Symbols, image: &mut Vec<i16>) -> R<()> {
    let line = stmt.line;
    let here = stmt.addr;

    let push_tryte = |image: &mut Vec<i16>, v: i128| -> R<()> {
        if !word::fits(v, 9) {
            return err(line, format!("{v} does not fit in a tryte"));
        }
        image.push(v as i16);
        Ok(())
    };

    match &stmt.kind {
        Kind::Nothing => {}

        Kind::Trytes(exprs) => {
            for e in exprs {
                let v = eval_i128(e, line, here, syms)?;
                push_tryte(image, v)?;
            }
        }

        Kind::Words(exprs) => {
            for e in exprs {
                let v = eval_i128(e, line, here, syms)?;
                if !word::fits(v, 27) {
                    return err(line, format!("{v} does not fit in a word"));
                }
                image.extend(word::word_trytes(v));
            }
        }

        Kind::Trits(trits) => {
            for chunk in trits.chunks(9) {
                image.push(word::from_trits(chunk) as i16);
            }
        }

        Kind::Pad(n) => image.extend(std::iter::repeat_n(0i16, *n as usize)),

        Kind::Fill(n, e) => {
            let v = eval_i128(e, line, here, syms)?;
            for _ in 0..*n {
                push_tryte(image, v)?;
            }
        }

        Kind::Inst(form, _) => {
            for w in resolve(form, line, here, syms)? {
                image.extend(word::word_trytes(w));
            }
        }
    }
    Ok(())
}

fn resolve(form: &InstForm, line: u32, here: i128, syms: &Symbols) -> R<Vec<i128>> {
    Ok(match form {
        InstForm::Fixed(i) => vec![i.encode()],

        InstForm::Imm {
            build,
            rd,
            rs1,
            expr,
            width,
        } => {
            let v = eval_i128(expr, line, here, syms)?;
            if !word::fits(v, *width) {
                return err(
                    line,
                    format!("{v} does not fit in the {width}-trit immediate"),
                );
            }
            vec![build(*rd, *rs1, v).encode()]
        }

        InstForm::Br3 { rs1, targets } => {
            let mut offs = [0i128; 3];
            for (i, t) in targets.iter().enumerate() {
                offs[i] = displacement(t, line, here, syms, 7)?;
            }
            vec![
                Inst::Br3 {
                    rs1: *rs1,
                    neg: offs[0],
                    zero: offs[1],
                    pos: offs[2],
                }
                .encode(),
            ]
        }

        InstForm::Jal { rd, target } => {
            let off = displacement(target, line, here, syms, 21)?;
            vec![Inst::Jal { rd: *rd, off }.encode()]
        }

        // The split needs no correction term: a balanced constant divides
        // into high and low trits with nothing borrowed (TRISC-27 §4.3).
        InstForm::Load { rd, expr, words } => {
            let v = eval_i128(expr, line, here, syms)?;
            if !word::fits(v, 27) {
                return err(line, format!("{v} does not fit in a word"));
            }
            let addi = |rd: Reg, rs1: Reg, imm: i128| Inst::AluI {
                op: AluOp::Add,
                flavor: Flavor::Wrap,
                rd,
                rs1,
                imm,
            };
            if *words == 1 {
                vec![addi(*rd, Reg::ZERO, v).encode()]
            } else {
                vec![
                    Inst::Lui {
                        rd: *rd,
                        imm: word::shr3(v, 14),
                    }
                    .encode(),
                    addi(*rd, *rd, word::wrap_to(v, 14)).encode(),
                ]
            }
        }
    })
}

/// A control-transfer operand is a target address; the displacement in words
/// is computed here and range-checked (TRISC-27 §4.5).
fn displacement(expr: &[Tok], line: u32, here: i128, syms: &Symbols, width: u32) -> R<i128> {
    let target = eval_i128(expr, line, here, syms)?;
    let delta = target - here;
    if delta.rem_euclid(3) != 0 {
        return err(line, format!("target {target} is not word-aligned"));
    }
    let off = delta / 3;
    if !word::fits(off, width) {
        return err(
            line,
            format!(
                "target {target} is {off} words away, out of range for a {width}-trit \
                 displacement; place the block closer or jump (§4.5)"
            ),
        );
    }
    Ok(off)
}
