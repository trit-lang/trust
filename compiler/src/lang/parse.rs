//! The parser (Language Ch. 0 §§2–6).
//!
//! Recursive descent, with the binary operators driven by §2.1's precedence
//! table. The two comparison levels are non-associative, so `a < b < c` and
//! `a <=> b <=> c` are syntax errors rather than surprises.

use super::ast::*;
use super::lex::{Line, SyntaxError, Tok, lex};

type R<T> = Result<T, SyntaxError>;

/// Parse a source file.
pub fn parse(src: &str) -> R<File> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
        no_struct: false,
    };
    let mut file = File::default();
    while !p.at(&Tok::Eof) {
        file.items.push(p.item()?);
    }
    Ok(file)
}

struct Parser {
    toks: Vec<(Tok, Line)>,
    pos: usize,
    /// Set while parsing a condition, where a struct literal's `{` would be
    /// read as the block that follows (§2.8).
    no_struct: bool,
}

/// §2.1's table, loosest level first, indexed by level so that the
/// non-associative comparison levels can sit at their own index without
/// shifting the others. Level 2 is the comparison pair and is empty here.
const LEVELS: &[&[&str]] = &[
    &["||"],          // 11
    &["&&"],          // 10
    &[],              // 9 and 8: comparisons and `<=>`, non-associative
    &["<<", ">>"],    // 7
    &["+", "-"],      // 6
    &["*", "/", "%"], // 5
];

/// The level at which the non-associative comparisons sit.
const COMPARE_LEVEL: usize = 2;

const COMPARISONS: &[&str] = &["==", "!=", "<", "<=", ">", ">="];
const ASSIGNMENTS: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>="];

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn line(&self) -> Line {
        self.toks[self.pos].1
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn at_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Kw(k) if *k == kw)
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(SyntaxError {
            line: self.line(),
            message: msg.into(),
        })
    }

    fn expect_op(&mut self, op: &str) -> R<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            self.err(format!("expected `{op}`, found {}", self.peek()))
        }
    }

    fn expect_ident(&mut self) -> R<String> {
        match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                Ok(n)
            }
            other => self.err(format!("expected a name, found {other}")),
        }
    }

    // ------------------------------------------------------------- items

    fn item(&mut self) -> R<Item> {
        // Attributes attach to the item that follows (§3.4). Draft 0.1
        // defines exactly one, `repr`, taking `lang` or `linear`.
        let mut repr = Repr::Lang;
        while self.at_op("#") {
            let line = self.line();
            self.bump();
            self.expect_op("[")?;
            let name = self.expect_ident()?;
            if name != "repr" {
                return Err(SyntaxError {
                    line,
                    message: format!("`{name}` is not an attribute; draft 0.1 defines only `repr`"),
                });
            }
            self.expect_op("(")?;
            let arg = self.expect_ident()?;
            repr = match arg.as_str() {
                "lang" => Repr::Lang,
                "linear" => Repr::Linear,
                other => {
                    return Err(SyntaxError {
                        line,
                        message: format!(
                            "`repr({other})` is not a layout regime; use `lang` or `linear`"
                        ),
                    });
                }
            };
            self.expect_op(")")?;
            self.expect_op("]")?;
        }

        if self.at_kw("fn") {
            return Ok(Item::Fn(self.fn_item()?));
        }
        if self.at_kw("const") {
            return Ok(Item::Const(self.const_item()?));
        }
        if self.at_kw("struct") {
            return Ok(Item::Struct(self.struct_item(repr)?));
        }
        if self.at_kw("enum") {
            return Ok(Item::Enum(self.enum_item(repr)?));
        }
        self.err(format!("expected an item, found {}", self.peek()))
    }

    /// `struct Name { … }`, `struct Name(…);`, `struct Name;` (§3.3).
    fn struct_item(&mut self, repr: Repr) -> R<StructItem> {
        let line = self.line();
        self.bump(); // struct
        let name = self.expect_ident()?;
        let fields = if self.at_op("{") {
            self.named_fields()?
        } else if self.at_op("(") {
            let tys = self.type_list("(", ")")?;
            self.expect_op(";")?;
            tys.into_iter()
                .enumerate()
                .map(|(i, t)| (i.to_string(), t))
                .collect()
        } else {
            self.expect_op(";")?;
            Vec::new()
        };
        Ok(StructItem {
            name,
            repr,
            fields,
            line,
        })
    }

    fn enum_item(&mut self, repr: Repr) -> R<EnumItem> {
        let line = self.line();
        self.bump(); // enum
        let name = self.expect_ident()?;
        self.expect_op("{")?;
        let mut variants = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated enum");
            }
            let vline = self.line();
            let vname = self.expect_ident()?;
            let fields = if self.at_op("{") {
                self.named_fields()?
            } else if self.at_op("(") {
                self.type_list("(", ")")?
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (i.to_string(), t))
                    .collect()
            } else {
                Vec::new()
            };
            // An explicit discriminant may be negative (Ch. 2 §5.1).
            let discriminant = if self.eat_op("=") {
                let negative = self.eat_op("-");
                match self.bump() {
                    Tok::Int(v) => {
                        let v = v
                            .to_i128()
                            .ok_or(())
                            .or_else(|()| self.err::<i128>("discriminant is too large"))?;
                        Some(if negative { -v } else { v })
                    }
                    other => return self.err(format!("expected a discriminant, found {other}")),
                }
            } else {
                None
            };
            variants.push(Variant {
                name: vname,
                fields,
                discriminant,
                line: vline,
            });
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between variants");
            }
        }
        Ok(EnumItem {
            name,
            repr,
            variants,
            line,
        })
    }

    fn named_fields(&mut self) -> R<Vec<(String, Ty)>> {
        self.expect_op("{")?;
        let mut fields = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated field list");
            }
            let name = self.expect_ident()?;
            self.expect_op(":")?;
            fields.push((name, self.ty()?));
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between fields");
            }
        }
        Ok(fields)
    }

    fn type_list(&mut self, open: &str, close: &str) -> R<Vec<Ty>> {
        self.expect_op(open)?;
        let mut tys = Vec::new();
        while !self.eat_op(close) {
            if self.at(&Tok::Eof) {
                return self.err("unterminated type list");
            }
            tys.push(self.ty()?);
            if !self.eat_op(",") && !self.at_op(close) {
                return self.err(format!("expected `,` or `{close}`"));
            }
        }
        Ok(tys)
    }

    fn fn_item(&mut self) -> R<FnItem> {
        let line = self.line();
        self.bump(); // fn
        let name = self.expect_ident()?;
        self.expect_op("(")?;
        let mut params = Vec::new();
        if !self.eat_op(")") {
            loop {
                let pname = self.expect_ident()?;
                self.expect_op(":")?;
                params.push((pname, self.ty()?));
                if self.eat_op(",") {
                    if self.eat_op(")") {
                        break;
                    }
                    continue;
                }
                self.expect_op(")")?;
                break;
            }
        }
        let ret = if self.eat_op("->") {
            Some(self.ty()?)
        } else {
            None
        };

        // A function without a body is a declaration (§3.1) — the same rule
        // TIR §1 states for its own.
        let body = if self.eat_op(";") {
            None
        } else {
            Some(self.block()?)
        };
        Ok(FnItem {
            name,
            params,
            ret,
            body,
            line,
        })
    }

    fn const_item(&mut self) -> R<ConstItem> {
        let line = self.line();
        self.bump(); // const
        let name = self.expect_ident()?;
        self.expect_op(":")?;
        let ty = self.ty()?;
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;
        Ok(ConstItem {
            name,
            ty,
            value,
            line,
        })
    }

    fn ty(&mut self) -> R<Ty> {
        let line = self.line();
        if self.at_op("(") {
            let tys = self.type_list("(", ")")?;
            return Ok(if tys.is_empty() {
                Ty::Unit(line)
            } else {
                Ty::Tuple(tys, line)
            });
        }
        if self.eat_op("[") {
            let elem = self.ty()?;
            self.expect_op(";")?;
            let n = self.expr()?;
            self.expect_op("]")?;
            return Ok(Ty::Array(Box::new(elem), Box::new(n), line));
        }
        if self.at_op("&") {
            return self.err("references are Chapter 3, which is not written yet");
        }
        Ok(Ty::Name(self.expect_ident()?, line))
    }

    // -------------------------------------------------------- statements

    fn block(&mut self) -> R<Block> {
        let line = self.line();
        self.expect_op("{")?;
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated block");
            }
            if self.at_kw("let") {
                stmts.push(self.let_stmt()?);
                continue;
            }
            if self.eat_op(";") {
                continue;
            }

            let e = self.expr()?;
            if self.eat_op(";") {
                stmts.push(Stmt::Expr(e));
            } else if self.at_op("}") {
                self.bump();
                tail = Some(Box::new(e));
                break;
            } else if block_like(&e) {
                // A block-shaped expression may stand as a statement without
                // a terminator, as in Rust.
                stmts.push(Stmt::Expr(e));
            } else {
                return self.err(format!("expected `;` or `}}`, found {}", self.peek()));
            }
        }
        Ok(Block { stmts, tail, line })
    }

    fn let_stmt(&mut self) -> R<Stmt> {
        let line = self.line();
        self.bump(); // let
        let mutable = self.eat_kw("mut");
        let name = self.expect_ident()?;
        let ty = if self.eat_op(":") {
            Some(self.ty()?)
        } else {
            None
        };
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;
        Ok(Stmt::Let {
            mutable,
            name,
            ty,
            value,
            line,
        })
    }

    // ------------------------------------------------------- expressions

    /// The whole expression grammar, loosest first: assignment.
    pub fn expr(&mut self) -> R<Expr> {
        let line = self.line();
        let lhs = self.binary(0)?;
        if let Tok::Op(op) = self.peek()
            && ASSIGNMENTS.contains(op)
        {
            let op = *op;
            self.bump();
            let rhs = self.expr()?; // right-associative
            return Ok(Expr::Assign(op, Box::new(lhs), Box::new(rhs), line));
        }
        Ok(lhs)
    }

    /// The left-associative levels, plus the two non-associative comparison
    /// levels wedged between `&&` and the shifts.
    fn binary(&mut self, level: usize) -> R<Expr> {
        if level == COMPARE_LEVEL {
            return self.comparison();
        }
        if level >= LEVELS.len() {
            return self.cast();
        }
        let ops = LEVELS[level];
        let mut lhs = self.binary(level + 1)?;
        loop {
            let Tok::Op(op) = self.peek() else {
                return Ok(lhs);
            };
            if !ops.contains(op) {
                return Ok(lhs);
            }
            let (op, line) = (*op, self.line());
            self.bump();
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs), line);
        }
    }

    /// `==` and friends, then `<=>`. Both are non-associative (§2.1).
    fn comparison(&mut self) -> R<Expr> {
        let lhs = self.spaceship()?;
        let Tok::Op(op) = self.peek() else {
            return Ok(lhs);
        };
        if !COMPARISONS.contains(op) {
            return Ok(lhs);
        }
        let (op, line) = (*op, self.line());
        self.bump();
        let rhs = self.spaceship()?;
        if let Tok::Op(next) = self.peek()
            && COMPARISONS.contains(next)
        {
            return self.err("comparison operators do not chain: parenthesize");
        }
        Ok(Expr::Binary(op, Box::new(lhs), Box::new(rhs), line))
    }

    fn spaceship(&mut self) -> R<Expr> {
        let lhs = self.binary(COMPARE_LEVEL + 1)?;
        if !self.at_op("<=>") {
            return Ok(lhs);
        }
        let line = self.line();
        self.bump();
        let rhs = self.binary(COMPARE_LEVEL + 1)?;
        if self.at_op("<=>") {
            return self.err("`<=>` does not chain: parenthesize");
        }
        Ok(Expr::Binary("<=>", Box::new(lhs), Box::new(rhs), line))
    }

    fn cast(&mut self) -> R<Expr> {
        let mut e = self.unary()?;
        while self.at_kw("as") {
            let line = self.line();
            self.bump();
            e = Expr::Cast(Box::new(e), self.ty()?, line);
        }
        Ok(e)
    }

    fn unary(&mut self) -> R<Expr> {
        let line = self.line();
        for op in ["-", "!"] {
            if self.at_op(op) {
                self.bump();
                return Ok(Expr::Unary(op, Box::new(self.unary()?), line));
            }
        }
        if self.at_op("&") {
            return self.err("borrowing is Chapter 3, which is not written yet");
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let line = self.line();
            if self.eat_op(".") {
                // `x.0` is a tuple index; `x.f` is a field; `x.f(…)` a method.
                if let Tok::Int(v) = self.peek().clone() {
                    self.bump();
                    let index = v.to_i128().unwrap_or(-1);
                    e = Expr::Field(Box::new(e), index.to_string(), line);
                    continue;
                }
                let name = self.expect_ident()?;
                if self.at_op("(") {
                    let args = self.args()?;
                    e = Expr::Method(Box::new(e), name, args, line);
                } else {
                    e = Expr::Field(Box::new(e), name, line);
                }
                continue;
            }
            if self.at_op("[") {
                self.bump();
                let index = self.expr()?;
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), Box::new(index), line);
                continue;
            }
            return Ok(e);
        }
    }

    fn args(&mut self) -> R<Vec<Expr>> {
        self.expect_op("(")?;
        let mut args = Vec::new();
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if self.eat_op(",") {
                if self.eat_op(")") {
                    break;
                }
                continue;
            }
            self.expect_op(")")?;
            break;
        }
        Ok(args)
    }

    fn primary(&mut self) -> R<Expr> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Int(v) => {
                self.bump();
                Ok(Expr::Int(v, line))
            }
            Tok::TritLit(t) => {
                self.bump();
                Ok(Expr::Trit(t, line))
            }
            Tok::Kw("true") => {
                self.bump();
                Ok(Expr::Bool(true, line))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Expr::Bool(false, line))
            }
            Tok::Ident(name) => {
                self.bump();
                let mut segments = vec![name];
                while self.eat_op("::") {
                    segments.push(self.expect_ident()?);
                }
                let path = Path { segments, line };

                // `Name(args)` is a call when the name is a function and a
                // tuple-struct or variant literal when it is a type; the two
                // are told apart during lowering, where the names are known.
                if self.at_op("(") {
                    let args = self.args()?;
                    if path.segments.len() == 1 {
                        return Ok(Expr::Call(path.segments[0].clone(), args, line));
                    }
                    let fields = args
                        .into_iter()
                        .enumerate()
                        .map(|(i, e)| (i.to_string(), e))
                        .collect();
                    return Ok(Expr::Aggregate(path, fields, line));
                }
                // A struct literal is ambiguous with a block in a condition
                // position, and is not permitted there without parentheses
                // (§2.8).
                if self.at_op("{") && !self.no_struct {
                    let fields = self.field_values()?;
                    return Ok(Expr::Aggregate(path, fields, line));
                }
                if path.segments.len() > 1 {
                    return Ok(Expr::Aggregate(path, Vec::new(), line));
                }
                Ok(Expr::Path(path.segments[0].clone(), line))
            }
            Tok::Op("(") => {
                self.bump();
                // A struct literal is legal again inside parentheses.
                let outer = std::mem::replace(&mut self.no_struct, false);
                if self.eat_op(")") {
                    self.no_struct = outer;
                    return Ok(Expr::Unit(line));
                }
                let first = self.expr()?;
                let result = if self.at_op(",") {
                    let mut items = vec![first];
                    while self.eat_op(",") {
                        if self.at_op(")") {
                            break;
                        }
                        items.push(self.expr()?);
                    }
                    self.expect_op(")")?;
                    Expr::Tuple(items, line)
                } else {
                    self.expect_op(")")?;
                    first
                };
                self.no_struct = outer;
                Ok(result)
            }
            Tok::Op("[") => {
                self.bump();
                if self.eat_op("]") {
                    return Ok(Expr::Array(Vec::new(), line));
                }
                let first = self.expr()?;
                if self.eat_op(";") {
                    let count = self.expr()?;
                    self.expect_op("]")?;
                    return Ok(Expr::Repeat(Box::new(first), Box::new(count), line));
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.at_op("]") {
                        break;
                    }
                    items.push(self.expr()?);
                }
                self.expect_op("]")?;
                Ok(Expr::Array(items, line))
            }
            Tok::Op("{") => Ok(Expr::Block(self.block()?)),
            Tok::Kw("if") => self.if_expr(),
            Tok::Kw("match") => self.match_expr(),
            Tok::Kw("loop") => {
                self.bump();
                Ok(Expr::Loop(self.block()?, line))
            }
            Tok::Kw("while") => {
                self.bump();
                let cond = self.no_struct_expr()?;
                Ok(Expr::While(Box::new(cond), self.block()?, line))
            }
            Tok::Kw("break") => {
                self.bump();
                let v = self.optional_value()?;
                Ok(Expr::Break(v, line))
            }
            Tok::Kw("continue") => {
                self.bump();
                Ok(Expr::Continue(line))
            }
            Tok::Kw("return") => {
                self.bump();
                let v = self.optional_value()?;
                Ok(Expr::Return(v, line))
            }
            other => self.err(format!("expected an expression, found {other}")),
        }
    }

    /// A value after `break` or `return`, if one is there.
    fn optional_value(&mut self) -> R<Option<Box<Expr>>> {
        if self.at_op(";") || self.at_op("}") || self.at_op(",") {
            return Ok(None);
        }
        Ok(Some(Box::new(self.expr()?)))
    }

    /// The condition of `if`, `while` and `match`, where a struct literal
    /// would be ambiguous with the block that follows (§2.8).
    fn no_struct_expr(&mut self) -> R<Expr> {
        let outer = std::mem::replace(&mut self.no_struct, true);
        let e = self.expr();
        self.no_struct = outer;
        e
    }

    /// `{ name: value, … }`, or `{ name, … }` where the field and the local
    /// share a name.
    fn field_values(&mut self) -> R<Vec<(String, Expr)>> {
        self.expect_op("{")?;
        let mut fields = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated struct literal");
            }
            let line = self.line();
            let name = self.expect_ident()?;
            let value = if self.eat_op(":") {
                self.expr()?
            } else {
                Expr::Path(name.clone(), line)
            };
            fields.push((name, value));
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between fields");
            }
        }
        Ok(fields)
    }

    fn if_expr(&mut self) -> R<Expr> {
        let line = self.line();
        self.bump(); // if
        let cond = self.no_struct_expr()?;
        let then = self.block()?;
        let els = if self.eat_kw("else") {
            if self.at_kw("if") {
                Some(Box::new(self.if_expr()?))
            } else {
                Some(Box::new(Expr::Block(self.block()?)))
            }
        } else {
            None
        };
        Ok(Expr::If(Box::new(cond), then, els, line))
    }

    fn match_expr(&mut self) -> R<Expr> {
        let line = self.line();
        self.bump(); // match
        let scrutinee = self.no_struct_expr()?;
        self.expect_op("{")?;
        let mut arms = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated `match`");
            }
            let arm_line = self.line();
            let mut patterns = vec![self.pattern()?];
            while self.eat_op("|") {
                patterns.push(self.pattern()?);
            }
            let guard = if self.at_kw("if") {
                self.bump();
                Some(self.expr()?)
            } else {
                None
            };
            self.expect_op("=>")?;
            let body = self.expr()?;
            let needs_comma = !block_like(&body);
            arms.push(Arm {
                patterns,
                guard,
                body,
                line: arm_line,
            });
            if !self.eat_op(",") && needs_comma && !self.at_op("}") {
                return self.err("expected `,` between match arms");
            }
        }
        Ok(Expr::Match(Box::new(scrutinee), arms, line))
    }

    fn pattern_list(&mut self, open: &str, close: &str) -> R<Vec<Pattern>> {
        self.expect_op(open)?;
        let mut out = Vec::new();
        while !self.eat_op(close) {
            if self.at(&Tok::Eof) {
                return self.err("unterminated pattern list");
            }
            out.push(self.pattern()?);
            if !self.eat_op(",") && !self.at_op(close) {
                return self.err(format!("expected `,` or `{close}`"));
            }
        }
        Ok(out)
    }

    fn field_patterns(&mut self) -> R<Vec<(String, Pattern)>> {
        self.expect_op("{")?;
        let mut out = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated field pattern");
            }
            let line = self.line();
            let name = self.expect_ident()?;
            let pat = if self.eat_op(":") {
                self.pattern()?
            } else {
                Pattern::Bind(name.clone(), line)
            };
            out.push((name, pat));
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between field patterns");
            }
        }
        Ok(out)
    }

    fn pattern(&mut self) -> R<Pattern> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Op("_") => {
                self.bump();
                Ok(Pattern::Wild(line))
            }
            Tok::Int(v) => {
                self.bump();
                Ok(Pattern::Int(v, line))
            }
            Tok::TritLit(t) => {
                self.bump();
                Ok(Pattern::Trit(t, line))
            }
            Tok::Op("-") => {
                self.bump();
                match self.bump() {
                    Tok::Int(v) => Ok(Pattern::Int(v.neg(), line)),
                    Tok::TritLit(t) => Ok(Pattern::Trit(t.tneg(), line)),
                    other => Err(SyntaxError {
                        line,
                        message: format!("expected a literal after `-`, found {other}"),
                    }),
                }
            }
            Tok::Kw("true") => {
                self.bump();
                Ok(Pattern::Bool(true, line))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Pattern::Bool(false, line))
            }
            Tok::Ident(name) => {
                self.bump();
                let mut segments = vec![name];
                while self.eat_op("::") {
                    segments.push(self.expect_ident()?);
                }
                let path = Path { segments, line };

                if self.at_op("(") {
                    let inner = self.pattern_list("(", ")")?;
                    let fields = inner
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| (i.to_string(), p))
                        .collect();
                    return Ok(Pattern::Aggregate(path, fields, line));
                }
                if self.at_op("{") {
                    let fields = self.field_patterns()?;
                    return Ok(Pattern::Aggregate(path, fields, line));
                }
                if path.segments.len() > 1 {
                    return Ok(Pattern::Aggregate(path, Vec::new(), line));
                }
                // `name @ pattern` binds the whole while matching (§4).
                if self.eat_op("@") {
                    let inner = self.pattern()?;
                    return Ok(Pattern::Aggregate(
                        Path {
                            segments: vec![path.segments[0].clone()],
                            line,
                        },
                        vec![("@".to_string(), inner)],
                        line,
                    ));
                }
                Ok(Pattern::Bind(path.segments[0].clone(), line))
            }
            Tok::Op("(") => {
                let inner = self.pattern_list("(", ")")?;
                Ok(Pattern::Tuple(inner, line))
            }
            other => self.err(format!("expected a pattern, found {other}")),
        }
    }
}

/// True for expressions written with braces, which may stand as statements
/// without a `;` and as match arms without a `,`.
fn block_like(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Block(_) | Expr::If(..) | Expr::Match(..) | Expr::Loop(..) | Expr::While(..)
    )
}
