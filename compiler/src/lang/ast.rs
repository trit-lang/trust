//! The abstract syntax tree (Language Ch. 0 §§3–5).

use super::lex::Span;
use trit_core::{Bt, Trit};

/// A whole source file.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct File {
    /// Items, in the order written. Order does not affect visibility: every
    /// item in the file is visible to every other (§3).
    pub items: Vec<Item>,
}

/// A top-level definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Item {
    /// `fn name(params) -> ret { body }`, or a declaration when there is no
    /// body (§3.1).
    Fn(FnItem),
    /// `const NAME: T = expr;`
    Const(ConstItem),
    /// `struct Name { … }`, `struct Name(…);` or `struct Name;` (§3.3).
    Struct(StructItem),
    /// `enum Name { … }` (§3.3).
    Enum(EnumItem),
    /// `trait Name: Super { … }` (Ch. 4 §1.1).
    Trait(TraitItem),
    /// `impl Type { … }` or `impl Trait for Type { … }` (Ch. 4 §1.2).
    Impl(ImplItem),
    /// `mod name;` — a module, which is a file (Ch. 6 §1).
    Mod(ModItem),
    /// `use a::b::c;` — a name for something already visible (Ch. 6 §3.2).
    Use(UseItem),
    /// `type Name = T;` — another name for a type (Ch. 0 §3.6).
    Alias(AliasItem),
    /// `macro name($a, $b) { … }` (Ch. 7 §1).
    Macro(MacroItem),
}

/// `macro name($a, $($x),*) { body }` (Ch. 7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MacroItem {
    /// Its name.
    pub name: String,
    /// Where the name was written.
    pub name_span: Span,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// The fixed parameters, in order, without their `$`.
    pub params: Vec<String>,
    /// The trailing repetition's parameter, if it has one (§2).
    pub rest: Option<String>,
    /// The body, which is a block and expands to one (§3).
    pub body: Block,
    /// Where it was written.
    pub span: Span,
}

/// `type Name = T;`.
///
/// Only a name: `String` and `Vec<char>` are the same type, and a diagnostic
/// may say either (Ch. 0 §3.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AliasItem {
    /// Its name.
    pub name: String,
    /// Where the name was written.
    pub name_span: Span,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// What it names.
    pub ty: Ty,
    /// Where it was written.
    pub span: Span,
}

/// `mod name;`.
///
/// It carries no items: a module is a file, and which file is computed from
/// where this declaration was written (Ch. 6 §1.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModItem {
    /// Its name, which is also its file's stem.
    pub name: String,
    /// Where the name was written.
    pub name_span: Span,
    /// Whether the module is visible outside the one declaring it.
    pub public: bool,
    /// Where it was written.
    pub span: Span,
}

/// `use a::b::c;` — binds `c` in this module (Ch. 6 §3.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UseItem {
    /// The segments, in order. The last is the name it binds.
    pub segments: Vec<String>,
    /// Where the last segment was written, which is what the name means.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

impl Item {
    /// Where it was written, from its first keyword to its closing brace.
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(f) => f.span,
            Item::Const(c) => c.span,
            Item::Struct(s) => s.span,
            Item::Enum(e) => e.span,
            Item::Trait(t) => t.span,
            Item::Impl(i) => i.span,
            Item::Mod(m) => m.span,
            Item::Use(u) => u.span,
            Item::Alias(a) => a.span,
            Item::Macro(m) => m.span,
        }
    }

    /// Where its name was written — an `impl` has none, and answers with the
    /// `impl` keyword, which is what a reader points at when they mean it.
    pub fn name_span(&self) -> Span {
        match self {
            Item::Fn(f) => f.name_span,
            Item::Const(c) => c.name_span,
            Item::Struct(s) => s.name_span,
            Item::Enum(e) => e.name_span,
            Item::Trait(t) => t.name_span,
            Item::Impl(i) => i.span,
            Item::Mod(m) => m.name_span,
            Item::Use(u) => u.name_span,
            Item::Alias(a) => a.name_span,
            Item::Macro(m) => m.name_span,
        }
    }

    /// The same item, told how far it reaches (see `Expr::spanning`).
    pub fn spanning(mut self, span: Span) -> Item {
        match &mut self {
            Item::Fn(f) => f.span = span,
            Item::Const(c) => c.span = span,
            Item::Struct(s) => s.span = span,
            Item::Enum(e) => e.span = span,
            Item::Trait(t) => t.span = span,
            Item::Impl(i) => i.span = span,
            Item::Mod(m) => m.span = span,
            Item::Use(u) => u.span = span,
            Item::Alias(a) => a.span = span,
            Item::Macro(m) => m.span = span,
        }
        self
    }
}

/// Which of §4.3's three traits a closure bound names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FnKind {
    /// `Fn` — reads its captures only.
    Fn,
    /// `FnMut` — writes one.
    FnMut,
    /// `FnOnce` — moves one out.
    FnOnce,
}

impl FnKind {
    /// Its written name.
    pub fn name(self) -> &'static str {
        match self {
            FnKind::Fn => "Fn",
            FnKind::FnMut => "FnMut",
            FnKind::FnOnce => "FnOnce",
        }
    }
}

/// One generic parameter (Ch. 4 §2.1). Lifetimes are parsed and dropped,
/// since Ch. 3 §3.1 erases them; the other two reach code generation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GenericParam {
    /// `T: Bound + Other`.
    Type {
        /// Its name.
        name: String,
        /// The traits it is required to implement (§2.2).
        bounds: Vec<Bound>,
    },
    /// `const N: taddr`.
    Const {
        /// Its name.
        name: String,
        /// Its type, one of Ch. 1's integers or `bool` (§2.4).
        ty: Ty,
    },
}

impl GenericParam {
    /// Its name.
    pub fn name(&self) -> &str {
        match self {
            GenericParam::Type { name, .. } | GenericParam::Const { name, .. } => name,
        }
    }
}

/// A name with a type: a function's parameter, or a struct or variant's
/// field.
///
/// It is a struct and not a `(String, Ty)` because a name written in a file
/// has a place in it, and something that wants to point at `x` in
/// `fn f(x: t27)` cannot point at `t27` instead.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Named {
    /// Its name. A tuple struct's fields are named `0`, `1`, ….
    pub name: String,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Where the name was written.
    pub name_span: Span,
    /// Its type.
    pub ty: Ty,
}

impl Named {
    /// A name and a type, for the places the compiler invents one.
    pub fn new(name: impl Into<String>, ty: Ty) -> Named {
        Named {
            public: false,
            name: name.into(),
            name_span: ty.span(),
            ty,
        }
    }
}

/// A trait declaration (Ch. 4 §1.1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TraitItem {
    /// Its name.
    pub name: String,
    /// Its type parameters — `trait From<T>` (§1.7). A trait with these may
    /// be implemented by one type many times, once per argument; a trait
    /// without them, once.
    pub params: Vec<String>,
    /// Its supertraits (§1.6).
    pub supertraits: Vec<String>,
    /// Its methods, required (no body) or provided (with one, §1.5).
    pub methods: Vec<FnItem>,
    /// Its associated types, by name (Ch. 4 §1.7).
    pub assoc: Vec<String>,
    /// Its associated constants: name and type (Ch. 4 §1.7).
    pub consts: Vec<(String, Ty)>,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// One requirement on a type parameter: `T: From<U>` is `From` with `[U]`.
///
/// A bound with arguments is what makes a trait implementable many times by
/// one type, so the arguments are part of the requirement and not decoration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bound {
    /// The trait's name.
    pub name: String,
    /// Its type arguments, empty for a trait that takes none.
    pub args: Vec<Ty>,
    /// Associated type bindings: `Iterator<Item = t27>` (Ch. 4 §1.7). A
    /// binding constrains what the implementation chose, where an argument
    /// says which implementation is meant.
    pub assoc: Vec<(String, Ty)>,
}

impl Bound {
    /// A bound on a trait that takes no arguments.
    pub fn plain(name: impl Into<String>) -> Bound {
        Bound {
            name: name.into(),
            args: Vec::new(),
            assoc: Vec::new(),
        }
    }
}

/// An impl block (Ch. 4 §1.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImplItem {
    /// Its generic parameters (Ch. 4 §2.1).
    pub generics: Vec<GenericParam>,
    /// `impl !Copy for T` — the one negative implementation the language has
    /// (Ch. 4 §5.1).
    pub negative: bool,
    /// The trait being implemented, or `None` for an inherent impl.
    pub trait_name: Option<String>,
    /// The trait's type arguments — `impl From<t9> for t27` (§1.7).
    pub trait_args: Vec<Ty>,
    /// The type being implemented.
    pub self_ty: String,
    /// Set for `impl Trait for &Type`. The methods are keyed under the same
    /// name — a reference's methods have always been the referent's — but
    /// `Self` is the reference, which is what lets a method take `self` by
    /// value for a type that could not otherwise be passed at all
    /// (Ch. 4 §2.1).
    pub self_ref: bool,
    /// Set for `impl Trait for &mut Type`.
    pub self_mut: bool,
    /// Its type arguments: `impl<T> Pair<T, T>` (Ch. 4 §2.1).
    pub self_args: Vec<Ty>,
    /// Its methods and associated functions.
    pub methods: Vec<FnItem>,
    /// The types it chooses for the trait's associated ones (Ch. 4 §1.7).
    pub assoc: Vec<(String, Ty)>,
    /// The values it gives the trait's associated constants (Ch. 4 §1.7).
    pub consts: Vec<ConstItem>,
    /// Where it was written.
    pub span: Span,
}

/// How a nominal type is laid out (§3.4, Ch. 2 §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Repr {
    /// The default: the compiler may order, pad and exploit niches.
    #[default]
    Lang,
    /// Declaration order, documented offsets.
    Linear,
}

/// A struct item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StructItem {
    /// Its name.
    pub name: String,
    /// Its generic parameters (Ch. 4 §2.1).
    pub generics: Vec<GenericParam>,
    /// Traits derived for it (Ch. 4 §6).
    pub derives: Vec<String>,
    /// Its layout regime.
    pub repr: Repr,
    /// Fields in declaration order. A tuple struct's fields are named `0`,
    /// `1`, …; a unit struct has none.
    pub fields: Vec<Named>,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// An enum item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumItem {
    /// Its name.
    pub name: String,
    /// Its generic parameters (Ch. 4 §2.1).
    pub generics: Vec<GenericParam>,
    /// Traits derived for it (Ch. 4 §6).
    pub derives: Vec<String>,
    /// Its layout regime.
    pub repr: Repr,
    /// Its variants, in declaration order.
    pub variants: Vec<Variant>,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// One enum variant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    /// Its name.
    pub name: String,
    /// Its payload fields; empty for a fieldless variant.
    pub fields: Vec<Named>,
    /// An explicit discriminant, which may be negative (Ch. 2 §5.1).
    pub discriminant: Option<i128>,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// A function item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnItem {
    /// Its name.
    pub name: String,
    /// Its generic parameters (Ch. 4 §2.1).
    pub generics: Vec<GenericParam>,
    /// Its parameters.
    pub params: Vec<Named>,
    /// Its return type; `None` means `()`.
    pub ret: Option<Ty>,
    /// The body, or `None` for a declaration — a signature whose body is
    /// external, exactly as in TIR §1.
    pub body: Option<Block>,
    /// What the caller must have established, written in the same `where`
    /// clause as the type bounds and checked once on entry (Ch. 4 §2.8).
    pub requires: Vec<Expr>,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Whether `#[test]` was written on it: a function `trust test` calls
    /// and nothing else does. No chapter defines it — it is a *tool*
    /// convention, like the image format (docs/spec-gaps.md G9.68).
    pub test: bool,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// A constant item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConstItem {
    /// Its name.
    pub name: String,
    /// Its type, which is written out and not inferred (§3.2).
    pub ty: Ty,
    /// Its value.
    pub value: Expr,
    /// Whether it is visible outside the module defining it (Ch. 6 §2).
    pub public: bool,
    /// Where its name was written, which is where a reader means
    /// when they point at the item.
    pub name_span: Span,
    /// Where it was written.
    pub span: Span,
}

/// A type as written (§3.5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// A named primitive: `trit`, `bool`, `t9`, `t27`, `taddr`.
    Name(String, Span),
    /// `()`.
    Unit(Span),
    /// `!` — the type with no values (Ch. 1 §2). Writable only in return
    /// position, where it says the function does not return.
    Never(Span),
    /// `[T; N]`.
    Array(Box<Ty>, Box<Expr>, Span),
    /// `(T, U, …)`.
    Tuple(Vec<Ty>, Span),
    /// `&T` or `&mut T`; the lifetime, if written, is erased (Ch. 3 §3.1).
    Ref(Box<Ty>, bool, Span),
    /// `[T]` — dynamically sized, and legal only behind a reference.
    Slice(Box<Ty>, Span),
    /// `Self` — the implementing type, substituted away before lowering
    /// (Ch. 4 §1.2).
    SelfTy(Span),
    /// `Name<T, U>` — a generic type applied to arguments (Ch. 4 §2.1).
    /// Substituted and mangled to a plain `Name` before layout ever sees it.
    App(String, Vec<Ty>, Span),
    /// `dyn Trait` — dynamically sized, and legal only behind a reference
    /// (Ch. 4 §3.1).
    Dyn(String, Span),
    /// `T::Item` — an associated type (Ch. 4 §1.7).
    Assoc(Box<Ty>, String, Span),
    /// `impl Fn(T) -> R` in argument position: sugar for an anonymous type
    /// parameter bounded by one of §4.3's traits (Ch. 4 §2.2).
    ImplFn(FnKind, Vec<Ty>, Option<Box<Ty>>, Span),
}

impl Ty {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Ty::Name(_, l)
            | Ty::Unit(l)
            | Ty::Never(l)
            | Ty::Array(_, _, l)
            | Ty::Tuple(_, l)
            | Ty::Ref(_, _, l)
            | Ty::Slice(_, l)
            | Ty::SelfTy(l)
            | Ty::App(_, _, l)
            | Ty::Dyn(_, l)
            | Ty::Assoc(_, _, l)
            | Ty::ImplFn(_, _, _, l) => *l,
        }
    }

    /// The same type, told how wide it turned out to be.
    ///
    /// A production knows where it started before it knows where it ends, so
    /// it builds the node with the span of its first token and widens it once
    /// the last is read (`Parser::since`).
    pub fn spanning(mut self, span: Span) -> Ty {
        match &mut self {
            Ty::Name(_, l)
            | Ty::Unit(l)
            | Ty::Never(l)
            | Ty::Array(_, _, l)
            | Ty::Tuple(_, l)
            | Ty::Ref(_, _, l)
            | Ty::Slice(_, l)
            | Ty::SelfTy(l)
            | Ty::App(_, _, l)
            | Ty::Dyn(_, l)
            | Ty::Assoc(_, _, l)
            | Ty::ImplFn(_, _, _, l) => *l = span,
        }
        self
    }
}

/// A block: statements, then an optional trailing expression whose value is
/// the block's (§5.1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Block {
    /// The statements.
    pub stmts: Vec<Stmt>,
    /// The trailing expression, if any.
    pub tail: Option<Box<Expr>>,
    /// Where it opened.
    pub span: Span,
}

/// A statement (§5.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Stmt {
    /// `let mut? name: T? = expr;`
    Let {
        /// Whether the binding may be assigned again.
        mutable: bool,
        /// The bound name.
        name: String,
        /// Where the name was written, which is where a jump to it lands.
        name_span: Span,
        /// Its written type, if any.
        ty: Option<Ty>,
        /// Its initializer.
        value: Expr,
        /// A pattern to take the value apart with, when one was written
        /// (§4). `name` then binds the whole and is moved into the pattern,
        /// which is what lets a struct with two non-`Copy` fields be taken
        /// apart at all (G9.46).
        ///
        /// Boxed because a `Stmt` is mostly `Let` and mostly has no pattern,
        /// and a `Pattern` is the largest thing in one.
        pattern: Option<Box<Pattern>>,
        /// Where it was written.
        span: Span,
    },
    /// An expression evaluated for its effect.
    Expr(Expr),
}

/// An expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expr {
    /// An integer literal, whose type is inferred (§5.2).
    Int(Bt, Span),
    /// A `trit` literal.
    Trit(Trit, Span),
    /// A character literal, as its Unicode scalar value (Ch. 5 §1.4).
    Char(i128, Span),
    /// A string literal, as its characters. Its type is `&'static str`, and
    /// its storage is a global (Ch. 5 §1.4).
    Str(Vec<i128>, Span),
    /// `e?` — propagate a failure (Ch. 5 §4.1).
    Try(Box<Expr>, Span),
    /// `(e)(args)` — call whatever an expression evaluates to. The only
    /// thing it can be is a closure, and the only place one is not already a
    /// name is a field (Ch. 4 §4.2).
    CallExpr(Box<Expr>, Vec<Expr>, Span),
    /// `true` or `false`.
    Bool(bool, Span),
    /// `()`.
    Unit(Span),
    /// A name: a local, a parameter, or a constant.
    Path(String, Span),
    /// `[a, b, c]`.
    Array(Vec<Expr>, Span),
    /// `[value; count]`.
    Repeat(Box<Expr>, Box<Expr>, Span),
    /// A unary operator: `-`, `!`.
    Unary(&'static str, Box<Expr>, Span),
    /// A binary operator (§2.1).
    Binary(&'static str, Box<Expr>, Box<Expr>, Span),
    /// `lhs = rhs`, or a compound form, which is sugar for `lhs = lhs op rhs`.
    Assign(&'static str, Box<Expr>, Box<Expr>, Span),
    /// `x as T`.
    Cast(Box<Expr>, Ty, Span),
    /// `f(args)`, with any type arguments written `f::<T>(args)`.
    ///
    /// Two spans, because two questions have different answers: the last is
    /// the whole call, which is what a diagnostic about it underlines, and
    /// the first is the callee's name, which is what a rename changes and
    /// what a hover is about.
    Call(String, Span, Vec<Ty>, Vec<Expr>, Span),
    /// `receiver.method(args)` — how the trit-wise operations are spelled
    /// (Ch. 1 §4, Ch. 0 §2.5).
    /// The spans are the method's name and then the whole call, for the
    /// reason `Call` gives.
    Method(Box<Expr>, String, Span, Vec<Expr>, Span),
    /// `base[index]`.
    Index(Box<Expr>, Box<Expr>, Span),
    /// `x.field` or `x.0`.
    Field(Box<Expr>, String, Span),
    /// `&place` or `&mut place`.
    Borrow(Box<Expr>, bool, Span),
    /// `*r`.
    Deref(Box<Expr>, Span),
    /// `(a, b, …)`.
    Tuple(Vec<Expr>, Span),
    /// `Name { field: value, … }` or `Name(a, b)` or `Name::Variant …`.
    Aggregate(Path, Vec<(String, Expr)>, Span),
    /// A block used as an expression.
    Block(Block),
    /// `if cond { … } else { … }`.
    If(Box<Expr>, Block, Option<Box<Expr>>, Span),
    /// `match scrutinee { arms }`.
    Match(Box<Expr>, Vec<Arm>, Span),
    /// `loop { … }`.
    Loop(Block, Span),
    /// `while cond { … }`.
    While(Box<Expr>, Block, Span),
    /// `for name in iter { … }` (Ch. 4 §5.7).
    ///
    /// It is §5.7's desugaring written as itself, and becomes the `loop` and
    /// `match` there at *lowering*. Draft 0.1 desugared it in the parser,
    /// which put a construct in the language that no tree ever showed: an
    /// editor could not point at it, and a second parser could not agree
    /// about it without also agreeing about the names the desugaring
    /// invents.
    For(String, Span, Box<Expr>, Block, Span),
    /// `break` with an optional value.
    Break(Option<Box<Expr>>, Span),
    /// `continue`.
    Continue(Span),
    /// `return` with an optional value.
    Return(Option<Box<Expr>>, Span),
    /// `|x| body` or `|x: T| -> R { body }` (Ch. 4 §4.1).
    Closure(Vec<(String, Option<Ty>)>, Option<Ty>, Box<Expr>, Span),
    /// `name!(args)` — a macro invocation, which expands to an expression
    /// before anything else reads the program (Ch. 7 §5). Nothing below the
    /// frontend's expansion pass ever sees one.
    MacroCall(String, Vec<Expr>, Span),
    /// `$name` inside a macro body — a parameter, substituted at expansion.
    MacroParam(String, Span),
    /// `$( … )*` inside a macro body — repeated once per argument the
    /// repetition bound (Ch. 7 §3).
    MacroRepeat(Vec<Stmt>, Span),
}

impl Expr {
    /// Where it was written.
    pub fn span(&self) -> Span {
        use Expr::*;
        match self {
            Int(_, l)
            | Trit(_, l)
            | Char(_, l)
            | Str(_, l)
            | Try(_, l)
            | CallExpr(_, _, l)
            | Bool(_, l)
            | Unit(l)
            | Path(_, l)
            | Array(_, l)
            | Repeat(_, _, l)
            | Unary(_, _, l)
            | Binary(_, _, _, l)
            | Assign(_, _, _, l)
            | Cast(_, _, l)
            | Call(_, _, _, _, l)
            | Method(_, _, _, _, l)
            | Index(_, _, l)
            | Field(_, _, l)
            | Borrow(_, _, l)
            | Deref(_, l)
            | Tuple(_, l)
            | Aggregate(_, _, l)
            | If(_, _, _, l)
            | Match(_, _, l)
            | Loop(_, l)
            | While(_, _, l)
            | For(_, _, _, _, l)
            | Break(_, l)
            | Continue(l)
            | Return(_, l)
            | Closure(_, _, _, l)
            | MacroCall(_, _, l)
            | MacroParam(_, l)
            | MacroRepeat(_, l) => *l,
            Block(b) => b.span,
        }
    }

    /// The same expression, told how wide it turned out to be.
    ///
    /// Every expression's span is the whole of what it covers rather than the
    /// token that names it: `a + b` reaches from `a` to `b`, not to the `+`.
    /// What reads a span is something drawing a line under what is wrong, and
    /// what is wrong with `a + b` is `a + b`.
    pub fn spanning(mut self, span: Span) -> Expr {
        use Expr::*;
        match &mut self {
            Int(_, l)
            | Trit(_, l)
            | Char(_, l)
            | Str(_, l)
            | Try(_, l)
            | CallExpr(_, _, l)
            | Bool(_, l)
            | Unit(l)
            | Path(_, l)
            | Array(_, l)
            | Repeat(_, _, l)
            | Unary(_, _, l)
            | Binary(_, _, _, l)
            | Assign(_, _, _, l)
            | Cast(_, _, l)
            | Call(_, _, _, _, l)
            | Method(_, _, _, _, l)
            | Index(_, _, l)
            | Field(_, _, l)
            | Borrow(_, _, l)
            | Deref(_, l)
            | Tuple(_, l)
            | Aggregate(_, _, l)
            | If(_, _, _, l)
            | Match(_, _, l)
            | Loop(_, l)
            | While(_, _, l)
            | For(_, _, _, _, l)
            | Break(_, l)
            | Continue(l)
            | Return(_, l)
            | Closure(_, _, _, l) => *l = span,
            MacroCall(_, _, l) | MacroParam(_, l) | MacroRepeat(_, l) => *l = span,
            Block(b) => b.span = span,
        }
        self
    }
}

/// One arm of a `match` (§5.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Arm {
    /// The patterns; more than one means an or-pattern (§4).
    pub patterns: Vec<Pattern>,
    /// An optional guard. A guarded arm never counts toward exhaustiveness.
    pub guard: Option<Expr>,
    /// The arm's value.
    pub body: Expr,
    /// Where it was written.
    pub span: Span,
}

/// A path: one name, or a type and a variant (`Sign::Neg`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Path {
    /// The segments, in order.
    pub segments: Vec<String>,
    /// Type arguments written with `::<…>` (Ch. 4 §2.3).
    pub targs: Vec<Ty>,
    /// Where it was written.
    pub span: Span,
}

impl Path {
    /// The last segment.
    pub fn last(&self) -> &str {
        self.segments.last().expect("a path has segments")
    }
}

/// A pattern (§4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pattern {
    /// `_` — matches anything, binds nothing.
    Wild(Span),
    /// A binding.
    Bind(String, Span),
    /// An integer literal.
    Int(Bt, Span),
    /// A `trit` literal.
    Trit(Trit, Span),
    /// A character literal, as its Unicode scalar value (Ch. 5 §1.4).
    Char(i128, Span),
    /// `true` or `false`.
    Bool(bool, Span),
    /// A struct or variant pattern: `Sign::Neg`, `Shape::Line(n)`,
    /// `Point { x, y }`.
    Aggregate(Path, Vec<(String, Pattern)>, Span),
    /// `(a, b)`.
    Tuple(Vec<Pattern>, Span),
}

impl Pattern {
    /// Where it was written.
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wild(l)
            | Pattern::Bind(_, l)
            | Pattern::Int(_, l)
            | Pattern::Trit(_, l)
            | Pattern::Char(_, l)
            | Pattern::Bool(_, l)
            | Pattern::Aggregate(_, _, l)
            | Pattern::Tuple(_, l) => *l,
        }
    }

    /// The same pattern, told how wide it turned out to be.
    pub fn spanning(mut self, span: Span) -> Pattern {
        match &mut self {
            Pattern::Wild(l)
            | Pattern::Bind(_, l)
            | Pattern::Int(_, l)
            | Pattern::Trit(_, l)
            | Pattern::Char(_, l)
            | Pattern::Bool(_, l)
            | Pattern::Aggregate(_, _, l)
            | Pattern::Tuple(_, l) => *l = span,
        }
        self
    }
}

/// Visit every type written in a file.
///
/// The AST knows its own shape, so the walk lives here rather than in each
/// pass that wants one. `f` sees a type before its parts, so a substitution
/// that replaces a whole type is not then asked about the pieces it replaced.
/// The hooks a walk over a file's types offers: every type, and every path
/// in expression position — `Enum::Variant`, `Type::function(…)`.
///
/// Two, because the head of such a path names a **type** and is not written
/// as one: `String::new()` is `Vec<char>`'s `new`, and an alias is a name
/// for a type wherever a type is named (Ch. 0 §3.6, G9.87). A walk that saw
/// only written types would expand the alias in `let s: String` and leave
/// the one beside it alone.
pub struct Visit<'a> {
    /// Every type written in the file, and every type inside one.
    pub ty: &'a mut dyn FnMut(&mut Ty),
    /// Every path in expression position, head first.
    pub path: &'a mut dyn FnMut(&mut Path),
}

/// Every type in a file, in no particular order.
pub fn for_each_ty(file: &mut File, f: &mut impl FnMut(&mut Ty)) {
    walk(
        file,
        &mut Visit {
            ty: f,
            path: &mut |_| {},
        },
    )
}

/// Every type *and* every path in expression position.
pub fn for_each_ty_and_path(
    file: &mut File,
    ty: &mut dyn FnMut(&mut Ty),
    path: &mut dyn FnMut(&mut Path),
) {
    walk(file, &mut Visit { ty, path })
}

fn walk(file: &mut File, w: &mut Visit<'_>) {
    for item in &mut file.items {
        match item {
            Item::Fn(fun) => fn_tys(fun, w),
            Item::Const(c) => ty(&mut c.ty, w),
            Item::Alias(a) => ty(&mut a.ty, w),
            Item::Struct(s) => {
                for n in &mut s.fields {
                    ty(&mut n.ty, w);
                }
            }
            Item::Enum(e) => {
                for v in &mut e.variants {
                    for n in &mut v.fields {
                        ty(&mut n.ty, w);
                    }
                }
            }
            Item::Trait(t) => {
                for (_, c) in &mut t.consts {
                    ty(c, w);
                }
                for m in &mut t.methods {
                    fn_tys(m, w);
                }
            }
            Item::Impl(i) => {
                for a in i.trait_args.iter_mut().chain(i.self_args.iter_mut()) {
                    ty(a, w);
                }
                for (_, t) in &mut i.assoc {
                    ty(t, w);
                }
                for c in &mut i.consts {
                    ty(&mut c.ty, w);
                }
                for m in &mut i.methods {
                    fn_tys(m, w);
                }
            }
            // A macro's body is not a program until it is expanded, and it
            // is expanded before this runs (Ch. 7 §5).
            Item::Macro(_) | Item::Mod(_) | Item::Use(_) => {}
        }
    }
}

/// Every type in one function's signature and body.
fn fn_tys(fun: &mut FnItem, w: &mut Visit<'_>) {
    for g in &mut fun.generics {
        if let GenericParam::Const { ty: t, .. } = g {
            ty(t, w);
        }
    }
    for p in &mut fun.params {
        ty(&mut p.ty, w);
    }
    if let Some(r) = &mut fun.ret {
        ty(r, w);
    }
    if let Some(b) = &mut fun.body {
        block_tys(b, w);
    }
}

fn block_tys(b: &mut Block, w: &mut Visit<'_>) {
    for s in &mut b.stmts {
        match s {
            Stmt::Let { ty: t, value, .. } => {
                if let Some(t) = t {
                    ty(t, w);
                }
                expr_tys(value, w);
            }
            Stmt::Expr(e) => expr_tys(e, w),
        }
    }
    if let Some(t) = &mut b.tail {
        expr_tys(t, w);
    }
}

fn expr_tys(e: &mut Expr, w: &mut Visit<'_>) {
    match e {
        Expr::Cast(v, t, _) => {
            expr_tys(v, w);
            ty(t, w);
        }
        Expr::Call(_, _, targs, args, _) => {
            for t in targs {
                ty(t, w);
            }
            for a in args {
                expr_tys(a, w);
            }
        }
        Expr::Closure(params, ret, body, _) => {
            for (_, t) in params {
                if let Some(t) = t {
                    ty(t, w);
                }
            }
            if let Some(r) = ret {
                ty(r, w);
            }
            expr_tys(body, w);
        }
        Expr::Aggregate(path, fields, _) => {
            (w.path)(path);
            for t in &mut path.targs {
                ty(t, w);
            }
            for (_, value) in fields {
                expr_tys(value, w);
            }
        }
        Expr::Block(b) => block_tys(b, w),
        Expr::If(c, then, other, _) => {
            expr_tys(c, w);
            block_tys(then, w);
            if let Some(o) = other {
                expr_tys(o, w);
            }
        }
        Expr::Match(v, arms, _) => {
            expr_tys(v, w);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    expr_tys(g, w);
                }
                expr_tys(&mut a.body, w);
            }
        }
        Expr::While(c, b, _) => {
            expr_tys(c, w);
            block_tys(b, w);
        }
        Expr::For(_, _, it, b, _) => {
            expr_tys(it, w);
            block_tys(b, w);
        }
        Expr::Loop(b, _) => block_tys(b, w),
        Expr::Method(r, _, _, args, _) => {
            expr_tys(r, w);
            for a in args {
                expr_tys(a, w);
            }
        }
        Expr::CallExpr(g, args, _) => {
            expr_tys(g, w);
            for a in args {
                expr_tys(a, w);
            }
        }
        Expr::Field(v, _, _)
        | Expr::Unary(_, v, _)
        | Expr::Borrow(v, _, _)
        | Expr::Deref(v, _)
        | Expr::Try(v, _) => expr_tys(v, w),
        Expr::Repeat(a, b, _)
        | Expr::Binary(_, a, b, _)
        | Expr::Assign(_, a, b, _)
        | Expr::Index(a, b, _) => {
            expr_tys(a, w);
            expr_tys(b, w);
        }
        Expr::Array(items, _) | Expr::Tuple(items, _) => {
            for i in items {
                expr_tys(i, w);
            }
        }
        Expr::Break(held, _) | Expr::Return(held, _) => {
            if let Some(held) = held {
                expr_tys(held, w);
            }
        }
        _ => {}
    }
}

/// One type, and then its parts.
fn ty(t: &mut Ty, w: &mut Visit<'_>) {
    (w.ty)(t);
    match t {
        Ty::Array(inner, count, _) => {
            ty(inner, w);
            expr_tys(count, w);
        }
        Ty::Ref(inner, _, _) | Ty::Slice(inner, _) | Ty::Assoc(inner, _, _) => ty(inner, w),
        Ty::Tuple(items, _) | Ty::App(_, items, _) => {
            for i in items {
                ty(i, w);
            }
        }
        Ty::ImplFn(_, args, ret, _) => {
            for a in args {
                ty(a, w);
            }
            if let Some(r) = ret {
                ty(r, w);
            }
        }
        _ => {}
    }
}

/// Visit an expression's immediate sub-expressions.
///
/// The AST knows its own shape, so the walk lives here. It descends into a
/// `$( … )*` group's statements too: a group is part of the expression it
/// sits in, and something renaming or substituting has to reach inside it.
pub fn for_each_child_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match e {
        Expr::Unary(_, a, _)
        | Expr::Borrow(a, _, _)
        | Expr::Deref(a, _)
        | Expr::Try(a, _)
        | Expr::Field(a, _, _)
        | Expr::Cast(a, _, _) => f(a),
        Expr::Binary(_, a, b, _)
        | Expr::Assign(_, a, b, _)
        | Expr::Index(a, b, _)
        | Expr::Repeat(a, b, _) => {
            f(a);
            f(b);
        }
        Expr::Call(_, _, _, args, _) | Expr::Array(args, _) | Expr::Tuple(args, _) => {
            for a in args {
                f(a);
            }
        }
        Expr::MacroCall(_, args, _) => {
            for a in args {
                f(a);
            }
        }
        Expr::Method(r, _, _, args, _) => {
            f(r);
            for a in args {
                f(a);
            }
        }
        Expr::CallExpr(c, args, _) => {
            f(c);
            for a in args {
                f(a);
            }
        }
        Expr::Aggregate(_, fields, _) => {
            for (_, v) in fields {
                f(v);
            }
        }
        Expr::Closure(_, _, body, _) => f(body),
        Expr::Block(b) => block_children(b, f),
        Expr::If(c, then, other, _) => {
            f(c);
            block_children(then, f);
            if let Some(o) = other {
                f(o);
            }
        }
        Expr::Match(v, arms, _) => {
            f(v);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    f(g);
                }
                f(&mut a.body);
            }
        }
        Expr::While(c, b, _) => {
            f(c);
            block_children(b, f);
        }
        Expr::For(_, _, it, b, _) => {
            f(it);
            block_children(b, f);
        }
        Expr::Loop(b, _) => block_children(b, f),
        Expr::Break(v, _) | Expr::Return(v, _) => {
            if let Some(v) = v {
                f(v);
            }
        }
        Expr::MacroRepeat(stmts, _) => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } => f(value),
                    Stmt::Expr(e) => f(e),
                }
            }
        }
        Expr::Int(..)
        | Expr::Trit(..)
        | Expr::Char(..)
        | Expr::Str(..)
        | Expr::Bool(..)
        | Expr::Unit(_)
        | Expr::Path(..)
        | Expr::Continue(_)
        | Expr::MacroParam(..) => {}
    }
}

fn block_children(b: &mut Block, f: &mut impl FnMut(&mut Expr)) {
    for s in &mut b.stmts {
        match s {
            Stmt::Let { value, .. } => f(value),
            Stmt::Expr(e) => f(e),
        }
    }
    if let Some(t) = &mut b.tail {
        f(t);
    }
}
