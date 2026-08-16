//! Modules (Language Ch. 6): a program is a tree of files, and this makes it
//! one file again.
//!
//! Two steps, and they are separate because they fail differently.
//!
//! **Loading** reads the root, finds its `mod` declarations, computes where
//! each one's file is (§1.3), and repeats. What it can fail at is a file that
//! is not there.
//!
//! **Resolution** renames. A module is a naming construct and nothing else
//! (§4), so every item is renamed to its full path with `.` for `::`, every
//! path written in that module is rewritten to what it names, and the result
//! is one `ast::File` in which no name is ambiguous. Everything downstream —
//! layout, lowering, monomorphization — is unchanged, and none of it knows
//! that modules exist.
//!
//! What resolution can fail at is a name: not found, not visible, or written
//! twice. Each of those is reported where it was written, in the file it was
//! written in, which is what the `file` on a `Span` is for.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::ast::*;
use super::lex::{File as SourceFile, Span, SyntaxError};

/// One module: where it came from, what it is called, and its text.
#[derive(Clone)]
pub struct Source {
    /// Its path from the root, empty for the root itself.
    pub path: Vec<String>,
    /// The file it was read from, for a diagnostic to name.
    pub file: PathBuf,
    /// Its text, which a `LineMap` needs to place anything in it.
    pub text: String,
}

impl Source {
    /// How a diagnostic names it.
    pub fn label(&self) -> String {
        self.file.display().to_string()
    }
}

/// A program, loaded but not yet resolved.
#[derive(Clone)]
pub struct Program {
    /// Every module, indexed by the `file` on its spans.
    pub sources: Vec<Source>,
    /// Each module's items, in the same order.
    pub parsed: Vec<File>,
    /// Anything wrong with the files themselves.
    pub errors: Vec<SyntaxError>,
}

/// Read a program from its root file, following `mod` declarations (§1.3).
///
/// A module that cannot be read is an error and the rest is still loaded: an
/// editor asks about a program with a missing file as readily as a finished
/// one.
pub fn load(root: &Path) -> Program {
    let mut p = Program {
        sources: Vec::new(),
        parsed: Vec::new(),
        errors: Vec::new(),
    };
    let text = match std::fs::read_to_string(root) {
        Ok(t) => t,
        Err(e) => {
            p.errors.push(SyntaxError {
                span: Span::NONE,
                message: format!("cannot read `{}`: {e}", root.display()),
            });
            return p;
        }
    };
    read(&mut p, Vec::new(), root.to_path_buf(), text);
    p
}

/// A program from text with no files under it, for a test or an editor
/// holding one buffer.
pub fn one(text: &str) -> Program {
    let mut p = Program {
        sources: Vec::new(),
        parsed: Vec::new(),
        errors: Vec::new(),
    };
    read(&mut p, Vec::new(), PathBuf::from("-"), text.to_string());
    p
}

/// Parse one module and read whatever it declares.
fn read(p: &mut Program, path: Vec<String>, file: PathBuf, text: String) {
    let id = p.sources.len() as SourceFile;
    let (parsed, errs) = super::parse::parse_recovering_in(&text, id);
    p.errors.extend(errs);
    p.sources.push(Source {
        path: path.clone(),
        file: file.clone(),
        text,
    });
    p.parsed.push(parsed);

    // §1.3: a module declared in `<dir>/p.tr` is `<dir>/p/m.tr`, and one
    // declared in the root is beside it. The root's own stem names no
    // directory, which is what makes those two rules one.
    let dir = match path.is_empty() {
        true => file.parent().unwrap_or(Path::new(".")).to_path_buf(),
        false => file.with_extension(""),
    };
    let mods: Vec<ModItem> = p.parsed[id as usize]
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Mod(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    for m in &mods {
        if !seen.insert(&m.name) {
            p.errors.push(SyntaxError {
                span: m.name_span,
                message: format!("`{}` is declared more than once (Ch. 6 §1.4)", m.name),
            });
            continue;
        }
        let child = dir.join(format!("{}.tr", m.name));
        let mut sub = path.clone();
        sub.push(m.name.clone());
        match std::fs::read_to_string(&child) {
            Ok(t) => read(p, sub, child, t),
            Err(e) => p.errors.push(SyntaxError {
                span: m.name_span,
                message: format!(
                    "cannot read `{}` for `mod {}`: {e}",
                    child.display(),
                    m.name
                ),
            }),
        }
    }
}

/// What one module can name, and what each name means.
struct Scope {
    /// Item name to full path, for what this module defines and what it
    /// brought in by `use`.
    names: HashMap<String, String>,
    /// Module name to full path, for what a path's first segment may be.
    mods: HashMap<String, String>,
}

/// Every item in a program, in one file, with every name resolved (§4).
///
/// The result is what the rest of the compiler has always been given: a flat
/// list of items whose names are unique. A single-file program comes through
/// unchanged, because the root's path is empty and `join` of nothing is the
/// name itself.
pub fn resolve(p: &Program) -> (File, Vec<SyntaxError>) {
    let mut errors = Vec::new();

    // Pass one: what each module defines, and which module paths exist.
    let mut w = World {
        modules: HashMap::new(),
        defined: Vec::new(),
        public: HashMap::new(),
        kinds: HashMap::new(),
        sources: &p.sources,
    };
    for (i, file) in p.parsed.iter().enumerate() {
        let here = &p.sources[i].path;
        let mut mine = HashMap::new();
        for item in &file.items {
            if let Item::Mod(m) = item {
                w.modules.insert(join(here, &m.name), m.public);
                continue;
            }
            let Some(name) = defines(item) else { continue };
            let full = join(here, name);
            if mine.insert(name.to_string(), full.clone()).is_some() {
                errors.push(SyntaxError {
                    span: item.name_span(),
                    message: format!("`{name}` is defined more than once in this module"),
                });
            }
            w.public.insert(full.clone(), is_public(item));
            w.kinds.insert(full, kind_of(item));
            // An enum's variants are named through the enum, so they need no
            // entry of their own; a path stops at the enum (§3.1).
        }
        w.defined.push(mine);
    }

    // Pass two: each module's scope, which is what it defines, the modules
    // it declares, and what its `use`s bring in.
    let mut scopes: Vec<Scope> = Vec::new();
    for (i, file) in p.parsed.iter().enumerate() {
        let here = &p.sources[i].path;
        let mut scope = Scope {
            names: w.defined[i].clone(),
            mods: HashMap::new(),
        };
        for item in &file.items {
            if let Item::Mod(m) = item {
                scope.mods.insert(m.name.clone(), join(here, &m.name));
            }
        }
        for item in &file.items {
            let Item::Use(u) = item else { continue };
            let Some(last) = u.segments.last() else {
                continue;
            };
            match lookup(&u.segments, here, &[], &w) {
                Ok((full, took)) if took == u.segments.len() => {
                    // A `use` of a *module* binds a module name, which is
                    // what a path's first segment may be — so it goes where
                    // a declared module goes and not only among the items.
                    if w.modules.contains_key(&full) {
                        scope.mods.insert(last.clone(), full.clone());
                    }
                    if scope.names.insert(last.clone(), full).is_some() {
                        errors.push(SyntaxError {
                            span: u.name_span,
                            message: format!(
                                "`{last}` is already a name in this module, and a `use` does \
                                 not shadow (Ch. 6 §3.2)"
                            ),
                        });
                    }
                }
                Ok((full, _)) => errors.push(SyntaxError {
                    span: u.name_span,
                    message: format!(
                        "`{}` names something inside `{full}`, and a `use` names an item or \
                         a module (Ch. 6 §3.2)",
                        u.segments.join("::")
                    ),
                }),
                Err(why) => errors.push(SyntaxError {
                    span: u.name_span,
                    message: why,
                }),
            }
        }
        scopes.push(scope);
    }

    // Pass three: rename everything.
    let mut out = File::default();
    for (i, file) in p.parsed.iter().enumerate() {
        let here = &p.sources[i].path;
        for item in &file.items {
            if matches!(item, Item::Mod(_) | Item::Use(_)) {
                continue;
            }
            let mut item = item.clone();
            rename_item(&mut item, here);
            let mut r = Rewriter {
                scope: &scopes[i],
                here,
                w: &w,
                errors: &mut errors,
                locals: Vec::new(),
            };
            r.item(&mut item);
            out.items.push(item);
        }
    }
    (out, errors)
}

/// A path joined with `.`, which is what §4 makes a module path below the
/// language: TIR §1 admits a dot in an identifier and not a colon.
fn join(path: &[String], name: &str) -> String {
    if path.is_empty() {
        return name.to_string();
    }
    format!("{}.{name}", path.join("."))
}

/// The name an item defines, or `None` for an `impl`, which defines none.
fn defines(i: &Item) -> Option<&str> {
    match i {
        Item::Fn(f) => Some(&f.name),
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Trait(t) => Some(&t.name),
        Item::Const(c) => Some(&c.name),
        Item::Alias(a) => Some(&a.name),
        Item::Macro(m) => Some(&m.name),
        Item::Impl(_) | Item::Mod(_) | Item::Use(_) => None,
    }
}

fn is_public(i: &Item) -> bool {
    match i {
        Item::Fn(f) => f.public,
        Item::Struct(s) => s.public,
        Item::Enum(e) => e.public,
        Item::Trait(t) => t.public,
        Item::Const(c) => c.public,
        Item::Alias(a) => a.public,
        Item::Macro(m) => m.public,
        Item::Impl(_) | Item::Mod(_) | Item::Use(_) => false,
    }
}

/// Resolve a written path from module `here` to a full name (§3.1).
///
/// The first segment names a module, and then the rest is a path within it,
/// or it names an item and there is no rest. That order is what tells
/// `lang::lex::Span` from `Sign::Neg`.
/// `here` is where the path was written, which decides **visibility**;
/// `from` is where it starts, which decides **resolution**. They are the same
/// for a path in an expression and differ for a `use`, which is absolute
/// (§3.1) and is the one way a module names something outside itself.
fn lookup(
    segments: &[String],
    here: &[String],
    from: &[String],
    w: &World,
) -> Result<(String, usize), String> {
    // Walk as far as the segments keep naming modules. What stops the walk
    // is an item, and whatever follows *it* — a variant, an associated name
    // — is not this pass's business (§3.1).
    let mut at = from.to_vec();
    let mut taken = 0;
    while taken < segments.len() {
        let mut next = at.clone();
        next.push(segments[taken].clone());
        if !w.modules.contains_key(&next.join(".")) {
            break;
        }
        at = next;
        taken += 1;
    }
    if taken == segments.len() {
        // The path ended at a module — `use lang::lex;` names one.
        return Ok((at.join("."), taken));
    }
    let name = &segments[taken];
    let Some(i) = w.sources.iter().position(|s| s.path == at) else {
        let what = match at.is_empty() {
            true => format!("`{name}` is not a module or an item in scope"),
            false => format!("`{}` has no source", at.join("::")),
        };
        return Err(what);
    };
    let Some(full) = w.defined[i].get(name) else {
        return Err(match at.is_empty() {
            true => format!("`{name}` is not a module or an item in scope"),
            false => format!("`{name}` is not in `{}`", at.join("::")),
        });
    };
    // Visibility is checked against where the path was *written*: a module
    // can see into what is inside it (§2.1), and a `use` grants no access
    // however far from the root it resolved (§3.2).
    if !w.public.get(full).copied().unwrap_or(false) && !inside(here, &at) {
        return Err(format!(
            "`{name}` is not `pub`, so it is visible only in `{}` and what is inside it \
             (Ch. 6 §2.1)",
            at.join("::")
        ));
    }
    Ok((full.clone(), taken + 1))
}

/// Everything resolution knows about the program, gathered once.
struct World<'a> {
    /// Every module path that exists, and whether it is `pub`.
    modules: HashMap<String, bool>,
    /// Per source, the items it defines, by written name.
    defined: Vec<HashMap<String, String>>,
    /// Whether each full name is `pub`.
    public: HashMap<String, bool>,
    /// What kind of thing each full name is, for §4's shape corrections.
    kinds: HashMap<String, Kind>,
    sources: &'a [Source],
}

/// What a resolved name turned out to be. A written path does not say which
/// of these it is, and the node it sits in has to be the right shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A function, so `m::f(x)` is a call and not a literal.
    Fn,
    /// A constant, so `m::N` is a value and not a unit struct.
    Const,
    /// A struct, enum or trait.
    Type,
    /// A macro, which is invoked and never named on its own (Ch. 7 §1).
    Macro,
}

/// Whether module `here` is `at` or inside it.
fn inside(here: &[String], at: &[String]) -> bool {
    here.len() >= at.len() && here[..at.len()] == *at
}

/// Give an item's own name its module path.
fn rename_item(item: &mut Item, here: &[String]) {
    match item {
        Item::Fn(f) => f.name = join(here, &f.name),
        Item::Struct(s) => s.name = join(here, &s.name),
        // A variant keeps the name it was written with: it is named through
        // its enum (`Kind::Trit`), and the enum's name is what carries the
        // module (§3.1).
        Item::Enum(e) => e.name = join(here, &e.name),
        Item::Trait(t) => t.name = join(here, &t.name),
        Item::Const(c) => c.name = join(here, &c.name),
        Item::Alias(a) => a.name = join(here, &a.name),
        Item::Macro(m) => m.name = join(here, &m.name),
        Item::Impl(_) | Item::Mod(_) | Item::Use(_) => {}
    }
}

/// Rewrites every name written in one module to what it names.
struct Rewriter<'a> {
    scope: &'a Scope,
    here: &'a [String],
    w: &'a World<'a>,
    errors: &'a mut Vec<SyntaxError>,
    /// Names bound nearer than the module: locals, parameters and type
    /// parameters, innermost last.
    locals: Vec<String>,
}

impl Rewriter<'_> {
    /// What a single written name means here, or itself if nothing does.
    ///
    /// A name nothing here defines is the prelude's, or a type parameter, or
    /// a local — all of which are resolved further down and none of which
    /// this may touch (§3.3).
    fn name(&self, n: &str) -> String {
        self.scope
            .names
            .get(n)
            .cloned()
            .unwrap_or_else(|| n.to_string())
    }

    /// A written path. Answers what the head resolved to, when it named
    /// something through a module — which is what tells the node's shape.
    fn path(&mut self, segments: &mut Vec<String>, at: Span) -> Option<Kind> {
        // A path whose head names a module resolves *inside* that module.
        // The head may be a submodule declared here or a name a `use`
        // brought in, and both answer with the module's full path — so the
        // rest is looked up from there and not from where it was written.
        if segments.len() >= 2
            && let Some(base) = self.scope.mods.get(&segments[0]).cloned()
        {
            let from: Vec<String> = base.split('.').map(str::to_string).collect();
            let rest: Vec<String> = segments[1..].to_vec();
            return match lookup(&rest, self.here, &from, self.w) {
                Ok((full, took)) => {
                    let kind = self.w.kinds.get(&full).copied();
                    let tail: Vec<String> = rest[took..].to_vec();
                    *segments = std::iter::once(full).chain(tail).collect();
                    kind
                }
                Err(why) => {
                    self.errors.push(SyntaxError {
                        span: at,
                        message: why,
                    });
                    None
                }
            };
        }
        segments[0] = self.local_or_name(&segments[0]);
        None
    }
}

/// What kind of thing an item is, for §4's shape corrections.
fn kind_of(i: &Item) -> Kind {
    match i {
        Item::Fn(_) => Kind::Fn,
        Item::Macro(_) => Kind::Macro,
        Item::Const(_) => Kind::Const,
        _ => Kind::Type,
    }
}

impl Rewriter<'_> {
    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(f) => self.function(f),
            Item::Const(c) => {
                self.ty(&mut c.ty);
                self.expr(&mut c.value);
            }
            Item::Struct(s) => {
                for f in &mut s.fields {
                    self.ty(&mut f.ty);
                }
            }
            Item::Enum(e) => {
                for v in &mut e.variants {
                    for f in &mut v.fields {
                        self.ty(&mut f.ty);
                    }
                }
            }
            Item::Trait(t) => {
                for s in &mut t.supertraits {
                    *s = self.name(s);
                }
                for (_, ty) in &mut t.consts {
                    self.ty(ty);
                }
                for m in &mut t.methods {
                    self.function(m);
                }
            }
            Item::Impl(i) => {
                if let Some(t) = &mut i.trait_name {
                    *t = self.name(t);
                }
                i.self_ty = self.name(&i.self_ty);
                for a in i.trait_args.iter_mut().chain(i.self_args.iter_mut()) {
                    self.ty(a);
                }
                for (_, ty) in &mut i.assoc {
                    self.ty(ty);
                }
                for c in &mut i.consts {
                    self.ty(&mut c.ty);
                    self.expr(&mut c.value);
                }
                for m in &mut i.methods {
                    self.function(m);
                }
            }
            Item::Alias(a) => self.ty(&mut a.ty),
            // A macro's body is rewritten like any other code: the names it
            // uses are resolved where it was written (Ch. 7 §4.1).
            Item::Macro(m) => {
                let mut b = m.body.clone();
                self.block(&mut b);
                m.body = b;
            }
            Item::Mod(_) | Item::Use(_) => {}
        }
    }

    fn function(&mut self, f: &mut FnItem) {
        let mark = self.locals.len();
        // A type parameter is a name for a type and not a path to one, so it
        // hides whatever it is spelled like for the whole signature.
        for g in &f.generics {
            self.locals.push(g.name().to_string());
        }
        for p in &mut f.params {
            self.ty(&mut p.ty);
            self.locals.push(p.name.clone());
        }
        if let Some(r) = &mut f.ret {
            self.ty(r);
        }
        for p in &mut f.requires {
            self.expr(p);
        }
        if let Some(b) = &mut f.body {
            self.block(b);
        }
        self.locals.truncate(mark);
    }

    fn ty(&mut self, t: &mut Ty) {
        match t {
            Ty::Name(n, _) | Ty::Dyn(n, _) => *n = self.local_or_name(n),
            // `lex::Taken` — a type named through a module. The parser reads
            // it as an associated type, which is what it would be if `lex`
            // were one; a module makes it an ordinary name (Ch. 6 §4).
            Ty::Assoc(base, name, at) => {
                if let Ty::Name(head, _) = &**base
                    && self.scope.mods.contains_key(head)
                {
                    let mut segments = vec![head.clone(), name.clone()];
                    let at = *at;
                    self.path(&mut segments, at);
                    if segments.len() == 1 {
                        *t = Ty::Name(segments.remove(0), at);
                        return;
                    }
                }
                self.ty(base);
            }
            Ty::App(n, args, _) => {
                *n = self.local_or_name(n);
                for a in args {
                    self.ty(a);
                }
            }
            Ty::Array(inner, count, _) => {
                self.ty(inner);
                self.expr(count);
            }
            Ty::Ref(inner, _, _) | Ty::Slice(inner, _) => self.ty(inner),
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

    fn block(&mut self, b: &mut Block) {
        let mark = self.locals.len();
        for s in &mut b.stmts {
            match s {
                Stmt::Let {
                    name, ty, value, ..
                } => {
                    // The initializer is read before the name is bound
                    // (Ch. 0 §5.2), so `let n = n;` means the outer one.
                    self.expr(value);
                    if let Some(t) = ty {
                        self.ty(t);
                    }
                    self.locals.push(name.clone());
                }
                Stmt::Expr(e) => self.expr(e),
            }
        }
        if let Some(t) = &mut b.tail {
            self.expr(t);
        }
        self.locals.truncate(mark);
    }

    fn expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Path(n, _) => *n = self.local_or_name(n),
            Expr::Call(n, _, targs, args, _) => {
                // A call by name is a function, a tuple struct or a variant,
                // and never a local: calling one is `CallExpr` (Ch. 4 §4.2).
                *n = self.name(n);
                for t in targs {
                    self.ty(t);
                }
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Method(recv, _, _, args, _) => {
                self.expr(recv);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Aggregate(..) => self.aggregate(e),
            Expr::Cast(v, t, _) => {
                self.expr(v);
                self.ty(t);
            }
            Expr::Closure(params, ret, body, _) => {
                let mark = self.locals.len();
                for (n, t) in params {
                    if let Some(t) = t {
                        self.ty(t);
                    }
                    self.locals.push(n.clone());
                }
                if let Some(r) = ret {
                    self.ty(r);
                }
                self.expr(body);
                self.locals.truncate(mark);
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
                    let mark = self.locals.len();
                    for p in &mut arm.patterns {
                        self.pattern(p);
                    }
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.expr(&mut arm.body);
                    self.locals.truncate(mark);
                }
            }
            Expr::While(c, body, _) => {
                self.expr(c);
                self.block(body);
            }
            Expr::Loop(body, _) => self.block(body),
            Expr::Field(v, _, _)
            | Expr::Unary(_, v, _)
            | Expr::Borrow(v, _, _)
            | Expr::Deref(v, _)
            | Expr::Try(v, _) => self.expr(v),
            Expr::Repeat(a, b, _)
            | Expr::Binary(_, a, b, _)
            | Expr::Assign(_, a, b, _)
            | Expr::Index(a, b, _) => {
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
            // A macro's arguments are the caller's expressions and are
            // rewritten like any others; `$x` is neither a name nor a place.
            Expr::MacroCall(_, args, _) => {
                for a in args {
                    self.expr(a);
                }
            }
            Expr::MacroRepeat(stmts, _) => {
                for s in stmts {
                    match s {
                        Stmt::Let { value, .. } => self.expr(value),
                        Stmt::Expr(e) => self.expr(e),
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

    fn pattern(&mut self, p: &mut Pattern) {
        match p {
            Pattern::Bind(n, _) => self.locals.push(n.clone()),
            Pattern::Aggregate(path, fields, _) => {
                let at = path.span;
                self.path(&mut path.segments, at);
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

    /// `m::f(x)` and `m::N` are parsed as aggregates, because a path of two
    /// segments followed by `(` is a tuple literal and one followed by
    /// nothing is a unit literal — which is what they are when the head is a
    /// type. When the head is a **function** or a **constant** they are a
    /// call and a value, and the node has to become one.
    ///
    /// Nothing else in the compiler has to know: a module is a naming
    /// construct (§4), so this is the whole of what one changes.
    fn aggregate(&mut self, e: &mut Expr) {
        let Expr::Aggregate(path, fields, span) = e else {
            unreachable!("called on an aggregate")
        };
        let at = path.span;
        let kind = self.path(&mut path.segments, at);
        for t in &mut path.targs {
            self.ty(t);
        }
        for (_, v) in fields.iter_mut() {
            self.expr(v);
        }
        let one = path.segments.len() == 1;
        match kind {
            Some(Kind::Fn) if one => {
                let args = fields.iter().map(|(_, v)| v.clone()).collect();
                *e = Expr::Call(
                    path.segments[0].clone(),
                    at,
                    path.targs.clone(),
                    args,
                    *span,
                );
            }
            Some(Kind::Const) if one && fields.is_empty() => {
                *e = Expr::Path(path.segments[0].clone(), *span);
            }
            _ => {}
        }
    }

    /// A written name, unless something nearer is called that.
    ///
    /// A local, a parameter or a type parameter hides an item of the same
    /// name, exactly as it does when the whole program is one file.
    fn local_or_name(&self, n: &str) -> String {
        if self.locals.iter().any(|l| l == n) {
            return n.to_string();
        }
        self.name(n)
    }
}
