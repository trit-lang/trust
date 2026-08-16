//! The parser (Language Ch. 0 §§2–6).
//!
//! Recursive descent, with the binary operators driven by §2.1's precedence
//! table. The two comparison levels are non-associative, so `a < b < c` and
//! `a <=> b <=> c` are syntax errors rather than surprises.

use super::ast::*;
use super::lex::{File as SourceFile, Span, SyntaxError, Tok, lex_in};

type R<T> = Result<T, SyntaxError>;

/// Parse a source file.
pub fn parse(src: &str) -> R<File> {
    parse_in(src, Span::ROOT)
}

/// Parse a source file, whose spans name `file` (Ch. 6 §1).
pub fn parse_in(src: &str, file: SourceFile) -> R<File> {
    let mut p = Parser {
        counter: 0,
        toks: lex_in(src, file)?,
        pos: 0,
        no_struct: false,
        in_macro: false,
    };
    let mut file = File::default();
    while !p.at(&Tok::Eof) {
        file.items.push(p.item()?);
    }
    Ok(file)
}

/// Parse a file, keeping going past an item that does not parse.
///
/// An editor asks about a file that is being typed into, which is a file with
/// a syntax error in it most of the time. Stopping at the first one means the
/// other twenty functions have no outline, no hover and no completion —
/// which is to say the editor goes blank exactly when it is being used.
///
/// So an item that fails is skipped, up to the next token that could start
/// one, and the rest of the file is parsed. Recovery is at **item** level and
/// nowhere finer: inside an item the parser knows too much about what it
/// expected to guess well, and half an item is worse than none.
///
/// A lexical error stops everything, because there is nothing to skip *to* —
/// an unterminated string swallows the rest of the file by definition.
pub fn parse_recovering(src: &str) -> (File, Vec<SyntaxError>) {
    parse_recovering_in(src, Span::ROOT)
}

/// Parse with recovery, whose spans name `file`.
pub fn parse_recovering_in(src: &str, file: SourceFile) -> (File, Vec<SyntaxError>) {
    let toks = match lex_in(src, file) {
        Ok(t) => t,
        Err(e) => return (File::default(), vec![e]),
    };
    let mut p = Parser {
        counter: 0,
        toks,
        pos: 0,
        no_struct: false,
        in_macro: false,
    };
    let (mut file, mut errs) = (File::default(), Vec::new());
    while !p.at(&Tok::Eof) {
        let start = p.pos;
        match p.item() {
            Ok(item) => file.items.push(item),
            Err(e) => {
                errs.push(e);
                // Always move: an item that failed on its very first token
                // would otherwise fail on it again forever.
                if p.pos == start {
                    p.bump();
                }
                p.skip_to_item();
            }
        }
    }
    (file, errs)
}

struct Parser {
    toks: Vec<(Tok, Span)>,
    pos: usize,
    /// Set while reading a macro's body, where `$` means something.
    in_macro: bool,
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

    fn span(&self) -> Span {
        self.toks[self.pos].1
    }

    /// The span of the token just read.
    fn prev(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].1
    }

    /// From where a production began to the end of what it has read.
    ///
    /// A production knows where it starts before it knows where it ends, so
    /// it notes the first token's span, parses, and asks this how wide the
    /// node turned out to be. That is the whole extent and not the token that
    /// names it, because what reads a span is something drawing a line under
    /// what is wrong (see `Expr::spanning`).
    fn since(&self, start: Span) -> Span {
        start.to(self.prev())
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

    /// Skip to where an item could begin, for recovery.
    ///
    /// Braces are counted, so a `fn` written inside the body of the item that
    /// failed is not mistaken for the next one — the failure is usually
    /// *before* the body, and its `{ … }` is still there to step over.
    fn skip_to_item(&mut self) {
        let mut depth = 0i32;
        while !self.at(&Tok::Eof) {
            match self.peek() {
                Tok::Op("{") => depth += 1,
                Tok::Op("}") => {
                    depth -= 1;
                    // The brace that closed the item that failed: what
                    // follows it starts the next one.
                    if depth <= 0 {
                        self.bump();
                        return;
                    }
                }
                Tok::Kw("fn" | "struct" | "enum" | "trait" | "impl" | "const") | Tok::Op("#")
                    if depth <= 0 =>
                {
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(SyntaxError {
            span: self.span(),
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

    /// A name, and where it was written.
    fn expect_ident_at(&mut self) -> R<(String, Span)> {
        let span = self.span();
        Ok((self.expect_ident()?, span))
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
        let start = self.span();
        Ok(self.item_inner()?.spanning(self.since(start)))
    }

    fn item_inner(&mut self) -> R<Item> {
        // Attributes attach to the item that follows (§3.4). Draft 0.1
        // defines exactly one, `repr`, taking `lang` or `linear`.
        let mut repr = Repr::Lang;
        let mut derives: Vec<String> = Vec::new();
        while self.at_op("#") {
            let span = self.span();
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
                                span,
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
                            span,
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
                        span,
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

        // Ch. 6 §2.2: one degree of visibility, written before the item.
        let public = self.eat_kw("pub");
        if self.at_kw("mod") {
            return Ok(Item::Mod(self.mod_item(public)?));
        }
        if self.at_kw("use") {
            if public {
                return self.err(
                    "`pub use` is a re-export, which Ch. 6 §2.4 declines: a name is reached \
                     by the path to where it was defined",
                );
            }
            return Ok(Item::Use(self.use_item()?));
        }
        if self.at_kw("macro") {
            return Ok(Item::Macro(self.macro_item(public)?));
        }
        if self.at_kw("type") {
            return Ok(Item::Alias(self.alias_item(public)?));
        }
        if self.at_kw("fn") {
            return Ok(Item::Fn(self.fn_item(public)?));
        }
        if self.at_kw("const") {
            return Ok(Item::Const(self.const_item(public)?));
        }
        if self.at_kw("struct") {
            return Ok(Item::Struct(self.struct_item(repr, derives, public)?));
        }
        if self.at_kw("enum") {
            return Ok(Item::Enum(self.enum_item(repr, derives, public)?));
        }
        if !derives.is_empty() {
            return self.err("`derive` attaches to a struct or an enum (Ch. 4 §6)");
        }
        if self.at_kw("trait") {
            return Ok(Item::Trait(self.trait_item(public)?));
        }
        if self.at_kw("impl") {
            if public {
                return self.err(
                    "an `impl` takes no `pub`: it is as visible as the more private of the \
                     type and the trait (Ch. 6 §5)",
                );
            }
            return Ok(Item::Impl(self.impl_item()?));
        }
        self.err(format!("expected an item, found {}", self.peek()))
    }

    /// `macro name($a, $($x),*) { body }` (Ch. 7 §6).
    fn macro_item(&mut self, public: bool) -> R<MacroItem> {
        let span = self.span();
        self.bump(); // macro
        let (name, name_span) = self.expect_ident_at()?;
        self.expect_op("(")?;
        let (mut params, mut rest) = (Vec::new(), None);
        while !self.eat_op(")") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated macro parameter list");
            }
            if rest.is_some() {
                return self.err("a macro has at most one repetition and it is last (Ch. 7 §2)");
            }
            self.expect_op("$")?;
            // `$($x),*` — the repetition.
            if self.eat_op("(") {
                self.expect_op("$")?;
                let (r, _) = self.expect_ident_at()?;
                self.expect_op(")")?;
                self.expect_op(",")?;
                self.expect_op("*")?;
                rest = Some(r);
            } else {
                let (p, at) = self.expect_ident_at()?;
                if params.contains(&p) {
                    return Err(SyntaxError {
                        span: at,
                        message: format!("`${p}` is a parameter of this macro twice (Ch. 7 §2)"),
                    });
                }
                params.push(p);
            }
            if !self.eat_op(",") && !self.at_op(")") {
                return self.err("expected `,` or `)` in a macro's parameters");
            }
        }
        if rest.as_ref().is_some_and(|r| params.contains(r)) {
            return self.err("a macro's repetition may not repeat a fixed parameter's name");
        }
        let was = std::mem::replace(&mut self.in_macro, true);
        let body = self.block();
        self.in_macro = was;
        Ok(MacroItem {
            name,
            name_span,
            public,
            params,
            rest,
            body: body?,
            span,
        })
    }

    /// `type Name = T;` — another name for a type (Ch. 0 §3.6).
    fn alias_item(&mut self, public: bool) -> R<AliasItem> {
        let span = self.span();
        self.bump(); // type
        let (name, name_span) = self.expect_ident_at()?;
        if self.at_op("<") {
            return self.err(
                "a type alias takes no parameters in draft 0.1 (Ch. 0 §3.6); \
                 `type Pair<T> = (T, T)` is reserved",
            );
        }
        self.expect_op("=")?;
        let ty = self.ty()?;
        self.expect_op(";")?;
        Ok(AliasItem {
            name,
            name_span,
            public,
            ty,
            span,
        })
    }

    /// `mod name;` — a module, which is a file (Ch. 6 §1.2).
    fn mod_item(&mut self, public: bool) -> R<ModItem> {
        let span = self.span();
        self.bump(); // mod
        let (name, name_span) = self.expect_ident_at()?;
        if self.at_op("{") {
            return self.err(
                "a module is a file (Ch. 6 §1): there is no `mod name { … }`, because \
                 grouping inside a file is what an `impl` block and a heading comment are for",
            );
        }
        self.expect_op(";")?;
        Ok(ModItem {
            name,
            name_span,
            public,
            span,
        })
    }

    /// `use a::b::c;` (Ch. 6 §3.2).
    fn use_item(&mut self) -> R<UseItem> {
        let span = self.span();
        self.bump(); // use
        let mut segments = Vec::new();
        let (first, mut name_span) = self.expect_ident_at()?;
        segments.push(first);
        while self.eat_op("::") {
            if self.at_op("{") || self.at_op("*") {
                return self.err(
                    "a `use` names one thing (Ch. 6 §3.2): there is no `use a::{b, c}` and \
                     no `use a::*`",
                );
            }
            let (seg, at) = self.expect_ident_at()?;
            segments.push(seg);
            name_span = at;
        }
        if self.at_kw("as") {
            return self.err("a `use` does not rename (Ch. 6 §3.2)");
        }
        self.expect_op(";")?;
        Ok(UseItem {
            segments,
            name_span,
            span,
        })
    }

    /// `struct Name { … }`, `struct Name(…);`, `struct Name;` (§3.3).
    fn struct_item(&mut self, repr: Repr, derives: Vec<String>, public: bool) -> R<StructItem> {
        let span = self.span();
        self.bump(); // struct
        let (name, name_span) = self.expect_ident_at()?;
        let generics = self.generic_params()?;
        let fields = if self.at_op("{") {
            self.named_fields()?
        } else if self.at_op("(") {
            let tys = self.type_list("(", ")")?;
            self.expect_op(";")?;
            tys.into_iter()
                .enumerate()
                .map(|(i, t)| Named::new(i.to_string(), t))
                .collect()
        } else {
            self.expect_op(";")?;
            Vec::new()
        };
        Ok(StructItem {
            public,
            name,
            name_span,
            generics,
            derives,
            repr,
            fields,
            span,
        })
    }

    fn enum_item(&mut self, repr: Repr, derives: Vec<String>, public: bool) -> R<EnumItem> {
        let span = self.span();
        self.bump(); // enum
        let (name, name_span) = self.expect_ident_at()?;
        let generics = self.generic_params()?;
        self.expect_op("{")?;
        let mut variants = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated enum");
            }
            let vline = self.span();
            let (vname, vname_span) = self.expect_ident_at()?;
            let fields = if self.at_op("{") {
                self.named_fields()?
            } else if self.at_op("(") {
                self.type_list("(", ")")?
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| Named::new(i.to_string(), t))
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
                name_span: vname_span,
                fields,
                discriminant,
                span: vline,
            });
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between variants");
            }
        }
        Ok(EnumItem {
            public,
            name,
            name_span,
            generics,
            derives,
            repr,
            variants,
            span,
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
        // `Fn(A) -> B` is the same bound `impl Fn(A) -> B` gives an anonymous
        // parameter, written where the parameter has a name (Ch. 4 §4.3).
        // It is stored as arguments and an `Output` binding — which is what
        // the bound would be if `Fn` were an ordinary trait — so the bound
        // machinery needs no case of its own until the desugaring.
        if matches!(name.as_str(), "Fn" | "FnMut" | "FnOnce") && self.at_op("(") {
            let args = self.type_list("(", ")")?;
            let mut assoc = Vec::new();
            if self.eat_op("->") {
                assoc.push(("Output".to_string(), self.ty()?));
            }
            return Ok(Bound { name, args, assoc });
        }
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
    /// The `where` clause, which carries two different things.
    ///
    /// `T: Ord` constrains a *type*; `n <= a.len()` constrains a *value*, is
    /// checked once when the function is entered, and is a fact its body may
    /// use (Ch. 4 §2.8). They are told apart by the `:` — a value predicate
    /// is an expression and has none where a bound has one.
    fn where_clause(&mut self, generics: &mut [GenericParam]) -> R<Vec<Expr>> {
        let mut requires = Vec::new();
        if !self.eat_kw("where") {
            return Ok(requires);
        }
        loop {
            // A predicate rather than a bound: parse it as what it is.
            let save = self.pos;
            if !matches!(self.peek(), Tok::Lifetime(_)) {
                let looks_like_a_bound = self.expect_ident().is_ok() && self.at_op(":");
                self.pos = save;
                if !looks_like_a_bound {
                    // A predicate is followed by the body's `{`, so a struct
                    // literal is off here for the reason it is off in an
                    // `if` condition (Ch. 0 §5.3).
                    let saved = self.no_struct;
                    self.no_struct = true;
                    let pred = self.expr();
                    self.no_struct = saved;
                    requires.push(pred?);
                    if self.eat_op(",") {
                        continue;
                    }
                    return Ok(requires);
                }
            }
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
                return Ok(requires);
            }
            if self.at_op("{") {
                return Ok(requires);
            }
        }
    }

    fn named_fields(&mut self) -> R<Vec<Named>> {
        self.expect_op("{")?;
        let mut fields = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated field list");
            }
            let public = self.eat_kw("pub");
            let (name, name_span) = self.expect_ident_at()?;
            self.expect_op(":")?;
            fields.push(Named {
                public,
                name,
                name_span,
                ty: self.ty()?,
            });
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
    fn trait_item(&mut self, public: bool) -> R<TraitItem> {
        let span = self.span();
        self.bump(); // trait
        let (name, name_span) = self.expect_ident_at()?;
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
            public,
            name,
            name_span,
            params,
            supertraits,
            methods,
            assoc: names,
            consts: declared,
            span,
        })
    }

    /// `impl Type { … }` or `impl Trait for Type { … }` (Ch. 4 §1.2).
    fn impl_item(&mut self) -> R<ImplItem> {
        let span = self.span();
        self.bump(); // impl
        let mut generics = self.generic_params()?;
        // `impl !Copy for T` — the one negative impl (Ch. 4 §5.1).
        let negative = self.eat_op("!");
        // `impl Name<A> for Ty` and `impl Name<A>` both begin the same way,
        // and only the `for` says which the name and arguments belonged to.
        let first = self.expect_ident()?;
        let first_args = self.generic_args()?;
        let mut self_ref = false;
        let mut self_mut = false;
        let (trait_name, trait_args, self_ty, self_args) = if self.eat_kw("for") {
            // `impl Trait for &Type` — the reference is the implementing
            // type, so `Self` is `&Type` (Ch. 4 §2.1).
            self_ref = self.eat_op("&");
            self_mut = self_ref && self.eat_kw("mut");
            let self_ty = self.expect_ident()?;
            let self_args = self.generic_args()?;
            (Some(first), first_args, self_ty, self_args)
        } else {
            (None, Vec::new(), first, first_args)
        };
        let _ = self.where_clause(&mut generics)?;
        // `impl<T, U: From<T>> Into<U> for T` — the self type is one of the
        // impl's own parameters, so this impl is a *rule* over every type
        // satisfying the bounds rather than one type's (Ch. 4 §5.6).
        let blanket = generics.iter().any(|g| g.name() == self_ty);
        // An impl with parameters must name them in its self type, or they
        // are parameters nothing determines. The other direction is fine:
        // `impl Vec<char>` has no parameters because its self type has no
        // holes — it is an impl for *one* instantiation (Ch. 4 §2.1).
        if !blanket && !generics.is_empty() && self_args.is_empty() {
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
                // A trait's default value for an associated constant: the
                // name was read as part of the declaration list, and the
                // whole clause is as narrow a place as there is to point at.
                Some(value) => given.push(ConstItem {
                    public: true,
                    name,
                    name_span: span,
                    ty,
                    value,
                    span,
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
            self_ref,
            self_mut,
            methods,
            span,
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
            methods.push(self.fn_item(true)?);
        }
        Ok((methods, assoc, consts))
    }

    /// One of §1.4's four shortened receiver forms, if that is what comes
    /// next. Restores the position when it is not, since `&mut x: T` and
    /// `&mut self` share a prefix.
    fn self_param(&mut self) -> Option<Named> {
        let start = self.pos;
        let span = self.span();
        if self.at_kw("self") {
            self.bump();
            // `self: Buffer` is the long form Ch. 3 §1.4 writes; leave it to
            // the ordinary parameter path.
            if self.at_op(":") {
                self.pos = start;
                return None;
            }
            return Some(Named {
                public: false,
                name: "self".to_string(),
                name_span: span,
                ty: Ty::SelfTy(span),
            });
        }
        if self.eat_op("&") {
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            }
            let mutable = self.eat_kw("mut");
            if self.at_kw("self") {
                self.bump();
                return Some(Named {
                    public: false,
                    name: "self".to_string(),
                    name_span: span,
                    ty: Ty::Ref(Box::new(Ty::SelfTy(span)), mutable, span),
                });
            }
        }
        self.pos = start;
        None
    }

    fn fn_item(&mut self, public: bool) -> R<FnItem> {
        let span = self.span();
        self.bump(); // fn
        let (name, name_span) = self.expect_ident_at()?;
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
                let pspan = self.span();
                let pname = if self.eat_kw("self") {
                    "self".to_string()
                } else {
                    self.expect_ident()?
                };
                self.expect_op(":")?;
                params.push(Named {
                    public: false,
                    name: pname,
                    name_span: pspan,
                    ty: self.ty()?,
                });
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
        let requires = self.where_clause(&mut generics)?;

        // A function without a body is a declaration (§3.1) — the same rule
        // TIR §1 states for its own.
        let body = if self.eat_op(";") {
            None
        } else {
            Some(self.block()?)
        };
        Ok(FnItem {
            public,
            name,
            name_span,
            generics,
            params,
            ret,
            body,
            requires,
            span,
        })
    }

    fn const_item(&mut self, public: bool) -> R<ConstItem> {
        let span = self.span();
        self.bump(); // const
        let (name, name_span) = self.expect_ident_at()?;
        self.expect_op(":")?;
        let ty = self.ty()?;
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;
        Ok(ConstItem {
            public,
            name,
            name_span,
            ty,
            value,
            span,
        })
    }

    fn ty(&mut self) -> R<Ty> {
        let start = self.span();
        Ok(self.ty_inner()?.spanning(self.since(start)))
    }

    fn ty_inner(&mut self) -> R<Ty> {
        let span = self.span();
        if self.at_op("(") {
            let tys = self.type_list("(", ")")?;
            return Ok(if tys.is_empty() {
                Ty::Unit(span)
            } else {
                Ty::Tuple(tys, span)
            });
        }
        if self.eat_op("[") {
            let elem = self.ty()?;
            // `[T]` is a slice — dynamically sized, legal only behind a
            // reference (Ch. 3 §5.1). `[T; N]` is an array.
            if self.eat_op("]") {
                return Ok(Ty::Slice(Box::new(elem), span));
            }
            self.expect_op(";")?;
            let n = self.expr()?;
            self.expect_op("]")?;
            return Ok(Ty::Array(Box::new(elem), Box::new(n), span));
        }
        if self.eat_op("&") {
            // A lifetime is erased before TIR (Ch. 3 §3.1), so it is parsed
            // and dropped rather than carried through the compiler.
            if let Tok::Lifetime(_) = self.peek() {
                self.bump();
            }
            let mutable = self.eat_kw("mut");
            return Ok(Ty::Ref(Box::new(self.ty()?), mutable, span));
        }
        if self.at_kw("Self") {
            self.bump();
            return self.assoc_tail(Ty::SelfTy(span), span);
        }
        // `dyn Trait` (Ch. 4 §3.1).
        if self.eat_kw("dyn") {
            return Ok(Ty::Dyn(self.expect_ident()?, span));
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
            return Ok(Ty::ImplFn(kind, params, ret, span));
        }
        // `!` — the type with no values (Ch. 1 §2). It takes no arguments
        // and has no associated types, so it is returned as it stands.
        if self.eat_op("!") {
            return Ok(Ty::Never(span));
        }
        let name = self.expect_ident()?;
        let args = self.generic_args()?;
        let base = if args.is_empty() {
            Ty::Name(name, span)
        } else {
            Ty::App(name, args, span)
        };
        self.assoc_tail(base, span)
    }

    /// `::Item` after a type, as many times as it is written (Ch. 4 §1.7).
    fn assoc_tail(&mut self, mut base: Ty, span: Span) -> R<Ty> {
        while self.eat_op("::") {
            base = Ty::Assoc(Box::new(base), self.expect_ident()?, span);
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
        let start = self.span();
        let mut b = self.block_inner()?;
        b.span = self.since(start);
        Ok(b)
    }

    fn block_inner(&mut self) -> R<Block> {
        let span = self.span();
        self.expect_op("{")?;
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated block");
            }
            if self.at_kw("let") {
                stmts.extend(self.let_stmt()?);
                continue;
            }
            if self.eat_op(";") {
                continue;
            }
            // Ch. 6 §1.2: a `mod` is an item and appears at the top level of
            // a module. A `use` likewise (§3.2). Neither is an expression,
            // and a block is where expressions go.
            if self.at_kw("mod") || self.at_kw("use") {
                let what = if self.at_kw("mod") { "mod" } else { "use" };
                return self.err(format!(
                    "a `{what}` is an item and belongs at the top level of a module, not \
                     inside a function (Ch. 6 §§1.2, 3.2)"
                ));
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
        Ok(Block { stmts, tail, span })
    }

    /// True where a block-shaped expression begins.
    fn at_block_start(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Kw("if") | Tok::Kw("match") | Tok::Kw("loop") | Tok::Kw("while")
        ) || self.at_op("{")
    }

    /// `let`, which binds either one name or a tuple of them.
    ///
    /// The tuple form is sugar and is expanded here: it becomes a hidden
    /// binding for the whole tuple and one `let` per element reading a field
    /// of it. Nothing below the parser learns a new shape, and the ownership
    /// rules need no case of their own — moving out of `#t.0` leaves `#t.1`
    /// usable, which Ch. 3 §1.3 already says.
    fn let_stmt(&mut self) -> R<Vec<Stmt>> {
        let span = self.span();
        self.bump(); // let
        if self.at_op("(") {
            return self.let_tuple(span);
        }
        let mutable = self.eat_kw("mut");
        let (name, name_span) = self.expect_ident_at()?;
        let ty = if self.eat_op(":") {
            Some(self.ty()?)
        } else {
            None
        };
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;
        Ok(vec![Stmt::Let {
            mutable,
            name,
            name_span,
            ty,
            value,
            span,
        }])
    }

    fn let_tuple(&mut self, span: Span) -> R<Vec<Stmt>> {
        self.expect_op("(")?;
        let mut names = Vec::new();
        while !self.eat_op(")") {
            let mutable = self.eat_kw("mut");
            let (n, at) = self.expect_ident_at()?;
            names.push((mutable, n, at));
            if !self.eat_op(",") && !self.at_op(")") {
                return self.err("expected `,` or `)` in a tuple binding");
            }
        }
        if names.is_empty() {
            return self.err("a tuple binding names nothing");
        }
        let ty = if self.eat_op(":") {
            Some(self.ty()?)
        } else {
            None
        };
        self.expect_op("=")?;
        let value = self.expr()?;
        self.expect_op(";")?;

        // `#` is not an identifier character, so the whole-tuple binding
        // cannot collide with anything a program can write.
        self.counter += 1;
        let whole = format!("#t{}", self.counter);
        let mut out = vec![Stmt::Let {
            mutable: false,
            // The whole-tuple binding is the compiler's, not the file's, so
            // it points at the `let` that stands for it.
            name: whole.clone(),
            name_span: span,
            ty,
            value,
            span,
        }];
        for (i, (mutable, name, name_span)) in names.into_iter().enumerate() {
            out.push(Stmt::Let {
                mutable,
                name,
                name_span,
                ty: None,
                value: Expr::Field(
                    Box::new(Expr::Path(whole.clone(), span)),
                    i.to_string(),
                    span,
                ),
                span,
            });
        }
        Ok(out)
    }

    // ------------------------------------------------------- expressions

    /// The whole expression grammar, loosest first: assignment.
    pub fn expr(&mut self) -> R<Expr> {
        let span = self.span();
        let lhs = self.range()?;
        if let Tok::Op(op) = self.peek()
            && ASSIGNMENTS.contains(op)
        {
            let op = *op;
            self.bump();
            let rhs = self.expr()?; // right-associative
            return Ok(
                Expr::Assign(op, Box::new(lhs), Box::new(rhs), span).spanning(self.since(span))
            );
        }
        Ok(lhs)
    }

    /// The left-associative levels, plus the two non-associative comparison
    /// levels wedged between `&&` and the shifts.
    /// `a..b` — the half-open range, and the loosest thing that is not an
    /// assignment (Ch. 0 §5.6).
    ///
    /// It is sugar: `Range { start: a, end: b }`, so the type is the
    /// library's and the loop is `for` over an iterator like any other.
    /// `..=` is reserved — an inclusive range cannot express an empty one
    /// and cannot reach the top of its type, and neither is decided here.
    fn range(&mut self) -> R<Expr> {
        let span = self.span();
        let lhs = self.binary(0)?;
        if self.at_op("..=") {
            return self.err(
                "`..=` is reserved; write `a..b + 1`, or `a..b` if the end was meant \
                 to be excluded (Ch. 0 §5.6)",
            );
        }
        if !self.eat_op("..") {
            return Ok(lhs);
        }
        let rhs = self.binary(0)?;
        Ok(Expr::Aggregate(
            Path {
                segments: vec!["Range".to_string()],
                targs: Vec::new(),
                span,
            },
            vec![("start".to_string(), lhs), ("end".to_string(), rhs)],
            span,
        )
        .spanning(self.since(span)))
    }

    fn binary(&mut self, level: usize) -> R<Expr> {
        if level == COMPARE_LEVEL {
            return self.comparison();
        }
        if level >= LEVELS.len() {
            return self.cast();
        }
        let ops = LEVELS[level];
        let start = self.span();
        let mut lhs = self.binary(level + 1)?;
        loop {
            let Tok::Op(op) = self.peek() else {
                return Ok(lhs);
            };
            if !ops.contains(op) {
                return Ok(lhs);
            }
            let (op, span) = (*op, self.span());
            self.bump();
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs), span).spanning(self.since(start));
        }
    }

    /// `==` and friends, then `<=>`. Both are non-associative (§2.1).
    fn comparison(&mut self) -> R<Expr> {
        let start = self.span();
        let lhs = self.spaceship()?;
        let Tok::Op(op) = self.peek() else {
            return Ok(lhs);
        };
        if !COMPARISONS.contains(op) {
            return Ok(lhs);
        }
        let (op, span) = (*op, self.span());
        self.bump();
        let rhs = self.spaceship()?;
        if let Tok::Op(next) = self.peek()
            && COMPARISONS.contains(next)
        {
            return self.err("comparison operators do not chain: parenthesize");
        }
        Ok(Expr::Binary(op, Box::new(lhs), Box::new(rhs), span).spanning(self.since(start)))
    }

    fn spaceship(&mut self) -> R<Expr> {
        let start = self.span();
        let lhs = self.binary(COMPARE_LEVEL + 1)?;
        if !self.at_op("<=>") {
            return Ok(lhs);
        }
        let span = self.span();
        self.bump();
        let rhs = self.binary(COMPARE_LEVEL + 1)?;
        if self.at_op("<=>") {
            return self.err("`<=>` does not chain: parenthesize");
        }
        Ok(Expr::Binary("<=>", Box::new(lhs), Box::new(rhs), span).spanning(self.since(start)))
    }

    fn cast(&mut self) -> R<Expr> {
        let start = self.span();
        let mut e = self.unary()?;
        while self.at_kw("as") {
            let span = self.span();
            self.bump();
            e = Expr::Cast(Box::new(e), self.ty()?, span).spanning(self.since(start));
        }
        Ok(e)
    }

    fn unary(&mut self) -> R<Expr> {
        let span = self.span();
        for op in ["-", "!"] {
            if self.at_op(op) {
                self.bump();
                let e = Expr::Unary(op, Box::new(self.unary()?), span);
                return Ok(e.spanning(self.since(span)));
            }
        }
        if self.eat_op("&") {
            let mutable = self.eat_kw("mut");
            let e = Expr::Borrow(Box::new(self.unary()?), mutable, span);
            return Ok(e.spanning(self.since(span)));
        }
        if self.eat_op("*") {
            let e = Expr::Deref(Box::new(self.unary()?), span);
            return Ok(e.spanning(self.since(span)));
        }
        // A closure (Ch. 4 §4.1). `||` here is two parameter delimiters and
        // not the logical-or of §2.1 — the re-examination of `|` that Ch. 0
        // §7 anticipated, and the whole of it.
        if self.at_op("|") || self.at_op("||") {
            return self.closure(span);
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
    fn desugar_for(
        &mut self,
        name: String,
        name_span: Span,
        iter: Expr,
        body: Block,
        span: Span,
    ) -> Expr {
        self.counter += 1;
        let it = format!("it.{}", self.counter);
        let path = |segs: &[&str]| Path {
            segments: segs.iter().map(|s| s.to_string()).collect(),
            targs: Vec::new(),
            span,
        };
        let next = Expr::Method(
            Box::new(Expr::Path(it.clone(), span)),
            "next".to_string(),
            span,
            Vec::new(),
            span,
        );
        // The arm reaches to the end of the body, because that is how far
        // the loop's binding is in scope — a desugaring that collapsed it to
        // the `for` keyword would put the name nowhere.
        let arm_span = span.to(body.span);
        let arms = vec![
            Arm {
                patterns: vec![Pattern::Aggregate(
                    path(&["Option", "Some"]),
                    // The name is the file's, not the desugaring's, so it
                    // keeps the place it was written.
                    vec![("0".to_string(), Pattern::Bind(name, name_span))],
                    span,
                )],
                guard: None,
                body: Expr::Block(body),
                span: arm_span,
            },
            Arm {
                patterns: vec![Pattern::Aggregate(
                    path(&["Option", "None"]),
                    Vec::new(),
                    span,
                )],
                guard: None,
                body: Expr::Break(None, span),
                span: arm_span,
            },
        ];
        Expr::Block(Block {
            stmts: vec![
                Stmt::Let {
                    mutable: true,
                    name: it,
                    name_span: span,
                    ty: None,
                    // Ch. 4 §5.7: the loop's expression is turned into an
                    // iterator first, which is what lets `for c in s` walk a
                    // string and not only something that is already one.
                    value: Expr::Method(
                        Box::new(iter),
                        "into_iter".to_string(),
                        span,
                        Vec::new(),
                        span,
                    ),
                    span,
                },
                Stmt::Expr(Expr::Loop(
                    Block {
                        stmts: Vec::new(),
                        tail: Some(Box::new(Expr::Match(Box::new(next), arms, span))),
                        span,
                    },
                    span,
                )),
            ],
            tail: None,
            span,
        })
    }

    fn closure(&mut self, span: Span) -> R<Expr> {
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
        Ok(Expr::Closure(params, ret, Box::new(body), span).spanning(self.since(span)))
    }

    fn postfix(&mut self) -> R<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            let span = self.span();
            if self.eat_op(".") {
                // `x.0` is a tuple index; `x.f` is a field; `x.f(…)` a method.
                if let Tok::Int(v) = self.peek().clone() {
                    self.bump();
                    let index = v.to_i128().unwrap_or(-1);
                    e = Expr::Field(Box::new(e), index.to_string(), span)
                        .spanning(self.since(start));
                    continue;
                }
                let (name, at) = self.expect_ident_at()?;
                if self.at_op("(") {
                    let args = self.args()?;
                    e = Expr::Method(Box::new(e), name, at, args, span).spanning(self.since(start));
                } else {
                    e = Expr::Field(Box::new(e), name, span).spanning(self.since(start));
                }
                continue;
            }
            if self.at_op("[") {
                self.bump();
                let index = self.expr()?;
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), Box::new(index), span).spanning(self.since(start));
                continue;
            }
            // `e?` — propagate a failure (Ch. 5 §4.1). Postfix, so it binds
            // tighter than any operator and `a? + b?` is what it looks like.
            if self.eat_op("?") {
                e = Expr::Try(Box::new(e), span).spanning(self.since(start));
                continue;
            }
            // `(e)(args)` — calling something that is not a name. A path
            // followed by `(` is a call and is read in `path_expr`; what
            // reaches here is a field, an index or a parenthesized
            // expression, and the only callable thing any of them holds is a
            // closure (Ch. 4 §4.2).
            if self.at_op("(") {
                let args = self.args()?;
                e = Expr::CallExpr(Box::new(e), args, span).spanning(self.since(start));
                continue;
            }
            return Ok(e);
        }
    }

    /// The tail of a path expression, after its first segment.
    fn path_expr(&mut self, first: String, span: Span) -> R<Expr> {
        Ok(self
            .path_expr_inner(first, span)?
            .spanning(self.since(span)))
    }

    fn path_expr_inner(&mut self, first: String, span: Span) -> R<Expr> {
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
            span,
        };

        // `Name(args)` is a call when the name is a function and a
        // tuple-struct or variant literal when it is a type; the two are told
        // apart during lowering, where the names are known.
        if self.at_op("(") {
            let args = self.args()?;
            if path.segments.len() == 1 {
                return Ok(Expr::Call(
                    path.segments[0].clone(),
                    path.span,
                    path.targs,
                    args,
                    span,
                ));
            }
            let fields = args
                .into_iter()
                .enumerate()
                .map(|(i, e)| (i.to_string(), e))
                .collect();
            return Ok(Expr::Aggregate(path, fields, span));
        }
        // A struct literal is ambiguous with a block in a condition position,
        // and is not permitted there without parentheses (§2.8).
        if self.at_op("{") && !self.no_struct {
            let fields = self.field_values()?;
            return Ok(Expr::Aggregate(path, fields, span));
        }
        if path.segments.len() > 1 {
            return Ok(Expr::Aggregate(path, Vec::new(), span));
        }
        Ok(Expr::Path(path.segments[0].clone(), span))
    }

    /// One statement inside a `$( … )*` group, which is a body like any
    /// other and so may hold a `let` or an expression.
    fn repeat_stmt(&mut self) -> R<Vec<Stmt>> {
        if self.at_kw("let") {
            return self.let_stmt();
        }
        let e = self.expr()?;
        let _ = self.eat_op(";");
        Ok(vec![Stmt::Expr(e)])
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

    /// A primary expression, widened to the whole of what it turned out to
    /// cover: the productions below build their node from the token that
    /// opened it, and none of them can know where it closes.
    fn primary(&mut self) -> R<Expr> {
        let start = self.span();
        Ok(self.primary_inner()?.spanning(self.since(start)))
    }

    fn primary_inner(&mut self) -> R<Expr> {
        // `$name` and `$( … )*` are only a macro body's (Ch. 7 §6).
        if self.at_op("$") {
            let span = self.span();
            if !self.in_macro {
                return self.err("`$` appears only in a macro (Ch. 7 §6)");
            }
            self.bump();
            if self.eat_op("(") {
                let mut stmts = Vec::new();
                while !self.eat_op(")") {
                    if self.at(&Tok::Eof) {
                        return self.err("unterminated `$( … )*`");
                    }
                    stmts.extend(self.repeat_stmt()?);
                }
                self.expect_op("*")?;
                return Ok(Expr::MacroRepeat(stmts, span));
            }
            let (name, _) = self.expect_ident_at()?;
            return Ok(Expr::MacroParam(name, span));
        }

        let span = self.span();
        match self.peek().clone() {
            Tok::Int(v) => {
                self.bump();
                Ok(Expr::Int(v, span))
            }
            Tok::TritLit(t) => {
                self.bump();
                Ok(Expr::Trit(t, span))
            }
            Tok::CharLit(v) => {
                self.bump();
                Ok(Expr::Char(v, span))
            }
            Tok::StrLit(cs) => {
                self.bump();
                Ok(Expr::Str(cs, span))
            }
            Tok::Kw("self") => {
                self.bump();
                Ok(Expr::Path("self".to_string(), span))
            }
            // `Self::new()` and `Self { … }` are paths like any other; the
            // name is substituted away before lowering (Ch. 4 §1.2).
            Tok::Kw("Self") => {
                self.bump();
                self.path_expr("Self".to_string(), span)
            }
            // `for name in iter { … }` (Ch. 4 §5.7) — sugar, and nothing
            // more, so it is expanded here and no later pass learns it
            // existed. The desugaring uses only Ch. 0 constructs, which is
            // the point §5.7 makes about it.
            Tok::Kw("for") => {
                self.bump();
                let (name, name_span) = self.expect_ident_at()?;
                if !self.eat_kw("in") {
                    return self.err("expected `in` after the binding of a `for` loop");
                }
                let saved = self.no_struct;
                self.no_struct = true;
                let iter = self.expr()?;
                self.no_struct = saved;
                let body = self.block()?;
                Ok(self.desugar_for(name, name_span, iter, body, span))
            }
            Tok::Kw("true") => {
                self.bump();
                Ok(Expr::Bool(true, span))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Expr::Bool(false, span))
            }
            Tok::Ident(name) => {
                self.bump();
                // `name!(args)` is a macro invocation (Ch. 7 §6). It is read
                // here rather than in `postfix` because `!` is prefix
                // negation everywhere else, and the two never meet: a `!`
                // *after* a name is nothing else in this language.
                if self.at_op("!") && matches!(self.peek_at(1), Tok::Op("(")) {
                    self.bump();
                    let args = self.args()?;
                    return Ok(Expr::MacroCall(name, args, span));
                }
                self.path_expr(name, span)
            }
            Tok::Op("(") => {
                self.bump();
                // A struct literal is legal again inside parentheses.
                let outer = std::mem::replace(&mut self.no_struct, false);
                if self.eat_op(")") {
                    self.no_struct = outer;
                    return Ok(Expr::Unit(span));
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
                    Expr::Tuple(items, span)
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
                    return Ok(Expr::Array(Vec::new(), span));
                }
                let first = self.expr()?;
                if self.eat_op(";") {
                    let count = self.expr()?;
                    self.expect_op("]")?;
                    return Ok(Expr::Repeat(Box::new(first), Box::new(count), span));
                }
                let mut items = vec![first];
                while self.eat_op(",") {
                    if self.at_op("]") {
                        break;
                    }
                    items.push(self.expr()?);
                }
                self.expect_op("]")?;
                Ok(Expr::Array(items, span))
            }
            Tok::Op("{") => Ok(Expr::Block(self.block()?)),
            Tok::Kw("if") => self.if_expr(),
            Tok::Kw("match") => self.match_expr(),
            Tok::Kw("loop") => {
                self.bump();
                Ok(Expr::Loop(self.block()?, span))
            }
            Tok::Kw("while") => {
                self.bump();
                let cond = self.no_struct_expr()?;
                Ok(Expr::While(Box::new(cond), self.block()?, span))
            }
            Tok::Kw("break") => {
                self.bump();
                let v = self.optional_value()?;
                Ok(Expr::Break(v, span))
            }
            Tok::Kw("continue") => {
                self.bump();
                Ok(Expr::Continue(span))
            }
            Tok::Kw("return") => {
                self.bump();
                let v = self.optional_value()?;
                Ok(Expr::Return(v, span))
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
            let span = self.span();
            let name = self.expect_ident()?;
            let value = if self.eat_op(":") {
                self.expr()?
            } else {
                Expr::Path(name.clone(), span)
            };
            fields.push((name, value));
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between fields");
            }
        }
        Ok(fields)
    }

    fn if_expr(&mut self) -> R<Expr> {
        let start = self.span();
        Ok(self.if_expr_inner()?.spanning(self.since(start)))
    }

    fn if_expr_inner(&mut self) -> R<Expr> {
        let span = self.span();
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
        Ok(Expr::If(Box::new(cond), then, els, span))
    }

    fn match_expr(&mut self) -> R<Expr> {
        let start = self.span();
        Ok(self.match_expr_inner()?.spanning(self.since(start)))
    }

    fn match_expr_inner(&mut self) -> R<Expr> {
        let span = self.span();
        self.bump(); // match
        let scrutinee = self.no_struct_expr()?;
        self.expect_op("{")?;
        let mut arms = Vec::new();
        while !self.eat_op("}") {
            if self.at(&Tok::Eof) {
                return self.err("unterminated `match`");
            }
            let arm_line = self.span();
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
                span: arm_line,
            });
            if !self.eat_op(",") && needs_comma && !self.at_op("}") {
                return self.err("expected `,` between match arms");
            }
        }
        Ok(Expr::Match(Box::new(scrutinee), arms, span))
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
            let span = self.span();
            let name = self.expect_ident()?;
            let pat = if self.eat_op(":") {
                self.pattern()?
            } else {
                Pattern::Bind(name.clone(), span)
            };
            out.push((name, pat));
            if !self.eat_op(",") && !self.at_op("}") {
                return self.err("expected `,` between field patterns");
            }
        }
        Ok(out)
    }

    fn pattern(&mut self) -> R<Pattern> {
        let start = self.span();
        Ok(self.pattern_inner()?.spanning(self.since(start)))
    }

    fn pattern_inner(&mut self) -> R<Pattern> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Op("_") => {
                self.bump();
                Ok(Pattern::Wild(span))
            }
            Tok::Int(v) => {
                self.bump();
                Ok(Pattern::Int(v, span))
            }
            Tok::TritLit(t) => {
                self.bump();
                Ok(Pattern::Trit(t, span))
            }
            Tok::CharLit(v) => {
                self.bump();
                Ok(Pattern::Char(v, span))
            }
            Tok::Op("-") => {
                self.bump();
                match self.bump() {
                    Tok::Int(v) => Ok(Pattern::Int(v.neg(), span)),
                    Tok::TritLit(t) => Ok(Pattern::Trit(t.tneg(), span)),
                    other => Err(SyntaxError {
                        span,
                        message: format!("expected a literal after `-`, found {other}"),
                    }),
                }
            }
            Tok::Kw("true") => {
                self.bump();
                Ok(Pattern::Bool(true, span))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Pattern::Bool(false, span))
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
                    span,
                };

                if self.at_op("(") {
                    let inner = self.pattern_list("(", ")")?;
                    let fields = inner
                        .into_iter()
                        .enumerate()
                        .map(|(i, p)| (i.to_string(), p))
                        .collect();
                    return Ok(Pattern::Aggregate(path, fields, span));
                }
                if self.at_op("{") {
                    let fields = self.field_patterns()?;
                    return Ok(Pattern::Aggregate(path, fields, span));
                }
                if path.segments.len() > 1 {
                    return Ok(Pattern::Aggregate(path, Vec::new(), span));
                }
                // `name @ pattern` binds the whole while matching (§4).
                if self.eat_op("@") {
                    let inner = self.pattern()?;
                    return Ok(Pattern::Aggregate(
                        Path {
                            segments: vec![path.segments[0].clone()],
                            targs: Vec::new(),
                            span,
                        },
                        vec![("@".to_string(), inner)],
                        span,
                    ));
                }
                Ok(Pattern::Bind(path.segments[0].clone(), span))
            }
            Tok::Op("(") => {
                let inner = self.pattern_list("(", ")")?;
                Ok(Pattern::Tuple(inner, span))
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
        Expr::Block(_)
            | Expr::If(..)
            | Expr::Match(..)
            | Expr::Loop(..)
            | Expr::While(..)
            // `$( … )*` ends at its `*` and holds statements of its own, so
            // it stands as one without a terminator (Ch. 7 §3).
            | Expr::MacroRepeat(..)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source text a span covers, which is what an editor draws under.
    fn text(src: &str, span: Span) -> String {
        src.chars()
            .skip(span.lo as usize)
            .take((span.hi - span.lo) as usize)
            .collect()
    }

    fn tail(src: &str) -> (Expr, String) {
        let file = parse(src).expect("parses");
        let Item::Fn(f) = &file.items[0] else {
            panic!("a function")
        };
        let e = *f.body.as_ref().unwrap().tail.clone().unwrap();
        (e.clone(), text(src, e.span()))
    }

    #[test]
    fn an_expression_spans_all_of_itself_and_not_its_operator() {
        let src = "fn f() -> t27 { a + b * c }";
        let (e, whole) = tail(src);
        assert_eq!(whole, "a + b * c");
        // And so does each part, down to the leaves.
        let Expr::Binary("+", lhs, rhs, _) = e else {
            panic!("a sum")
        };
        assert_eq!(text(src, lhs.span()), "a");
        assert_eq!(text(src, rhs.span()), "b * c");
    }

    #[test]
    fn a_postfix_chain_grows_leftward_from_where_it_started() {
        let src = "fn f() -> t27 { xs[0].len().max(y) }";
        let (e, whole) = tail(src);
        assert_eq!(whole, "xs[0].len().max(y)");
        let Expr::Method(recv, name, at, args, _) = e else {
            panic!("a method call")
        };
        assert_eq!(name, "max");
        assert_eq!(text(src, recv.span()), "xs[0].len()");
        assert_eq!(text(src, args[0].span()), "y");
        // The call covers the whole chain and the name covers the name: a
        // diagnostic about the call underlines one, a rename changes the
        // other.
        assert_eq!(text(src, at), "max");
    }

    #[test]
    fn a_broken_item_does_not_take_the_rest_of_the_file_with_it() {
        let src = "fn a() -> t27 { 1 }\n                   fn b( -> t27 { 2 }\n                   fn c() -> t27 { 3 }\n                   struct P { x: t27 }\n";
        let (file, errs) = parse_recovering(src);
        assert_eq!(errs.len(), 1, "{errs:?}");
        let names: Vec<&str> = file
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Fn(f) => Some(f.name.as_str()),
                Item::Struct(s) => Some(s.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["a", "c", "P"], "the broken one, and only it");
    }

    #[test]
    fn recovery_steps_over_the_body_of_what_failed() {
        // The failure is in the signature, and the body's own `fn`-looking
        // insides must not be read as the next item.
        let src = "fn bad(x: ) -> t27 {\n    let f = 1;\n    f\n}\n                   fn good() -> t27 { 7 }\n";
        let (file, errs) = parse_recovering(src);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(file.items.len(), 1);
        let Item::Fn(f) = &file.items[0] else {
            panic!("a function")
        };
        assert_eq!(f.name, "good");
    }

    #[test]
    fn a_file_that_parses_recovers_into_exactly_what_it_parsed() {
        let src = "struct P { x: t27 }\nfn f(p: P) -> t27 { p.x }\n";
        let (file, errs) = parse_recovering(src);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(file, parse(src).expect("parses"));
    }

    #[test]
    fn an_unterminated_string_stops_rather_than_recovers() {
        // There is nothing to skip to: the quote swallows the rest by
        // definition, so what follows is not the file anyone wrote.
        let (file, errs) = parse_recovering("fn f() { \"oops }\nfn g() {}\n");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("unterminated"), "{errs:?}");
        assert!(file.items.is_empty());
    }

    #[test]
    fn a_call_knows_both_its_extent_and_its_callee() {
        let src = "fn f() -> t27 { helper(1, 2) }";
        let (e, whole) = tail(src);
        assert_eq!(whole, "helper(1, 2)");
        let Expr::Call(name, at, _, _, _) = e else {
            panic!("a call")
        };
        assert_eq!(name, "helper");
        assert_eq!(text(src, at), "helper");
    }

    #[test]
    fn a_span_reaches_across_the_lines_a_construct_covers() {
        let src = "fn f() -> t27 {\n    if a {\n        1\n    } else {\n        2\n    }\n}";
        let (_, whole) = tail(src);
        assert!(whole.starts_with("if a {"), "{whole:?}");
        assert!(whole.ends_with('}'), "{whole:?}");
        assert_eq!(whole.lines().count(), 5);
    }
}
