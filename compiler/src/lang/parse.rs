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
        counter: 0,
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
    /// Names the parser has to invent, for §5.7's desugaring.
    counter: u32,
}

/// A bound's angle brackets: type arguments, and associated type bindings.
type BoundArgs = (Vec<Ty>, Vec<(String, Ty)>);

/// What a `trait` or `impl` body contains: methods, and associated types
/// either declared (`type Item;`) or chosen (`type Item = t27;`).
type MethodBlock = (
    Vec<FnItem>,
    Vec<(String, Option<Ty>)>,
    Vec<(String, Ty, Option<Expr>)>,
);

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

    /// The token `n` ahead, for the one place a single token is not enough to
    /// decide: `Item = t27` against `t27` inside a bound's arguments.
    fn peek_at(&self, n: usize) -> &Tok {
        let at = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[at].0
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

    /// Close one level of angle brackets.
    ///
    /// `Option<Option<t27>>` ends in a token the lexer has every reason to
    /// read as the shift operator, and only the parser knows it is two
    /// brackets. Splitting it here — taking one `>` and leaving the other in
    /// place — is what lets generic arguments nest at all.
    fn close_angle(&mut self) -> bool {
        if self.eat_op(">") {
            return true;
        }
        if self.at_op(">>") {
            self.toks[self.pos].0 = Tok::Op(">");
            return true;
        }
        false
    }

    /// The same, as an expectation.
    fn expect_angle(&mut self) -> R<()> {
        if self.close_angle() {
            Ok(())
        } else {
            self.err(format!("expected `>`, found {}", self.peek()))
        }
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
        let mut derives: Vec<String> = Vec::new();
        while self.at_op("#") {
            let line = self.line();
            self.bump();
            self.expect_op("[")?;
            let name = self.expect_ident()?;
            self.expect_op("(")?;
            match name.as_str() {
                "repr" => {
                    let arg = self.expect_ident()?;
                    repr = match arg.as_str() {
                        "lang" => Repr::Lang,
                        "linear" => Repr::Linear,
                        other => {
                            return Err(SyntaxError {
                                line,
                                message: format!(
                                    "`repr({other})` is not a layout regime; use `lang` or \
                                     `linear`"
                                ),
                            });
                        }
                    };
                }
                // Ch. 4 §6: the second attribute the language defines.
                "derive" => loop {
                    let t = self.expect_ident()?;
                    if !matches!(t.as_str(), "Eq" | "Ord" | "Clone") {
                        return Err(SyntaxError {
                            line,
                            message: format!(
                                "`{t}` is not derivable; Ch. 4 §6 derives `Eq`, `Ord` and \
                                 `Clone`, and says why `Copy`, `Sized` and `Drop` are not"
                            ),
                        });
                    }
                    derives.push(t);
                    if !self.eat_op(",") || self.at_op(")") {
                        break;
                    }
                },
                other => {
                    return Err(SyntaxError {
                        line,
                        message: format!(
                            "`{other}` is not an attribute; draft 0.1 defines `repr` (Ch. 2 §1) \
                             and `derive` (Ch. 4 §6)"
                        ),
                    });
                }
            }
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
            return Ok(Item::Struct(self.struct_item(repr, derives)?));
        }
        if self.at_kw("enum") {
            return Ok(Item::Enum(self.enum_item(repr, derives)?));
        }
        if !derives.is_empty() {
            return self.err("`derive` attaches to a struct or an enum (Ch. 4 §6)");
        }
        if self.at_kw("trait") {
            return Ok(Item::Trait(self.trait_item()?));
        }
        if self.at_kw("impl") {
            return Ok(Item::Impl(self.impl_item()?));
        }
        self.err(format!("expected an item, found {}", self.peek()))
    }

    /// `struct Name { … }`, `struct Name(…);`, `struct Name;` (§3.3).
    fn struct_item(&mut self, repr: Repr, derives: Vec<String>) -> R<StructItem> {
        let line = self.line();
        self.bump(); // struct
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
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
            generics,
            derives,
            repr,
            fields,
            line,
        })
    }

    fn enum_item(&mut self, repr: Repr, derives: Vec<String>) -> R<EnumItem> {
        let line = self.line();
        self.bump(); // enum
        let name = self.expect_ident()?;
        let generics = self.generic_params()?;
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
            generics,
            derives,
            repr,
            variants,
            line,
        })
    }

    /// `<'a, 'b>` — parsed and discarded, since lifetimes are erased before
    /// TIR (Ch. 3 §3.1). Ch. 4 will put type parameters in the same list.
    /// `<'a, T: Bound, const N: taddr>` (Ch. 3 §3.2, Ch. 4 §2.1).
    ///
    /// Lifetimes are parsed and dropped: they exist for the borrow checker
    /// and are erased before TIR (Ch. 3 §3.1).
    fn generic_params(&mut self) -> R<Vec<GenericParam>> {
        let mut out = Vec::new();
        if !self.at_op("<") {
            return Ok(out);
        }
        self.bump();
        if self.close_angle() {
            return Ok(out);
        }
        loop {
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
                // `'a: 'b` outlives, also erased.
                if self.eat_op(":") {
                    while matches!(self.peek(), Tok::Lifetime(_)) {
                        self.bump();
                        if !self.eat_op("+") {
                            break;
                        }
                    }
                }
            } else if self.eat_kw("const") {
                let name = self.expect_ident()?;
                self.expect_op(":")?;
                let ty = self.ty()?;
                out.push(GenericParam::Const { name, ty });
            } else {
                let name = self.expect_ident()?;
                let mut bounds = Vec::new();
                if self.eat_op(":") {
                    loop {
                        // A lifetime bound — `T: 'a` — is erased with the
                        // rest of them (Ch. 4 §2.6).
                        if let Tok::Lifetime(_) = self.peek() {
                            self.bump();
                        } else {
                            bounds.push(self.bound()?);
                        }
                        if !self.eat_op("+") {
                            break;
                        }
                    }
                }
                out.push(GenericParam::Type { name, bounds });
            }
            if self.eat_op(",") {
                if self.close_angle() {
                    return Ok(out);
                }
                continue;
            }
            self.expect_angle()?;
            return Ok(out);
        }
    }

    /// One bound: a trait's name, and its type arguments when it takes any
    /// (Ch. 4 §1.7). `From<t9>` is a different requirement from `From<t27>`,
    /// which is the whole reason a trait may carry parameters.
    fn bound(&mut self) -> R<Bound> {
        let name = self.expect_ident()?;
        let (args, assoc) = self.bound_args()?;
        Ok(Bound { name, args, assoc })
    }

    /// A bound's `<…>`, which may hold type arguments, associated type
    /// bindings, or both: `From<t9>`, `Iterator<Item = t27>`.
    ///
    /// The two are told apart by the `=`, which is why this cannot simply be
    /// `generic_args`.
    fn bound_args(&mut self) -> R<BoundArgs> {
        let mut args = Vec::new();
        let mut assoc = Vec::new();
        if !self.at_op("<") {
            return Ok((args, assoc));
        }
        self.bump();
        if self.close_angle() {
            return Ok((args, assoc));
        }
        loop {
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            } else if let (Tok::Ident(n), Tok::Op("=")) = (self.peek().clone(), self.peek_at(1)) {
                self.bump();
                self.bump();
                assoc.push((n, self.ty()?));
            } else {
                args.push(self.ty()?);
            }
            if self.eat_op(",") {
                if self.close_angle() {
                    return Ok((args, assoc));
                }
                continue;
            }
            self.expect_angle()?;
            return Ok((args, assoc));
        }
    }

    /// `where T: Bound, U: Other` — the same bounds, written after the
    /// signature instead of inside the angle brackets (Ch. 4 §2.2).
    fn where_clause(&mut self, generics: &mut [GenericParam]) -> R<()> {
        if !self.eat_kw("where") {
            return Ok(());
        }
        loop {
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
                self.expect_op(":")?;
                while matches!(self.peek(), Tok::Lifetime(_)) {
                    self.bump();
                    if !self.eat_op("+") {
                        break;
                    }
                }
            } else {
                let name = self.expect_ident()?;
                self.expect_op(":")?;
                let mut bounds = Vec::new();
                loop {
                    if let Tok::Lifetime(_) = self.peek() {
                        self.bump();
                    } else {
                        bounds.push(self.bound()?);
                    }
                    if !self.eat_op("+") {
                        break;
                    }
                }
                match generics.iter_mut().find(|g| g.name() == name) {
                    Some(GenericParam::Type { bounds: b, .. }) => b.extend(bounds),
                    _ => {
                        return self.err(format!(
                            "`{name}` is not a type parameter of this item, so a \
                             `where` clause cannot bound it"
                        ));
                    }
                }
            }
            if !self.eat_op(",") {
                return Ok(());
            }
            if self.at_op("{") {
                return Ok(());
            }
        }
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

    /// `trait Name<T>: Super + Other { … }` (Ch. 4 §§1.1, 1.7).
    fn trait_item(&mut self) -> R<TraitItem> {
        let line = self.line();
        self.bump(); // trait
        let name = self.expect_ident()?;
        // A trait's parameters are chosen by whoever implements it, once per
        // implementation, which is what lets one type implement it many
        // times (Ch. 4 §1.7).
        let mut params = Vec::new();
        for p in self.generic_params()? {
            match p {
                GenericParam::Type { name, bounds } if bounds.is_empty() => params.push(name),
                GenericParam::Type { name, .. } => {
                    return self.err(format!(
                        "a bound on a trait's own parameter (`{name}`) is not implemented; \
                         write it on the impl instead"
                    ));
                }
                GenericParam::Const { name, .. } => {
                    return self.err(format!(
                        "`const {name}` as a trait parameter is not implemented (Ch. 4 §2.4)"
                    ));
                }
            }
        }
        let mut supertraits = Vec::new();
        if self.eat_op(":") {
            loop {
                supertraits.push(self.expect_ident()?);
                if !self.eat_op("+") {
                    break;
                }
            }
        }
        let (methods, assoc, consts) = self.method_block("trait")?;
        let mut names = Vec::new();
        for (n, v) in assoc {
            if v.is_some() {
                return self.err(format!(
                    "`type {n} = …` chooses a type, which is an impl's business; a trait \
                     declares `type {n};` (Ch. 4 §1.7)"
                ));
            }
            names.push(n);
        }
        let mut declared = Vec::new();
        for (n, ty, v) in consts {
            if v.is_some() {
                return self.err(format!(
                    "`const {n} = …` gives a value, which is an impl's business; a trait \
                     declares `const {n}: T;` (Ch. 4 §1.7)"
                ));
            }
            declared.push((n, ty));
        }
        Ok(TraitItem {
            name,
            params,
            supertraits,
            methods,
            assoc: names,
            consts: declared,
            line,
        })
    }

    /// `impl Type { … }` or `impl Trait for Type { … }` (Ch. 4 §1.2).
    fn impl_item(&mut self) -> R<ImplItem> {
        let line = self.line();
        self.bump(); // impl
        let mut generics = self.generic_params()?;
        // `impl !Copy for T` — the one negative impl (Ch. 4 §5.1).
        let negative = self.eat_op("!");
        // `impl Name<A> for Ty` and `impl Name<A>` both begin the same way,
        // and only the `for` says which the name and arguments belonged to.
        let first = self.expect_ident()?;
        let first_args = self.generic_args()?;
        let (trait_name, trait_args, self_ty, self_args) = if self.eat_kw("for") {
            let self_ty = self.expect_ident()?;
            let self_args = self.generic_args()?;
            (Some(first), first_args, self_ty, self_args)
        } else {
            (None, Vec::new(), first, first_args)
        };
        self.where_clause(&mut generics)?;
        // `impl<T, U: From<T>> Into<U> for T` — the self type is one of the
        // impl's own parameters, so this impl is a *rule* over every type
        // satisfying the bounds rather than one type's (Ch. 4 §5.6).
        let blanket = generics.iter().any(|g| g.name() == self_ty);
        if !blanket && generics.is_empty() != self_args.is_empty() {
            return self.err(
                "an impl's type parameters and its self type's arguments must agree: \
                 `impl<T> Name<T>` (Ch. 4 §2.1)",
            );
        }
        if blanket && !self_args.is_empty() {
            return self.err(format!(
                "`{self_ty}` is a type parameter of this impl, so it takes no arguments \
                 of its own"
            ));
        }
        let (methods, assoc, consts) = self.method_block("impl")?;
        let mut chosen = Vec::new();
        for (n, v) in assoc {
            match v {
                Some(t) => chosen.push((n, t)),
                None => {
                    return self.err(format!(
                        "`type {n};` declares an associated type, which is a trait's \
                         business; an impl writes `type {n} = …;` (Ch. 4 §1.7)"
                    ));
                }
            }
        }
        let mut given = Vec::new();
        for (name, ty, v) in consts {
            match v {
                Some(value) => given.push(ConstItem {
                    name,
                    ty,
                    value,
                    line,
                }),
                None => {
                    return self.err(format!(
                        "`const {name}: T;` declares one, which is a trait's business; an \
                         impl writes `const {name}: T = …;` (Ch. 4 §1.7)"
                    ));
                }
            }
        }
        Ok(ImplItem {
            generics,
            trait_args,
            negative,
            trait_name,
            consts: given,
            assoc: chosen,
            self_args,
            self_ty,
            methods,
            line,
        })
    }

    /// The `{ type … fn … }` body shared by `trait` and `impl`.
    ///
    /// Returns the methods and the associated types: `type Item;` in a trait
    /// declares one, `type Item = t27;` in an impl chooses it (Ch. 4 §1.7).
    fn method_block(&mut self, what: &str) -> R<MethodBlock> {
        self.expect_op("{")?;
        let mut methods = Vec::new();
        let mut assoc = Vec::new();
        let mut consts = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err(format!("unterminated `{what}` body"));
            }
            if self.eat_kw("type") {
                let name = self.expect_ident()?;
                // A bound on an associated type is accepted and ignored: it
                // constrains the impl, and the impl is checked directly.
                if self.eat_op(":") {
                    loop {
                        self.expect_ident()?;
                        if !self.eat_op("+") {
                            break;
                        }
                    }
                }
                let value = if self.eat_op("=") {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect_op(";")?;
                assoc.push((name, value));
                continue;
            }
            // `const MIN: Self;` declares one, `const MIN: t27 = …;` gives
            // it a value (Ch. 4 §1.7).
            if self.eat_kw("const") {
                let name = self.expect_ident()?;
                self.expect_op(":")?;
                let ty = self.ty()?;
                let value = if self.eat_op("=") {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect_op(";")?;
                consts.push((name, ty, value));
                continue;
            }
            if !self.at_kw("fn") {
                return self.err(format!(
                    "expected `fn`, `type` or `const`, found {}; that is what a {what} \
                     body contains (Ch. 4 §1.7)",
                    self.peek()
                ));
            }
            methods.push(self.fn_item()?);
        }
        Ok((methods, assoc, consts))
    }

    /// One of §1.4's four shortened receiver forms, if that is what comes
    /// next. Restores the position when it is not, since `&mut x: T` and
    /// `&mut self` share a prefix.
    fn self_param(&mut self) -> Option<(String, Ty)> {
        let start = self.pos;
        let line = self.line();
        if self.at_kw("self") {
            self.bump();
            // `self: Buffer` is the long form Ch. 3 §1.4 writes; leave it to
            // the ordinary parameter path.
            if self.at_op(":") {
                self.pos = start;
                return None;
            }
            return Some(("self".to_string(), Ty::SelfTy(line)));
        }
        if self.eat_op("&") {
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            }
            let mutable = self.eat_kw("mut");
            if self.at_kw("self") {
                self.bump();
                return Some((
                    "self".to_string(),
                    Ty::Ref(Box::new(Ty::SelfTy(line)), mutable, line),
                ));
            }
        }
        self.pos = start;
        None
    }

    fn fn_item(&mut self) -> R<FnItem> {
        let line = self.line();
        self.bump(); // fn
        let name = self.expect_ident()?;
        let mut generics = self.generic_params()?;
        self.expect_op("(")?;
        let mut params = Vec::new();
        if !self.eat_op(")") {
            loop {
                if let Some(p) = self.self_param() {
                    params.push(p);
                    if self.eat_op(",") {
                        if self.eat_op(")") {
                            break;
                        }
                        continue;
                    }
                    self.expect_op(")")?;
                    break;
                }
                let pname = if self.eat_kw("self") {
                    "self".to_string()
                } else {
                    self.expect_ident()?
                };
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
        self.where_clause(&mut generics)?;

        // A function without a body is a declaration (§3.1) — the same rule
        // TIR §1 states for its own.
        let body = if self.eat_op(";") {
            None
        } else {
            Some(self.block()?)
        };
        Ok(FnItem {
            name,
            generics,
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
            // `[T]` is a slice — dynamically sized, legal only behind a
            // reference (Ch. 3 §5.1). `[T; N]` is an array.
            if self.eat_op("]") {
                return Ok(Ty::Slice(Box::new(elem), line));
            }
            self.expect_op(";")?;
            let n = self.expr()?;
            self.expect_op("]")?;
            return Ok(Ty::Array(Box::new(elem), Box::new(n), line));
        }
        if self.eat_op("&") {
            // A lifetime is erased before TIR (Ch. 3 §3.1), so it is parsed
            // and dropped rather than carried through the compiler.
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            }
            let mutable = self.eat_kw("mut");
            return Ok(Ty::Ref(Box::new(self.ty()?), mutable, line));
        }
        if self.at_kw("Self") {
            self.bump();
            return self.assoc_tail(Ty::SelfTy(line), line);
        }
        // `dyn Trait` (Ch. 4 §3.1).
        if self.eat_kw("dyn") {
            return Ok(Ty::Dyn(self.expect_ident()?, line));
        }
        // `impl Fn(T) -> R` in argument position (Ch. 4 §2.2, §4.3).
        if self.eat_kw("impl") {
            let name = self.expect_ident()?;
            let kind = match name.as_str() {
                "Fn" => FnKind::Fn,
                "FnMut" => FnKind::FnMut,
                "FnOnce" => FnKind::FnOnce,
                other => {
                    return self.err(format!(
                        "`impl {other}` in argument position is an anonymous type parameter \
                         (Ch. 4 §2.2), which is implemented only for the `Fn` traits; \
                         write `<T: {other}>` instead"
                    ));
                }
            };
            let params = self.type_list("(", ")")?;
            let ret = if self.eat_op("->") {
                Some(Box::new(self.ty()?))
            } else {
                None
            };
            return Ok(Ty::ImplFn(kind, params, ret, line));
        }
        // `!` — the type with no values (Ch. 1 §2). It takes no arguments
        // and has no associated types, so it is returned as it stands.
        if self.eat_op("!") {
            return Ok(Ty::Never(line));
        }
        let name = self.expect_ident()?;
        let args = self.generic_args()?;
        let base = if args.is_empty() {
            Ty::Name(name, line)
        } else {
            Ty::App(name, args, line)
        };
        self.assoc_tail(base, line)
    }

    /// `::Item` after a type, as many times as it is written (Ch. 4 §1.7).
    fn assoc_tail(&mut self, mut base: Ty, line: Line) -> R<Ty> {
        while self.eat_op("::") {
            base = Ty::Assoc(Box::new(base), self.expect_ident()?, line);
        }
        Ok(base)
    }

    /// `<T, U>` after a type name (Ch. 4 §2.1), empty when there is none.
    fn generic_args(&mut self) -> R<Vec<Ty>> {
        let mut args = Vec::new();
        if !self.at_op("<") {
            return Ok(args);
        }
        self.bump();
        if self.close_angle() {
            return Ok(args);
        }
        loop {
            // A lifetime argument is erased, like the parameter it fills.
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            } else {
                args.push(self.ty()?);
            }
            if self.eat_op(",") {
                if self.close_angle() {
                    return Ok(args);
                }
                continue;
            }
            self.expect_angle()?;
            return Ok(args);
        }
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

            // In statement position a block-shaped expression is parsed
            // *alone*: it ends where its closing brace does, and an operator
            // after it begins the next statement rather than continuing it.
            //
            // Without this rule `if c { … } (a) * 2` reads as a call of the
            // `if`'s value, and the diagnostic is about `()` not being
            // callable — which is true and useless. It is still the tail if
            // a `}` follows, so `fn f() -> t27 { if c { 1 } else { 2 } }`
            // means what it looks like.
            let e = if self.at_block_start() {
                self.primary()?
            } else {
                self.expr()?
            };
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

    /// True where a block-shaped expression begins.
    fn at_block_start(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Kw("if") | Tok::Kw("match") | Tok::Kw("loop") | Tok::Kw("while")
        ) || self.at_op("{")
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
        if self.eat_op("&") {
            let mutable = self.eat_kw("mut");
            return Ok(Expr::Borrow(Box::new(self.unary()?), mutable, line));
        }
        if self.eat_op("*") {
            return Ok(Expr::Deref(Box::new(self.unary()?), line));
        }
        // A closure (Ch. 4 §4.1). `||` here is two parameter delimiters and
        // not the logical-or of §2.1 — the re-examination of `|` that Ch. 0
        // §7 anticipated, and the whole of it.
        if self.at_op("|") || self.at_op("||") {
            return self.closure(line);
        }
        self.postfix()
    }

    /// §5.7's desugaring, written out:
    ///
    /// ```text
    /// { let mut it = e;
    ///   loop { match it.next() { Some(x) => { body } None => break, } } }
    /// ```
    ///
    /// The iterator's name contains a dot, which no Trust identifier may, so
    /// it cannot shadow or be shadowed by anything a program wrote.
    fn desugar_for(&mut self, name: String, iter: Expr, body: Block, line: Line) -> Expr {
        self.counter += 1;
        let it = format!("it.{}", self.counter);
        let path = |segs: &[&str]| Path {
            segments: segs.iter().map(|s| s.to_string()).collect(),
            targs: Vec::new(),
            line,
        };
        let next = Expr::Method(
            Box::new(Expr::Path(it.clone(), line)),
            "next".to_string(),
            Vec::new(),
            line,
        );
        let arms = vec![
            Arm {
                patterns: vec![Pattern::Aggregate(
                    path(&["Option", "Some"]),
                    vec![("0".to_string(), Pattern::Bind(name, line))],
                    line,
                )],
                guard: None,
                body: Expr::Block(body),
                line,
            },
            Arm {
                patterns: vec![Pattern::Aggregate(
                    path(&["Option", "None"]),
                    Vec::new(),
                    line,
                )],
                guard: None,
                body: Expr::Break(None, line),
                line,
            },
        ];
        Expr::Block(Block {
            stmts: vec![
                Stmt::Let {
                    mutable: true,
                    name: it,
                    ty: None,
                    value: iter,
                    line,
                },
                Stmt::Expr(Expr::Loop(
                    Block {
                        stmts: Vec::new(),
                        tail: Some(Box::new(Expr::Match(Box::new(next), arms, line))),
                        line,
                    },
                    line,
                )),
            ],
            tail: None,
            line,
        })
    }

    fn closure(&mut self, line: Line) -> R<Expr> {
        let mut params = Vec::new();
        if self.eat_op("||") {
            // no parameters
        } else {
            self.expect_op("|")?;
            while !self.eat_op("|") {
                let name = self.expect_ident()?;
                let ty = if self.eat_op(":") {
                    Some(self.ty()?)
                } else {
                    None
                };
                params.push((name, ty));
                if !self.eat_op(",") && !self.at_op("|") {
                    return self.err("expected `,` or `|` in a closure's parameters");
                }
            }
        }
        let ret = if self.eat_op("->") {
            Some(self.ty()?)
        } else {
            None
        };
        // With a written return type the body is a block, as in Rust: the
        // alternative is an ambiguity nobody enjoys.
        let body = if ret.is_some() {
            Expr::Block(self.block()?)
        } else {
            self.expr()?
        };
        Ok(Expr::Closure(params, ret, Box::new(body), line))
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
            // `e?` — propagate a failure (Ch. 5 §4.1). Postfix, so it binds
            // tighter than any operator and `a? + b?` is what it looks like.
            if self.eat_op("?") {
                e = Expr::Try(Box::new(e), line);
                continue;
            }
            // `(e)(args)` — calling something that is not a name. A path
            // followed by `(` is a call and is read in `path_expr`; what
            // reaches here is a field, an index or a parenthesized
            // expression, and the only callable thing any of them holds is a
            // closure (Ch. 4 §4.2).
            if self.at_op("(") {
                let args = self.args()?;
                e = Expr::CallExpr(Box::new(e), args, line);
                continue;
            }
            return Ok(e);
        }
    }

    /// The tail of a path expression, after its first segment.
    fn path_expr(&mut self, first: String, line: Line) -> R<Expr> {
        let mut segments = vec![first];
        let mut targs = Vec::new();
        while self.eat_op("::") {
            // `f::<T>` — the turbofish (Ch. 4 §2.3). The `::` is what tells
            // it from a comparison, which is why Rust has one too.
            if self.at_op("<") {
                targs = self.generic_args()?;
                continue;
            }
            segments.push(self.expect_ident()?);
        }
        let path = Path {
            segments,
            targs,
            line,
        };

        // `Name(args)` is a call when the name is a function and a
        // tuple-struct or variant literal when it is a type; the two are told
        // apart during lowering, where the names are known.
        if self.at_op("(") {
            let args = self.args()?;
            if path.segments.len() == 1 {
                return Ok(Expr::Call(path.segments[0].clone(), path.targs, args, line));
            }
            let fields = args
                .into_iter()
                .enumerate()
                .map(|(i, e)| (i.to_string(), e))
                .collect();
            return Ok(Expr::Aggregate(path, fields, line));
        }
        // A struct literal is ambiguous with a block in a condition position,
        // and is not permitted there without parentheses (§2.8).
        if self.at_op("{") && !self.no_struct {
            let fields = self.field_values()?;
            return Ok(Expr::Aggregate(path, fields, line));
        }
        if path.segments.len() > 1 {
            return Ok(Expr::Aggregate(path, Vec::new(), line));
        }
        Ok(Expr::Path(path.segments[0].clone(), line))
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
            Tok::CharLit(v) => {
                self.bump();
                Ok(Expr::Char(v, line))
            }
            Tok::StrLit(cs) => {
                self.bump();
                Ok(Expr::Str(cs, line))
            }
            Tok::Kw("self") => {
                self.bump();
                Ok(Expr::Path("self".to_string(), line))
            }
            // `Self::new()` and `Self { … }` are paths like any other; the
            // name is substituted away before lowering (Ch. 4 §1.2).
            Tok::Kw("Self") => {
                self.bump();
                self.path_expr("Self".to_string(), line)
            }
            // `for name in iter { … }` (Ch. 4 §5.7) — sugar, and nothing
            // more, so it is expanded here and no later pass learns it
            // existed. The desugaring uses only Ch. 0 constructs, which is
            // the point §5.7 makes about it.
            Tok::Kw("for") => {
                self.bump();
                let name = self.expect_ident()?;
                if !self.eat_kw("in") {
                    return self.err("expected `in` after the binding of a `for` loop");
                }
                let saved = self.no_struct;
                self.no_struct = true;
                let iter = self.expr()?;
                self.no_struct = saved;
                let body = self.block()?;
                Ok(self.desugar_for(name, iter, body, line))
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
                self.path_expr(name, line)
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
            Tok::CharLit(v) => {
                self.bump();
                Ok(Pattern::Char(v, line))
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
                let path = Path {
                    segments,
                    targs: Vec::new(),
                    line,
                };

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
                            targs: Vec::new(),
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
