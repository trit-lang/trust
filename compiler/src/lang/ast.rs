//! The abstract syntax tree (Language Ch. 0 §§3–5).

use super::lex::Line;
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
    /// Where it was written.
    pub line: Line,
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
}

impl Bound {
    /// A bound on a trait that takes no arguments.
    pub fn plain(name: impl Into<String>) -> Bound {
        Bound {
            name: name.into(),
            args: Vec::new(),
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
    /// Its type arguments: `impl<T> Pair<T, T>` (Ch. 4 §2.1).
    pub self_args: Vec<Ty>,
    /// Its methods and associated functions.
    pub methods: Vec<FnItem>,
    /// The types it chooses for the trait's associated ones (Ch. 4 §1.7).
    pub assoc: Vec<(String, Ty)>,
    /// The values it gives the trait's associated constants (Ch. 4 §1.7).
    pub consts: Vec<ConstItem>,
    /// Where it was written.
    pub line: Line,
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
    pub fields: Vec<(String, Ty)>,
    /// Where it was written.
    pub line: Line,
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
    /// Where it was written.
    pub line: Line,
}

/// One enum variant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    /// Its name.
    pub name: String,
    /// Its payload fields; empty for a fieldless variant.
    pub fields: Vec<(String, Ty)>,
    /// An explicit discriminant, which may be negative (Ch. 2 §5.1).
    pub discriminant: Option<i128>,
    /// Where it was written.
    pub line: Line,
}

/// A function item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnItem {
    /// Its name.
    pub name: String,
    /// Its generic parameters (Ch. 4 §2.1).
    pub generics: Vec<GenericParam>,
    /// Its parameters.
    pub params: Vec<(String, Ty)>,
    /// Its return type; `None` means `()`.
    pub ret: Option<Ty>,
    /// The body, or `None` for a declaration — a signature whose body is
    /// external, exactly as in TIR §1.
    pub body: Option<Block>,
    /// Where it was written.
    pub line: Line,
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
    /// Where it was written.
    pub line: Line,
}

/// A type as written (§3.5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// A named primitive: `trit`, `bool`, `t9`, `t27`, `taddr`.
    Name(String, Line),
    /// `()`.
    Unit(Line),
    /// `[T; N]`.
    Array(Box<Ty>, Box<Expr>, Line),
    /// `(T, U, …)`.
    Tuple(Vec<Ty>, Line),
    /// `&T` or `&mut T`; the lifetime, if written, is erased (Ch. 3 §3.1).
    Ref(Box<Ty>, bool, Line),
    /// `[T]` — dynamically sized, and legal only behind a reference.
    Slice(Box<Ty>, Line),
    /// `Self` — the implementing type, substituted away before lowering
    /// (Ch. 4 §1.2).
    SelfTy(Line),
    /// `Name<T, U>` — a generic type applied to arguments (Ch. 4 §2.1).
    /// Substituted and mangled to a plain `Name` before layout ever sees it.
    App(String, Vec<Ty>, Line),
    /// `dyn Trait` — dynamically sized, and legal only behind a reference
    /// (Ch. 4 §3.1).
    Dyn(String, Line),
    /// `T::Item` — an associated type (Ch. 4 §1.7).
    Assoc(Box<Ty>, String, Line),
    /// `impl Fn(T) -> R` in argument position: sugar for an anonymous type
    /// parameter bounded by one of §4.3's traits (Ch. 4 §2.2).
    ImplFn(FnKind, Vec<Ty>, Option<Box<Ty>>, Line),
}

impl Ty {
    /// Where it was written.
    pub fn line(&self) -> Line {
        match self {
            Ty::Name(_, l)
            | Ty::Unit(l)
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
    pub line: Line,
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
        /// Its written type, if any.
        ty: Option<Ty>,
        /// Its initializer.
        value: Expr,
        /// Where it was written.
        line: Line,
    },
    /// An expression evaluated for its effect.
    Expr(Expr),
}

/// An expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expr {
    /// An integer literal, whose type is inferred (§5.2).
    Int(Bt, Line),
    /// A `trit` literal.
    Trit(Trit, Line),
    /// A character literal, as its Unicode scalar value (Ch. 5 §1.4).
    Char(i128, Line),
    /// `true` or `false`.
    Bool(bool, Line),
    /// `()`.
    Unit(Line),
    /// A name: a local, a parameter, or a constant.
    Path(String, Line),
    /// `[a, b, c]`.
    Array(Vec<Expr>, Line),
    /// `[value; count]`.
    Repeat(Box<Expr>, Box<Expr>, Line),
    /// A unary operator: `-`, `!`.
    Unary(&'static str, Box<Expr>, Line),
    /// A binary operator (§2.1).
    Binary(&'static str, Box<Expr>, Box<Expr>, Line),
    /// `lhs = rhs`, or a compound form, which is sugar for `lhs = lhs op rhs`.
    Assign(&'static str, Box<Expr>, Box<Expr>, Line),
    /// `x as T`.
    Cast(Box<Expr>, Ty, Line),
    /// `f(args)`, with any type arguments written `f::<T>(args)`.
    Call(String, Vec<Ty>, Vec<Expr>, Line),
    /// `receiver.method(args)` — how the trit-wise operations are spelled
    /// (Ch. 1 §4, Ch. 0 §2.5).
    Method(Box<Expr>, String, Vec<Expr>, Line),
    /// `base[index]`.
    Index(Box<Expr>, Box<Expr>, Line),
    /// `x.field` or `x.0`.
    Field(Box<Expr>, String, Line),
    /// `&place` or `&mut place`.
    Borrow(Box<Expr>, bool, Line),
    /// `*r`.
    Deref(Box<Expr>, Line),
    /// `(a, b, …)`.
    Tuple(Vec<Expr>, Line),
    /// `Name { field: value, … }` or `Name(a, b)` or `Name::Variant …`.
    Aggregate(Path, Vec<(String, Expr)>, Line),
    /// A block used as an expression.
    Block(Block),
    /// `if cond { … } else { … }`.
    If(Box<Expr>, Block, Option<Box<Expr>>, Line),
    /// `match scrutinee { arms }`.
    Match(Box<Expr>, Vec<Arm>, Line),
    /// `loop { … }`.
    Loop(Block, Line),
    /// `while cond { … }`.
    While(Box<Expr>, Block, Line),
    /// `break` with an optional value.
    Break(Option<Box<Expr>>, Line),
    /// `continue`.
    Continue(Line),
    /// `return` with an optional value.
    Return(Option<Box<Expr>>, Line),
    /// `|x| body` or `|x: T| -> R { body }` (Ch. 4 §4.1).
    Closure(Vec<(String, Option<Ty>)>, Option<Ty>, Box<Expr>, Line),
}

impl Expr {
    /// Where it was written.
    pub fn line(&self) -> Line {
        use Expr::*;
        match self {
            Int(_, l)
            | Trit(_, l)
            | Char(_, l)
            | Bool(_, l)
            | Unit(l)
            | Path(_, l)
            | Array(_, l)
            | Repeat(_, _, l)
            | Unary(_, _, l)
            | Binary(_, _, _, l)
            | Assign(_, _, _, l)
            | Cast(_, _, l)
            | Call(_, _, _, l)
            | Method(_, _, _, l)
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
            | Break(_, l)
            | Continue(l)
            | Return(_, l)
            | Closure(_, _, _, l) => *l,
            Block(b) => b.line,
        }
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
    pub line: Line,
}

/// A path: one name, or a type and a variant (`Sign::Neg`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Path {
    /// The segments, in order.
    pub segments: Vec<String>,
    /// Type arguments written with `::<…>` (Ch. 4 §2.3).
    pub targs: Vec<Ty>,
    /// Where it was written.
    pub line: Line,
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
    Wild(Line),
    /// A binding.
    Bind(String, Line),
    /// An integer literal.
    Int(Bt, Line),
    /// A `trit` literal.
    Trit(Trit, Line),
    /// A character literal, as its Unicode scalar value (Ch. 5 §1.4).
    Char(i128, Line),
    /// `true` or `false`.
    Bool(bool, Line),
    /// A struct or variant pattern: `Sign::Neg`, `Shape::Line(n)`,
    /// `Point { x, y }`.
    Aggregate(Path, Vec<(String, Pattern)>, Line),
    /// `(a, b)`.
    Tuple(Vec<Pattern>, Line),
}

impl Pattern {
    /// Where it was written.
    pub fn line(&self) -> Line {
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
}
