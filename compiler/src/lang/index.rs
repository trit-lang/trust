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
    /// A `mod` declaration (Ch. 6 §1.2).
    Module,
    /// A type alias (Ch. 0 §3.6).
    Alias,
    /// A macro (Ch. 7 §1).
    Macro,
    /// A `let` binding or a pattern binding.
    Local,
    /// A function's parameter.
    Parameter,
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
    /// What it is.
    pub kind: SymbolKind,
    /// The name itself.
    pub name: String,
    /// How it reads: `fn f(x: t27) -> t27`, `n: taddr`, `let mut i`.
    pub label: String,
    /// Whether this is a method, which is resolved by spelling alone and so
    /// cannot be renamed — see `NoRename::Method`.
    pub method: bool,
}

/// Why a rename will not be done.
///
/// Each of these is a case where the set of names to change cannot be known
/// from this file, and a rename that changes some of them is worse than one
/// that changes none.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NoRename {
    /// The cursor is not on a name this file defines.
    NotAName,
    /// A method. It resolves by spelling, and the prelude — which this index
    /// never sees — defines methods of its own, so the calls found here may
    /// not be the calls that mean this one.
    Method,
    /// Something of that name is already written where this one is used, so
    /// the rename would shadow or be shadowed rather than rename.
    Taken(String),
    /// The new text is not a name (Ch. 0 §1.3).
    NotAnIdentifier,
}

impl std::fmt::Display for NoRename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRename::NotAName => f.write_str("there is no name here to rename"),
            NoRename::Method => f.write_str(
                "a method is found by its spelling, and the prelude has methods this                  file cannot see, so renaming one here might rename only some of it",
            ),
            NoRename::Taken(n) => write!(f, "`{n}` is already written where this is used"),
            NoRename::NotAnIdentifier => f.write_str(
                "a name is a letter or `_` followed by letters, digits or `_` (Ch. 0 §1.3)",
            ),
        }
    }
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
    /// Names visible everywhere in the file: items, and an enum's variants.
    globals: Vec<Span>,
    /// A binding, and the stretch of the file over which it can be named.
    scoped: Vec<Scoped>,
    /// What each nominal type has written on it: a struct's fields, and the
    /// methods of every `impl` for it. By the type's own name.
    members: std::collections::HashMap<String, Vec<Span>>,
}

/// A binding and where it is in scope.
///
/// A definition is in scope from where it is written to the end of whatever
/// held it — a block, a function, a match arm. That is the whole of Ch. 0
/// §5.2's rule, and it is what a completion needs that a jump does not.
struct Scoped {
    /// The definition, by where its name is.
    at: Span,
    /// The first offset that can name it: just past its own name, so that
    /// `let n = n;` offers the outer one.
    from: u32,
    /// One past the last offset that can name it.
    to: u32,
}

impl Index {
    /// Read a file.
    pub fn new(file: &File) -> Index {
        let mut b = Builder {
            symbols: Vec::new(),
            scoped: Vec::new(),
            members: std::collections::HashMap::new(),
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
        b.close_file();
        b.uses.sort_by_key(|(at, _)| (at.lo, at.hi));
        b.defs.sort_by_key(|d| (d.at.lo, d.at.hi));
        b.defs.dedup_by_key(|d| (d.at.lo, d.at.hi));
        Index {
            symbols: b.symbols,
            uses: b.uses,
            defs: b.defs,
            globals: b.items.iter().map(|(_, at)| *at).collect(),
            scoped: b.scoped,
            members: b.members,
        }
    }

    /// The names this file makes visible everywhere.
    ///
    /// Not the ones the compiler wrote for itself: a mangled instantiation
    /// (`Option.unwrap.char`) holds a dot, which no Trust identifier may, so
    /// it is a name no program can type.
    pub fn globals(&self) -> Vec<&Definition> {
        self.globals
            .iter()
            .filter_map(|at| self.describe(*at))
            .filter(|d| !d.name.contains('.') && !d.name.starts_with('#'))
            .collect()
    }

    /// What can be written after a `.` on something of this type.
    ///
    /// `ty` is a type as it is written — `&mut Point`, `Vec<t27>`, `dyn Area`
    /// — and what it is written *on* is the same either way: a reference's
    /// methods have always been the referent's (Ch. 4 §1.4), and a generic
    /// type's are its head's.
    pub fn members(&self, ty: &str) -> Vec<&Definition> {
        // Two keys, because an impl is written on one of two things.
        // `impl<T> Vec<T>` is written on every `Vec`; `impl Vec<char>` is
        // written on that one, and a `Vec<t27>` has none of it.
        let bare = stripped(ty);
        let keys = [nominal(ty), bare];
        let mut out: Vec<&Definition> = keys
            .iter()
            .filter(|k| !k.is_empty())
            .flat_map(|k| self.members.get(*k).into_iter().flatten())
            .filter_map(|at| self.describe(*at))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.name == b.name);
        out
    }
}

/// What the language itself writes on a type, from the compiler's own table.
///
/// A `Vec`'s `push` is not in the prelude and not in any file: its storage is
/// the compiler's (Ch. 5 §2), so the only place it is written down is the
/// table `lower` matches against. Reading that same table is what keeps a
/// completion from being a second list of what exists.
pub fn builtin_members(ty: &str) -> Vec<(&'static str, &'static str)> {
    // The same classifier the compiler dispatches by, so a completion offers
    // exactly what a call would find.
    let Some(kind) = super::lower::builtin_kind(nominal(ty)) else {
        return Vec::new();
    };
    super::lower::BUILTIN_METHODS
        .iter()
        .filter(|(on, _, _)| *on == kind)
        .map(|(_, n, sig)| (*n, *sig))
        .collect()
}

/// What an `impl` block's methods are written on.
///
/// `impl<T> Vec<T>` is written on every `Vec` and keys by its head;
/// `impl Vec<char>` is written on that one and keys by the whole thing. What
/// tells them apart is whether the arguments are the impl's own parameters.
fn impl_owner(i: &ImplItem) -> String {
    let params: Vec<&str> = i.generics.iter().map(GenericParam::name).collect();
    let generic = |t: &Ty| match t {
        Ty::Name(n, _) => params.contains(&n.as_str()),
        _ => false,
    };
    if i.self_args.is_empty() || i.self_args.iter().any(generic) {
        return i.self_ty.clone();
    }
    let args: Vec<String> = i.self_args.iter().map(type_name).collect();
    format!("{}<{}>", i.self_ty, args.join(", "))
}

/// A written type with what it is held by taken off: `&mut Vec<t27>` is a
/// `Vec<t27>`, arguments and all.
fn stripped(ty: &str) -> &str {
    let mut t = ty.trim();
    loop {
        let next = t
            .strip_prefix('&')
            .map(str::trim_start)
            .map(|t| t.strip_prefix("mut ").unwrap_or(t))
            .or_else(|| t.strip_prefix("dyn ").map(str::trim_start));
        match next {
            Some(rest) if rest != t => t = rest.trim_start(),
            _ => break,
        }
    }
    t
}

/// The name a written type is written *on*: `&mut Vec<t27>` is a `Vec`.
fn nominal(ty: &str) -> &str {
    let t = stripped(ty);
    t.split('<').next().unwrap_or(t).trim()
}

impl Index {
    /// Everything that can be named at `offset`, nearest first.
    ///
    /// A name written twice is offered once, as whichever is in scope — the
    /// innermost, which is the one that would be meant. Nothing here needs a
    /// type, which is why it exists at all: see the note on what a
    /// *member* completion would need instead.
    pub fn visible(&self, offset: u32) -> Vec<&Definition> {
        let mut out: Vec<&Definition> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        // Innermost first: the scopes are pushed as they close, so the
        // narrowest ones come first.
        for s in self
            .scoped
            .iter()
            .filter(|s| s.from <= offset && offset < s.to)
        {
            if let Some(d) = self.describe(s.at)
                && !seen.contains(&d.name.as_str())
            {
                seen.push(&d.name);
                out.push(d);
            }
        }
        for at in &self.globals {
            if let Some(d) = self.describe(*at)
                && !seen.contains(&d.name.as_str())
            {
                seen.push(&d.name);
                out.push(d);
            }
        }
        out
    }

    /// The name at `offset` and where it was defined.
    pub fn use_at(&self, offset: u32) -> Option<(Span, Span)> {
        // The last mention starting at or before the offset — the mentions do
        // not overlap, so there is at most one candidate to check.
        let i = self.uses.partition_point(|(at, _)| at.lo <= offset);
        let (at, to) = *self.uses.get(i.checked_sub(1)?)?;
        (offset < at.hi).then_some((at, to))
    }

    /// Every place that means the definition at `at`, including its own
    /// name. In the order they are written.
    ///
    /// This is exact rather than textual: a different `n` in another function
    /// is a different definition and is not here.
    pub fn references(&self, at: Span) -> Vec<Span> {
        self.uses
            .iter()
            .filter(|(_, to)| to.lo == at.lo && to.hi == at.hi)
            .map(|(from, _)| *from)
            .collect()
    }

    /// Every place a rename at `offset` would have to change, or why it will
    /// not be done.
    pub fn rename(&self, offset: u32, new: &str) -> Result<Vec<Span>, NoRename> {
        if !is_identifier(new) {
            return Err(NoRename::NotAnIdentifier);
        }
        let (_, at) = self.use_at(offset).ok_or(NoRename::NotAName)?;
        let def = self.describe(at).ok_or(NoRename::NotAName)?;
        if def.method {
            return Err(NoRename::Method);
        }
        if let Some(taken) = self.taken(new, at) {
            return Err(NoRename::Taken(taken));
        }
        Ok(self.references(at))
    }

    /// Whether `new` is already written somewhere that would collide with a
    /// definition at `at`.
    ///
    /// Shadowing happens inside an item, so that is the region searched —
    /// plus the file's own top-level names, which every item can see. A local
    /// called `i` in another function is not a collision and is not refused.
    fn taken(&self, new: &str, at: Span) -> Option<String> {
        if self.symbols.iter().any(|s| s.name == new) {
            return Some(new.to_string());
        }
        let region = self.symbol_at(at.lo).map(|s| s.span);
        let inside = |d: &Definition| match region {
            Some(r) => r.lo <= d.at.lo && d.at.hi <= r.hi,
            // A definition outside every item is a top-level one, already
            // checked above.
            None => true,
        };
        self.defs
            .iter()
            .find(|d| d.name == new && inside(d))
            .map(|d| d.name.clone())
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

/// What the prelude offers, by name.
///
/// The prelude is a source file like any other (`lang::PRELUDE`), so it is
/// indexed like any other — but it is not the file being edited, and its
/// spans are offsets into *it*. Only the names and how they read are used
/// here, never a place: a completion may offer `println`, and nothing may
/// claim to know where in this file it is, because it is not.
pub fn prelude() -> &'static Index {
    static ONCE: std::sync::OnceLock<Index> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        // The prelude parses — a test in `lang` says so — and a broken one
        // is this compiler's fault. Recovery makes the failure partial
        // rather than total, which is the right shape for a fault that is
        // ours: whatever still reads is still true.
        let (file, _) = super::parse::parse_recovering(super::PRELUDE);
        Index::new(&file)
    })
}

/// One name in scope, and where it was bound.
struct Binding {
    name: String,
    at: Span,
}

struct Builder {
    symbols: Vec<Symbol>,
    scoped: Vec<Scoped>,
    members: std::collections::HashMap<String, Vec<Span>>,
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
                Item::Fn(f) => {
                    self.define_item(&f.name, f.name_span, SymbolKind::Function, fn_label(f))
                }
                Item::Const(c) => self.define_item(
                    &c.name,
                    c.name_span,
                    SymbolKind::Const,
                    format!("const {}: {}", c.name, type_name(&c.ty)),
                ),
                Item::Struct(s) => {
                    let label = format!("struct {}", s.name);
                    self.define_item(&s.name, s.name_span, SymbolKind::Struct, label);
                    for f in &s.fields {
                        let label = format!("{}: {}", f.name, type_name(&f.ty));
                        self.define(f.name_span, &f.name, SymbolKind::Field, label);
                        self.own(&s.name, f.name_span);
                    }
                }
                Item::Trait(t) => {
                    let label = format!("trait {}", t.name);
                    self.define_item(&t.name, t.name_span, SymbolKind::Trait, label);
                    for m in &t.methods {
                        self.methods.push((m.name.clone(), m.name_span));
                        self.define_method(m.name_span, &m.name, fn_label(m));
                        // A `dyn Trait` has the trait's methods and nothing
                        // else (Ch. 4 §3.1), so they are written on it.
                        self.own(&t.name, m.name_span);
                    }
                }
                Item::Enum(e) => {
                    let label = format!("enum {}", e.name);
                    self.define_item(&e.name, e.name_span, SymbolKind::Enum, label);
                    // A variant is reachable as `Enum::Variant` and, inside a
                    // pattern, often as itself.
                    for v in &e.variants {
                        let label = variant_label(&e.name, v);
                        self.define_item(&v.name, v.name_span, SymbolKind::Variant, label);
                        for f in &v.fields {
                            let label = format!("{}: {}", f.name, type_name(&f.ty));
                            self.define(f.name_span, &f.name, SymbolKind::Field, label);
                        }
                    }
                }
                Item::Impl(i) => {
                    for m in &i.methods {
                        self.methods.push((m.name.clone(), m.name_span));
                        let label = format!("{}\n{}", impl_label(i), fn_label(m));
                        self.define_method(m.name_span, &m.name, label);
                        self.own(&impl_owner(i), m.name_span);
                    }
                }
                Item::Mod(m) => {
                    let label = format!("mod {}", m.name);
                    self.define_item(&m.name, m.name_span, SymbolKind::Module, label);
                }
                Item::Alias(a) => {
                    let label = format!("type {} = {}", a.name, type_name(&a.ty));
                    self.define_item(&a.name, a.name_span, SymbolKind::Alias, label);
                }
                Item::Macro(m) => {
                    let mut ps: Vec<String> = m.params.iter().map(|p| format!("${p}")).collect();
                    if let Some(r) = &m.rest {
                        ps.push(format!("$(${r}),*"));
                    }
                    let label = format!("macro {}({})", m.name, ps.join(", "));
                    self.define_item(&m.name, m.name_span, SymbolKind::Macro, label);
                }
                // A `use` binds a name for something defined elsewhere, so
                // the definition it names is not in this file to point at.
                Item::Use(_) => {}
            }
        }
    }

    /// A name visible everywhere, and how it reads.
    fn define_item(&mut self, name: &str, at: Span, kind: SymbolKind, label: String) {
        self.items.push((name.to_string(), at));
        self.define(at, name, kind, label);
    }

    /// Record that something is written on a type, for `Index::members`.
    fn own(&mut self, ty: &str, at: Span) {
        if at.line != 0 {
            self.members.entry(ty.to_string()).or_default().push(at);
        }
    }

    /// A definition a hover can show, wherever it was written.
    ///
    /// A definition is also a place a cursor can be, and asking there should
    /// answer with itself — hovering `fn area` on the line that declares it
    /// is the most obvious hover there is, and jumping from a definition
    /// should stay put rather than go nowhere.
    fn define(&mut self, at: Span, name: &str, kind: SymbolKind, label: String) {
        if at.line != 0 {
            self.defs.push(Definition {
                at,
                kind,
                name: name.to_string(),
                label,
                method: false,
            });
            self.uses.push((at, at));
        }
    }

    /// A method, which a rename must refuse: it is found by its spelling, and
    /// the prelude has methods this index never sees.
    fn define_method(&mut self, at: Span, name: &str, label: String) {
        self.define(at, name, SymbolKind::Method, label);
        if let Some(d) = self.defs.last_mut() {
            d.method = true;
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
    fn bind_and_mark(&mut self, name: &str, at: Span, kind: SymbolKind, label: String) {
        self.define(at, name, kind, label);
        self.bind(name, at);
    }

    /// Bindings made inside `f` are gone after it. Shadowing needs nothing
    /// more: `use_name` searches from the innermost outwards.
    ///
    /// `region` is what held them, and closing the scope is where each one
    /// learns how far it reached — which is what `visible` reads.
    fn scope<T>(&mut self, region: Span, f: impl FnOnce(&mut Self) -> T) -> T {
        let mark = self.scopes.len();
        let out = f(self);
        for b in self.scopes.drain(mark..) {
            if b.at.line != 0 {
                self.scoped.push(Scoped {
                    at: b.at,
                    from: b.at.hi,
                    to: region.hi,
                });
            }
        }
        out
    }

    /// Anything still bound when the file ends was bound outside every
    /// scope, which nothing here does — but draining is what records a
    /// binding, so this makes sure none is lost silently.
    fn close_file(&mut self) {
        debug_assert!(self.scopes.is_empty(), "a scope was left open");
        self.scopes.clear();
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
            Item::Mod(m) => Some(Symbol {
                name: m.name.clone(),
                kind: SymbolKind::Module,
                detail: String::new(),
                span: m.span,
                name_span: m.name_span,
                children: Vec::new(),
            }),
            Item::Use(_) => None,
            Item::Macro(m) => Some(Symbol {
                name: m.name.clone(),
                kind: SymbolKind::Macro,
                detail: String::new(),
                span: m.span,
                name_span: m.name_span,
                children: Vec::new(),
            }),
            Item::Alias(a) => {
                self.ty(&a.ty);
                Some(Symbol {
                    name: a.name.clone(),
                    kind: SymbolKind::Alias,
                    detail: type_name(&a.ty),
                    span: a.span,
                    name_span: a.name_span,
                    children: Vec::new(),
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
        self.scope(f.span, |b| {
            for p in &f.params {
                b.ty(&p.ty);
                b.bind_and_mark(
                    &p.name,
                    p.name_span,
                    SymbolKind::Parameter,
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
        self.scope(b.span, |s| {
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
                        s.define(*name_span, name, SymbolKind::Local, label);
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
            // The name, not the whole call: what a rename changes is
            // `helper`, and what it must not touch is `helper()`.
            Expr::Call(name, at, targs, args, _) => {
                self.use_name(name, *at);
                for t in targs {
                    self.ty(t);
                }
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Method(recv, name, at, args, _) => {
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
            Expr::Closure(params, ret, body, at) => {
                self.scope(*at, |s| {
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
                    self.scope(arm.span, |s| {
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
            // A macro call is an ordinary call for an editor's sake: the
            // name resolves and the arguments are expressions.
            Expr::MacroCall(name, args, at) => {
                self.use_name(name, *at);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::MacroRepeat(stmts, _) => {
                for s in stmts {
                    if let Stmt::Expr(e) = s {
                        self.expr(e);
                    }
                }
            }
            Expr::Int(..)
            | Expr::Trit(..)
            | Expr::Char(..)
            | Expr::Str(..)
            | Expr::Bool(..)
            | Expr::MacroParam(..)
            | Expr::Unit(_)
            | Expr::Continue(_) => {}
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            // A pattern binding's type is the scrutinee's, which needs a
            // type this cannot compute; the name is all there is to show.
            Pattern::Bind(name, at) => {
                self.bind_and_mark(name, *at, SymbolKind::Local, name.clone())
            }
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

/// Ch. 0 §1.3: a letter or `_`, then letters, digits or `_`.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !super::lex::KEYWORDS.contains(&text)
        && !super::lex::RESERVED.contains(&text)
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
        let names: Vec<(&str, SymbolKind)> = i
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
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

    /// The text each returned span covers, in order.
    fn spans_text(src: &str, spans: &[Span]) -> Vec<String> {
        spans
            .iter()
            .map(|s| {
                src.chars()
                    .skip(s.lo as usize)
                    .take((s.hi - s.lo) as usize)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn references_are_exact_and_not_textual() {
        // Two locals called `n` in two functions. Asking about one finds
        // three places, not six.
        let src = "fn a() -> t27 {\n    let n = 1;\n    n + n\n}\n                   fn b() -> t27 {\n    let n = 2;\n    n\n}\n";
        let i = index(src);
        let at = i.definition_at(src.find("n + n").unwrap() as u32).unwrap();
        let refs = i.references(at);
        assert_eq!(spans_text(src, &refs), vec!["n", "n", "n"]);
        assert!(refs.iter().all(|s| s.line <= 3), "{refs:?}");
    }

    #[test]
    fn a_rename_lists_the_definition_and_every_use() {
        let src = "fn helper() -> t27 { 1 }\nfn main() -> t27 { helper() + helper() }\n";
        let at = src.find("helper() +").unwrap() as u32;
        let spans = index(src).rename(at, "worker").expect("renames");
        assert_eq!(spans_text(src, &spans), vec!["helper"; 3]);
    }

    #[test]
    fn a_rename_refuses_a_method_because_the_prelude_is_not_here() {
        let src = "struct P;\nimpl P { fn go(&self) {} }\nfn f(p: P) { p.go() }\n";
        let at = src.find("p.go()").unwrap() as u32 + 2;
        assert_eq!(index(src).rename(at, "run"), Err(NoRename::Method));
    }

    #[test]
    fn a_rename_refuses_a_name_that_is_already_written_where_it_would_go() {
        // `a` to `b` would capture the `b` beside it.
        let src = "fn f(a: t27) -> t27 {\n    let b = 1;\n    a + b\n}\n";
        let at = src.find("a + b").unwrap() as u32;
        assert_eq!(
            index(src).rename(at, "b"),
            Err(NoRename::Taken("b".to_string()))
        );
        // A local of that name in *another* function is not a collision.
        let src = "fn f(a: t27) -> t27 { a }\nfn g() -> t27 {\n    let b = 1;\n    b\n}\n";
        let at = src.find("{ a }").unwrap() as u32 + 2;
        assert!(index(src).rename(at, "b").is_ok());
    }

    #[test]
    fn a_rename_refuses_what_is_not_a_name() {
        let src = "fn f(a: t27) -> t27 { a }\n";
        let at = src.find("{ a }").unwrap() as u32 + 2;
        let i = index(src);
        assert_eq!(i.rename(at, "2fast"), Err(NoRename::NotAnIdentifier));
        assert_eq!(i.rename(at, ""), Err(NoRename::NotAnIdentifier));
        // A keyword is not a name either (Ch. 0 §1.3).
        assert_eq!(i.rename(at, "match"), Err(NoRename::NotAnIdentifier));
        assert_eq!(i.rename(at, "unsafe"), Err(NoRename::NotAnIdentifier));
    }

    #[test]
    fn a_rename_refuses_where_there_is_no_name() {
        let src = "fn f() -> t27 { 1 + 2 }\n";
        let at = src.find("+").unwrap() as u32;
        assert_eq!(index(src).rename(at, "x"), Err(NoRename::NotAName));
    }

    fn names_at(src: &str, marker: &str) -> Vec<String> {
        let (src, offset) = at(&src.replace(marker, &format!("|{marker}")));
        index(&src)
            .visible(offset)
            .iter()
            .map(|d| d.name.clone())
            .collect()
    }

    #[test]
    fn what_is_visible_is_what_is_in_scope_and_nothing_else() {
        let src = "fn outer(p: t27) -> t27 {\n    let a = 1;\n    { let b = 2; b }\n    a\n}\n                   fn far() -> t27 { 0 }\n";
        // At the tail `a`, the block's `b` has closed.
        let here = names_at(src, "a\n}");
        assert!(here.contains(&"a".to_string()), "{here:?}");
        assert!(here.contains(&"p".to_string()), "the parameter: {here:?}");
        assert!(
            !here.contains(&"b".to_string()),
            "the block closed: {here:?}"
        );
        // Items are visible everywhere, in any order (§3).
        assert!(here.contains(&"far".to_string()), "{here:?}");
        assert!(here.contains(&"outer".to_string()), "{here:?}");
    }

    #[test]
    fn a_shadowed_name_is_offered_once_and_it_is_the_near_one() {
        let src = "const n: t27 = 1;\nfn f() -> t27 {\n    let n = 2;\n    n\n}\n";
        let names = names_at(src, "n\n}");
        assert_eq!(names.iter().filter(|x| *x == "n").count(), 1, "{names:?}");
        // The near one is the `let`, so its label is a `let`.
        let (source, offset) = at(&src.replace("n\n}", "|n\n}"));
        let d = index(&source).visible(offset)[0].clone();
        assert_eq!((d.name.as_str(), d.label.as_str()), ("n", "let n"));
    }

    #[test]
    fn a_for_loops_binding_is_in_scope_in_its_body() {
        // `for` is desugared into a `match` (§5.7), and the arm it becomes
        // has to reach as far as the body or the name is in scope nowhere.
        let src = "fn f(n: t27) -> t27 {\n    for i in 0..n {\n        n\n    }\n    n\n}\n";
        let inside = names_at(src, "n\n    }");
        assert!(inside.contains(&"i".to_string()), "{inside:?}");
        let after = names_at(src, "n\n}");
        assert!(
            !after.contains(&"i".to_string()),
            "the loop ended: {after:?}"
        );
    }

    #[test]
    fn a_for_loops_binding_is_where_the_file_wrote_it() {
        assert_eq!(
            jump("fn f(n: t27) { for i|tem in 0..n { } }\n").as_deref(),
            Some("item")
        );
    }

    #[test]
    fn the_prelude_offers_names_a_program_may_type() {
        let globals = prelude().globals();
        let names: Vec<&str> = globals.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"println"), "{names:?}");
        assert!(names.contains(&"Option"), "{names:?}");
        // Not what the compiler wrote for itself.
        assert!(!names.iter().any(|n| n.contains('.')), "{names:?}");
    }

    #[test]
    fn a_type_carries_what_is_written_on_it() {
        let src = "struct P { x: t27, y: t27 }\n                   impl P { fn area(&self) -> t27 { 0 } }\n                   trait Draw { fn draw(&self); }\n                   impl Draw for P { fn draw(&self) {} }\n";
        let i = index(src);
        let names: Vec<&str> = i.members("P").iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["area", "draw", "x", "y"]);
        // A reference's members are the referent's, and a `dyn Trait` has
        // the trait's.
        assert_eq!(i.members("&mut P").len(), 4);
        let dynamic: Vec<&str> = i
            .members("&dyn Draw")
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(dynamic, vec!["draw"]);
        assert!(i.members("t27").is_empty(), "a primitive has none here");
    }

    fn member_names(index: &Index, ty: &str) -> Vec<String> {
        index.members(ty).iter().map(|d| d.name.clone()).collect()
    }

    #[test]
    fn an_impl_on_one_instantiation_is_written_on_that_one_only() {
        // The prelude has `impl Vec<char> { push_str }` and a blanket
        // `impl FromIterator for Vec<A>`. A `Vec<t27>` has the second.
        let all = member_names(prelude(), "Vec<t27>");
        assert!(all.contains(&"from_iter".to_string()), "{all:?}");
        assert!(!all.contains(&"push_str".to_string()), "{all:?}");
        let chars = member_names(prelude(), "&mut Vec<char>");
        assert!(chars.contains(&"push_str".to_string()), "{chars:?}");
        assert!(
            chars.contains(&"from_iter".to_string()),
            "the blanket one too"
        );
    }

    #[test]
    fn what_the_language_writes_on_a_type_comes_from_the_compilers_own_table() {
        // `Vec::push` is nowhere in any file: its storage is the compiler's,
        // so the only place it is written down is the table `lower` matches
        // against, and that is the table this reads.
        let vec: Vec<&str> = builtin_members("Vec<t27>")
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert!(vec.contains(&"push"), "{vec:?}");
        assert!(vec.contains(&"len"), "{vec:?}");
        let int: Vec<&str> = builtin_members("&t9").iter().map(|(n, _)| *n).collect();
        assert!(int.contains(&"tmin"), "{int:?}");
        assert!(int.contains(&"checked_add"), "{int:?}");
        assert!(!int.contains(&"push"), "an integer is not a Vec: {int:?}");
        assert!(builtin_members("Point").is_empty(), "a struct has none");
        // A slice knows how long it is, and that is all.
        let slice: Vec<&str> = builtin_members("&[t27]").iter().map(|(n, _)| *n).collect();
        assert_eq!(slice, vec!["len", "is_empty"]);
    }

    #[test]
    fn a_cursor_on_nothing_answers_nothing() {
        assert_eq!(jump("fn f() -> t27 { 1 |+ 2 }\n"), None);
    }
}
