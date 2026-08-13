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
}

/// A function item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnItem {
    /// Its name.
    pub name: String,
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
}

impl Ty {
    /// Where it was written.
    pub fn line(&self) -> Line {
        match self {
            Ty::Name(_, l) | Ty::Unit(l) | Ty::Array(_, _, l) => *l,
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
    /// `f(args)`.
    Call(String, Vec<Expr>, Line),
    /// `receiver.method(args)` — how the trit-wise operations are spelled
    /// (Ch. 1 §4, Ch. 0 §2.5).
    Method(Box<Expr>, String, Vec<Expr>, Line),
    /// `base[index]`.
    Index(Box<Expr>, Box<Expr>, Line),
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
}

impl Expr {
    /// Where it was written.
    pub fn line(&self) -> Line {
        use Expr::*;
        match self {
            Int(_, l)
            | Trit(_, l)
            | Bool(_, l)
            | Unit(l)
            | Path(_, l)
            | Array(_, l)
            | Repeat(_, _, l)
            | Unary(_, _, l)
            | Binary(_, _, _, l)
            | Assign(_, _, _, l)
            | Cast(_, _, l)
            | Call(_, _, l)
            | Method(_, _, _, l)
            | Index(_, _, l)
            | If(_, _, _, l)
            | Match(_, _, l)
            | Loop(_, l)
            | While(_, _, l)
            | Break(_, l)
            | Continue(l)
            | Return(_, l) => *l,
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
    /// `true` or `false`.
    Bool(bool, Line),
}

impl Pattern {
    /// Where it was written.
    pub fn line(&self) -> Line {
        match self {
            Pattern::Wild(l)
            | Pattern::Bind(_, l)
            | Pattern::Int(_, l)
            | Pattern::Trit(_, l)
            | Pattern::Bool(_, l) => *l,
        }
    }
}
