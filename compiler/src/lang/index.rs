//! What is where, in one file.
//!
//! An editor asks two questions a compiler does not: *what is in this file*
//! (an outline) and *where was the thing under my cursor defined*. Both are
//! answered from the AST alone — no types, no lowering — so this runs on a
//! file that does not compile, which is the state a file is in while it is
//! being written.
//!
//! It lives here rather than in the language server because it is about the
//! shape of Trust, and a second thing that knew that shape would be a second
//! thing to keep right.
//!
//! ## What it can answer, and what it cannot
//!
//! A name resolves by **scope and spelling**, which is enough for locals,
//! parameters, functions, types, constants and variants — every one of which
//! has a name with a span, since `Named` gave parameters and fields one. It is not enough for
//! two things, and neither is guessed at:
//!
//!   * `x.f` — which field that is depends on the type of `x`.
//!   * `x.m()` — which method, likewise. Except when the file defines exactly
//!     one method called `m`, in which case there is nothing to be wrong
//!     about, and that one is the answer.
//!
//! Anything unresolved is simply absent. An editor that jumps to the wrong
//! place is worse than one that does not jump.

use super::ast::*;
use super::lex::Span;

/// What kind of thing a symbol is. These are the distinctions an outline
/// draws, which are not quite the ones the language draws — a method and a
/// function differ only in where they are written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymbolKind {
    /// A free function.
    Function,
    /// A function written in an `impl` or `trait` block.
    Method,
    /// A `struct`.
    Struct,
    /// An `enum`.
    Enum,
    /// One variant of an enum.
    Variant,
    /// A field of a struct or a variant.
    Field,
    /// A `trait`.
    Trait,
    /// An `impl` block.
    Impl,
    /// A `const`.
    Const,
}

// An associated type is not here. `TraitItem.assoc` is a list of names with
// no span, so there is nowhere to put one in an outline, and inventing a
// range for it would be an outline entry that selects the wrong text.

/// One named thing, and what it contains.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Symbol {
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: SymbolKind,
    /// A signature or a type, for an outline that shows more than names.
    pub detail: String,
    /// The whole item.
    pub span: Span,
    /// Just the name, which is what an editor selects when it takes you here.
    pub name_span: Span,
    /// What is written inside it.
    pub children: Vec<Symbol>,
}

/// A definition, written out as the file writes it.
///
/// This is what a hover shows, and the rule it follows is exactly that: **the
/// definition as it was written**. A `let` with no written type has no type
/// here either, because inferring it is lowering's work and this runs before
/// and without it. Saying less is the price of never saying something false.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Definition {
    /// Where the name is.
    pub at: Span,
    /// How it reads: `fn f(x: t27) -> t27`, `n: taddr`, `let mut i`.
    pub label: String,
}

/// Everything one file says about where things are.
pub struct Index {
    /// The file's items, in the order written, nested as they are written.
    pub symbols: Vec<Symbol>,
    /// Each name mentioned, and where it was defined. Sorted by the mention,
    /// and non-overlapping, so a cursor lands in at most one.
    uses: Vec<(Span, Span)>,
    /// Every definition, by where its name is. Sorted.
    defs: Vec<Definition>,
}

impl Index {
    /// Read a file.
    pub fn new(file: &File) -> Index {
        let mut b = Builder {
            symbols: Vec::new(),
            uses: Vec::new(),
            defs: Vec::new(),
            items: Vec::new(),
            methods: Vec::new(),
            scopes: Vec::new(),
        };
        b.collect_items(file);
        for item in &file.items {
            b.item(item);
        }
        b.uses.sort_by_key(|(at, _)| (at.lo, at.hi));
        b.defs.sort_by_key(|d| (d.at.lo, d.at.hi));
        b.defs.dedup_by_key(|d| (d.at.lo, d.at.hi));
        Index {
            symbols: b.symbols,
            uses: b.uses,
            defs: b.defs,
        }
    }

    /// The name at `offset` and where it was defined.
    pub fn use_at(&self, offset: u32) -> Option<(Span, Span)> {
        // The last mention starting at or before the offset — the mentions do
        // not overlap, so there is at most one candidate to check.
        let i = self.uses.partition_point(|(at, _)| at.lo <= offset);
        let (at, to) = *self.uses.get(i.checked_sub(1)?)?;
        (offset < at.hi).then_some((at, to))
    }

    /// How the definition at `at` was written, for a hover to show.
    pub fn describe(&self, at: Span) -> Option<&Definition> {
        let i = self
            .defs
            .binary_search_by_key(&(at.lo, at.hi), |d| (d.at.lo, d.at.hi))
            .ok()?;
        self.defs.get(i)
    }

    /// Where the name at `offset` was defined, if this can tell.
    ///
    /// An offset inside a definition's own name answers with that definition,
    /// which is what an editor wants: asking twice should not walk away.
    pub fn definition_at(&self, offset: u32) -> Option<Span> {
        self.use_at(offset).map(|(_, to)| to)
    }

    /// The innermost symbol whose extent contains `offset`.
    pub fn symbol_at(&self, offset: u32) -> Option<&Symbol> {
        fn walk(symbols: &[Symbol], offset: u32) -> Option<&Symbol> {
            let here = symbols
                .iter()
                .find(|s| s.span.lo <= offset && offset < s.span.hi)?;
            Some(walk(&here.children, offset).unwrap_or(here))
        }
        walk(&self.symbols, offset)
    }
}

/// One name in scope, and where it was bound.
struct Binding {
    name: String,
    at: Span,
}

struct Builder {
    symbols: Vec<Symbol>,
    uses: Vec<(Span, Span)>,
    defs: Vec<Definition>,
    /// File-level names: functions, types, constants, variants.
    items: Vec<(String, Span)>,
    /// Method names, with how many share each — a name defined twice cannot
    /// be resolved by spelling, and is not.
    methods: Vec<(String, Span)>,
    scopes: Vec<Binding>,
}

impl Builder {
    // ------------------------------------------------------------- pass one

    /// Names visible everywhere. Order does not affect visibility (§3), so
    /// they are all gathered before anything is resolved.
    fn collect_items(&mut self, file: &File) {
        for item in &file.items {
            match item {
                Item::Fn(f) => self.define_item(&f.name, f.name_span, fn_label(f)),
                Item::Const(c) => self.define_item(
                    &c.name,
                    c.name_span,
                    format!("const {}: {}", c.name, type_name(&c.ty)),
                ),
                Item::Struct(s) => {
                    self.define_item(&s.name, s.name_span, format!("struct {}", s.name));
                    for f in &s.fields {
                        self.define(f.name_span, format!("{}: {}", f.name, type_name(&f.ty)));
                    }
                }
                Item::Trait(t) => {
                    self.define_item(&t.name, t.name_span, format!("trait {}", t.name));
                    for m in &t.methods {
                        self.methods.push((m.name.clone(), m.name_span));
                        self.define(m.name_span, fn_label(m));
                    }
                }
                Item::Enum(e) => {
                    self.define_item(&e.name, e.name_span, format!("enum {}", e.name));
                    // A variant is reachable as `Enum::Variant` and, inside a
                    // pattern, often as itself.
                    for v in &e.variants {
                        self.define_item(&v.name, v.name_span, variant_label(&e.name, v));
                        for f in &v.fields {
                            self.define(f.name_span, format!("{}: {}", f.name, type_name(&f.ty)));
                        }
                    }
                }
                Item::Impl(i) => {
                    for m in &i.methods {
                        self.methods.push((m.name.clone(), m.name_span));
                        self.define(m.name_span, format!("{}\n{}", impl_label(i), fn_label(m)));
                    }
                }
            }
        }
    }

    /// A name visible everywhere, and how it reads.
    fn define_item(&mut self, name: &str, at: Span, label: String) {
        self.items.push((name.to_string(), at));
        self.define(at, label);
    }

    /// A definition a hover can show, wherever it was written.
    ///
    /// A definition is also a place a cursor can be, and asking there should
    /// answer with itself — hovering `fn area` on the line that declares it
    /// is the most obvious hover there is, and jumping from a definition
    /// should stay put rather than go nowhere.
    fn define(&mut self, at: Span, label: String) {
        if at.line != 0 {
            self.defs.push(Definition { at, label });
            self.uses.push((at, at));
        }
    }

    // ------------------------------------------------------ resolving names

    /// Record that `name` at `at` refers to whatever is in scope.
    fn use_name(&mut self, name: &str, at: Span) {
        if at.line == 0 {
            return; // synthesized: it is in no file
        }
        // Innermost first: a local shadows a parameter shadows an item.
        if let Some(b) = self.scopes.iter().rev().find(|b| b.name == name) {
            let to = b.at;
            self.uses.push((at, to));
            return;
        }
        if let Some((_, to)) = self.items.iter().rev().find(|(n, _)| n == name) {
            let to = *to;
            self.uses.push((at, to));
        }
    }

    /// Record a method name, which resolves only when the file defines one.
    fn use_method(&mut self, name: &str, at: Span) {
        if at.line == 0 {
            return;
        }
        let mut found = self.methods.iter().filter(|(n, _)| n == name);
        let Some((_, to)) = found.next() else { return };
        let to = *to;
        if found.next().is_some() {
            return; // more than one, and nothing here can say which
        }
        self.uses.push((at, to));
    }

    fn bind(&mut self, name: &str, at: Span) {
        self.scopes.push(Binding {
            name: name.to_string(),
            at,
        });
    }

    /// A binding that is also somewhere to point at.
    fn bind_and_mark(&mut self, name: &str, at: Span, label: String) {
        self.define(at, label);
        self.bind(name, at);
    }

    /// Bindings made inside `f` are gone after it. Shadowing needs nothing
    /// more: `use_name` searches from the innermost outwards.
    fn scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let mark = self.scopes.len();
        let out = f(self);
        self.scopes.truncate(mark);
        out
    }

    // ------------------------------------------------------------- pass two

    fn item(&mut self, item: &Item) {
        let sym = match item {
            Item::Fn(f) => {
                let s = self.function(f, SymbolKind::Function);
                Some(s)
            }
            Item::Const(c) => {
                self.expr(&c.value);
                self.ty(&c.ty);
                Some(Symbol {
                    name: c.name.clone(),
                    kind: SymbolKind::Const,
                    detail: type_name(&c.ty),
                    span: c.span,
                    name_span: c.name_span,
                    children: Vec::new(),
                })
            }
            Item::Struct(s) => {
                let children = s
                    .fields
                    .iter()
                    .map(|f| {
                        self.ty(&f.ty);
                        Symbol {
                            name: f.name.clone(),
                            kind: SymbolKind::Field,
                            detail: type_name(&f.ty),
                            span: f.name_span.to(f.ty.span()),
                            name_span: f.name_span,
                            children: Vec::new(),
                        }
                    })
                    .collect();
                Some(Symbol {
                    name: s.name.clone(),
                    kind: SymbolKind::Struct,
                    detail: String::new(),
                    span: s.span,
                    name_span: s.name_span,
                    children,
                })
            }
            Item::Enum(e) => {
                let children = e
                    .variants
                    .iter()
                    .map(|v| {
                        for f in &v.fields {
                            self.ty(&f.ty);
                        }
                        Symbol {
                            name: v.name.clone(),
                            kind: SymbolKind::Variant,
                            detail: String::new(),
                            span: v.span,
                            name_span: v.name_span,
                            children: Vec::new(),
                        }
                    })
                    .collect();
                Some(Symbol {
                    name: e.name.clone(),
                    kind: SymbolKind::Enum,
                    detail: String::new(),
                    span: e.span,
                    name_span: e.name_span,
                    children,
                })
            }
            Item::Trait(t) => {
                let children = t
                    .methods
                    .iter()
                    .map(|m| self.function(m, SymbolKind::Method))
                    .collect();
                Some(Symbol {
                    name: t.name.clone(),
                    kind: SymbolKind::Trait,
                    detail: String::new(),
                    span: t.span,
                    name_span: t.name_span,
                    children,
                })
            }
            Item::Impl(i) => {
                let children = i
                    .methods
                    .iter()
                    .map(|m| self.function(m, SymbolKind::Method))
                    .collect();
                let name = match &i.trait_name {
                    Some(t) => format!("impl {t} for {}", i.self_ty),
                    None => format!("impl {}", i.self_ty),
                };
                Some(Symbol {
                    name,
                    kind: SymbolKind::Impl,
                    detail: String::new(),
                    span: i.span,
                    name_span: i.span,
                    children,
                })
            }
        };
        if let Some(sym) = sym {
            self.symbols.push(sym);
        }
    }

    fn function(&mut self, f: &FnItem, kind: SymbolKind) -> Symbol {
        self.scope(|b| {
            for p in &f.params {
                b.ty(&p.ty);
                b.bind_and_mark(
                    &p.name,
                    p.name_span,
                    format!("{}: {}", p.name, type_name(&p.ty)),
                );
            }
            if let Some(r) = &f.ret {
                b.ty(r);
            }
            for p in &f.requires {
                b.expr(p);
            }
            if let Some(body) = &f.body {
                b.block(body);
            }
        });
        Symbol {
            name: f.name.clone(),
            kind,
            detail: signature(f),
            span: f.span,
            name_span: f.name_span,
            children: Vec::new(),
        }
    }

    fn ty(&mut self, ty: &Ty) {
        match ty {
            Ty::Name(n, at) | Ty::Dyn(n, at) => self.use_name(n, *at),
            Ty::App(n, args, at) => {
                self.use_name(n, *at);
                for a in args {
                    self.ty(a);
                }
            }
            Ty::Array(inner, count, _) => {
                self.ty(inner);
                self.expr(count);
            }
            Ty::Ref(inner, _, _) | Ty::Slice(inner, _) | Ty::Assoc(inner, _, _) => self.ty(inner),
            Ty::Tuple(items, _) => {
                for t in items {
                    self.ty(t);
                }
            }
            Ty::ImplFn(_, args, ret, _) => {
                for a in args {
                    self.ty(a);
                }
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
            Ty::Unit(_) | Ty::Never(_) | Ty::SelfTy(_) => {}
        }
    }

    fn block(&mut self, b: &Block) {
        self.scope(|s| {
            for stmt in &b.stmts {
                match stmt {
                    Stmt::Let {
                        mutable,
                        name,
                        name_span,
                        ty,
                        value,
                        span: _,
                    } => {
                        // The initializer is read before the name is bound,
                        // so `let x = x;` means the outer `x` (§5.2).
                        s.expr(value);
                        if let Some(t) = ty {
                            s.ty(t);
                        }
                        // A `let` with no written type has none here: what
                        // it would be is inferred during lowering, which
                        // this runs before and without.
                        let m = if *mutable { "mut " } else { "" };
                        let label = match ty {
                            Some(t) => format!("let {m}{name}: {}", type_name(t)),
                            None => format!("let {m}{name}"),
                        };
                        s.define(*name_span, label);
                        s.bind(name, *name_span);
                    }
                    Stmt::Expr(e) => s.expr(e),
                }
            }
            if let Some(tail) = &b.tail {
                s.expr(tail);
            }
        });
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Path(name, at) => self.use_name(name, *at),
            Expr::Call(name, targs, args, at) => {
                self.use_name(name, *at);
                for t in targs {
                    self.ty(t);
                }
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Method(recv, name, args, at) => {
                self.expr(recv);
                self.use_method(name, *at);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Aggregate(path, fields, _) => {
                // `Name { … }` and `Enum::Variant { … }`: the last segment is
                // the one a reader means.
                self.use_name(path.last(), path.span);
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            Expr::Cast(v, t, _) => {
                self.expr(v);
                self.ty(t);
            }
            Expr::Closure(params, ret, body, _) => {
                self.scope(|s| {
                    for (name, ty) in params {
                        if let Some(t) = ty {
                            s.ty(t);
                        }
                        // A closure's parameter may be written without a
                        // type, and its name has no span either way
                        // (Ch. 4 §4.1): it binds, so that it shadows, and
                        // is not somewhere to jump to.
                        s.bind(name, Span::NONE);
                    }
                    if let Some(r) = ret {
                        s.ty(r);
                    }
                    s.expr(body);
                });
            }
            Expr::Block(b) => self.block(b),
            Expr::If(c, then, other, _) => {
                self.expr(c);
                self.block(then);
                if let Some(o) = other {
                    self.expr(o);
                }
            }
            Expr::Match(scrutinee, arms, _) => {
                self.expr(scrutinee);
                for arm in arms {
                    self.scope(|s| {
                        for p in &arm.patterns {
                            s.pattern(p);
                        }
                        if let Some(g) = &arm.guard {
                            s.expr(g);
                        }
                        s.expr(&arm.body);
                    });
                }
            }
            Expr::While(c, body, _) => {
                self.expr(c);
                self.block(body);
            }
            Expr::Loop(body, _) => self.block(body),
            Expr::Field(v, _, _) => self.expr(v),
            Expr::Unary(_, v, _)
            | Expr::Borrow(v, _, _)
            | Expr::Deref(v, _)
            | Expr::Try(v, _)
            | Expr::Repeat(v, _, _) => self.expr(v),
            Expr::Binary(_, a, b, _) | Expr::Assign(_, a, b, _) | Expr::Index(a, b, _) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::CallExpr(f, args, _) => {
                self.expr(f);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Array(items, _) | Expr::Tuple(items, _) => {
                for i in items {
                    self.expr(i);
                }
            }
            Expr::Break(v, _) | Expr::Return(v, _) => {
                if let Some(v) = v {
                    self.expr(v);
                }
            }
            Expr::Int(..)
            | Expr::Trit(..)
            | Expr::Char(..)
            | Expr::Str(..)
            | Expr::Bool(..)
            | Expr::Unit(_)
            | Expr::Continue(_) => {}
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            // A pattern binding's type is the scrutinee's, which needs a
            // type this cannot compute; the name is all there is to show.
            Pattern::Bind(name, at) => self.bind_and_mark(name, *at, name.clone()),
            Pattern::Aggregate(path, fields, _) => {
                self.use_name(path.last(), path.span);
                for (_, sub) in fields {
                    self.pattern(sub);
                }
            }
            Pattern::Tuple(items, _) => {
                for i in items {
                    self.pattern(i);
                }
            }
            Pattern::Wild(_)
            | Pattern::Int(..)
            | Pattern::Trit(..)
            | Pattern::Char(..)
            | Pattern::Bool(..) => {}
        }
    }
}

/// A function as it was written: `fn name(params) -> ret`.
fn fn_label(f: &FnItem) -> String {
    format!("fn {}{}", f.name, signature(f))
}

/// A variant as it was written, under the enum that holds it.
fn variant_label(enum_name: &str, v: &Variant) -> String {
    if v.fields.is_empty() {
        return format!("{enum_name}::{}", v.name);
    }
    let fields: Vec<String> = v
        .fields
        .iter()
        .map(|f| {
            if f.name.chars().all(|c| c.is_ascii_digit()) {
                type_name(&f.ty)
            } else {
                format!("{}: {}", f.name, type_name(&f.ty))
            }
        })
        .collect();
    format!("{enum_name}::{}({})", v.name, fields.join(", "))
}

/// An impl block's header, so a method hover says which one it came from.
fn impl_label(i: &ImplItem) -> String {
    match &i.trait_name {
        Some(t) => format!("impl {t} for {}", i.self_ty),
        None => format!("impl {}", i.self_ty),
    }
}

/// A function's signature, as an outline would show it.
fn signature(f: &FnItem) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| {
            if p.name == "self" {
                "self".to_string()
            } else {
                format!("{}: {}", p.name, type_name(&p.ty))
            }
        })
        .collect();
    let ret = match &f.ret {
        Some(t) => format!(" -> {}", type_name(t)),
        None => String::new(),
    };
    format!("({}){ret}", params.join(", "))
}

/// A type as it was written, near enough for an outline.
fn type_name(t: &Ty) -> String {
    match t {
        Ty::Name(n, _) => n.clone(),
        Ty::Unit(_) => "()".to_string(),
        Ty::Never(_) => "!".to_string(),
        Ty::SelfTy(_) => "Self".to_string(),
        Ty::Ref(inner, true, _) => format!("&mut {}", type_name(inner)),
        Ty::Ref(inner, false, _) => format!("&{}", type_name(inner)),
        Ty::Slice(inner, _) => format!("[{}]", type_name(inner)),
        Ty::Array(inner, _, _) => format!("[{}; _]", type_name(inner)),
        Ty::Dyn(n, _) => format!("dyn {n}"),
        Ty::Assoc(inner, n, _) => format!("{}::{n}", type_name(inner)),
        Ty::App(n, args, _) => {
            let args: Vec<String> = args.iter().map(type_name).collect();
            format!("{n}<{}>", args.join(", "))
        }
        Ty::Tuple(items, _) => {
            let items: Vec<String> = items.iter().map(type_name).collect();
            format!("({})", items.join(", "))
        }
        Ty::ImplFn(kind, args, ret, _) => {
            let args: Vec<String> = args.iter().map(type_name).collect();
            let ret = match ret {
                Some(r) => format!(" -> {}", type_name(r)),
                None => String::new(),
            };
            format!("impl {}({}){ret}", kind.name(), args.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::parse::parse;

    fn index(src: &str) -> Index {
        Index::new(&parse(src).expect("parses"))
    }

    /// Where the cursor is, given a source with a `|` marking it.
    fn at(src: &str) -> (String, u32) {
        let offset = src.chars().position(|c| c == '|').expect("a cursor") as u32;
        (src.replace('|', ""), offset)
    }

    fn jump(marked: &str) -> Option<String> {
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset)?;
        Some(
            src.chars()
                .skip(to.lo as usize)
                .take((to.hi - to.lo) as usize)
                .collect(),
        )
    }

    #[test]
    fn an_outline_nests_what_is_written_inside_what_it_is_written_in() {
        let i = index(
            "struct P { x: t27 }\n\
             enum E { A, B(t9) }\n\
             impl P { fn get(&self) -> t27 { self.x } }\n",
        );
        let names: Vec<(&str, SymbolKind)> =
            i.symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            vec![
                ("P", SymbolKind::Struct),
                ("E", SymbolKind::Enum),
                ("impl P", SymbolKind::Impl),
            ]
        );
        assert_eq!(i.symbols[1].children.len(), 2, "two variants");
        assert_eq!(i.symbols[2].children[0].name, "get");
        assert_eq!(i.symbols[2].children[0].detail, "(self) -> t27");
    }

    #[test]
    fn a_call_goes_to_the_function_it_calls() {
        assert_eq!(
            jump("fn helper() -> t27 { 1 }\nfn main() -> t27 { hel|per() }\n").as_deref(),
            Some("helper")
        );
    }

    #[test]
    fn a_local_beats_an_item_of_the_same_name() {
        // `n` is both a constant and a binding, and the binding is nearer.
        let marked = "const n: t27 = 1;\nfn f() -> t27 {\n    let n = 2;\n    |n\n}\n";
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset).expect("resolves");
        assert_eq!(to.line, 3, "the `let`, not the `const`");
    }

    #[test]
    fn an_initializer_is_read_before_its_own_name_is_bound() {
        let marked = "fn f() -> t27 {\n    let n = 1;\n    let n = |n;\n    n\n}\n";
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset).expect("resolves");
        assert_eq!(to.line, 2, "`let n = n;` means the outer one");
    }

    #[test]
    fn a_binding_leaves_the_scope_that_bound_it() {
        // The `x` in the tail is the parameter, not the one the block bound.
        let marked = "fn f(x: t27) -> t27 {\n    { let x = 1; x }\n    |x\n}\n";
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset).expect("resolves");
        assert_eq!(to.line, 1, "the parameter, not the block's `let`");
    }

    #[test]
    fn a_parameter_is_somewhere_to_jump_to() {
        // Its name, not its type: `Named` carries a span for exactly this.
        assert_eq!(
            jump("fn f(count: t27) -> t27 { co|unt + 1 }\n").as_deref(),
            Some("count")
        );
    }

    #[test]
    fn a_field_in_an_outline_selects_the_field() {
        let src = "struct P {\n    x: t27,\n    y: t27,\n}\n";
        let i = index(src);
        let fields = &i.symbols[0].children;
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[1].name, "y");
        assert_eq!(fields[1].name_span.line, 3);
        let text: String = src
            .chars()
            .skip(fields[1].name_span.lo as usize)
            .take((fields[1].name_span.hi - fields[1].name_span.lo) as usize)
            .collect();
        assert_eq!(text, "y");
    }

    #[test]
    fn a_method_resolves_only_when_one_of_that_name_exists() {
        assert_eq!(
            jump("struct P;\nimpl P { fn go(&self) {} }\nfn f(p: P) { p.g|o() }\n").as_deref(),
            Some("go")
        );
        // Two `go`s, and nothing here can say which — so it says nothing.
        assert_eq!(
            jump(
                "struct P;\nstruct Q;\n\
                 impl P { fn go(&self) {} }\n\
                 impl Q { fn go(&self) {} }\n\
                 fn f(p: P) { p.g|o() }\n"
            ),
            None
        );
    }

    #[test]
    fn a_type_in_a_signature_goes_to_the_type() {
        assert_eq!(
            jump("struct Point { x: t27 }\nfn f(p: Po|int) {}\n").as_deref(),
            Some("Point")
        );
    }

    #[test]
    fn a_cursor_on_a_definition_stays_where_it_is() {
        let marked = "fn f(v: t27) -> t27 {\n    match v { |n => n }\n}\n";
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset).expect("resolves");
        assert_eq!((to.line, to.lo), (2, offset));
    }

    fn hover(marked: &str) -> Option<String> {
        let (src, offset) = at(marked);
        let i = index(&src);
        let (_, to) = i.use_at(offset)?;
        Some(i.describe(to)?.label.clone())
    }

    #[test]
    fn a_hover_shows_the_definition_as_it_was_written() {
        assert_eq!(
            hover("fn add(a: t27, b: t27) -> t27 { a + b }\nfn f() -> t27 { a|dd(1, 2) }\n")
                .as_deref(),
            Some("fn add(a: t27, b: t27) -> t27")
        );
        assert_eq!(
            hover("struct Point { x: t27 }\nfn f(p: Poi|nt) {}\n").as_deref(),
            Some("struct Point")
        );
        assert_eq!(
            hover("const N: taddr = 8;\nfn f() -> taddr { |N }\n").as_deref(),
            Some("const N: taddr")
        );
        assert_eq!(
            hover("fn f(count: &mut [t27]) { let n = coun|t; }\n").as_deref(),
            Some("count: &mut [t27]")
        );
    }

    #[test]
    fn a_hover_on_a_variant_names_the_enum_it_belongs_to() {
        assert_eq!(
            hover("enum Shape { Dot, Rect(t27, t27) }\nfn f() -> Shape { Re|ct(1, 2) }\n")
                .as_deref(),
            Some("Shape::Rect(t27, t27)")
        );
    }

    #[test]
    fn a_hover_on_a_method_says_which_impl_it_came_from() {
        let text = hover(
            "struct P;\nimpl Area for P { fn area(&self) -> t27 { 0 } }\n             fn f(p: P) -> t27 { p.ar|ea() }\n",
        )
        .expect("resolves");
        assert_eq!(text, "impl Area for P\nfn area(self) -> t27");
    }

    #[test]
    fn a_let_without_a_written_type_does_not_invent_one() {
        // What it would be is inferred while lowering, which this runs
        // before and without — so the hover says what the file says.
        assert_eq!(
            hover("fn f() -> t27 {\n    let mut n = 1;\n    |n\n}\n").as_deref(),
            Some("let mut n")
        );
        assert_eq!(
            hover("fn f() -> t27 {\n    let n: t9 = 1;\n    |n as t27\n}\n").as_deref(),
            Some("let n: t9")
        );
    }

    #[test]
    fn a_hover_on_a_declaration_answers_with_itself() {
        // The most obvious hover there is, and the one that used to answer
        // nothing: a definition is a place a cursor can be.
        assert_eq!(
            hover("fn ar|ea(s: t27) -> t27 { s }\n").as_deref(),
            Some("fn area(s: t27) -> t27")
        );
        assert_eq!(
            hover("struct P { wid|th: t27 }\n").as_deref(),
            Some("width: t27")
        );
        assert_eq!(
            hover("fn f() { let mut n|ext: t9 = 1; }\n").as_deref(),
            Some("let mut next: t9")
        );
    }

    #[test]
    fn a_jump_from_a_declaration_stays_put() {
        let marked = "fn f() -> t27 {\n    let tot|al = 1;\n    total\n}\n";
        let (src, offset) = at(marked);
        let to = index(&src).definition_at(offset).expect("resolves");
        assert_eq!((to.line, to.lo <= offset && offset < to.hi), (2, true));
    }

    #[test]
    fn a_cursor_on_nothing_answers_nothing() {
        assert_eq!(jump("fn f() -> t27 { 1 |+ 2 }\n"), None);
    }
}
