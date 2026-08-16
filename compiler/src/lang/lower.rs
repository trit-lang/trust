//! Type checking and lowering to TIR (Language Ch. 0, Ch. 1).
//!
//! One pass does both: an expression is checked and emitted together, with
//! the expected type passed down so that an integer literal takes its type
//! from context (Ch. 1 §3) rather than needing inference machinery.
//!
//! # Every local lives in a slot
//!
//! TIR is SSA, and a mutable local is not. Rather than construct SSA in the
//! frontend, every local gets a `slot`: a read is a `load`, a write is a
//! `store`. Control flow then needs no block parameters at all, because no
//! value crosses a block edge in a register — which is why this file has no
//! phi placement in it. The cost is real and is paid back by the optimizer
//! that does not exist yet; the benefit is that this pass is small enough to
//! be obviously right.
//!
//! Expressions whose value is produced in more than one block — `if`, `match`,
//! a `loop` with `break`-with-value — land in a temporary slot for the same
//! reason.

use super::ast;
use super::lex::{Span, SyntaxError};
use crate::layout;
use crate::tir::ir::{self, *};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use trit_core::{Bt, FaultCode, Flavor};

/// A type-checking error.
pub type Error = SyntaxError;

type R<T> = Result<T, Error>;

fn err<T>(span: Span, message: impl Into<String>) -> R<T> {
    Err(SyntaxError {
        span,
        message: message.into(),
    })
}

/// The types this milestone knows (Ch. 1 §2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// `trit`.
    Trit,
    /// `bool` — a distinct nominal type, not a subrange of `trit`.
    Bool,
    /// `t9`.
    T9,
    /// `t27`.
    T27,
    /// `taddr` — distinct from `t27` even where it is the same width, so that
    /// mixing them needs an explicit `as` (Ch. 1, P2).
    TAddr,
    /// `char` — a Unicode scalar value, one word (Ch. 5 §1.2). A scalar like
    /// the integers, and not one of them: it is not arithmetic, and the only
    /// conversion is the explicit one to `t27`.
    Char,
    /// `()`.
    Unit,
    /// `[T; N]`.
    Array(Box<Ty>, i128),
    /// `(T, U, …)` — an anonymous product, always `repr(lang)` (Ch. 2 §2).
    Tuple(Vec<Ty>),
    /// A named struct.
    Struct(String),
    /// A named enum.
    Enum(String),
    /// `&T` or `&mut T`. A reference to a sized type is one word; a reference
    /// to a slice is two (Ch. 3 §5.2).
    Ref(Box<Ty>, bool),
    /// `Box<T>` — one `T` on the heap, owned (Ch. 5 §2.3). One word, never
    /// null, and dropped by dropping the `T` and then freeing.
    Boxed(Box<Ty>),
    /// `Vec<T>` — a growable array (Ch. 5 §2.6). Three words: the
    /// allocation, the number of elements in it, and how many it has room
    /// for. A language item for the same reason `Box` is, and one more: the
    /// room beyond the length is memory that is *not yet a `T`*, and this
    /// language has no way to say that.
    VecOf(Box<Ty>),
    /// `[T]` — dynamically sized, and never the type of a place.
    Slice(Box<Ty>),
    /// `dyn Trait` — dynamically sized, and never the type of a place
    /// (Ch. 4 §3.1).
    Dyn(String),
    /// The type of an expression that never produces a value: `break`,
    /// `continue`, `return`.
    Never,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Trit => f.write_str("trit"),
            Ty::Bool => f.write_str("bool"),
            Ty::T9 => f.write_str("t9"),
            Ty::T27 => f.write_str("t27"),
            Ty::TAddr => f.write_str("taddr"),
            Ty::Char => f.write_str("char"),
            Ty::Boxed(t) => write!(f, "Box<{t}>"),
            Ty::VecOf(t) => write!(f, "Vec<{t}>"),
            Ty::Unit => f.write_str("()"),
            Ty::Dyn(t) => write!(f, "dyn {t}"),
            Ty::Array(t, n) => write!(f, "[{t}; {n}]"),
            Ty::Tuple(ts) => {
                let parts: Vec<String> = ts.iter().map(Ty::to_string).collect();
                write!(f, "({})", parts.join(", "))
            }
            Ty::Struct(n) | Ty::Enum(n) => f.write_str(n),
            Ty::Ref(t, true) => write!(f, "&mut {t}"),
            Ty::Ref(t, false) => write!(f, "&{t}"),
            // `str` is `[char]`, and a reader who wrote `str` should see it
            // back (Ch. 5 §1.3).
            Ty::Slice(t) if **t == Ty::Char => f.write_str("str"),
            Ty::Slice(t) => write!(f, "[{t}]"),
            Ty::Never => f.write_str("!"),
        }
    }
}

impl Ty {
    /// The TIR type a value of this type is held in.
    fn tir(&self) -> Type {
        match self {
            Ty::Trit | Ty::Bool => Type::Int(1),
            Ty::T9 => Type::Int(9),
            Ty::T27 | Ty::TAddr | Ty::Char | Ty::Never | Ty::Unit => Type::Int(27),
            // Unsized on its own; only a reference to one is a value.
            Ty::Dyn(_) => Type::Ptr,
            // A thin reference is an address — a word-sized value. So is a
            // `Box`, which differs from one in what it owns, not in what it
            // holds (Ch. 5 §2.3).
            Ty::Boxed(_) => Type::Ptr,
            Ty::Ref(t, _) if !t.is_unsized() => Type::Ptr,
            // An aggregate is never an SSA value (TIR §2); it lives in
            // memory and its value is its address. A fat reference is two
            // words and travels the same way.
            Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Struct(_)
            | Ty::Enum(_)
            | Ty::Ref(..)
            | Ty::VecOf(_)
            | Ty::Slice(_) => Type::Ptr,
        }
    }

    /// Width in trits, for the numeric types.
    fn width(&self) -> Option<u32> {
        match self {
            Ty::Trit | Ty::Bool => Some(1),
            Ty::T9 => Some(9),
            Ty::T27 | Ty::TAddr | Ty::Char => Some(27),
            _ => None,
        }
    }

    /// True for the types arithmetic operates on (Ch. 1 §4).
    fn is_arithmetic(&self) -> bool {
        matches!(self, Ty::T9 | Ty::T27 | Ty::TAddr)
    }

    /// Whether a value of this type is held in a register or in memory.
    fn is_scalar(&self) -> bool {
        match self {
            Ty::Ref(t, _) => !t.is_unsized(),
            Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Struct(_)
            | Ty::Enum(_)
            | Ty::Unit
            | Ty::Never
            | Ty::VecOf(_)
            | Ty::Slice(_) => false,
            _ => true,
        }
    }

    /// True for the dynamically sized types, which are never the type of a
    /// place and appear only behind a reference (Ch. 3 §5.1).
    fn is_unsized(&self) -> bool {
        matches!(self, Ty::Slice(_) | Ty::Dyn(_))
    }

    /// True for the aggregates, which live in memory and are named by their
    /// address.
    fn is_aggregate(&self) -> bool {
        match self {
            // A fat reference is two words and travels like an aggregate:
            // by address, and copied field by field (Ch. 3 §5.2).
            Ty::Ref(t, _) => t.is_unsized(),
            Ty::Array(..) | Ty::Tuple(_) | Ty::Struct(_) | Ty::Enum(_) | Ty::VecOf(_) => true,
            _ => false,
        }
    }

    /// The layout-engine spelling of this type.
    fn layout_ty(&self) -> layout::Ty {
        match self {
            Ty::Trit => layout::Ty::Trit,
            Ty::Bool => layout::Ty::Bool,
            Ty::T9 => layout::Ty::Int(layout::IntTy::T9),
            Ty::T27 => layout::Ty::Int(layout::IntTy::T27),
            Ty::TAddr => layout::Ty::Int(layout::IntTy::TAddr),
            Ty::Char => layout::Ty::Char,
            Ty::Unit | Ty::Never => layout::Ty::Unit,
            Ty::Array(t, n) => layout::Ty::array(t.layout_ty(), *n),
            Ty::Tuple(ts) => layout::Ty::Tuple(ts.iter().map(Ty::layout_ty).collect()),
            Ty::Struct(n) | Ty::Enum(n) => layout::Ty::named(n),
            // A `Box` is pointer-sized and never null, which is the layout
            // engine's `Box` and the source of `Option<Box<T>>`'s niche.
            Ty::Boxed(_) => layout::Ty::boxed(layout::Ty::Unit),
            // A `Vec` is the allocation, the length, and the capacity —
            // three words, in that order (Ch. 5 §2.6). The pointer is *not* a
            // `Box` here: an empty `Vec` has no allocation, so 0 is a value
            // it takes and not a niche it offers.
            Ty::VecOf(_) => layout::Ty::Tuple(vec![
                layout::Ty::Int(layout::IntTy::TAddr),
                layout::Ty::Int(layout::IntTy::TAddr),
                layout::Ty::Int(layout::IntTy::TAddr),
            ]),
            // A thin reference has the layout of a reference; a fat one is a
            // pointer and a length (Ch. 3 §5.2).
            Ty::Ref(t, _) if !t.is_unsized() => layout::Ty::reference(layout::Ty::Unit),
            Ty::Ref(..) => layout::Ty::Tuple(vec![
                layout::Ty::reference(layout::Ty::Unit),
                layout::Ty::Int(layout::IntTy::TAddr),
            ]),
            // A slice and a trait object have no layout of their own; only a
            // reference to one does, and both are two words (Ch. 3 §5.2,
            // Ch. 4 §3.2).
            Ty::Slice(_) | Ty::Dyn(_) => layout::Ty::Unit,
        }
    }
}

/// A place, as a root local and a path of projections (Ch. 3 §1.3).
///
/// Two places conflict when one is a prefix of the other, with an index
/// matching any element — which is what makes `xs[0]` and `xs[1]` conflict
/// here although they do not overlap. Distinguishing them needs to know the
/// indices, and a checker that guesses is worse than one that is coarse.
#[derive(Clone, PartialEq, Eq, Debug)]
struct PlacePath {
    root: String,
    projections: Vec<Proj>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Proj {
    Field(String),
    Index,
}

impl PlacePath {
    fn conflicts(&self, other: &PlacePath) -> bool {
        if self.root != other.root {
            return false;
        }
        let n = self.projections.len().min(other.projections.len());
        self.projections[..n] == other.projections[..n]
    }
}

/// A borrow that is still live (Ch. 3 §2.2).
#[derive(Clone, Debug)]
struct Loan {
    place: PlacePath,
    mutable: bool,
    /// The statement after which this loan is dead. A borrow lives to its
    /// last use, not to the end of its scope (Ch. 3 §4.2).
    dies: u32,
    span: Span,
}

/// What is being done to a place, for the aliasing rule.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
    Move,
    Borrow(bool),
}

/// Whether a local still owns its value (Ch. 3 §1.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owns {
    /// It does.
    Yes,
    /// It was moved out of.
    No,
    /// One path moved out of it and another did not; a drop needs a flag.
    Maybe,
}

impl Owns {
    /// Joining two paths: a value moved on either is not certainly owned.
    fn join(self, other: Owns) -> Owns {
        match (self, other) {
            (Owns::Yes, Owns::Yes) => Owns::Yes,
            (Owns::No, Owns::No) => Owns::No,
            _ => Owns::Maybe,
        }
    }
}

/// A local binding.
#[derive(Clone)]
struct Local {
    /// The SSA value holding its slot's address.
    slot: String,
    ty: Ty,
    mutable: bool,
    /// A slot holding 1 while this local still owns its value, for the case
    /// where control flow makes ownership uncertain (Ch. 3 §1.4).
    drop_flag: Option<String>,
    /// Set for a `match` binding read out of *borrowed* storage.
    ///
    /// `Owns` answers "must this be dropped", and a place also has to answer
    /// "may this be read" — for every local but this one the two answers
    /// agree, and that is why one enum carried both. A binding taken through
    /// a reference is initialized and readable while the referent, not it,
    /// owns what it names: it may be read and borrowed, and moving it would
    /// make a second owner of one value (Ch. 3 §1.2).
    borrowed: bool,
}

/// Everything the frontend knows about the nominal types in a file, plus the
/// layout engine's answers about them.
pub struct Types {
    /// The layout engine's view. Behind a cell because a generic type is
    /// registered the moment it is first applied (Ch. 4 §2.7), which can
    /// happen anywhere a type is resolved.
    db: RefCell<layout::TypeDb>,
    /// Types with a destructor of their own.
    destructors: std::collections::BTreeSet<String>,
    /// Types that opted out of copying with `impl !Copy` (Ch. 4 §5.1).
    no_copy: std::collections::BTreeSet<String>,
    /// Field names and semantic types of each struct, in declaration order.
    structs: RefCell<HashMap<String, Vec<(String, Ty)>>>,
    /// Variants of each enum, in declaration order.
    enums: RefCell<HashMap<String, Vec<VariantInfo>>>,
    /// Generic struct and enum definitions, un-instantiated.
    generic_structs: HashMap<String, ast::StructItem>,
    generic_enums: HashMap<String, ast::EnumItem>,
    /// The anonymous types closures were given, and what each one calls
    /// (Ch. 4 §4.2).
    closures: RefCell<HashMap<String, ClosureInfo>>,
    /// What each impl chose for each associated type (Ch. 4 §1.7). A cell
    /// because an impl on an instantiated generic chooses at instantiation.
    assoc: RefCell<HashMap<(String, String), Ty>>,
    /// Integer constants, for the places a *type* needs a number: an array's
    /// length is a constant expression (Ch. 0 §3.2), and a `const` is one.
    consts: HashMap<String, i128>,
    /// Trait declarations, so that `dyn Trait` can be checked for object
    /// safety where it is written rather than where it is coerced to.
    traits: HashMap<String, ast::TraitItem>,
    /// What each mangled name was an instantiation of. A mangled name is not
    /// parseable back into its arguments, and a generic impl needs them.
    instantiations: RefCell<HashMap<String, (String, Vec<Ty>)>>,
    /// What a *generic* impl chooses for an associated type, as written, with
    /// the impl's parameter names. `impl<I> Iterator for Map<I>` may choose
    /// `I::Item`, and what that is depends on the instantiation, so the
    /// choice cannot be resolved until there is one (Ch. 4 §1.7).
    generic_assoc: HashMap<(String, String), AssocChoice>,
}

/// What a closure's anonymous type stands for (Ch. 4 §4.2).
#[derive(Clone)]
struct ClosureInfo {
    /// The function its body became.
    call: String,
    /// Its parameter types, without the capture struct.
    params: Vec<Ty>,
    /// Its result type.
    ret: Ty,
    /// Which of §4.3's traits it implements.
    kind: ast::FnKind,
}

/// What a generic impl chose for one associated type: its parameters in the
/// self type's order, the ones the self type does not name, and the choice
/// as written.
type AssocChoice = (Vec<String>, Vec<Extra>, ast::Ty);

/// `impl<T> Make<T> for Pair<T>` — which trait arguments a generic impl gives
/// depends on which instantiation is asking, so the answer is worked out then
/// rather than recorded now (Ch. 4 §1.7).
#[derive(Clone, Debug)]
struct Parameterized {
    /// The type being implemented, by base name.
    base: String,
    /// Its parameters, in the order the self type names them.
    params: Vec<String>,
    /// The trait.
    trait_name: String,
    /// Its arguments, as written — names of `params`.
    args: Vec<ast::Ty>,
}

/// A generic impl's type parameter that the *self type* does not name, and
/// where its value comes from: a closure argument's signature.
///
/// `impl<I, B, F: Fn(I::Item) -> B> Iterator for Map<I, F>` has three
/// parameters and `Map` takes two. `B` is settled by `F`, which is the
/// closure — and a closure has one signature, recorded when it was lowered
/// (Ch. 4 §4.3).
#[derive(Clone, Debug)]
struct Extra {
    /// The parameter being settled.
    param: String,
    /// Which of the self type's arguments holds the closure.
    from: usize,
    /// Its result (`None`) or the parameter at this position.
    part: Option<usize>,
}

/// The impl's parameters in the order its self type names them, and the ones
/// it does not name at all.
fn impl_params(imp: &ast::ImplItem) -> (Vec<String>, Vec<Extra>) {
    let mut named: Vec<String> = Vec::new();
    for a in &imp.self_args {
        if let ast::Ty::Name(n, _) = a
            && imp.generics.iter().any(|g| g.name() == n)
            && !named.contains(n)
        {
            named.push(n.clone());
        }
    }
    let mut extras = Vec::new();
    for g in &imp.generics {
        let name = g.name().to_string();
        if named.contains(&name) {
            continue;
        }
        let ast::GenericParam::Type { bounds, .. } = g else {
            continue;
        };
        // Which other parameter's `Fn` bound mentions this one, and where.
        'search: for other in &imp.generics {
            let ast::GenericParam::Type {
                name: on,
                bounds: ob,
            } = other
            else {
                continue;
            };
            let Some(from) = named.iter().position(|n| n == on) else {
                continue;
            };
            for b in ob {
                if fn_kind(&b.name).is_none() {
                    continue;
                }
                if let Some((_, t)) = b.assoc.iter().find(|(n, _)| n == "Output")
                    && matches!(t, ast::Ty::Name(n, _) if *n == name)
                {
                    extras.push(Extra {
                        param: name.clone(),
                        from,
                        part: None,
                    });
                    break 'search;
                }
                for (i, a) in b.args.iter().enumerate() {
                    if matches!(a, ast::Ty::Name(n, _) if *n == name) {
                        extras.push(Extra {
                            param: name.clone(),
                            from,
                            part: Some(i),
                        });
                        break 'search;
                    }
                }
            }
        }
        let _ = bounds;
    }
    (named, extras)
}

/// One enum variant, resolved.
#[derive(Clone)]
struct VariantInfo {
    name: String,
    fields: Vec<(String, Ty)>,
}

impl Types {
    /// What a generic impl chose for an associated type, for one
    /// instantiation of the type it implements.
    ///
    /// The choice was kept as written because it may name the impl's own
    /// parameters — `impl<I: Iterator> Iterator for Map<I> { type Item =
    /// I::Item; }` — and only an instantiation says what those are. Resolved
    /// once and cached, so the second ask is a lookup.
    fn assoc_of_instantiation(&self, ty: &Ty, name: &str, span: Span) -> R<Option<Ty>> {
        let Some(mangled) = nominal_name(ty) else {
            return Ok(None);
        };
        let Some((base, args)) = self.instantiations.borrow().get(&mangled).cloned() else {
            return Ok(None);
        };
        let Some((params, extras, written)) =
            self.generic_assoc.get(&(base, name.to_string())).cloned()
        else {
            return Ok(None);
        };
        if params.len() != args.len() {
            return err(
                span,
                format!("`{mangled}` chooses no type for `{name}` (Ch. 4 §1.7)"),
            );
        }
        let mut env: HashMap<String, Ty> = params.into_iter().zip(args.iter().cloned()).collect();
        // The parameters the self type does not name, each settled by a
        // closure's recorded signature (Ch. 4 §4.3). This is what lets
        // `Map`'s `Item` be `B` rather than a fixed type.
        for e in &extras {
            let Some(cname) = args.get(e.from).and_then(nominal_name) else {
                return err(
                    span,
                    format!("`{mangled}` chooses no type for `{name}` (Ch. 4 §1.7)"),
                );
            };
            let Some(info) = self.closures.borrow().get(&cname).cloned() else {
                return err(
                    span,
                    format!("`{mangled}` chooses no type for `{name}` (Ch. 4 §1.7)"),
                );
            };
            let t = match e.part {
                None => info.ret.clone(),
                Some(i) => match info.params.get(i) {
                    Some(t) => t.clone(),
                    None => {
                        return err(
                            span,
                            format!("`{mangled}` chooses no type for `{name}` (Ch. 4 §1.7)"),
                        );
                    }
                },
            };
            env.insert(e.param.clone(), t);
        }
        let resolved = resolve_ty_env(&written, self, &env)?;
        self.assoc
            .borrow_mut()
            .insert((mangled, name.to_string()), resolved.clone());
        Ok(Some(resolved))
    }

    /// The layout of a type, which is where every size, offset, discriminant
    /// and niche comes from (Ch. 2).
    fn layout(&self, ty: &Ty) -> layout::Layout {
        layout::layout_of(&self.db.borrow(), &ty.layout_ty())
            .unwrap_or_else(|e| panic!("layout of {ty} failed after checking: {e}"))
    }

    fn size(&self, ty: &Ty) -> i128 {
        self.layout(ty).size as i128
    }

    /// The fields of a struct or a tuple, with their offsets.
    fn fields(&self, ty: &Ty) -> Vec<(String, Ty, i128)> {
        let l = self.layout(ty);
        match ty {
            Ty::Struct(name) => self.structs.borrow()[name]
                .iter()
                .enumerate()
                .map(|(i, (n, t))| (n.clone(), t.clone(), l.offsets[i] as i128))
                .collect(),
            Ty::Tuple(ts) => ts
                .iter()
                .enumerate()
                .map(|(i, t)| (i.to_string(), t.clone(), l.offsets[i] as i128))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// True if this type has a destructor of its own or contains one
    /// (Ch. 3 §1.2, §1.4).
    fn needs_drop(&self, ty: &Ty) -> bool {
        match ty {
            // §5.1: a negative `Copy` impl makes a type move, and so does
            // anything containing it — the same rule a destructor triggers,
            // without the destructor.
            Ty::Struct(n) => {
                let fields = self.structs.borrow()[n].clone();
                self.destructors.contains(n)
                    || self.no_copy.contains(n)
                    || fields.iter().any(|(_, t)| self.needs_drop(t))
            }
            Ty::Enum(n) => {
                let variants = self.enums.borrow()[n].clone();
                self.destructors.contains(n)
                    || self.no_copy.contains(n)
                    || variants
                        .iter()
                        .any(|v| v.fields.iter().any(|(_, t)| self.needs_drop(t)))
            }
            Ty::Array(t, n) => *n > 0 && self.needs_drop(t),
            Ty::Tuple(ts) => ts.iter().any(|t| self.needs_drop(t)),
            // A `Box` owns what it points at, so it is never `Copy` and it
            // always has something to do when it dies — even for a `T` that
            // does not, because the allocation is itself a resource
            // (Ch. 5 §2.3). A `Vec` owns its allocation the same way.
            Ty::Boxed(_) | Ty::VecOf(_) => true,
            _ => false,
        }
    }

    /// A type is copyable if it has no destructor and everything it contains
    /// is copyable (Ch. 3 §1.2). Everything else moves.
    fn is_copyable(&self, ty: &Ty) -> bool {
        !self.needs_drop(ty)
    }

    /// The index of a variant by name.
    fn variant(&self, enum_name: &str, variant: &str) -> Option<usize> {
        self.enums
            .borrow()
            .get(enum_name)?
            .iter()
            .position(|v| v.name == variant)
    }

    /// Register a struct built by the compiler — today, a closure's captures
    /// (Ch. 4 §4.2). It is an ordinary nominal type from here on.
    fn register_struct(&self, name: &str, fields: Vec<(String, Ty)>) -> Ty {
        self.db.borrow_mut().struct_(
            name,
            layout::Repr::Lang,
            fields
                .iter()
                .map(|(n, t)| (n.as_str(), t.layout_ty()))
                .collect(),
        );
        self.structs.borrow_mut().insert(name.to_string(), fields);
        Ty::Struct(name.to_string())
    }

    /// Instantiate a generic struct or enum, or return the one already made
    /// (Ch. 4 §2.7).
    ///
    /// The instantiation is an ordinary nominal type under a mangled name, so
    /// the layout engine, the drop machinery and code generation never learn
    /// that generics exist.
    fn instantiate(&self, name: &str, args: &[Ty], span: Span) -> R<Ty> {
        let mangled = mangle(name, args);
        self.instantiations
            .borrow_mut()
            .entry(mangled.clone())
            .or_insert_with(|| (name.to_string(), args.to_vec()));
        if self.structs.borrow().contains_key(&mangled) {
            return Ok(Ty::Struct(mangled));
        }
        if self.enums.borrow().contains_key(&mangled) {
            return Ok(Ty::Enum(mangled));
        }

        let (params, is_struct) =
            match (self.generic_structs.get(name), self.generic_enums.get(name)) {
                (Some(s), _) => (&s.generics, true),
                (_, Some(e)) => (&e.generics, false),
                _ => {
                    let known = self.structs.borrow().contains_key(name)
                        || self.enums.borrow().contains_key(name);
                    return err(
                        span,
                        if known {
                            format!("`{name}` takes no type arguments")
                        } else {
                            format!("`{name}` is not a type in scope")
                        },
                    );
                }
            };
        if params.len() != args.len() {
            return err(
                span,
                format!(
                    "`{name}` takes {} type argument(s), {} given",
                    params.len(),
                    args.len()
                ),
            );
        }
        let env: HashMap<String, Ty> = params
            .iter()
            .map(|p| p.name().to_string())
            .zip(args.iter().cloned())
            .collect();

        // Registered before its fields are resolved, so that a type which
        // mentions itself behind a reference terminates rather than recurses.
        if is_struct {
            self.structs
                .borrow_mut()
                .insert(mangled.clone(), Vec::new());
            let def = self.generic_structs[name].clone();
            let fields: Vec<(String, Ty)> = def
                .fields
                .iter()
                .map(|f| {
                    let ty = resolve_ty_env(&f.ty, self, &env)?;
                    check_sized(&ty, f.ty.span(), &format!("the field `{}`", f.name))?;
                    Ok((f.name.clone(), ty))
                })
                .collect::<R<_>>()?;
            self.db.borrow_mut().struct_(
                &mangled,
                repr_of(def.repr),
                fields
                    .iter()
                    .map(|(n, t)| (n.as_str(), t.layout_ty()))
                    .collect(),
            );
            self.structs.borrow_mut().insert(mangled.clone(), fields);
        } else {
            self.enums.borrow_mut().insert(mangled.clone(), Vec::new());
            let def = self.generic_enums[name].clone();
            let mut infos = Vec::new();
            let mut variants = Vec::new();
            for v in &def.variants {
                let fields: Vec<(String, Ty)> = v
                    .fields
                    .iter()
                    .map(|f| Ok((f.name.clone(), resolve_ty_env(&f.ty, self, &env)?)))
                    .collect::<R<_>>()?;
                variants.push(layout::Variant {
                    name: v.name.clone(),
                    fields: fields
                        .iter()
                        .map(|(n, t)| (n.clone(), t.layout_ty()))
                        .collect(),
                    discriminant: v.discriminant,
                });
                infos.push(VariantInfo {
                    name: v.name.clone(),
                    fields,
                });
            }
            self.db
                .borrow_mut()
                .enum_(&mangled, repr_of(def.repr), variants);
            self.enums.borrow_mut().insert(mangled.clone(), infos);
        }

        if let Err(e) = layout::layout_of(&self.db.borrow(), &layout::Ty::named(&mangled)) {
            return err(span, format!("`{name}` cannot be laid out here: {e}"));
        }
        Ok(if is_struct {
            Ty::Struct(mangled)
        } else {
            Ty::Enum(mangled)
        })
    }

    /// A variant's payload fields, with their offsets.
    fn variant_fields(&self, enum_name: &str, index: usize) -> Vec<(String, Ty, i128)> {
        let ty = Ty::Enum(enum_name.to_string());
        let l = self.layout(&ty);
        let e = l.enum_layout.expect("an enum");
        self.enums.borrow()[enum_name][index]
            .fields
            .clone()
            .iter()
            .enumerate()
            .map(|(i, (n, t))| (n.clone(), t.clone(), e.variant_offsets[index][i] as i128))
            .collect()
    }
}

/// What a name at item scope refers to.
enum Global {
    /// A scalar constant, inlined at every use.
    Const(Bt, Ty),
    /// An array constant, which lives in a TIR global.
    Array(String, Ty),
}

/// Lower a parsed file into a TIR module.
/// The type of each expression the frontend settled on, by where it was
/// written.
///
/// This is the one thing an editor wants that the AST cannot say, and the
/// reason is that it is not written anywhere: `let n = 1;` has a type, and
/// only lowering works out what it is.
///
/// Two things bound what is in here, and both are refusals rather than
/// approximations:
///
///   * **only the file's own functions.** The prelude is parsed as its own
///     file, so its spans are offsets into *it* and would collide with the
///     user's. Recording only functions the file itself names is exactly the
///     set whose bodies are in it.
///   * **only where the answer is one answer.** A generic function is
///     lowered once per instantiation, so one written expression may be
///     lowered at several types. Where two disagree the entry is dropped: an
///     editor shown one of several is shown a guess.
#[derive(Default, Debug)]
pub struct Noted {
    /// By `(lo, hi)`. `None` is a span lowered at more than one type.
    at: HashMap<(u32, u32), Option<String>>,
}

impl Noted {
    /// Record what an expression turned out to be.
    fn note(&mut self, span: Span, ty: &Ty) {
        if span.line == 0 {
            return; // synthesized: it is in no file
        }
        let text = ty.to_string();
        self.at
            .entry((span.lo, span.hi))
            .and_modify(|seen| {
                if seen.as_deref() != Some(text.as_str()) {
                    *seen = None;
                }
            })
            .or_insert(Some(text));
    }

    /// The type of the smallest expression covering `offset`.
    ///
    /// Smallest, because that is the one the cursor is on. When it was
    /// lowered at more than one type the answer is nothing, rather than the
    /// type of whatever contains it.
    pub fn at(&self, offset: u32) -> Option<&str> {
        self.at
            .iter()
            .filter(|((lo, hi), _)| *lo <= offset && offset < *hi)
            .min_by_key(|((lo, hi), _)| hi - lo)
            .and_then(|(_, t)| t.as_deref())
    }

    /// The type recorded for exactly this span, if there is one.
    pub fn exact(&self, span: Span) -> Option<&str> {
        self.at.get(&(span.lo, span.hi))?.as_deref()
    }

    /// How many expressions were typed. For a test to hold this to its word.
    pub fn len(&self) -> usize {
        self.at.len()
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
}

/// Lower a whole file to TIR, recording nothing on the way.
pub fn lower(file: &ast::File) -> Result<Module, Vec<Error>> {
    lower_noting(file, &HashSet::new(), None)
}

/// Lower, and record the type of every expression in a function named by
/// `record` — which is how an editor learns what `let n = 1` is.
///
/// `record` empty and `noted` `None` is the ordinary path, and costs one
/// `Option` test per expression.
pub fn lower_noting(
    file: &ast::File,
    record: &HashSet<String>,
    noted: Option<&RefCell<Noted>>,
) -> Result<Module, Vec<Error>> {
    let mut errs = Vec::new();
    let mut module = Module {
        version: TIR_VERSION.to_string(),
        target: "tritium".to_string(),
        ..Module::default()
    };

    // Nominal types first: signatures and constants may mention them, and
    // every item in the file is visible to every other whatever the order
    // (Ch. 0 §3).
    let types = match build_types(file) {
        Ok(t) => t,
        Err(e) => {
            errs.push(e);
            return Err(errs);
        }
    };

    // Which traits each type implements, which is all a bound needs to know
    // (Ch. 4 §2.2). Supertraits are included, since implementing `B: A`
    // requires implementing `A` and the impl for it is in the file too.
    // A trait that takes arguments is recorded by `expand_impls` instead,
    // with them: `From` alone is not a requirement anything can satisfy.
    let parameterized: std::collections::HashSet<&str> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Trait(t) if !t.params.is_empty() => Some(t.name.as_str()),
            _ => None,
        })
        .collect();
    let impls: std::collections::HashSet<(String, String)> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Impl(imp) => imp
                .trait_name
                .as_ref()
                .filter(|t| !parameterized.contains(t.as_str()))
                .map(|t| (imp.self_ty.clone(), t.clone())),
            _ => None,
        })
        .collect();

    // Impl blocks become ordinary functions before anything else looks at
    // the file, so the rest of lowering never learns they existed.
    let (expanded, impl_errs, mut table) = expand_impls(file, &types);
    errs.extend(impl_errs);
    table.pairs.extend(impls.iter().cloned());
    let fns: Vec<ast::FnItem> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Fn(f) => Some(f.clone()),
            _ => None,
        })
        .chain(expanded)
        .collect();

    // `impl Fn(…)` in argument position is an anonymous type parameter
    // (Ch. 4 §2.2). Naming it here is the whole of the desugaring, and it is
    // what lets a closure argument monomorphize like any other.
    let mut fn_bounds: HashMap<String, (ast::FnKind, Vec<ast::Ty>, Option<ast::Ty>)> =
        HashMap::new();
    let fns: Vec<ast::FnItem> = fns
        .into_iter()
        .map(|mut f| {
            let mut i = 0;
            for p in &mut f.params {
                let t = &mut p.ty;
                let ast::Ty::ImplFn(kind, ps, r, span) = t.clone() else {
                    continue;
                };
                let pname = format!("#F{i}");
                i += 1;
                let key = format!("{}{pname}", f.name);
                fn_bounds.insert(key.clone(), (kind, ps, r.map(|b| *b)));
                *t = ast::Ty::Name(pname.clone(), span);
                f.generics.push(ast::GenericParam::Type {
                    name: pname,
                    bounds: vec![ast::Bound::plain(format!("Fn@{key}"))],
                });
            }
            // The same bound written on a parameter that already has a name.
            // `impl Fn(…)` invents the name and then does exactly this, so
            // the two forms meet here and nothing below can tell them apart
            // (Ch. 4 §4.3).
            for g in &mut f.generics {
                let ast::GenericParam::Type { name, bounds } = g else {
                    continue;
                };
                for b in bounds {
                    let Some(kind) = fn_kind(&b.name) else {
                        continue;
                    };
                    let key = format!("{}{name}", f.name);
                    let ret = b
                        .assoc
                        .iter()
                        .find(|(n, _)| n == "Output")
                        .map(|(_, t)| t.clone());
                    fn_bounds.insert(key.clone(), (kind, b.args.clone(), ret));
                    *b = ast::Bound::plain(format!("Fn@{key}"));
                }
            }
            f
        })
        .collect();

    // A generic function is not code until it is instantiated (§2.7), so it
    // is set aside here and its body lowered once per instantiation.
    let mut generic_fns: HashMap<String, ast::FnItem> = HashMap::new();
    for f in &fns {
        if !f.generics.is_empty() {
            if f.body.is_none() {
                errs.push(SyntaxError {
                    span: f.span,
                    message: format!(
                        "`{}` is a declaration, so there is no body to instantiate; \
                         an external function cannot be generic",
                        f.name
                    ),
                });
                continue;
            }
            if generic_fns.insert(f.name.clone(), f.clone()).is_some() {
                errs.push(SyntaxError {
                    span: f.span,
                    message: format!("`{}` is defined more than once", f.name),
                });
            }
        }
    }
    let fns: Vec<ast::FnItem> = fns.into_iter().filter(|f| f.generics.is_empty()).collect();

    let sigs: RefCell<HashMap<String, (Vec<Ty>, Ty)>> = RefCell::new(HashMap::new());
    for f in &fns {
        {
            let params: Result<Vec<Ty>, Error> = f
                .params
                .iter()
                .map(|p| {
                    let ty = resolve_ty(&p.ty, &types)?;
                    check_sized(&ty, p.ty.span(), &format!("the parameter `{}`", p.name))?;
                    Ok(ty)
                })
                .collect();
            let ret = match &f.ret {
                None => Ok(Ty::Unit),
                Some(t) => resolve_ty(t, &types),
            };
            let self_ref = matches!(
                f.params.first(),
                Some(p) if p.name == "self" && matches!(p.ty, ast::Ty::Ref(..))
            );
            if let (Ok(p), Ok(r)) = (&params, &ret)
                && let Err(e) = check_returned_reference(p, r, self_ref, f.span)
            {
                errs.push(e);
                continue;
            }
            match (params, ret) {
                (Ok(p), Ok(r)) => {
                    if sigs.borrow_mut().insert(fn_key(f), (p, r)).is_some() {
                        errs.push(SyntaxError {
                            span: f.span,
                            message: format!("`{}` is defined more than once", f.name),
                        });
                    }
                }
                (Err(e), _) | (_, Err(e)) => errs.push(e),
            }
        }
    }

    // Constants next, since a function body may use one. An impl's
    // associated constants are ordinary constants under a qualified name
    // (Ch. 4 §1.7), which is all `Type::NAME` needs them to be.
    let mut consts: Vec<ast::ConstItem> = Vec::new();
    for item in &file.items {
        match item {
            ast::Item::Const(c) => consts.push(c.clone()),
            ast::Item::Impl(imp) => {
                let self_repr = SelfTy {
                    ty: ast::Ty::Name(imp.self_ty.clone(), imp.span),
                    name: imp.self_ty.clone(),
                };
                for c in &imp.consts {
                    let mut value = c.value.clone();
                    subst_expr(&mut value, &self_repr);
                    consts.push(ast::ConstItem {
                        public: true,
                        name: format!("{}.{}", imp.self_ty, c.name),
                        name_span: c.name_span,
                        ty: subst_self_ty(&c.ty, &self_repr),
                        value,
                        span: c.span,
                    });
                }
            }
            _ => {}
        }
    }

    let mut globals: HashMap<String, Global> = HashMap::new();
    for c in &consts {
        {
            match const_item(c, &mut module, &types) {
                Ok(g) => {
                    if globals.insert(c.name.clone(), g).is_some() {
                        errs.push(SyntaxError {
                            span: c.span,
                            message: format!("`{}` is defined more than once", c.name),
                        });
                    }
                }
                Err(e) => errs.push(e),
            }
        }
    }

    // Derived impls (Ch. 4 §6). They are TIR, not source, so they are added
    // to the module directly; their signatures go into the table so that a
    // call to one checks like any other.
    let (derived, derived_on, derive_errs) = derived_functions(file, &types);
    errs.extend(derive_errs);
    for ty in &derived_on {
        let r = Ty::Ref(Box::new(ty.clone()), false);
        let name = nominal_name(ty).expect("a nominal type");
        for f in &derived {
            if f.sig.name == format!("{name}.cmp") {
                sigs.borrow_mut()
                    .insert(f.sig.name.clone(), (vec![r.clone(), r.clone()], Ty::Trit));
            } else if f.sig.name == format!("{name}.eq") {
                sigs.borrow_mut()
                    .insert(f.sig.name.clone(), (vec![r.clone(), r.clone()], Ty::Bool));
            } else if f.sig.name == format!("{name}.clone") {
                sigs.borrow_mut()
                    .insert(f.sig.name.clone(), (vec![r.clone()], ty.clone()));
            }
        }
    }
    module.funcs.extend(derived);

    let pending: RefCell<Vec<Job>> = RefCell::new(Vec::new());
    let vtables: RefCell<Vec<(String, String, ir::Global)>> = RefCell::new(Vec::new());
    let data: RefCell<Vec<ir::Global>> = RefCell::new(Vec::new());
    let needs_heap = std::cell::Cell::new(false);
    let strings: RefCell<HashMap<Vec<i128>, String>> = RefCell::new(HashMap::new());
    let extra_fns: RefCell<Vec<ast::FnItem>> = RefCell::new(Vec::new());
    let specials: RefCell<HashMap<String, Special>> = RefCell::new(HashMap::new());
    let world = World {
        record,
        noted,
        traits: &types.traits.clone(),
        vtables: &vtables,
        data: &data,
        strings: &strings,
        needs_heap: &needs_heap,
        extra_fns: &extra_fns,
        fn_bounds: &fn_bounds,
        sigs: &sigs,
        generic_fns: &generic_fns,
        specials: &specials,
        impls: &table,
        pending: &pending,
        globals: &globals,
        types: &types,
    };
    for f in &fns {
        if !sigs.borrow().contains_key(&fn_key(f)) {
            continue; // its signature was already reported
        }
        let key = fn_key(f);
        let signature = signature_of(f, &key, &sigs);
        match &f.body {
            // A function without a body is a declaration, and lowers to TIR's
            // own declaration form — one mechanism, spelled twice.
            None => module.decls.push(signature),
            Some(body) => match function(f, signature, body, &key, HashMap::new(), &world) {
                Ok(func) => module.funcs.push(func),
                Err(e) => errs.push(e),
            },
        }
    }

    // Then the instantiations, and the instantiations they in turn need
    // (§2.7). The queue drains because every job is unique by key and the
    // depth limit stops a body that instantiates itself at a larger type.
    loop {
        // A closure body is an ordinary function that did not exist when the
        // file was read, so it joins the same queue (Ch. 4 §4.2).
        let extra = extra_fns.borrow_mut().pop();
        if let Some(f) = extra {
            let key = f.name.clone();
            let signature = signature_of(&f, &key, &sigs);
            let body = f.body.clone().expect("a closure has a body");
            match function(&f, signature, &body, &key, HashMap::new(), &world) {
                Ok(func) => module.funcs.push(func),
                Err(e) => errs.push(e),
            }
            continue;
        }
        // Not `while let Some(job) = pending.borrow_mut().pop()`: that keeps
        // the mutable borrow alive for the whole body, and the body queues
        // more jobs.
        let Some(job) = pending.borrow_mut().pop() else {
            break;
        };
        let def = match generic_fns.get(&job.from) {
            Some(d) => d.clone(),
            None => specials.borrow()[&job.from].def.clone(),
        };
        let body = def.body.clone().expect("a generic function has a body");
        let signature = signature_of(&def, &job.key, &sigs);
        match function(&def, signature, &body, &job.key, job.env, &world) {
            Ok(func) => module.funcs.push(func),
            Err(e) => errs.push(e),
        }
        let _ = job.depth;
    }

    // Vtables are globals like any other, and are emitted once every
    // coercion that needs one has been seen (Ch. 4 §3.3).
    module
        .globals
        .extend(vtables.into_inner().into_iter().map(|(_, _, g)| g));
    module.globals.extend(data.into_inner());

    // The allocator, declared where something calls it (Ch. 5 §2.1). It is
    // TIR rather than Trust because it returns a *pointer*, and Trust has no
    // way to name one — which is why `Box` is the compiler's and not the
    // library's.
    if needs_heap.get() {
        module.decls.push(Signature {
            name: ALLOC.to_string(),
            params: vec![
                ("size".into(), Type::Int(27)),
                ("align".into(), Type::Int(27)),
            ],
            ret: Some(Type::Ptr),
        });
        module.decls.push(Signature {
            name: FREE.to_string(),
            params: vec![
                ("at".into(), Type::Ptr),
                ("size".into(), Type::Int(27)),
                ("align".into(), Type::Int(27)),
            ],
            ret: None,
        });
    }

    if errs.is_empty() {
        Ok(module)
    } else {
        Err(errs)
    }
}

/// One error, for the places that build one rather than return it.
fn one_err(span: Span, message: String) -> Error {
    SyntaxError { span, message }
}

/// Whether an expression contains a closure anywhere inside it.
///
/// A closure has no type until it is lowered, and a literal holding one
/// inherits that: neither can be peeked, and both have to be lowered where
/// their type is first needed.
fn holds_a_closure(e: &ast::Expr) -> bool {
    if matches!(e, ast::Expr::Closure(..)) {
        return true;
    }
    let mut found = false;
    for_each_child(e, &mut |c| found |= holds_a_closure(c));
    if let ast::Expr::Aggregate(_, fields, _) = e {
        found |= fields.iter().any(|(_, v)| holds_a_closure(v));
    }
    found
}

/// The names a closure body uses that it did not bind itself.
///
/// Deliberately over-approximate: a name that turns out not to be a local of
/// the enclosing function is dropped by the caller, which knows the scope.
fn free_names(e: &ast::Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    use ast::Expr::*;
    let see = |n: &String, bound: &[String], out: &mut Vec<String>| {
        if !bound.contains(n) && !out.contains(n) {
            out.push(n.clone());
        }
    };
    match e {
        Char(..) | Str(..) => {}
        Try(a, _) => free_names(a, bound, out),
        CallExpr(f, args, _) => {
            free_names(f, bound, out);
            for a in args {
                free_names(a, bound, out);
            }
        }
        Path(n, _) => see(n, bound, out),
        Aggregate(_, fields, _) => {
            for (_, v) in fields {
                free_names(v, bound, out);
            }
        }
        Closure(ps, _, body, _) => {
            let depth = bound.len();
            bound.extend(ps.iter().map(|(n, _)| n.clone()));
            free_names(body, bound, out);
            bound.truncate(depth);
        }
        Block(b) => free_names_block(b, bound, out),
        Loop(b, _) => free_names_block(b, bound, out),
        If(c, t, e2, _) => {
            free_names(c, bound, out);
            free_names_block(t, bound, out);
            if let Some(e2) = e2 {
                free_names(e2, bound, out);
            }
        }
        While(c, b, _) => {
            free_names(c, bound, out);
            free_names_block(b, bound, out);
        }
        Match(sc, arms, _) => {
            free_names(sc, bound, out);
            for a in arms {
                let depth = bound.len();
                for p in &a.patterns {
                    bind_pattern(p, bound);
                }
                if let Some(g) = &a.guard {
                    free_names(g, bound, out);
                }
                free_names(&a.body, bound, out);
                bound.truncate(depth);
            }
        }
        Cast(a, _, _) | Unary(_, a, _) | Deref(a, _) | Borrow(a, _, _) | Field(a, _, _) => {
            free_names(a, bound, out)
        }
        Binary(_, a, b, _) | Assign(_, a, b, _) | Index(a, b, _) | Repeat(a, b, _) => {
            free_names(a, bound, out);
            free_names(b, bound, out);
        }
        Call(_, _, _, args, _) | Array(args, _) | Tuple(args, _) => {
            args.iter().for_each(|a| free_names(a, bound, out))
        }
        Method(r, _, _, args, _) => {
            free_names(r, bound, out);
            args.iter().for_each(|a| free_names(a, bound, out));
        }
        Break(v, _) | Return(v, _) => {
            if let Some(v) = v {
                free_names(v, bound, out);
            }
        }
        Int(..) | Trit(..) | Bool(..) | Unit(_) | Continue(_) => {}
    }
}

fn free_names_block(b: &ast::Block, bound: &mut Vec<String>, out: &mut Vec<String>) {
    let depth = bound.len();
    for st in &b.stmts {
        match st {
            ast::Stmt::Let { name, value, .. } => {
                free_names(value, bound, out);
                bound.push(name.clone());
            }
            ast::Stmt::Expr(e) => free_names(e, bound, out),
        }
    }
    if let Some(t) = &b.tail {
        free_names(t, bound, out);
    }
    bound.truncate(depth);
}

fn bind_pattern(p: &ast::Pattern, bound: &mut Vec<String>) {
    match p {
        ast::Pattern::Bind(n, _) => bound.push(n.clone()),
        ast::Pattern::Aggregate(_, fields, _) => {
            fields.iter().for_each(|(_, p)| bind_pattern(p, bound))
        }
        ast::Pattern::Tuple(ps, _) => ps.iter().for_each(|p| bind_pattern(p, bound)),
        _ => {}
    }
}

/// Whether a closure body writes through this name, which decides whether it
/// is captured by `&mut` and therefore whether the closure is `FnMut`
/// (Ch. 4 §4.4).
fn writes_name(e: &ast::Expr, name: &str) -> bool {
    use ast::Expr::*;
    let root = |mut e: &ast::Expr| loop {
        match e {
            Field(b, ..) | Index(b, ..) | Deref(b, _) => e = b,
            Path(n, _) => return Some(n.clone()),
            _ => return None,
        }
    };
    let here = match e {
        Assign(_, target, _, _) => root(target).as_deref() == Some(name),
        Borrow(inner, true, _) => root(inner).as_deref() == Some(name),
        // A method call may need `&mut`; assume it does, which costs a
        // closure the `Fn` bound but never accepts a wrong program.
        Method(recv, _, _, _, _) => root(recv).as_deref() == Some(name),
        _ => false,
    };
    if here {
        return true;
    }
    let mut found = false;
    for_each_child(e, &mut |c| {
        if writes_name(c, name) {
            found = true;
        }
    });
    found
}

/// Visit an expression's immediate sub-expressions.
fn for_each_child(e: &ast::Expr, f: &mut impl FnMut(&ast::Expr)) {
    use ast::Expr::*;
    match e {
        Char(..) | Str(..) => {}
        Try(a, _) => f(a),
        CallExpr(c, args, _) => {
            f(c);
            args.iter().for_each(f);
        }
        Cast(a, _, _)
        | Unary(_, a, _)
        | Deref(a, _)
        | Borrow(a, _, _)
        | Field(a, _, _)
        | Closure(_, _, a, _) => f(a),
        Binary(_, a, b, _) | Assign(_, a, b, _) | Index(a, b, _) | Repeat(a, b, _) => {
            f(a);
            f(b);
        }
        Call(_, _, _, args, _) | Array(args, _) | Tuple(args, _) => args.iter().for_each(f),
        Aggregate(_, fields, _) => fields.iter().for_each(|(_, v)| f(v)),
        Method(r, _, _, args, _) => {
            f(r);
            args.iter().for_each(f);
        }
        Block(b) | Loop(b, _) => for_each_child_block(b, f),
        If(c, t, e2, _) => {
            f(c);
            for_each_child_block(t, f);
            if let Some(e2) = e2 {
                f(e2);
            }
        }
        While(c, b, _) => {
            f(c);
            for_each_child_block(b, f);
        }
        Match(sc, arms, _) => {
            f(sc);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        Break(v, _) | Return(v, _) => {
            if let Some(v) = v {
                f(v);
            }
        }
        Int(..) | Trit(..) | Bool(..) | Unit(_) | Continue(_) | Path(..) => {}
    }
}

fn for_each_child_block(b: &ast::Block, f: &mut impl FnMut(&ast::Expr)) {
    for st in &b.stmts {
        match st {
            ast::Stmt::Let { value, .. } => f(value),
            ast::Stmt::Expr(e) => f(e),
        }
    }
    if let Some(t) = &b.tail {
        f(t);
    }
}

/// Rewrite each captured name to a dereference of the capture struct's
/// field, which is what makes the closure body an ordinary function body.
fn rewrite_captures(e: &mut ast::Expr, captures: &[String]) {
    use ast::Expr::*;
    if let Path(n, span) = e
        && captures.contains(n)
    {
        *e = Deref(
            Box::new(Field(
                Box::new(Path("self".to_string(), *span)),
                n.clone(),
                *span,
            )),
            *span,
        );
        return;
    }
    for_each_child_mut(e, &mut |c| rewrite_captures(c, captures));
}

fn for_each_child_mut(e: &mut ast::Expr, f: &mut impl FnMut(&mut ast::Expr)) {
    use ast::Expr::*;
    match e {
        Char(..) | Str(..) => {}
        Try(a, _) => f(a),
        CallExpr(c, args, _) => {
            f(c);
            args.iter_mut().for_each(f);
        }
        Cast(a, _, _)
        | Unary(_, a, _)
        | Deref(a, _)
        | Borrow(a, _, _)
        | Field(a, _, _)
        | Closure(_, _, a, _) => f(a),
        Binary(_, a, b, _) | Assign(_, a, b, _) | Index(a, b, _) | Repeat(a, b, _) => {
            f(a);
            f(b);
        }
        Call(_, _, _, args, _) | Array(args, _) | Tuple(args, _) => args.iter_mut().for_each(f),
        Aggregate(_, fields, _) => fields.iter_mut().for_each(|(_, v)| f(v)),
        Method(r, _, _, args, _) => {
            f(r);
            args.iter_mut().for_each(f);
        }
        Block(b) | Loop(b, _) => for_each_child_block_mut(b, f),
        If(c, t, e2, _) => {
            f(c);
            for_each_child_block_mut(t, f);
            if let Some(e2) = e2 {
                f(e2);
            }
        }
        While(c, b, _) => {
            f(c);
            for_each_child_block_mut(b, f);
        }
        Match(sc, arms, _) => {
            f(sc);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    f(g);
                }
                f(&mut a.body);
            }
        }
        Break(v, _) | Return(v, _) => {
            if let Some(v) = v {
                f(v);
            }
        }
        Int(..) | Trit(..) | Bool(..) | Unit(_) | Continue(_) | Path(..) => {}
    }
}

fn for_each_child_block_mut(b: &mut ast::Block, f: &mut impl FnMut(&mut ast::Expr)) {
    for st in &mut b.stmts {
        match st {
            ast::Stmt::Let { value, .. } => f(value),
            ast::Stmt::Expr(e) => f(e),
        }
    }
    if let Some(t) = &mut b.tail {
        f(t);
    }
}

/// Every method reachable through `dyn Trait`, in vtable order: the
/// supertraits' first, then the trait's own (Ch. 4 §3.3).
///
/// One list serves both the table and the dispatch, which is the only way
/// the two can be guaranteed to agree on an index.
fn object_methods(
    traits: &HashMap<String, ast::TraitItem>,
    name: &str,
    seen: &mut Vec<String>,
) -> Vec<ast::FnItem> {
    if seen.iter().any(|s| s == name) {
        return Vec::new();
    }
    seen.push(name.to_string());
    let Some(decl) = traits.get(name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for s in &decl.supertraits {
        out.extend(object_methods(traits, s, seen));
    }
    out.extend(decl.methods.iter().cloned());
    out
}

/// Ch. 4 §3.4: a method is object-safe when a trait object can call it.
fn object_safe(m: &ast::FnItem, trait_name: &str) -> R<()> {
    let complaint = |what: &str| {
        err(
            m.span,
            format!(
                "`{trait_name}::{}` is not object-safe: {what} (Ch. 4 §3.4). \
                 A trait object has erased its type, so any signature that needs \
                 it back cannot be called through one.",
                m.name
            ),
        )
    };
    if !m.generics.is_empty() {
        return complaint("it has type parameters of its own");
    }
    match m.params.first() {
        Some(p) if p.name == "self" => {}
        _ => return complaint("it takes no `self`"),
    }
    for (i, t) in m.params.iter().map(|p| &p.ty).enumerate() {
        if i > 0 && mentions_self(t) {
            return complaint("a parameter mentions `Self`");
        }
    }
    if m.ret.as_ref().is_some_and(mentions_self) {
        return complaint("it returns `Self`");
    }
    Ok(())
}

fn mentions_self(t: &ast::Ty) -> bool {
    match t {
        ast::Ty::SelfTy(_) => true,
        ast::Ty::Ref(t, _, _) | ast::Ty::Slice(t, _) | ast::Ty::Array(t, _, _) => mentions_self(t),
        ast::Ty::Tuple(ts, _) | ast::Ty::App(_, ts, _) => ts.iter().any(mentions_self),
        _ => false,
    }
}

/// The methods the language defines on its own types, by what they are
/// written on and how they read.
///
/// A user method of the same name on the same type would shadow a language
/// rule, so these are matched first and `impl` blocks never see them.
///
/// The receiver is a *written* type's name, so that an editor can ask what a
/// `Vec<t27>` has on it and be answered from the same list the compiler
/// matches against. There is one list, so there is nothing to drift.
pub const BUILTIN_METHODS: &[(&str, &str, &str)] = &[
    // Ch. 1 §4: the trit-wise operations, which are methods because §2.5
    // gives their symbols to nothing.
    ("int", "tmin", "fn tmin(self, other: Self) -> Self"),
    ("int", "tmax", "fn tmax(self, other: Self) -> Self"),
    ("int", "tmul", "fn tmul(self, other: Self) -> Self"),
    ("int", "tneg", "fn tneg(self) -> Self"),
    ("int", "mulh", "fn mulh(self, other: Self) -> Self"),
    // Ch. 1 §6: what a trit is, asked of a value.
    ("int", "is_pos", "fn is_pos(self) -> bool"),
    ("int", "is_zero", "fn is_zero(self) -> bool"),
    ("int", "is_neg", "fn is_neg(self) -> bool"),
    ("int", "to_trit", "fn to_trit(self) -> trit"),
    // Ch. 1 §5: the overflow flavours, written into the operation.
    (
        "int",
        "wrapping_add",
        "fn wrapping_add(self, other: Self) -> Self",
    ),
    (
        "int",
        "wrapping_sub",
        "fn wrapping_sub(self, other: Self) -> Self",
    ),
    (
        "int",
        "wrapping_mul",
        "fn wrapping_mul(self, other: Self) -> Self",
    ),
    (
        "int",
        "saturating_add",
        "fn saturating_add(self, other: Self) -> Self",
    ),
    (
        "int",
        "saturating_sub",
        "fn saturating_sub(self, other: Self) -> Self",
    ),
    (
        "int",
        "saturating_mul",
        "fn saturating_mul(self, other: Self) -> Self",
    ),
    (
        "int",
        "overflowing_add",
        "fn overflowing_add(self, other: Self) -> (Self, trit)",
    ),
    (
        "int",
        "overflowing_sub",
        "fn overflowing_sub(self, other: Self) -> (Self, trit)",
    ),
    (
        "int",
        "overflowing_mul",
        "fn overflowing_mul(self, other: Self) -> (Self, trit)",
    ),
    (
        "int",
        "checked_add",
        "fn checked_add(self, other: Self) -> Option<Self>",
    ),
    (
        "int",
        "checked_sub",
        "fn checked_sub(self, other: Self) -> Option<Self>",
    ),
    (
        "int",
        "checked_mul",
        "fn checked_mul(self, other: Self) -> Option<Self>",
    ),
    // Ch. 5 §2: the growable array, whose storage is the compiler's.
    ("Vec", "len", "fn len(&self) -> taddr"),
    ("Vec", "is_empty", "fn is_empty(&self) -> bool"),
    ("Vec", "push", "fn push(&mut self, value: T)"),
    ("Vec", "pop", "fn pop(&mut self) -> Option<T>"),
    ("Vec", "clear", "fn clear(&mut self)"),
    ("Vec", "reserve", "fn reserve(&mut self, more: taddr)"),
    ("Vec", "insert", "fn insert(&mut self, at: taddr, value: T)"),
    ("Vec", "remove", "fn remove(&mut self, at: taddr) -> T"),
    ("Vec", "capacity", "fn capacity(&self) -> taddr"),
    // A slice and a string know how long they are, and nothing else here.
    ("slice", "len", "fn len(&self) -> taddr"),
    ("slice", "is_empty", "fn is_empty(&self) -> bool"),
    ("str", "len", "fn len(&self) -> taddr"),
    ("str", "is_empty", "fn is_empty(&self) -> bool"),
];

/// Whether `name` is one the language defines, whatever it is written on.
///
/// The receiver is checked where the method is lowered, which is where the
/// diagnostic can say what went wrong; this only has to keep `impl` blocks
/// from claiming the name.
fn is_builtin_method(name: &str) -> bool {
    BUILTIN_METHODS.iter().any(|(_, n, _)| *n == name)
}

/// The name of the hidden out-pointer for an aggregate return.
const SRET: &str = "sret";

/// Strip one layer of reference: a method on a `Vec` is reached through one
/// as often as not, and nothing below cares which.
fn peel(ty: &Ty) -> &Ty {
    match ty {
        Ty::Ref(inner, _) => inner,
        other => other,
    }
}

/// The element type of a `Vec`, however the receiver is held.
fn vec_elem(ty: &Ty, method: &str, span: Span) -> R<Ty> {
    match peel(ty) {
        Ty::VecOf(e) => Ok((**e).clone()),
        _ => err(span, format!("`{method}` applies to a `Vec`, not {ty}")),
    }
}

/// The `Fn` family, by name, or `None` for any other trait.
fn fn_kind(name: &str) -> Option<ast::FnKind> {
    match name {
        "Fn" => Some(ast::FnKind::Fn),
        "FnMut" => Some(ast::FnKind::FnMut),
        "FnOnce" => Some(ast::FnKind::FnOnce),
        _ => None,
    }
}

/// Trytes in a word, which is the unit an aligned copy moves.
const WORD: i128 = 3;

/// A `taddr` constant, which is what every size and index here is.
fn konst_addr(v: i128) -> Operand {
    Operand::Const(Type::Int(27), Bt::from_i128(v))
}

/// The target's allocator (Ch. 5 §2.1). Declared in TIR rather than in Trust
/// because it returns a *pointer*, and Trust has no way to name one — which
/// is the whole reason `Box` is a language item.
const ALLOC: &str = "alloc";
const FREE: &str = "free";

/// The name a function is known by. A destructor is keyed by its type, since
/// every type may have one and they would otherwise collide.
fn fn_key(f: &ast::FnItem) -> String {
    match (&f.name[..], f.params.first()) {
        ("drop", Some(p)) if p.name == "self" => match &p.ty {
            ast::Ty::Name(ty, _) => format!("drop.{ty}"),
            _ => f.name.clone(),
        },
        _ => f.name.clone(),
    }
}

/// The TIR signature of a function.
///
/// An aggregate has no SSA representation (TIR §2), so one is passed by
/// address and returned through a hidden leading pointer the caller supplies
/// — the classic arrangement, and the only one TIR can express.
/// A returned reference is checked by elision rule 2 (Ch. 3 §3.3): with
/// exactly one reference among the parameters, the returned reference borrows
/// from it, and the caller's loan is extended to cover the result. With none
/// or several, the signature is the one §3.3 calls ill-formed.
fn check_returned_reference(params: &[Ty], ret: &Ty, self_ref: bool, span: Span) -> R<()> {
    if !contains_reference(ret) {
        return Ok(());
    }
    // Rule 3 first: a method borrowing `self` lends to its result, whatever
    // else it takes (Ch. 4 §1.4).
    if self_ref {
        return Ok(());
    }
    let sources = params.iter().filter(|t| contains_reference(t)).count();
    match sources {
        1 => Ok(()),
        0 => err(
            span,
            format!(
                "this function returns {ret} but borrows from nothing: the reference could \
                 only point into a local, which dies when the function does (Ch. 3 §4.1)"
            ),
        ),
        n => err(
            span,
            format!(
                "this function returns {ret} and has {n} reference parameters, so elision \
                 cannot choose which one it borrows from; writing the lifetimes out needs \
                 the region inference that is not implemented yet (Ch. 3 §3.3)"
            ),
        ),
    }
}

/// A dynamically sized type is never the type of a place (Ch. 3 §5.1): not a
/// parameter, not a local, not a field. It appears only behind a reference.
fn check_sized(ty: &Ty, span: Span, what: &str) -> R<()> {
    if ty.is_unsized() {
        return err(
            span,
            format!("{what} cannot have type {ty}: it has no size, so it lives only behind `&`"),
        );
    }
    Ok(())
}

fn contains_reference(ty: &Ty) -> bool {
    match ty {
        Ty::Ref(..) | Ty::Slice(_) => true,
        Ty::Array(t, _) => contains_reference(t),
        Ty::Tuple(ts) => ts.iter().any(contains_reference),
        _ => false,
    }
}

/// The name a method on this type is keyed by (Ch. 4 §1.2). Scalars have one
/// too, so `impl Trait for t27` is expressible — the orphan rule permits it
/// because the trait is local (§1.8).
fn nominal_name(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Struct(n) | Ty::Enum(n) => n.clone(),
        Ty::Trit => "trit".into(),
        Ty::Bool => "bool".into(),
        Ty::T9 => "t9".into(),
        Ty::T27 => "t27".into(),
        Ty::TAddr => "taddr".into(),
        Ty::Char => "char".into(),
        // `str` is the name `[char]` goes by, and the name its methods are
        // keyed under (Ch. 5 §1.3).
        Ty::Slice(t) if **t == Ty::Char => "str".into(),
        // A `Vec` is nominal under the name its instantiation was registered
        // with, so an impl written for `Vec<A>` can be found for it.
        Ty::VecOf(t) => mangle("Vec", std::slice::from_ref(&**t)),
        _ => return None,
    })
}

/// Substitute `Self` for the implementing type throughout a method, in type
/// and in path position, so that nothing downstream ever sees `Self`.
fn subst_self(f: &ast::FnItem, self_ty: &SelfTy) -> ast::FnItem {
    let mut f = f.clone();
    for p in &mut f.params {
        subst_ty(&mut p.ty, self_ty);
    }
    // Bounds too. `fn map<B, F: Fn(Self::Item) -> B>` writes `Self` where the
    // method's *constraints* are, not where its types are, and a `Self` that
    // survives to lowering is one written where there is no impl.
    for g in &mut f.generics {
        let ast::GenericParam::Type { bounds, .. } = g else {
            continue;
        };
        for b in bounds {
            b.args.iter_mut().for_each(|t| subst_ty(t, self_ty));
            b.assoc.iter_mut().for_each(|(_, t)| subst_ty(t, self_ty));
        }
    }
    if let Some(t) = &mut f.ret {
        subst_ty(t, self_ty);
    }
    if let Some(b) = &mut f.body {
        subst_block(b, self_ty);
    }
    f
}

/// What `Self` stands for in an impl block: the written type, and the bare
/// name a path like `Self::new` or `Self { … }` turns into.
struct SelfTy {
    ty: ast::Ty,
    name: String,
}

fn subst_ty(t: &mut ast::Ty, self_ty: &SelfTy) {
    match t {
        ast::Ty::Never(_) => {}
        ast::Ty::SelfTy(_) => *t = self_ty.ty.clone(),
        ast::Ty::Array(e, _, _) | ast::Ty::Ref(e, _, _) | ast::Ty::Slice(e, _) => {
            subst_ty(e, self_ty)
        }
        ast::Ty::Tuple(ts, _) | ast::Ty::App(_, ts, _) => {
            ts.iter_mut().for_each(|t| subst_ty(t, self_ty))
        }
        ast::Ty::Assoc(base, _, _) => subst_ty(base, self_ty),
        ast::Ty::ImplFn(_, ps, r, _) => {
            ps.iter_mut().for_each(|t| subst_ty(t, self_ty));
            if let Some(r) = r {
                subst_ty(r, self_ty);
            }
        }
        ast::Ty::Name(..) | ast::Ty::Unit(_) | ast::Ty::Dyn(..) => {}
    }
}

fn subst_block(b: &mut ast::Block, self_ty: &SelfTy) {
    for st in &mut b.stmts {
        match st {
            ast::Stmt::Let { ty, value, .. } => {
                if let Some(t) = ty {
                    subst_ty(t, self_ty);
                }
                subst_expr(value, self_ty);
            }
            ast::Stmt::Expr(e) => subst_expr(e, self_ty),
        }
    }
    if let Some(t) = &mut b.tail {
        subst_expr(t, self_ty);
    }
}

fn subst_expr(e: &mut ast::Expr, self_ty: &SelfTy) {
    use ast::Expr::*;
    let mut kids: Vec<&mut ast::Expr> = Vec::new();
    match e {
        Char(..) | Str(..) => {}
        Try(a, _) => subst_expr(a, self_ty),
        CallExpr(c, args, _) => {
            subst_expr(c, self_ty);
            for a in args {
                subst_expr(a, self_ty);
            }
        }
        Aggregate(path, fields, _) => {
            for seg in &mut path.segments {
                if seg == "Self" {
                    seg.clone_from(&self_ty.name);
                }
            }
            kids.extend(fields.iter_mut().map(|(_, e)| e));
        }
        Path(name, _) => {
            if name == "Self" {
                name.clone_from(&self_ty.name);
            }
        }
        Cast(a, t, _) => {
            subst_ty(t, self_ty);
            kids.push(a);
        }
        Unary(_, a, _) | Deref(a, _) | Borrow(a, _, _) | Field(a, _, _) => kids.push(a),
        Binary(_, a, b, _) | Assign(_, a, b, _) | Index(a, b, _) | Repeat(a, b, _) => {
            kids.push(a);
            kids.push(b);
        }
        Call(_, _, _, args, _) | Array(args, _) | Tuple(args, _) => kids.extend(args.iter_mut()),
        Method(r, _, _, args, _) => {
            kids.push(r);
            kids.extend(args.iter_mut());
        }
        Block(b) | Loop(b, _) => return subst_block(b, self_ty),
        If(c, t, e2, _) => {
            subst_expr(c, self_ty);
            subst_block(t, self_ty);
            if let Some(e2) = e2 {
                subst_expr(e2, self_ty);
            }
            return;
        }
        While(c, b, _) => {
            subst_expr(c, self_ty);
            subst_block(b, self_ty);
            return;
        }
        Match(sc, arms, _) => {
            subst_expr(sc, self_ty);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    subst_expr(g, self_ty);
                }
                subst_expr(&mut a.body, self_ty);
            }
            return;
        }
        Break(v, _) | Return(v, _) => {
            if let Some(v) = v {
                subst_expr(v, self_ty);
            }
            return;
        }
        Closure(ps, r, body, _) => {
            for (_, t) in ps.iter_mut().flat_map(|(n, t)| t.as_mut().map(|t| (n, t))) {
                subst_ty(t, self_ty);
            }
            if let Some(r) = r {
                subst_ty(r, self_ty);
            }
            kids.push(body);
        }
        Int(..) | Trit(..) | Bool(..) | Unit(_) | Continue(_) => {}
    }
    for k in kids {
        subst_expr(k, self_ty);
    }
}

/// Expand every `trait` and `impl` in the file into ordinary functions.
///
/// What the file's impl blocks say, in the two forms the rest of lowering
/// asks for.
///
/// A trait with type parameters may be implemented by one type many times,
/// so neither "does `T` implement `Trait`?" nor "which function is `T`'s
/// `method`?" has a single answer any more. `From<t9>` and `From<t27>` are
/// different requirements and different functions, and both are identified by
/// the trait's name with its arguments appended — `From.t9`, `From.t27` —
/// which is the same mangling instantiated generics use.
#[derive(Default, Clone)]
struct Impls {
    /// Every (type, trait-with-its-arguments) pair the file implements.
    pairs: std::collections::HashSet<(String, String)>,
    /// The trait-qualified functions each (type, method name) has. Only
    /// methods of a parameterized trait are here; every other method is found
    /// under `Type.method` as it always was.
    by_method: HashMap<(String, String), Vec<String>>,
    /// Impls that hold for every type satisfying a bound (Ch. 4 §5.6).
    blankets: Vec<Blanket>,
    /// Generic impls of a parameterized trait, whose arguments are the
    /// impl's own parameters — `impl<T> Make<T> for Pair<T>`.
    parameterized: Vec<Parameterized>,
}

/// An impl whose self type is one of its own parameters.
///
/// `impl<T, U: From<T>> Into<U> for T` is not an implementation for a type;
/// it is a rule about all of them. Every other impl in the file is found by
/// name — "does `Bar` have `area`?" is a lookup — and this one is found by
/// checking a condition, which is the whole of what makes it different.
///
/// Nothing else has to change, because a generic body here is lowered by
/// reading the same source under an environment: the rule's methods are
/// ordinary generic functions, and applying the rule is binding `T` and `U`
/// and instantiating.
#[derive(Clone)]
struct Blanket {
    /// The impl's parameters, with the bounds applying the rule must check.
    generics: Vec<ast::GenericParam>,
    /// Which parameter is the self type.
    self_param: String,
    /// The trait it provides.
    trait_name: String,
    /// Its arguments, as written — parameters of the impl.
    trait_args: Vec<ast::Ty>,
    /// Method name to the generic function its body became.
    methods: HashMap<String, String>,
    /// Where it was written.
    span: Span,
}

/// A method becomes a function named `Type.method`, `Self` substituted away
/// and the receiver an ordinary leading parameter. Everything downstream —
/// signatures, calls, drops, the borrow checker — then works unchanged, which
/// is the point: Ch. 4 §1.2's impl block is a naming construct, not a new
/// kind of code.
fn expand_impls(file: &ast::File, types: &Types) -> (Vec<ast::FnItem>, Vec<Error>, Impls) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let mut table = Impls::default();
    let mut traits: HashMap<String, &ast::TraitItem> = HashMap::new();

    for item in &file.items {
        if let ast::Item::Trait(t) = item
            && traits.insert(t.name.clone(), t).is_some()
        {
            errs.push(SyntaxError {
                span: t.span,
                message: format!("`{}` is defined more than once", t.name),
            });
        }
    }
    for t in traits.values() {
        for s in &t.supertraits {
            if !traits.contains_key(s) && s != "Eq" && s != "Ord" {
                errs.push(SyntaxError {
                    span: t.span,
                    message: format!("`{s}` is not a trait in scope"),
                });
            }
        }
    }

    // A rule that holds for every type overlaps every hand-written impl of
    // the same trait, and §1.8 makes overlapping impls an error. Closing the
    // trait is what keeps that from being a collision a reader discovers by
    // hitting it (Ch. 4 §5.6, which closes `Into` for exactly this reason).
    // A blanket impl covers every type satisfying its bounds — not every
    // type. `impl<I: Iterator> IntoIterator for I` says nothing about `&str`,
    // which is not an iterator, so `impl IntoIterator for &str` does not
    // overlap it (Ch. 4 §§1.8, 5.6).
    let covered: Vec<(String, Vec<String>)> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Impl(imp) if imp.generics.iter().any(|g| g.name() == imp.self_ty) => {
                let bounds = imp
                    .generics
                    .iter()
                    .find(|g| g.name() == imp.self_ty)
                    .and_then(|g| match g {
                        ast::GenericParam::Type { bounds, .. } => {
                            Some(bounds.iter().map(|b| b.name.clone()).collect())
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                imp.trait_name.clone().map(|t| (t, bounds))
            }
            _ => None,
        })
        .collect();
    // Which (type, trait) pairs the file writes out, which is what decides
    // whether a blanket's bounds are met.
    let written: std::collections::HashSet<(String, bool, String)> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Impl(imp) => imp
                .trait_name
                .clone()
                .map(|t| (imp.self_ty.clone(), imp.self_ref, t)),
            _ => None,
        })
        .collect();

    // Which methods each type already has, so a collision is reported rather
    // than silently resolved (§1.3).
    let mut defined: HashMap<String, Span> = HashMap::new();

    for item in &file.items {
        let ast::Item::Impl(imp) = item else { continue };
        // `impl Vec<char>` — an impl for *one* instantiation. Its methods
        // belong to that instantiation and are keyed by its name, which is
        // the name the type answers to everywhere else (Ch. 4 §2.7).
        let concrete_self;
        let one_instantiation = imp.generics.is_empty() && !imp.self_args.is_empty();
        let self_ty = if one_instantiation {
            let ty = match resolve_ty(
                &ast::Ty::App(imp.self_ty.clone(), imp.self_args.clone(), imp.span),
                types,
            ) {
                Ok(t) => t,
                Err(e) => {
                    errs.push(e);
                    continue;
                }
            };
            let Some(n) = nominal_name(&ty) else {
                errs.push(SyntaxError {
                    span: imp.span,
                    message: format!("`{}` has no methods", imp.self_ty),
                });
                continue;
            };
            concrete_self = n;
            &concrete_self
        } else {
            &imp.self_ty
        };

        if let Some(t) = &imp.trait_name
            && covered.iter().any(|(name, bounds)| {
                name == t
                    && bounds
                        .iter()
                        .all(|b| written.contains(&(imp.self_ty.clone(), imp.self_ref, b.clone())))
            })
            && !imp.generics.iter().any(|g| g.name() == *self_ty)
        {
            errs.push(SyntaxError {
                span: imp.span,
                message: format!(
                    "`{t}` holds for every type by a blanket impl, so it may not be \
                     implemented by hand: implementing it for `{self_ty}` would overlap, \
                     and overlapping impls are an error (Ch. 4 §§1.8, 5.6)"
                ),
            });
            continue;
        }

        // A rule over every type satisfying a bound, rather than an impl for
        // one type (Ch. 4 §5.6). Its methods become generic functions keyed
        // by the trait, and applying it is binding the parameters.
        if imp.generics.iter().any(|g| g.name() == *self_ty) {
            match blanket_impl(imp, &traits, &mut out) {
                Ok(b) => table.blankets.push(b),
                Err(e) => errs.push(e),
            }
            continue;
        }

        if !types.structs.borrow().contains_key(self_ty)
            && !types.enums.borrow().contains_key(self_ty)
            && !types.generic_structs.contains_key(self_ty)
            && !types.generic_enums.contains_key(self_ty)
            && !matches!(
                self_ty.as_str(),
                // `Vec` is a language item and a nominal generic type both,
                // so that the library can write `impl<A> FromIterator for
                // Vec<A>` the way Ch. 5 §3.3 says it does. It has no
                // declaration to read parameters from; `Vec<A>` names them.
                "trit" | "bool" | "t9" | "t27" | "taddr" | "char" | "str" | "Vec"
            )
            // A concrete instantiation answers to its mangled name, which is
            // registered rather than declared.
            && !types.instantiations.borrow().contains_key(self_ty)
        {
            errs.push(SyntaxError {
                span: imp.span,
                message: format!("`{self_ty}` is not a type in scope"),
            });
            continue;
        }

        if !imp.generics.is_empty()
            && !types.generic_structs.contains_key(self_ty)
            && !types.generic_enums.contains_key(self_ty)
            && self_ty != "Vec"
        {
            errs.push(SyntaxError {
                span: imp.span,
                message: format!(
                    "`{self_ty}` is not a generic type, so this impl has \
                                  type parameters nothing can determine (Ch. 4 §2.1)"
                ),
            });
            continue;
        }
        if !imp.generics.is_empty() && imp.trait_name.as_deref() == Some("Drop") {
            errs.push(SyntaxError {
                span: imp.span,
                message: "a destructor on a generic type is not implemented; whether a \
                          type needs dropping is decided before its instantiations exist"
                    .into(),
            });
            continue;
        }

        // A trait with parameters may be implemented by one type many
        // times, so the arguments are part of what identifies the impl —
        // and of what identifies each of its methods.
        let mut qualifier = String::new();
        if let Some(trait_name) = &imp.trait_name {
            let params = traits.get(trait_name).map_or(0, |t| t.params.len());
            if params != imp.trait_args.len() {
                errs.push(SyntaxError {
                    span: imp.span,
                    message: format!(
                        "`{trait_name}` takes {params} type argument(s), {} given",
                        imp.trait_args.len()
                    ),
                });
                continue;
            }
            // `impl<T> Make<T> for Pair<T>` — the trait's arguments are the
            // impl's own parameters, so they have no concrete value here and
            // one is determined by the self type. Such an impl can exist at
            // most once for a (type, trait): two of them would be the same
            // impl. So it needs no qualifier to tell it from another, and
            // which arguments it means is worked out per instantiation, in
            // `parameterized_rule` (Ch. 4 §§1.7, 2.1).
            let from_params = !imp.trait_args.is_empty()
                && imp.trait_args.iter().all(|a| {
                    matches!(a, ast::Ty::Name(n, _) if imp.generics.iter().any(|g| g.name() == n))
                });
            if from_params {
                table.parameterized.push(Parameterized {
                    base: self_ty.clone(),
                    params: impl_params(imp).0,
                    trait_name: trait_name.clone(),
                    args: imp.trait_args.clone(),
                });
                table.pairs.insert((self_ty.clone(), trait_name.clone()));
            } else {
                let mut args = Vec::new();
                let mut bad = false;
                for a in &imp.trait_args {
                    match resolve_ty(a, types) {
                        Ok(t) => args.push(t),
                        Err(e) => {
                            errs.push(e);
                            bad = true;
                        }
                    }
                }
                if bad {
                    continue;
                }
                if !args.is_empty() {
                    qualifier = format!("{}.", mangle(trait_name, &args));
                }
                table
                    .pairs
                    .insert((self_ty.clone(), mangle(trait_name, &args)));
            }
        }

        // What `Self` means here: the concrete type, or the applied generic.
        let mut self_written = if imp.self_args.is_empty() || one_instantiation {
            // For one instantiation the name *is* the type; the arguments
            // are already in it.
            ast::Ty::Name(self_ty.clone(), imp.span)
        } else {
            ast::Ty::App(self_ty.clone(), imp.self_args.clone(), imp.span)
        };
        // `impl Trait for &T` — the methods are keyed under `T`, because a
        // reference's methods have always been the referent's, and `Self` is
        // the reference (Ch. 4 §2.1).
        if imp.self_ref {
            self_written = ast::Ty::Ref(Box::new(self_written), imp.self_mut, imp.span);
        }
        let self_repr = SelfTy {
            ty: self_written,
            name: self_ty.clone(),
        };

        if imp.negative {
            continue;
        }

        let mut methods: Vec<ast::FnItem> = imp.methods.clone();

        if let Some(trait_name) = &imp.trait_name {
            // `Drop` is the language's own (Ch. 4 §5.2) and is not declared
            // in the file.
            if trait_name == "Drop" {
                if let Err(e) = check_drop_impl(imp) {
                    errs.push(e);
                    continue;
                }
            } else {
                let Some(decl) = traits.get(trait_name) else {
                    errs.push(SyntaxError {
                        span: imp.span,
                        message: format!("`{trait_name}` is not a trait in scope"),
                    });
                    continue;
                };
                match check_trait_impl(decl, imp, &self_repr, types) {
                    Ok(defaults) => methods.extend(defaults),
                    Err(e) => {
                        errs.push(e);
                        continue;
                    }
                }
            }
        }

        for m in &methods {
            let mut f = subst_self(m, &self_repr);
            // An impl's type parameters become the method's, so a generic
            // method is an ordinary generic function keyed by the base type.
            if !imp.generics.is_empty() {
                // An impl's parameters are matched to the self type's
                // arguments by *position*, so the ones the self type names
                // are put first, in its order, and any left over follow.
                //
                // `impl<I: Iterator, B, F: Fn(I::Item) -> B> Iterator for
                // Map<I, F>` has three parameters and `Map` takes two; `B`
                // is not written in the self type and is determined by `F`'s
                // bound instead (Ch. 4 §§2.1, 4.3).
                let (named, _) = impl_params(imp);
                let mut ordered: Vec<ast::GenericParam> = Vec::new();
                for n in &named {
                    if let Some(g) = imp.generics.iter().find(|g| g.name() == n) {
                        ordered.push(g.clone());
                    }
                }
                for g in &imp.generics {
                    if !ordered.iter().any(|o| o.name() == g.name()) {
                        ordered.push(g.clone());
                    }
                }
                // The method's own parameters follow the impl's, which is
                // also the order they are settled in: the impl's from the
                // receiver, the method's from the call.
                //
                // A method may not reuse one of the impl's names. Both live
                // in one environment — the receiver's type is written in the
                // impl's parameters and the method's body in its own — so a
                // shadow would make `Self` mean two things at once, and the
                // second `.map()` of a chain would look for a receiver the
                // first never produced.
                if let Some(clash) = f
                    .generics
                    .iter()
                    .find(|g| ordered.iter().any(|o| o.name() == g.name()))
                {
                    errs.push(SyntaxError {
                        span: f.span,
                        message: format!(
                            "`{}` names a type parameter this `impl` already has; \
                             a method's own parameters must be named differently \
                             (Ch. 4 §2.1)",
                            clash.name()
                        ),
                    });
                    continue;
                }
                ordered.extend(f.generics.iter().cloned());
                f.generics = ordered;
            }
            // A destructor keeps its own name so that `fn_key` gives it the
            // `drop.Type` key the drop machinery already uses (Ch. 3 §1.4).
            let is_destructor = imp.trait_name.as_deref() == Some("Drop") && f.name == "drop";
            if !is_destructor && f.name == "drop" {
                errs.push(SyntaxError {
                    span: f.span,
                    message: "a destructor is written `impl Drop for T` (Ch. 4 §5.2); \
                              an inherent `drop` would never be called"
                        .into(),
                });
                continue;
            }
            let key = if is_destructor {
                format!("drop.{self_ty}")
            } else {
                format!("{self_ty}.{qualifier}{}", f.name)
            };
            if !qualifier.is_empty() {
                table
                    .by_method
                    .entry((self_ty.clone(), f.name.clone()))
                    .or_default()
                    .push(key.clone());
            }
            if let Some(first) = defined.insert(key.clone(), f.span) {
                errs.push(SyntaxError {
                    span: f.span,
                    message: format!(
                        "`{self_ty}` already has a method `{}` (line {}); \
                         §1.3 requires the ambiguity be written out",
                        f.name, first.line
                    ),
                });
                continue;
            }
            out.push(ast::FnItem {
                name: if is_destructor { f.name.clone() } else { key },
                ..f
            });
        }
    }

    // A trait declaration's provided bodies are only reachable through an
    // impl, so a trait with no impl contributes nothing but is still checked
    // for a supertrait that does not exist, above.
    (out, errs, table)
}

/// Turn a blanket impl's methods into generic functions and record the rule.
///
/// The self type is a parameter, so `Self` is that parameter's name and the
/// receiver is an ordinary leading argument of that type — which is what the
/// rest of lowering already does for every other method.
fn blanket_impl(
    imp: &ast::ImplItem,
    traits: &HashMap<String, &ast::TraitItem>,
    out: &mut Vec<ast::FnItem>,
) -> R<Blanket> {
    let Some(trait_name) = imp.trait_name.clone() else {
        return err(
            imp.span,
            format!(
                "`{}` is a type parameter, so this impl gives methods to no type; \
                 an inherent impl needs a type (Ch. 4 §1.2)",
                imp.self_ty
            ),
        );
    };
    let Some(decl) = traits.get(&trait_name) else {
        return err(imp.span, format!("`{trait_name}` is not a trait in scope"));
    };
    if decl.params.len() != imp.trait_args.len() {
        return err(
            imp.span,
            format!(
                "`{trait_name}` takes {} type argument(s), {} given",
                decl.params.len(),
                imp.trait_args.len()
            ),
        );
    }

    let self_repr = SelfTy {
        ty: ast::Ty::Name(imp.self_ty.clone(), imp.span),
        name: imp.self_ty.clone(),
    };
    let mut methods = HashMap::new();
    for m in &imp.methods {
        let Some(want) = decl.methods.iter().find(|d| d.name == m.name) else {
            return err(
                m.span,
                format!(
                    "`{trait_name}` has no method `{}`, and a trait impl may supply \
                     nothing else",
                    m.name
                ),
            );
        };
        let want = subst_trait_params_fn(want, &decl.params, &imp.trait_args);
        // `Self::Out` in the declaration is whatever this rule chose for it,
        // exactly as in an ordinary impl (Ch. 4 §1.7) — and it was not
        // substituted here, so a blanket impl that declared an associated
        // type never matched the signature it was implementing.
        let want = subst_assoc_fn(&want, &imp.assoc);
        let want = subst_self(&want, &self_repr);
        let got = subst_self(m, &self_repr);
        let same_ret = match (&want.ret, &got.ret) {
            (None, None) => true,
            (Some(a), Some(b)) => same_ast_ty(a, b),
            _ => false,
        };
        if want.params.len() != got.params.len()
            || !same_ret
            || !want
                .params
                .iter()
                .zip(&got.params)
                .all(|(a, b)| same_ast_ty(&a.ty, &b.ty))
        {
            return err(
                m.span,
                format!(
                    "`{}` does not match the signature `{trait_name}` declares",
                    m.name
                ),
            );
        }
        if !got.generics.is_empty() {
            return err(
                m.span,
                "a method with type parameters of its own, inside a blanket impl, \
                 is not implemented",
            );
        }
        let key = format!("{trait_name}.{}", m.name);
        methods.insert(m.name.clone(), key.clone());
        out.push(ast::FnItem {
            name: key,
            generics: imp.generics.clone(),
            ..got
        });
    }

    Ok(Blanket {
        generics: imp.generics.clone(),
        self_param: imp.self_ty.clone(),
        trait_name,
        trait_args: imp.trait_args.clone(),
        methods,
        span: imp.span,
    })
}

/// `impl Drop for T` must supply exactly `fn drop(self)` (Ch. 4 §5.2).
fn check_drop_impl(imp: &ast::ImplItem) -> R<()> {
    let bad = |span| {
        err(
            span,
            "`impl Drop` supplies exactly one method, `fn drop(self)` (Ch. 4 §5.2)",
        )
    };
    if imp.methods.len() != 1 {
        return bad(imp.span);
    }
    let m = &imp.methods[0];
    if m.name != "drop" || m.ret.is_some() {
        return bad(m.span);
    }
    match m.params.as_slice() {
        [p] if p.name == "self" && matches!(p.ty, ast::Ty::SelfTy(_)) => Ok(()),
        _ => err(
            m.span,
            "a destructor takes `self` by value, so that dropping its fields is not \
             a drop of `self` (Ch. 3 §1.4)",
        ),
    }
}

/// Check an impl against its trait, and return the provided methods it did
/// not override (Ch. 4 §§1.2, 1.5).
fn check_trait_impl(
    decl: &ast::TraitItem,
    imp: &ast::ImplItem,
    self_repr: &SelfTy,
    types: &Types,
) -> R<Vec<ast::FnItem>> {
    for m in &imp.methods {
        let Some(want) = decl.methods.iter().find(|d| d.name == m.name) else {
            return err(
                m.span,
                format!(
                    "`{}` has no method `{}`, and a trait impl may supply nothing else",
                    decl.name, m.name
                ),
            );
        };
        // Compared after resolution, not as written: the trait says
        // `Option<Self::Item>` and the impl says `Option<t27>`, and only the
        // resolved types can tell that those are the same (Ch. 4 §1.7).
        let want = subst_trait_params_fn(want, &decl.params, &imp.trait_args);
        // `Self::Item` in the declaration is whatever this impl chose for it,
        // and a generic impl's signature is compared *as written* — so the
        // choice is substituted textually, before `Self` is (Ch. 4 §1.7).
        let want = subst_assoc_fn(&want, &imp.assoc);
        let want = subst_self(&want, self_repr);
        let mismatch = || {
            err::<()>(
                m.span,
                format!(
                    "`{}` does not match the signature `{}` declares",
                    m.name, decl.name
                ),
            )
        };
        // A generic impl's parameters stand for nothing yet, so its
        // signature is compared as written; a concrete one is compared after
        // resolution, which is the only way `Option<Self::Item>` and
        // `Option<t27>` can be seen to agree.
        let concrete = imp.generics.is_empty();
        let resolve = |t: &ast::Ty| resolve_ty(t, types);
        let same_ret = match (&want.ret, &m.ret) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                let (a, b) = (subst_self_ty(a, self_repr), subst_self_ty(b, self_repr));
                if concrete {
                    resolve(&a)? == resolve(&b)?
                } else {
                    same_ast_ty(&a, &b)
                }
            }
            _ => false,
        };
        if want.params.len() != m.params.len() || !same_ret {
            mismatch()?;
        }
        for (a, b) in want
            .params
            .iter()
            .zip(&m.params)
            .map(|(a, b)| (&a.ty, &b.ty))
        {
            // `self` is the one parameter whose written form differs by
            // design: the trait writes `Self` and the impl its own type.
            let (a, b) = (subst_self_ty(a, self_repr), subst_self_ty(b, self_repr));
            let same = if concrete {
                resolve(&a)? == resolve(&b)?
            } else {
                same_ast_ty(&a, &b)
            };
            if !same {
                mismatch()?;
            }
        }
        if m.body.is_none() {
            return err(m.span, format!("`{}` needs a body here", m.name));
        }
    }
    for (c, _) in &decl.consts {
        if !imp.consts.iter().any(|k| &k.name == c) {
            return err(
                imp.span,
                format!(
                    "`impl {} for {}` is missing `const {c}`, which the trait requires \
                     (Ch. 4 §1.7)",
                    decl.name, imp.self_ty
                ),
            );
        }
    }
    for a in &decl.assoc {
        if !imp.assoc.iter().any(|(n, _)| n == a) {
            return err(
                imp.span,
                format!(
                    "`impl {} for {}` is missing `type {a}`, which the trait requires \
                     (Ch. 4 §1.7)",
                    decl.name, imp.self_ty
                ),
            );
        }
    }
    for (n, _) in &imp.assoc {
        if !decl.assoc.contains(n) {
            return err(
                imp.span,
                format!("`{}` declares no associated type `{n}`", decl.name),
            );
        }
    }
    let mut defaults = Vec::new();
    for d in &decl.methods {
        if imp.methods.iter().any(|m| m.name == d.name) {
            continue;
        }
        match d.body {
            Some(_) => defaults.push(d.clone()),
            None => {
                return err(
                    imp.span,
                    format!(
                        "`impl {} for {}` is missing `{}`, which the trait requires",
                        decl.name, imp.self_ty, d.name
                    ),
                );
            }
        }
    }
    Ok(defaults)
}

/// `subst_self` for a single type.
fn subst_self_ty(t: &ast::Ty, self_ty: &SelfTy) -> ast::Ty {
    let mut t = t.clone();
    subst_ty(&mut t, self_ty);
    t
}

/// Replace `Self::Name` by the type an impl chose for it.
///
/// Textual, and deliberately so: it runs before `Self` itself is substituted,
/// while `Self::Item` is still spelled that way, and a generic impl's
/// signature is compared as written because its parameters stand for nothing
/// yet.
fn subst_assoc(t: &ast::Ty, chosen: &[(String, ast::Ty)]) -> ast::Ty {
    use ast::Ty::*;
    match t {
        Assoc(base, name, _) if matches!(**base, SelfTy(_)) => chosen
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| t.clone()),
        Assoc(base, n, l) => Assoc(Box::new(subst_assoc(base, chosen)), n.clone(), *l),
        App(n, xs, l) => App(
            n.clone(),
            xs.iter().map(|x| subst_assoc(x, chosen)).collect(),
            *l,
        ),
        Ref(x, m, l) => Ref(Box::new(subst_assoc(x, chosen)), *m, *l),
        Slice(x, l) => Slice(Box::new(subst_assoc(x, chosen)), *l),
        Array(x, n, l) => Array(Box::new(subst_assoc(x, chosen)), n.clone(), *l),
        Tuple(xs, l) => Tuple(xs.iter().map(|x| subst_assoc(x, chosen)).collect(), *l),
        other => other.clone(),
    }
}

/// The same, over a whole signature.
fn subst_assoc_fn(f: &ast::FnItem, chosen: &[(String, ast::Ty)]) -> ast::FnItem {
    if chosen.is_empty() {
        return f.clone();
    }
    let mut f = f.clone();
    for p in &mut f.params {
        p.ty = subst_assoc(&p.ty, chosen);
    }
    if let Some(r) = &mut f.ret {
        *r = subst_assoc(r, chosen);
    }
    f
}

/// Replace a trait's own type parameters by the arguments an impl gave them.
///
/// A trait declares `fn from(x: T) -> Self`, and `T` is neither a type nor a
/// parameter of the impl: it is the trait's, and the impl chose it. So an
/// impl's methods are compared against the declaration with the choice
/// already made.
fn subst_trait_params(t: &ast::Ty, params: &[String], args: &[ast::Ty]) -> ast::Ty {
    use ast::Ty::*;
    match t {
        Name(n, _) => match params.iter().position(|p| p == n) {
            Some(i) if i < args.len() => args[i].clone(),
            _ => t.clone(),
        },
        App(n, xs, l) => App(
            n.clone(),
            xs.iter()
                .map(|x| subst_trait_params(x, params, args))
                .collect(),
            *l,
        ),
        Ref(x, m, l) => Ref(Box::new(subst_trait_params(x, params, args)), *m, *l),
        Slice(x, l) => Slice(Box::new(subst_trait_params(x, params, args)), *l),
        Array(x, n, l) => Array(Box::new(subst_trait_params(x, params, args)), n.clone(), *l),
        Tuple(xs, l) => Tuple(
            xs.iter()
                .map(|x| subst_trait_params(x, params, args))
                .collect(),
            *l,
        ),
        Assoc(x, n, l) => Assoc(Box::new(subst_trait_params(x, params, args)), n.clone(), *l),
        other => other.clone(),
    }
}

/// The same, over a whole signature.
fn subst_trait_params_fn(f: &ast::FnItem, params: &[String], args: &[ast::Ty]) -> ast::FnItem {
    if params.is_empty() {
        return f.clone();
    }
    let mut f = f.clone();
    for p in &mut f.params {
        p.ty = subst_trait_params(&p.ty, params, args);
    }
    if let Some(r) = &mut f.ret {
        *r = subst_trait_params(r, params, args);
    }
    f
}

/// The candidate list, for a diagnostic that says what the choices were.
fn describe_candidates(keys: &[String]) -> String {
    let mut names: Vec<&str> = keys.iter().map(String::as_str).collect();
    names.sort_unstable();
    format!(
        "{} {}",
        if names.len() == 1 { "is" } else { "are" },
        names.join(", ")
    )
}

/// Structural equality of written types, ignoring where they were written.
fn same_ast_ty(a: &ast::Ty, b: &ast::Ty) -> bool {
    use ast::Ty::*;
    match (a, b) {
        (Name(x, _), Name(y, _)) => x == y,
        (App(x, xs, _), App(y, ys, _)) => {
            x == y && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same_ast_ty(x, y))
        }
        (Unit(_), Unit(_)) | (SelfTy(_), SelfTy(_)) => true,
        (Ref(x, m, _), Ref(y, n, _)) => m == n && same_ast_ty(x, y),
        (Slice(x, _), Slice(y, _)) => same_ast_ty(x, y),
        (Array(x, _, _), Array(y, _, _)) => same_ast_ty(x, y),
        (Tuple(x, _), Tuple(y, _)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_ast_ty(x, y))
        }
        // An associated type is written and compared like any other. Without
        // this arm `I::Item` did not equal `I::Item`, and a generic impl
        // could only choose an associated type it could *name* — which is
        // why every adaptor's `Item` was a fixed `t27`.
        (Assoc(x, n, _), Assoc(y, m, _)) => n == m && same_ast_ty(x, y),
        (Dyn(x, _), Dyn(y, _)) => x == y,
        (Never(_), Never(_)) => true,
        _ => false,
    }
}

fn signature_of(
    f: &ast::FnItem,
    key: &str,
    sigs: &RefCell<HashMap<String, (Vec<Ty>, Ty)>>,
) -> Signature {
    let (params, ret) = sigs
        .borrow()
        .get(key)
        .cloned()
        .unwrap_or_else(|| (Vec::new(), Ty::Unit));

    let mut tir_params = Vec::new();
    if ret.is_aggregate() {
        tir_params.push((SRET.to_string(), Type::Ptr));
    }
    tir_params.extend(
        f.params
            .iter()
            .zip(&params)
            .map(|(p, t)| (p.name.clone(), t.tir())),
    );

    Signature {
        name: key.to_string(),
        params: tir_params,
        ret: if ret == Ty::Unit || ret.is_aggregate() {
            None
        } else {
            Some(ret.tir())
        },
    }
}

/// Match a written type against a concrete one, binding any parameter it
/// mentions (Ch. 4 §2.3's inference, in the only form this draft needs).
///
/// Deliberately partial: what it cannot match it leaves unbound, and the
/// caller reports the parameter it could not determine rather than guessing.
fn unify(written: &ast::Ty, got: &Ty, params: &[ast::GenericParam], env: &mut HashMap<String, Ty>) {
    match written {
        ast::Ty::Name(n, _) if params.iter().any(|p| p.name() == n) => {
            env.entry(n.clone()).or_insert_with(|| got.clone());
        }
        ast::Ty::Ref(inner, _, _) => {
            if let Ty::Ref(t, _) = got {
                unify(inner, t, params, env);
            }
        }
        ast::Ty::Slice(inner, _) => match got {
            Ty::Slice(t) | Ty::Array(t, _) => unify(inner, t, params, env),
            _ => {}
        },
        ast::Ty::Array(inner, _, _) => {
            if let Ty::Array(t, _) = got {
                unify(inner, t, params, env);
            }
        }
        ast::Ty::Tuple(ws, _) => {
            if let Ty::Tuple(ts) = got
                && ws.len() == ts.len()
            {
                for (w, t) in ws.iter().zip(ts) {
                    unify(w, t, params, env);
                }
            }
        }
        ast::Ty::App(name, wargs, _) => {
            // `Pair<T, U>` against `Pair.t27.t9`: the instantiation's own
            // arguments are not recoverable from the mangled name, so this
            // matches only when the concrete type is the same application.
            let _ = (name, wargs);
        }
        _ => {}
    }
}

/// Build the functions a `#[derive(…)]` asks for (Ch. 4 §6).
///
/// These are emitted as TIR directly rather than as source, because §5.3.3
/// makes a promise source cannot keep: a derived `cmp` over scalar fields
/// must be straight-line code. The combination of two field results is "the
/// first that is nonzero", which is one `select3` and therefore one `sel3` —
/// and the language has no expression that spells that.
fn derived_functions(file: &ast::File, types: &Types) -> (Vec<Function>, Vec<Ty>, Vec<Error>) {
    let mut funcs = Vec::new();
    let mut on = Vec::new();
    let mut errs = Vec::new();

    for item in &file.items {
        let (name, derives, span, generics) = match item {
            ast::Item::Struct(s) => (&s.name, &s.derives, s.span, &s.generics),
            ast::Item::Enum(e) => (&e.name, &e.derives, e.span, &e.generics),
            _ => continue,
        };
        if derives.is_empty() {
            continue;
        }
        if !generics.is_empty() {
            errs.push(SyntaxError {
                span,
                message: "deriving for a generic type is not implemented; the derived \
                          impl would need the bound §6 puts on every parameter"
                    .into(),
            });
            continue;
        }
        let ty = if matches!(item, ast::Item::Struct(_)) {
            Ty::Struct(name.clone())
        } else {
            Ty::Enum(name.clone())
        };

        let fields: Vec<(String, Ty, i128)> = match &ty {
            Ty::Struct(_) => types.fields(&ty),
            _ => {
                let variants = types.enums.borrow()[name].clone();
                if variants.iter().any(|v| !v.fields.is_empty()) {
                    errs.push(SyntaxError {
                        span,
                        message: "deriving for an enum with a payload is not implemented; \
                                  §6 orders it by discriminant and then by payload, and \
                                  only the discriminant half is built"
                            .into(),
                    });
                    continue;
                }
                Vec::new()
            }
        };

        for d in derives {
            match d.as_str() {
                "Ord" => match derive_cmp(name, &ty, &fields, types) {
                    Ok(f) => funcs.push(f),
                    Err(e) => errs.push(SyntaxError { span, message: e }),
                },
                "Eq" => funcs.push(derive_eq(name)),
                "Clone" => funcs.push(derive_clone(name, &ty, types)),
                _ => {}
            }
        }
        if derives.iter().any(|d| d == "Ord") && !derives.iter().any(|d| d == "Eq") {
            // §1.6: `Ord: Eq`, so deriving one derives the other.
            funcs.push(derive_eq(name));
        }
        on.push(ty);
    }
    (funcs, on, errs)
}

/// `fn cmp(&self, other: &Self) -> trit`, lexicographic by declaration
/// order, and branchless when every field is a Ch. 1 scalar (§5.3.3).
fn derive_cmp(
    name: &str,
    ty: &Ty,
    fields: &[(String, Ty, i128)],
    types: &Types,
) -> Result<Function, String> {
    let mut insts = Vec::new();
    let mut n = 0u32;
    let mut fresh = |p: &str| {
        n += 1;
        format!("{p}.{n}")
    };

    // An enum with no payload compares by its discriminant, which for a
    // fieldless enum is the whole value.
    let parts: Vec<(Ty, i128)> = if fields.is_empty() {
        let l = types.layout(ty);
        vec![(
            match l.size {
                1 => Ty::T9,
                _ => Ty::T27,
            },
            0,
        )]
    } else {
        fields.iter().map(|(_, t, o)| (t.clone(), *o)).collect()
    };

    let mut results = Vec::new();
    for (fty, offset) in &parts {
        let (pa, pb) = (fresh("pa"), fresh("pb"));
        for (p, base) in [(&pa, "self"), (&pb, "other")] {
            insts.push(Inst {
                results: vec![p.clone()],
                kind: InstKind::Offset {
                    p: Operand::Value(base.to_string()),
                    d: Operand::Const(Type::Int(27), Bt::from_i128(*offset)),
                },
            });
        }
        let c = fresh("c");
        if fty.is_scalar() {
            let (va, vb) = (fresh("va"), fresh("vb"));
            for (v, p) in [(&va, &pa), (&vb, &pb)] {
                insts.push(Inst {
                    results: vec![v.clone()],
                    kind: InstKind::Load {
                        ty: fty.tir(),
                        p: Operand::Value(p.clone()),
                    },
                });
            }
            insts.push(Inst {
                results: vec![c.clone()],
                kind: InstKind::Cmp {
                    ty: fty.tir(),
                    a: Operand::Value(va),
                    b: Operand::Value(vb),
                },
            });
        } else {
            // A field that is itself nominal compares with its own `cmp`,
            // which is what makes the derivation recursive.
            let Some(fname) = nominal_name(fty) else {
                return Err(format!(
                    "`{name}` has a field of type {fty}, which has no `cmp`"
                ));
            };
            insts.push(Inst {
                results: vec![c.clone()],
                kind: InstKind::Call {
                    callee: Callee::Direct(format!("{fname}.cmp")),
                    args: vec![Operand::Value(pa), Operand::Value(pb)],
                    ret: Some(Type::Int(1)),
                },
            });
        }
        results.push(c);
    }

    // Fold from the right: the first nonzero result decides, which is one
    // `select3` per field after the first, and no branch at all (§5.3.3).
    let mut acc = Operand::Value(results.pop().expect("at least one part"));
    while let Some(c) = results.pop() {
        let r = fresh("r");
        insts.push(Inst {
            results: vec![r.clone()],
            kind: InstKind::Select3 {
                t: Operand::Value(c.clone()),
                ty: Type::Int(1),
                neg: Operand::Value(c.clone()),
                zero: acc,
                pos: Operand::Value(c),
            },
        });
        acc = Operand::Value(r);
    }

    Ok(Function {
        sig: Signature {
            name: format!("{name}.cmp"),
            params: vec![
                ("self".to_string(), Type::Ptr),
                ("other".to_string(), Type::Ptr),
            ],
            ret: Some(Type::Int(1)),
        },
        blocks: vec![Block {
            label: "entry".to_string(),
            params: Vec::new(),
            insts,
            term: Terminator::Ret(Some(acc)),
        }],
    })
}

/// `fn eq(&self, other: &Self) -> bool`, defined as `cmp(…) == 0t`, which
/// is the agreement §5.3.1 requires between the two.
fn derive_eq(name: &str) -> Function {
    let k = |v: i128| Operand::Const(Type::Int(1), Bt::from_i128(v));
    Function {
        sig: Signature {
            name: format!("{name}.eq"),
            params: vec![
                ("self".to_string(), Type::Ptr),
                ("other".to_string(), Type::Ptr),
            ],
            ret: Some(Type::Int(1)),
        },
        blocks: vec![Block {
            label: "entry".to_string(),
            params: Vec::new(),
            insts: vec![
                Inst {
                    results: vec!["c".to_string()],
                    kind: InstKind::Call {
                        callee: Callee::Direct(format!("{name}.cmp")),
                        args: vec![
                            Operand::Value("self".to_string()),
                            Operand::Value("other".to_string()),
                        ],
                        ret: Some(Type::Int(1)),
                    },
                },
                Inst {
                    results: vec!["b".to_string()],
                    kind: InstKind::Select3 {
                        t: Operand::Value("c".to_string()),
                        ty: Type::Int(1),
                        neg: k(0),
                        zero: k(1),
                        pos: k(0),
                    },
                },
            ],
            term: Terminator::Ret(Some(Operand::Value("b".to_string()))),
        }],
    }
}

/// `fn clone(&self) -> Self` — field-wise, which for a copyable type is the
/// copy the language would have made anyway (§5.5).
fn derive_clone(name: &str, ty: &Ty, types: &Types) -> Function {
    let size = types.size(ty);
    let mut insts = Vec::new();
    for i in 0..size {
        let (pa, pb, v) = (format!("pa.{i}"), format!("pb.{i}"), format!("v.{i}"));
        for (p, base) in [(&pa, "self"), (&pb, SRET)] {
            insts.push(Inst {
                results: vec![p.clone()],
                kind: InstKind::Offset {
                    p: Operand::Value(base.to_string()),
                    d: Operand::Const(Type::Int(27), Bt::from_i128(i)),
                },
            });
        }
        insts.push(Inst {
            results: vec![v.clone()],
            kind: InstKind::Load {
                ty: Type::Int(9),
                p: Operand::Value(pa),
            },
        });
        insts.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: Type::Int(9),
                v: Operand::Value(v),
                p: Operand::Value(pb),
            },
        });
    }
    Function {
        sig: Signature {
            name: format!("{name}.clone"),
            params: vec![
                (SRET.to_string(), Type::Ptr),
                ("self".to_string(), Type::Ptr),
            ],
            ret: None,
        },
        blocks: vec![Block {
            label: "entry".to_string(),
            params: Vec::new(),
            insts,
            term: Terminator::Ret(None),
        }],
    }
}

/// Whether a written path head names this type.
///
/// A generic type is written without its arguments in a path — `Opt::Some`,
/// not `Opt<t27>::Some` — while the type it resolved to carries them in its
/// mangled name, so the head matches either exactly or as a prefix.
fn heads_match(head: &str, name: &str) -> bool {
    head == name || name.starts_with(&format!("{head}."))
}

/// One instantiation waiting to be lowered (Ch. 4 §2.7).
struct Job {
    /// The name it is known by: the generic function's name, mangled with
    /// its type arguments.
    key: String,
    /// The generic definition it comes from.
    from: String,
    /// The environment its body is lowered under.
    env: HashMap<String, Ty>,
    /// How many instantiations deep this one is, so that a generic function
    /// that instantiates itself at a larger type is rejected rather than
    /// diverging (§2.7).
    depth: u32,
}

/// How deep §2.7's termination check lets instantiation go.
const INSTANTIATION_LIMIT: u32 = 64;

/// The name an instantiation is known by.
///
/// A dot, which is what `drop.Buffer` already uses and what both the TIR text
/// format and the assembler accept in an identifier. Diagnostics therefore
/// say `Pair.t27.t9` where the source said `Pair<t27, t9>`.
fn mangle(name: &str, args: &[Ty]) -> String {
    let mut out = name.to_string();
    for a in args {
        out.push('.');
        out.push_str(
            &a.to_string()
                .replace([' ', ',', '&', '[', ']', '(', ')', '<', '>'], "_"),
        );
    }
    out
}

/// Resolve every nominal type in the file and hand them to the layout engine.
fn build_types(file: &ast::File) -> R<Types> {
    let mut types = Types {
        db: RefCell::new(layout::TypeDb::new()),
        destructors: std::collections::BTreeSet::new(),
        no_copy: std::collections::BTreeSet::new(),
        structs: RefCell::new(HashMap::new()),
        enums: RefCell::new(HashMap::new()),
        generic_structs: HashMap::new(),
        generic_enums: HashMap::new(),
        assoc: RefCell::new(HashMap::new()),
        generic_assoc: HashMap::new(),
        consts: HashMap::new(),
        closures: RefCell::new(HashMap::new()),
        traits: file
            .items
            .iter()
            .filter_map(|i| match i {
                ast::Item::Trait(t) => Some((t.name.clone(), t.clone())),
                _ => None,
            })
            .collect(),
        instantiations: RefCell::new(HashMap::new()),
    };

    // Integer constants first of all: a type may need one for an array's
    // length, and types are built before anything else. Evaluated to a
    // fixpoint so that one constant may be written in terms of another.
    {
        let pending: Vec<(&String, &ast::Expr)> = file
            .items
            .iter()
            .filter_map(|i| match i {
                ast::Item::Const(c) => Some((&c.name, &c.value)),
                _ => None,
            })
            .collect();
        for _ in 0..pending.len().max(1) {
            let mut progress = false;
            for (name, value) in &pending {
                if types.consts.contains_key(*name) {
                    continue;
                }
                if let Ok(v) = const_int_in(value, &types.consts) {
                    types.consts.insert((*name).clone(), v);
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
    }

    // A generic definition is not a type until it is applied (Ch. 4 §2.7), so
    // it is set aside here and instantiated on demand.
    for item in &file.items {
        match item {
            ast::Item::Struct(s) if !s.generics.is_empty() => {
                types.generic_structs.insert(s.name.clone(), s.clone());
            }
            ast::Item::Enum(e) if !e.generics.is_empty() => {
                types.generic_enums.insert(e.name.clone(), e.clone());
            }
            _ => {}
        }
    }

    // Names first, so that a type may mention another declared later.
    for item in &file.items {
        match item {
            ast::Item::Struct(s) if s.generics.is_empty() => {
                types
                    .structs
                    .borrow_mut()
                    .insert(s.name.clone(), Vec::new());
            }
            ast::Item::Enum(e) if e.generics.is_empty() => {
                types.enums.borrow_mut().insert(e.name.clone(), Vec::new());
            }
            _ => {}
        }
    }

    for item in &file.items {
        match item {
            ast::Item::Struct(st) if st.generics.is_empty() => {
                let fields: Vec<(String, Ty)> = st
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = resolve_ty(&f.ty, &types)?;
                        check_sized(&ty, f.ty.span(), &format!("the field `{}`", f.name))?;
                        Ok((f.name.clone(), ty))
                    })
                    .collect::<R<_>>()?;
                types.db.borrow_mut().struct_(
                    &st.name,
                    repr_of(st.repr),
                    fields
                        .iter()
                        .map(|(n, t)| (n.as_str(), t.layout_ty()))
                        .collect(),
                );
                types.structs.borrow_mut().insert(st.name.clone(), fields);
            }
            ast::Item::Enum(en) if en.generics.is_empty() => {
                let mut infos = Vec::new();
                let mut variants = Vec::new();
                for v in &en.variants {
                    let fields: Vec<(String, Ty)> = v
                        .fields
                        .iter()
                        .map(|f| Ok((f.name.clone(), resolve_ty(&f.ty, &types)?)))
                        .collect::<R<_>>()?;
                    variants.push(layout::Variant {
                        name: v.name.clone(),
                        fields: fields
                            .iter()
                            .map(|(n, t)| (n.clone(), t.layout_ty()))
                            .collect(),
                        discriminant: v.discriminant,
                    });
                    infos.push(VariantInfo {
                        name: v.name.clone(),
                        fields,
                    });
                }
                types
                    .db
                    .borrow_mut()
                    .enum_(&en.name, repr_of(en.repr), variants);
                types.enums.borrow_mut().insert(en.name.clone(), infos);
            }
            _ => {}
        }
    }

    // `impl Drop for T` (Ch. 4 §5.2) — registered here rather than after
    // expansion, because whether a type has a destructor decides whether it
    // is copyable, and that decides how every use of it lowers.
    for item in &file.items {
        let ast::Item::Impl(imp) = item else { continue };
        if imp.trait_name.as_deref() != Some("Drop") {
            continue;
        }
        if !types.structs.borrow().contains_key(&imp.self_ty)
            && !types.enums.borrow().contains_key(&imp.self_ty)
        {
            return err(
                imp.span,
                format!(
                    "`{}` cannot have a destructor: it is not declared in this file",
                    imp.self_ty
                ),
            );
        }
        if !types.destructors.insert(imp.self_ty.clone()) {
            return err(
                imp.span,
                format!("`{}` has more than one destructor", imp.self_ty),
            );
        }
    }

    // A function named `drop` whose one parameter is named `self` is that
    // parameter's type's destructor (Ch. 3 §1.4). This is the spelling that
    // predates impl blocks, and Ch. 3 §1.4 promises it keeps its meaning.
    for item in &file.items {
        let ast::Item::Fn(f) = item else { continue };
        if f.name != "drop" {
            continue;
        }
        let [only] = &f.params[..] else {
            return err(f.span, "`drop` takes exactly one parameter, named `self`");
        };
        let (param, ty) = (&only.name, &only.ty);
        if param != "self" {
            return err(f.span, "a destructor's parameter must be named `self`");
        }
        let ty = resolve_ty(ty, &types)?;
        let name = match &ty {
            Ty::Struct(n) | Ty::Enum(n) => n.clone(),
            other => {
                return err(
                    f.span,
                    format!("`{other}` cannot have a destructor: it is not declared in this file"),
                );
            }
        };
        if f.ret.is_some() {
            return err(f.span, "a destructor returns nothing");
        }
        if !types.destructors.insert(name.clone()) {
            return err(f.span, format!("`{name}` has more than one destructor"));
        }
    }

    // `impl !Copy for T` (Ch. 4 §5.1) — the only negative impl, and it is
    // read here because copyability decides how every use of a type lowers.
    for item in &file.items {
        let ast::Item::Impl(imp) = item else { continue };
        if !imp.negative {
            continue;
        }
        if imp.trait_name.as_deref() != Some("Copy") {
            return err(
                imp.span,
                "`!Copy` is the only negative implementation the language has, and \
                 §5.1 says why it exists at all",
            );
        }
        if !imp.methods.is_empty() {
            return err(imp.span, "`impl !Copy` has an empty body");
        }
        types.no_copy.insert(imp.self_ty.clone());
    }

    // Associated types (Ch. 4 §1.7). After the nominal types, since one may
    // be chosen as another's.
    for item in &file.items {
        let ast::Item::Impl(imp) = item else { continue };
        // A generic impl chooses per instantiation, so the choice is kept as
        // written and resolved when the instantiation exists — which is what
        // `assoc` being a cell was always for.
        if !imp.generics.is_empty() {
            for (name, t) in &imp.assoc {
                let (named, extras) = impl_params(imp);
                types.generic_assoc.insert(
                    (imp.self_ty.clone(), name.clone()),
                    (named, extras, t.clone()),
                );
            }
            continue;
        }
        for (name, t) in &imp.assoc {
            let ty = resolve_ty(t, &types)?;
            types
                .assoc
                .borrow_mut()
                .insert((imp.self_ty.clone(), name.clone()), ty);
        }
    }

    // Ask the layout engine about every nominal type now, so that an
    // ill-formed one — an infinite type, a duplicate discriminant — is
    // reported here rather than at its first use.
    for item in &file.items {
        let (name, span) = match item {
            ast::Item::Struct(s) if s.generics.is_empty() => (&s.name, s.span),
            ast::Item::Enum(e) if e.generics.is_empty() => (&e.name, e.span),
            _ => continue,
        };
        if let Err(e) = layout::layout_of(&types.db.borrow(), &layout::Ty::named(name)) {
            return err(span, e.to_string());
        }
    }
    Ok(types)
}

fn repr_of(r: ast::Repr) -> layout::Repr {
    match r {
        ast::Repr::Lang => layout::Repr::Lang,
        ast::Repr::Linear => layout::Repr::Linear,
    }
}

fn resolve_ty(t: &ast::Ty, types: &Types) -> R<Ty> {
    resolve_ty_env(t, types, &HashMap::new())
}

/// Resolve a written type, with a generic environment in scope.
///
/// A type parameter is not a kind of `Ty`: it is a name the environment maps
/// to a concrete one. That is what makes monomorphization (Ch. 4 §2.7) a
/// matter of lowering the same body under a different environment, rather
/// than of rewriting it.
fn resolve_ty_env(t: &ast::Ty, types: &Types, env: &HashMap<String, Ty>) -> R<Ty> {
    match t {
        // `!` — the type with no values (Ch. 1 §2).
        ast::Ty::Never(_) => Ok(Ty::Never),
        // Substitution replaces every `Self` before lowering (Ch. 4 §1.2),
        // so one surviving here was written where there is no impl.
        ast::Ty::SelfTy(l) => err(
            *l,
            "`Self` names the implementing type, and there is none here",
        ),
        // `T::Item` — the type this impl chose (Ch. 4 §1.7).
        ast::Ty::Assoc(base, name, span) => {
            let base = resolve_ty_env(base, types, env)?;
            let Some(owner) = nominal_name(&base) else {
                return err(*span, format!("{base} has no associated types"));
            };
            if let Some(t) = types.assoc.borrow().get(&(owner, name.clone())) {
                return Ok(t.clone());
            }
            match types.assoc_of_instantiation(&base, name, *span)? {
                Some(t) => Ok(t),
                None => err(
                    *span,
                    format!("`{base}` chooses no type for `{name}` (Ch. 4 §1.7)"),
                ),
            }
        }
        // `impl Fn(…)` is an anonymous type parameter, desugared to a named
        // one before lowering, so one surviving here was written somewhere
        // that has no parameter list to add it to.
        ast::Ty::ImplFn(k, _, _, l) => err(
            *l,
            format!(
                "`impl {}` is a parameter type and nothing else (Ch. 4 §2.2). In return \
                 position it would mean returning a closure, which needs an allocator \
                 and waits for the library chapter (Ch. 4 §4.5)",
                k.name()
            ),
        ),
        // `Name<T, U>` — instantiate now, so that everything downstream sees
        // an ordinary nominal type (Ch. 4 §2.7).
        ast::Ty::App(name, args, span) => {
            let args: Vec<Ty> = args
                .iter()
                .map(|a| resolve_ty_env(a, types, env))
                .collect::<R<_>>()?;
            // `Box<T>` is a language item, not a struct anyone declared: its
            // inside is not Trust, because an allocator's job is the one
            // operation this language does not have (Ch. 5 §2.1).
            if name == "Box" {
                let [inner] = &args[..] else {
                    return err(*span, "`Box` takes one type argument");
                };
                check_sized(inner, *span, "a `Box`'s contents")?;
                return Ok(Ty::Boxed(Box::new(inner.clone())));
            }
            if name == "Vec" {
                let [inner] = &args[..] else {
                    return err(*span, "`Vec` takes one type argument");
                };
                check_sized(inner, *span, "a `Vec`'s elements")?;
                // A `Vec` is a language item *and* a nominal type, so that
                // the library can write `impl<A> FromIterator<A> for Vec<A>`
                // the way Ch. 5 §3.3 says it does. Registering the
                // instantiation is the whole of it: method resolution finds a
                // generic impl by asking what a mangled name was made from.
                types
                    .instantiations
                    .borrow_mut()
                    .entry(mangle("Vec", &args))
                    .or_insert_with(|| ("Vec".to_string(), args.clone()));
                return Ok(Ty::VecOf(Box::new(inner.clone())));
            }
            types.instantiate(name, &args, *span)
        }
        // §3.4: a trait may be used as an object only if every method is
        // object-safe, and this is where "used as" happens.
        ast::Ty::Dyn(name, span) => {
            if !types.traits.contains_key(name) {
                return err(*span, format!("`{name}` is not a trait in scope"));
            }
            for m in &object_methods(&types.traits, name, &mut Vec::new()) {
                object_safe(m, name)?;
            }
            Ok(Ty::Dyn(name.clone()))
        }
        ast::Ty::Unit(_) => Ok(Ty::Unit),
        ast::Ty::Tuple(ts, _) => Ok(Ty::Tuple(
            ts.iter()
                .map(|t| resolve_ty_env(t, types, env))
                .collect::<R<_>>()?,
        )),
        ast::Ty::Ref(t, mutable, _) => {
            Ok(Ty::Ref(Box::new(resolve_ty_env(t, types, env)?), *mutable))
        }
        ast::Ty::Slice(t, span) => {
            let elem = resolve_ty_env(t, types, env)?;
            if elem.is_unsized() {
                return err(*span, "a slice element must have a size");
            }
            Ok(Ty::Slice(Box::new(elem)))
        }
        ast::Ty::Name(name, span) => match name.as_str() {
            "trit" => Ok(Ty::Trit),
            "bool" => Ok(Ty::Bool),
            "t9" => Ok(Ty::T9),
            "t27" => Ok(Ty::T27),
            "taddr" => Ok(Ty::TAddr),
            "char" => Ok(Ty::Char),
            // `str` is `[char]` and nothing more: a slice of characters, and
            // dynamically sized for the same reason every slice is
            // (Ch. 5 §1.3).
            "str" => Ok(Ty::Slice(Box::new(Ty::Char))),
            // And `String` is `Vec<char>`, which is what Ch. 5 §2.6 says it
            // is — so `&String` becomes `&str` by the coercion above and
            // needs no rule of its own.
            "String" => {
                // Registering the instantiation is what makes `Vec`'s impls
                // findable for it: `String` is not a second type, it is this
                // one under a shorter name (Ch. 5 §2.6).
                types
                    .instantiations
                    .borrow_mut()
                    .entry(mangle("Vec", &[Ty::Char]))
                    .or_insert_with(|| ("Vec".to_string(), vec![Ty::Char]));
                Ok(Ty::VecOf(Box::new(Ty::Char)))
            }
            // Ch. 1 §8 claims these so no user identifier can take them.
            "t3" | "t81" | "f27" => err(
                *span,
                format!("`{name}` is a reserved type name (Ch. 1 §8)"),
            ),
            // A type parameter in scope.
            other if env.contains_key(other) => Ok(env[other].clone()),
            other
                if types.generic_structs.contains_key(other)
                    || types.generic_enums.contains_key(other) =>
            {
                err(
                    *span,
                    format!("`{other}` is generic and needs its arguments written: `{other}<…>`"),
                )
            }
            other if types.structs.borrow().contains_key(other) => {
                Ok(Ty::Struct(other.to_string()))
            }
            other if types.enums.borrow().contains_key(other) => Ok(Ty::Enum(other.to_string())),
            // A `Vec`'s instantiation answers to a mangled name like every
            // other, but has no declaration behind it to have made a struct
            // — `Vec` is a language item. `impl Vec<char>` writes `Self` as
            // this name, so it has to resolve back (Ch. 5 §2.6).
            other => {
                if let Some(("Vec", args)) = types
                    .instantiations
                    .borrow()
                    .get(other)
                    .map(|(b, a)| (b.as_str(), a.clone()))
                    && let [inner] = &args[..]
                {
                    return Ok(Ty::VecOf(Box::new(inner.clone())));
                }
                err(*span, format!("`{other}` is not a type in scope"))
            }
        },
        ast::Ty::Array(elem, count, span) => {
            let elem = resolve_ty_env(elem, types, env)?;
            let n = const_int_in(count, &types.consts)?;
            if n < 0 {
                // Ch. 2 §3: the type-level face of the signed-taddr decision.
                return err(*span, format!("array length {n} is negative"));
            }
            Ok(Ty::Array(Box::new(elem), n))
        }
    }
}

/// Evaluate a constant expression. Ch. 0 §3.2: exactly, in balanced ternary.
/// Evaluate a constant expression, exactly and in balanced ternary — the
/// same evaluation the assembler performs, and for the reason Ch. 0 §3.2
/// gives: a constant that means one thing to the compiler and another to the
/// assembler is a bug with nowhere to live.
/// Evaluate a constant expression with the named constants in scope, so that
/// `const N: taddr = 8;` may be an array's length (Ch. 0 §3.2 says a length
/// is a constant expression, and a `const` is one).
fn const_int_in(e: &ast::Expr, named: &HashMap<String, i128>) -> R<i128> {
    if let ast::Expr::Path(name, span) = e {
        return match named.get(name) {
            Some(v) => Ok(*v),
            None => err(
                *span,
                format!("`{name}` is not a constant this expression can use"),
            ),
        };
    }
    const_int_raw(e, named)
}

fn const_int_raw(e: &ast::Expr, named: &HashMap<String, i128>) -> R<i128> {
    let const_int = |e: &ast::Expr| const_int_in(e, named);
    let big = |v: &Bt, span: Span| {
        v.to_i128()
            .ok_or(())
            .or_else(|()| err(span, format!("{v} is too large")))
    };
    match e {
        ast::Expr::Int(v, span) => big(v, *span),
        ast::Expr::Trit(t, _) => Ok(i128::from(t.to_i8())),
        ast::Expr::Bool(b, _) => Ok(i128::from(*b)),
        ast::Expr::Unary("-", inner, _) => Ok(-const_int(inner)?),
        ast::Expr::Unary(op, _, span) => err(*span, format!("`{op}` is not a constant operation")),
        ast::Expr::Binary(op, a, b, span) => {
            let (a, b) = (const_int(a)?, const_int(b)?);
            let bt = |v: i128| Bt::from_i128(v);
            match *op {
                "+" => Ok(a + b),
                "-" => Ok(a - b),
                "*" => Ok(a * b),
                // Round-to-nearest, ties away from zero — the AM's only
                // division, so that a constant folds to what the machine
                // would compute (Ch. 1 §4).
                "/" | "%" => {
                    if b == 0 {
                        return err(*span, "division by zero in a constant");
                    }
                    let Some((q, r)) = bt(a).divrem(&bt(b)) else {
                        return err(*span, "division by zero in a constant");
                    };
                    big(if *op == "/" { &q } else { &r }, *span)
                }
                ">>" | "<<" => {
                    let k = u32::try_from(b.abs()).map_err(|_| SyntaxError {
                        span: *span,
                        message: "shift too large".into(),
                    })?;
                    if *op == "<<" {
                        big(&bt(a).shl(k), *span)
                    } else {
                        big(&bt(a).shr(k), *span)
                    }
                }
                other => err(*span, format!("`{other}` is not a constant operation")),
            }
        }
        ast::Expr::Cast(inner, _, _) => const_int(inner),
        other => err(
            other.span(),
            "this is not a constant expression: draft 0.1 evaluates literals and \
             arithmetic on them (Ch. 0 §3.2)",
        ),
    }
}

fn const_item(c: &ast::ConstItem, module: &mut Module, types: &Types) -> R<Global> {
    let const_int = |e: &ast::Expr| const_int_in(e, &types.consts);
    let ty = resolve_ty(&c.ty, types)?;
    match (&ty, &c.value) {
        (Ty::Array(elem, n), ast::Expr::Array(items, span)) => {
            if items.len() as i128 != *n {
                return err(
                    *span,
                    format!("expected {n} elements, found {}", items.len()),
                );
            }
            let mut trytes = Vec::new();
            for item in items {
                let v = const_int(item)?;
                let width = elem.width().unwrap_or(27);
                if !Bt::from_i128(v).fits_width(width) {
                    return err(item.span(), format!("{v} does not fit in {elem}"));
                }
                // Little-trytean, like every other multi-tryte value.
                let value = Bt::from_i128(v);
                for i in 0..types.size(elem) {
                    trytes.push(InitItem::Tryte(value.shr(i as u32 * 9).wrap_to(9)));
                }
            }
            let name = format!("const.{}", c.name);
            module.globals.push(ir::Global {
                name: name.clone(),
                trytes: trytes.len() as u32,
                init: Some(trytes),
            });
            Ok(Global::Array(name, ty))
        }
        (t, _) if t.is_scalar() => {
            let v = const_int(&c.value)?;
            let width = t.width().unwrap_or(27);
            if !Bt::from_i128(v).fits_width(width) {
                return err(c.value.span(), format!("{v} does not fit in {t}"));
            }
            Ok(Global::Const(Bt::from_i128(v), ty))
        }
        _ => err(c.span, "this constant's form is not supported yet"),
    }
}

// --------------------------------------------------------------- lowering

struct Fn<'a> {
    /// Where to record the type of each expression, if anyone asked.
    noted: Option<&'a RefCell<Noted>>,
    /// Signatures, shared and mutable: instantiating a generic function adds
    /// one, and the call site that caused it must be able to check its
    /// arguments against it immediately (Ch. 4 §2.7).
    sigs: &'a RefCell<HashMap<String, (Vec<Ty>, Ty)>>,
    /// Trait declarations, for `dyn Trait` (Ch. 4 §3).
    traits: &'a HashMap<String, ast::TraitItem>,
    /// Vtables built so far (Ch. 4 §3.3).
    vtables: &'a RefCell<Vec<(String, String, ir::Global)>>,
    /// Static data the program's literals need, and the globals already made
    /// for a set of characters so that identical literals share one.
    data: &'a RefCell<Vec<ir::Global>>,
    strings: &'a RefCell<HashMap<Vec<i128>, String>>,
    /// Functions synthesized while lowering this one — closure bodies.
    extra_fns: &'a RefCell<Vec<ast::FnItem>>,
    /// Set the first time a `Box` is made or dropped, so the module declares
    /// the allocator only where something calls it.
    needs_heap: &'a std::cell::Cell<bool>,
    /// The signature each `impl Fn(…)` parameter was written with.
    fn_bounds: &'a HashMap<String, (ast::FnKind, Vec<ast::Ty>, Option<ast::Ty>)>,
    /// Where the value being lowered is going, when that is already known.
    ///
    /// An aggregate has no value in TIR — it *is* its storage — so a literal
    /// has to be built somewhere, and building it in a temporary that is
    /// immediately copied is the cost this removes. Only the constructs that
    /// are *transparent* to a destination forward it (`if`, `match`, a
    /// block's tail); everything else drops it at the top of `expr_inner`,
    /// because a subexpression must never take a destination meant for the
    /// expression containing it (TIR §2).
    dest: Option<String>,
    /// This function's own name, so a closure inside it gets a unique one.
    name: String,
    /// Generic function definitions, un-instantiated.
    generic_fns: &'a HashMap<String, ast::FnItem>,
    /// Methods of a generic impl that have type parameters of their own,
    /// with the impl's half of the environment already settled (Ch. 4 §4.3).
    specials: &'a RefCell<HashMap<String, Special>>,
    /// What the file's impls say: which (type, trait-with-arguments) pairs
    /// exist, and which functions a parameterized trait's methods became.
    impls: &'a Impls,
    /// Instantiations discovered here and not yet lowered.
    pending: &'a RefCell<Vec<Job>>,
    /// The type arguments this instantiation was made with, which is what
    /// makes a type parameter a name for a concrete type rather than a kind
    /// of type.
    env: HashMap<String, Ty>,
    globals: &'a HashMap<String, Global>,
    types: &'a Types,
    ret: Ty,

    blocks: Vec<Block>,
    label: String,
    insts: Vec<Inst>,
    /// Slot allocations, which all live in the entry block so that they
    /// dominate every use.
    slots: Vec<Inst>,
    scopes: Vec<HashMap<String, Local>>,
    loops: Vec<LoopCtx>,
    /// Ownership of every local that has a destructor, innermost last.
    ///
    /// Each entry carries the storage it will drop, not just a name to look
    /// up later: two bindings may share a name, and only the newest is
    /// reachable. Draft 0.1 stored names and resolved them at scope exit, so
    /// a shadowed binding leaked and its shadower was dropped twice.
    owned: Vec<Owned>,
    /// Live borrows, and the statement each dies after (Ch. 3 §4.2).
    loans: Vec<Loan>,
    /// The statement being lowered, numbered in traversal order.
    stmt: u32,
    /// The running count, which must match the pre-pass's.
    stmt_index: u32,
    /// The last statement at which each local is used, from a pre-pass over
    /// the same traversal.
    last_use: HashMap<String, u32>,
    /// The names of this function's parameters, for §4.1's check that a
    /// returned reference does not point into a local.
    params: Vec<String>,
    /// Whether elision rule 3 applies here — a `&self` receiver, which lends
    /// to the result and leaves the other parameters out of it (Ch. 4 §1.4).
    self_lends: bool,
    /// Set while lowering a destructor: the type whose `self` this is.
    ///
    /// Inside it, `self` is not dropped as a whole — that would call this
    /// very destructor again. Its fields are dropped instead, which is what
    /// Ch. 3 §1.4 means by the body running before the fields.
    destructor_of: Option<String>,
    counter: u32,
    /// Set once the current block has been terminated.
    done: bool,
}

/// One value a scope is responsible for dropping.
#[derive(Clone)]
struct Owned {
    /// The name it was bound to, for `mark_moved` and `ownership`. Only the
    /// last entry with a given name is reachable by that name.
    name: String,
    /// Its storage, which is unique even when the name is not.
    slot: String,
    /// Its type, which decides the glue.
    ty: Ty,
    /// The flag that decides at run time, where a branch left it undecided.
    drop_flag: Option<String>,
    /// Whether it is still owned here.
    owns: Owns,
    /// The scope it belongs to.
    depth: usize,
}

#[derive(Clone)]
struct LoopCtx {
    /// Where `break` goes.
    exit: String,
    /// The scope depth the body opened at: `break` and `continue` leave
    /// every scope from here inwards, and must drop what those own.
    depth: usize,
    /// Where `continue` goes.
    head: String,
    /// The slot a `break` with a value writes to.
    result: Option<(String, Ty)>,
    /// Whether any `break` targeted this loop. A loop nothing breaks out of
    /// has no exit: its value is `!`, and emitting the exit block anyway
    /// leaves an unreachable block reading a slot nothing defined on a path
    /// that reaches it.
    broke: bool,
}

/// Everything a function body is lowered against, which is the same for
/// every function in the module and is therefore passed as one thing.
struct World<'a> {
    /// Which functions to record expression types for: the file's own, since
    /// the prelude's spans are offsets into a different file.
    record: &'a HashSet<String>,
    /// Where those types go, when anyone asked for them.
    noted: Option<&'a RefCell<Noted>>,
    /// Trait declarations, for `dyn Trait` (Ch. 4 §3).
    traits: &'a HashMap<String, ast::TraitItem>,
    /// Vtables built so far, keyed by (concrete type, trait), and the
    /// globals they became.
    vtables: &'a RefCell<Vec<(String, String, ir::Global)>>,
    /// Static data the program's literals need, and the globals already made
    /// for a set of characters so that identical literals share one.
    data: &'a RefCell<Vec<ir::Global>>,
    strings: &'a RefCell<HashMap<Vec<i128>, String>>,
    /// Functions synthesized while lowering — closure bodies (Ch. 4 §4.2).
    extra_fns: &'a RefCell<Vec<ast::FnItem>>,
    /// Set the first time a `Box` is made or dropped, so the module declares
    /// the allocator only where something calls it.
    needs_heap: &'a std::cell::Cell<bool>,
    /// The signature each `impl Fn(…)` parameter was written with.
    fn_bounds: &'a HashMap<String, (ast::FnKind, Vec<ast::Ty>, Option<ast::Ty>)>,
    sigs: &'a RefCell<HashMap<String, (Vec<Ty>, Ty)>>,
    generic_fns: &'a HashMap<String, ast::FnItem>,
    specials: &'a RefCell<HashMap<String, Special>>,
    impls: &'a Impls,
    pending: &'a RefCell<Vec<Job>>,
    globals: &'a HashMap<String, Global>,
    types: &'a Types,
}

/// A method of a generic impl that has type parameters of its own.
///
/// Instantiation is one step: `instantiate_with` wants an environment that
/// names every parameter. Such a method has two sets — the impl's, known as
/// soon as the receiver's type is, and its own, known only from the call's
/// arguments — so the impl's half is settled here and the method is put back
/// into the queue as an ordinary generic function of what is left
/// (Ch. 4 §§2.7, 4.3).
#[derive(Clone)]
struct Special {
    def: ast::FnItem,
    env: HashMap<String, Ty>,
}

fn function(
    f: &ast::FnItem,
    sig: Signature,
    body: &ast::Block,
    key: &str,
    env: HashMap<String, Ty>,
    w: &World,
) -> R<Function> {
    let World {
        record,
        noted,
        traits,
        vtables,
        data,
        strings,
        extra_fns,
        needs_heap,
        fn_bounds,
        sigs,
        generic_fns,
        specials,
        impls,
        pending,
        globals,
        types,
    } = *w;
    // An instantiation's key is mangled and is not the file's name for
    // anything, so a generic function's body is not recorded — which is the
    // same set the "one answer only" rule would have kept anyway.
    let noted = noted.filter(|_| record.contains(key));
    let (param_tys, ret) = sigs.borrow().get(key).cloned().unwrap();
    let destructor_of = key.strip_prefix("drop.").map(|t| t.to_string());
    let mut fx = Fn {
        noted,
        dest: None,
        traits,
        vtables,
        data,
        strings,
        extra_fns,
        needs_heap,
        fn_bounds,
        name: key.to_string(),
        sigs,
        generic_fns,
        specials,
        impls,
        pending,
        env,
        globals,
        types,
        ret: ret.clone(),
        blocks: Vec::new(),
        label: "entry".to_string(),
        insts: Vec::new(),
        slots: Vec::new(),
        scopes: vec![HashMap::new()],
        loops: Vec::new(),
        owned: Vec::new(),
        loans: Vec::new(),
        stmt: 0,
        stmt_index: 0,
        last_use: last_use_of(body),
        params: f.params.iter().map(|p| p.name.clone()).collect(),
        self_lends: matches!(
            f.params.first(),
            Some(p) if p.name == "self" && matches!(p.ty, ast::Ty::Ref(..))
        ),
        destructor_of,
        counter: 0,
        done: false,
    };

    // Parameters arrive as SSA values and are spilled into slots at once, so
    // that they read and write like any other local.
    for (name, ty) in f.params.iter().map(|p| &p.name).zip(&param_tys) {
        // The parameter arrives as an SSA value; give the local its own
        // storage so that writing to it is local, and copy an aggregate in.
        let incoming = Operand::Value(name.clone());
        let local = fx.declare(name, ty.clone(), true);
        let slot = local.slot.clone();
        fx.store_at(&slot, 0, ty, incoming, f.span)?;
    }

    // What the caller was required to have established, checked once here
    // (Ch. 4 §2.8). The check is a branch, so the body inherits it as a
    // *fact* — which is the whole point: a precondition tested once is a
    // bounds check not tested at all, every iteration of every loop below.
    for pred in &f.requires {
        let (c, ct) = fx.expr(pred, Some(&Ty::Bool))?;
        fx.check(&ct, &Ty::Bool, pred.span(), "a `where` predicate")?;
        let (ok, bad) = (fx.fresh("req.ok"), fx.fresh("req.no"));
        fx.br3(c, &bad, &bad, &ok);
        fx.start(bad);
        fx.finish(Terminator::Trap(FaultCode::Trap));
        fx.start(ok);
    }

    if let Some(tail) = &body.tail {
        fx.check_return_root(tail, &ret, body.span)?;
    }
    // An aggregate result is written through the caller's pointer, so that
    // is where the body's value should be built rather than somewhere it is
    // copied from (TIR §2).
    if ret.is_aggregate() {
        fx.dest = Some(SRET.to_string());
    }
    let (value, ty) = fx.block(body, Some(&ret))?;
    fx.dest = None;
    if !fx.done {
        // The parameters are this function's to drop too (Ch. 3 §1.1).
        if ret == Ty::Unit {
            fx.drop_all(f.span)?;
            fx.finish(Terminator::Ret(None));
        } else {
            // A body's type is its tail's, so that is what the complaint is
            // about: underlining the whole body says the function is wrong
            // when one expression in it is.
            let at = body.tail.as_ref().map_or(body.span, |t| t.span());
            fx.check(&ty, &ret, at, "function body")?;
            if ret.is_aggregate() {
                // Already there, if the body took the destination.
                if value != Operand::Value(SRET.to_string()) {
                    let dst = Operand::Value(SRET.to_string());
                    fx.copy_typed(dst, value, &ret, body.span)?;
                }
                fx.drop_all(f.span)?;
                fx.finish(Terminator::Ret(None));
            } else {
                fx.drop_all(f.span)?;
                fx.finish(Terminator::Ret(Some(value)));
            }
        }
    }

    // The slots are prepended to the entry block, where they dominate
    // everything.
    let mut blocks = fx.blocks;
    let mut entry = blocks.remove(0);
    let mut insts = std::mem::take(&mut fx.slots);
    insts.append(&mut entry.insts);
    entry.insts = insts;
    blocks.insert(0, entry);

    Ok(Function { sig, blocks })
}

impl Fn<'_> {
    // ------------------------------------------------------- block building

    fn fresh(&mut self, what: &str) -> String {
        self.counter += 1;
        format!("{what}.{}", self.counter)
    }

    fn push(&mut self, inst: Inst) {
        if !self.done {
            self.insts.push(inst);
        }
    }

    fn emit(&mut self, what: &str, ty: Type, kind: InstKind) -> Operand {
        let name = self.fresh(what);
        self.push(Inst {
            results: vec![name.clone()],
            kind,
        });
        let _ = ty;
        Operand::Value(name)
    }

    fn finish(&mut self, term: Terminator) {
        if self.done {
            return;
        }
        self.blocks.push(Block {
            label: std::mem::take(&mut self.label),
            params: Vec::new(),
            insts: std::mem::take(&mut self.insts),
            term,
        });
        self.done = true;
    }

    fn start(&mut self, label: String) {
        self.label = label;
        self.insts = Vec::new();
        self.done = false;
    }

    fn jump(&mut self, label: &str) {
        self.finish(Terminator::Br(Target {
            label: label.to_string(),
            args: Vec::new(),
        }));
    }

    /// A three-way branch on a trit-valued operand.
    fn br3(&mut self, t: Operand, neg: &str, zero: &str, pos: &str) {
        let target = |l: &str| Target {
            label: l.to_string(),
            args: Vec::new(),
        };
        self.finish(Terminator::Br3 {
            t,
            neg: target(neg),
            zero: target(zero),
            pos: target(pos),
        });
    }

    // ------------------------------------------------------------- scopes

    /// Give an existing slot a name, so that an argument the caller had to
    /// lower before it could infer anything can still be referred to by an
    /// expression. The name cannot collide: `#` is not an identifier
    /// character.
    fn bind_existing(&mut self, slot: String, ty: Ty) -> String {
        self.counter += 1;
        let name = format!("#a{}", self.counter);
        let local = Local {
            slot,
            ty,
            mutable: false,
            drop_flag: None,
            borrowed: false,
        };
        self.scopes
            .last_mut()
            .expect("a scope")
            .insert(name.clone(), local);
        name
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool) -> Local {
        let slot = self.fresh(&format!("{name}.slot"));
        let trytes = self.types.size(&ty).max(1) as u32;
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot { trytes },
        });
        self.declare_at(name, slot, ty, mutable)
    }

    /// Rename one SSA value everywhere this function has emitted it so far.
    ///
    /// Sound because the name is a *definition* this function made and has
    /// not finished with: nothing outside can hold it, and the new name is
    /// fresh.
    fn rename_value(&mut self, from: &str, to: &str) {
        fn one(inst: &mut Inst, from: &str, to: &str) {
            for r in &mut inst.results {
                if r == from {
                    *r = to.to_string();
                }
            }
            crate::tir::canon::map_operands(&mut inst.kind, &mut |o: &mut Operand| {
                if let Operand::Value(v) = o
                    && v == from
                {
                    *v = to.to_string();
                }
            });
        }
        for inst in self.slots.iter_mut().chain(self.insts.iter_mut()) {
            one(inst, from, to);
        }
        for b in &mut self.blocks {
            for inst in &mut b.insts {
                one(inst, from, to);
            }
            crate::tir::canon::map_terminator(&mut b.term, &mut |o: &mut Operand| {
                if let Operand::Value(v) = o
                    && v == from
                {
                    *v = to.to_string();
                }
            });
        }
    }

    /// The same, for storage that already exists.
    ///
    /// A `let` whose initializer *computed* an aggregate can take that
    /// storage as the binding's own rather than copy out of it — the
    /// temporary was made for this value and nothing else names it. A place
    /// is different: `let y = x` must not make `y` another name for `x`.
    fn declare_at(&mut self, name: &str, slot: String, ty: Ty, mutable: bool) -> Local {
        // A local whose type has a destructor is dropped when its scope ends,
        // unless it was moved out of (Ch. 3 §1.4).
        let needs_drop = self.types.needs_drop(&ty);
        let drop_flag = needs_drop.then(|| {
            let flag = self.fresh(&format!("{name}.owns"));
            self.slots.push(Inst {
                results: vec![flag.clone()],
                kind: InstKind::Slot { trytes: 1 },
            });
            flag
        });
        let local = Local {
            slot,
            ty,
            mutable,
            drop_flag: drop_flag.clone(),
            borrowed: false,
        };
        let scope = self.scopes.last_mut().expect("a scope");
        scope.insert(name.to_string(), local.clone());
        if let Some(flag) = &drop_flag {
            let one = Operand::Const(Type::Int(1), Bt::from_i128(1));
            let flag = flag.clone();
            self.store_slot(&flag, &Ty::Bool, one, Span::NONE);
            self.owned.push(Owned {
                name: name.to_string(),
                slot: local.slot.clone(),
                ty: local.ty.clone(),
                drop_flag: local.drop_flag.clone(),
                owns: Owns::Yes,
                depth: self.scopes.len() - 1,
            });
        }
        local
    }

    /// Declare a `match` binding that names storage somebody else owns.
    ///
    /// Not `declare` followed by `mark_moved`: that says the place is *not
    /// initialized*, and a reader of `p.id` through a `&Holder` was told `p`
    /// had been moved out of. This one gives the name a slot and no entry in
    /// the owned set at all, so nothing drops it — and marks it, so that
    /// moving out of it is refused with a diagnostic that is true.
    fn declare_borrowed(&mut self, name: &str, ty: Ty) -> Local {
        let slot = self.fresh(&format!("{name}.slot"));
        let trytes = self.types.size(&ty).max(1) as u32;
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot { trytes },
        });
        let local = Local {
            slot,
            ty,
            mutable: false,
            drop_flag: None,
            borrowed: true,
        };
        self.scopes
            .last_mut()
            .expect("a scope")
            .insert(name.to_string(), local.clone());
        local
    }

    /// Record that a place was moved out of (Ch. 3 §1.2).
    fn mark_moved(&mut self, name: &str) {
        if let Some(entry) = self.owned.iter_mut().rev().find(|o| o.name == name) {
            entry.owns = Owns::No;
        }
        if let Some(local) = self.lookup(name)
            && let Some(flag) = local.drop_flag
        {
            let zero = Operand::Const(Type::Int(1), Bt::ZERO);
            self.store_slot(&flag, &Ty::Bool, zero, Span::NONE);
        }
    }

    /// Record that a whole local was given a value again (Ch. 3 §1.2).
    ///
    /// `s = f(s)` moves `s` out and then puts something back, and after it
    /// `s` owns again — which is what makes the idiom writable. Only a *whole*
    /// local: writing one field of a moved-out value re-initializes part of
    /// it, and ownership here is tracked per local rather than per place.
    fn mark_initialized(&mut self, name: &str) {
        if let Some(entry) = self.owned.iter_mut().rev().find(|o| o.name == name) {
            entry.owns = Owns::Yes;
        }
        if let Some(local) = self.lookup(name)
            && let Some(flag) = local.drop_flag
        {
            let one = Operand::Const(Type::Int(1), Bt::from_i128(1));
            self.store_slot(&flag, &Ty::Bool, one, Span::NONE);
        }
    }

    /// The ownership state, for saving across a branch.
    fn owned_snapshot(&self) -> Vec<Owned> {
        self.owned.clone()
    }

    /// Join two paths' ownership: a value moved on either is not certainly
    /// owned afterwards, and its drop is decided by its flag. Matched by
    /// storage, since two entries may share a name.
    fn owned_join(&mut self, a: Vec<Owned>, b: Vec<Owned>) {
        self.owned = a
            .into_iter()
            .map(|mut e| {
                let other = b
                    .iter()
                    .find(|o| o.slot == e.slot)
                    .map(|o| o.owns)
                    .unwrap_or(e.owns);
                e.owns = e.owns.join(other);
                e
            })
            .collect();
    }

    /// Whether a local is known to still own its value.
    fn ownership(&self, name: &str) -> Option<Owns> {
        self.owned
            .iter()
            .rev()
            .find(|o| o.name == name)
            .map(|o| o.owns)
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
    }

    // -------------------------------------------------------------- checks

    fn check(&self, got: &Ty, want: &Ty, span: Span, what: &str) -> R<()> {
        if got == want || *got == Ty::Never {
            return Ok(());
        }
        err(
            span,
            format!("{what} has type {got}, expected {want} (there are no implicit conversions)"),
        )
    }

    // --------------------------------------------------------------- items

    fn block(&mut self, b: &ast::Block, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        let depth = self.scopes.len();
        // A block's *value* is its tail's, so only the tail inherits a
        // destination. A statement is not where the block's value comes
        // from, and would take it.
        let dest = self.dest.take();
        self.scopes.push(HashMap::new());
        for stmt in &b.stmts {
            self.stmt_index += 1;
            self.stmt = self.stmt_index;
            self.stmt(stmt)?;
        }
        let result = match &b.tail {
            Some(e) => {
                self.stmt_index += 1;
                self.stmt = self.stmt_index;
                self.dest = dest;
                self.expr(e, expected)?
            }
            None => (unit(), Ty::Unit),
        };
        self.drop_scope(depth, b.span)?;
        self.scopes.pop();
        Ok(result)
    }

    /// Drop every local of this scope that still owns its value, in reverse
    /// order of declaration (Ch. 3 §1.4).
    fn drop_scope(&mut self, depth: usize, span: Span) -> R<()> {
        if !self.done {
            self.drop_through(depth, span)?;
        }
        // Whether control left by this path or another, the scope is over.
        self.owned.retain(|o| o.depth < depth);
        Ok(())
    }

    /// Emit the drops that leaving these scopes needs, **without** retiring
    /// them: `break`, `continue` and `return` leave along one path while the
    /// scope's other paths still own the same values.
    fn drop_through(&mut self, depth: usize, span: Span) -> R<()> {
        let mine: Vec<Owned> = self
            .owned
            .iter()
            .filter(|o| o.depth >= depth)
            .cloned()
            .collect();

        for e in mine.into_iter().rev() {
            if e.owns == Owns::No {
                continue;
            }
            // Inside a destructor, `self` is not dropped as a whole — that
            // would call this very destructor again (Ch. 3 §1.4).
            let fields_only = e.name == "self" && self.destructor_of.is_some();
            match (e.owns, e.drop_flag.clone()) {
                (Owns::Yes, _) if fields_only => {
                    let addr = Operand::Value(e.slot.clone());
                    self.drop_fields(addr, &e.ty, span, 0)?;
                }
                (Owns::Yes, _) => self.emit_drop(&e.slot, &e.ty, span)?,
                // Ownership depends on the path taken, so the flag decides.
                (_, Some(flag)) => {
                    let f = self.load_slot(&flag, &Ty::Bool);
                    let (yes, join) = (self.fresh("drop.yes"), self.fresh("drop.join"));
                    self.br3(f, &join, &join, &yes);
                    self.start(yes);
                    if fields_only {
                        let addr = Operand::Value(e.slot.clone());
                        self.drop_fields(addr, &e.ty, span, 0)?;
                    } else {
                        self.emit_drop(&e.slot, &e.ty, span)?;
                    }
                    self.jump(&join);
                    self.start(join);
                }
                _ => self.emit_drop(&e.slot, &e.ty, span)?,
            }
        }
        Ok(())
    }

    // ---------------------------------------------------- the borrow rule

    /// The place an expression names, syntactically. `None` where the
    /// checker cannot see through — a dereference, or anything that is not a
    /// place — in which case nothing is checked rather than something wrong.
    fn path_of(&self, e: &ast::Expr) -> Option<PlacePath> {
        match e {
            ast::Expr::Path(name, _) => Some(PlacePath {
                root: name.clone(),
                projections: Vec::new(),
            }),
            ast::Expr::Field(base, name, _) => {
                let mut p = self.path_of(base)?;
                p.projections.push(Proj::Field(name.clone()));
                Some(p)
            }
            ast::Expr::Index(base, ..) => {
                let mut p = self.path_of(base)?;
                p.projections.push(Proj::Index);
                Some(p)
            }
            _ => None,
        }
    }

    /// Retire the loans that are dead at this statement.
    fn retire_loans(&mut self) {
        let now = self.stmt;
        self.loans.retain(|l| l.dies >= now);
    }

    /// Check an access against the live loans (Ch. 3 §2.2).
    fn check_access(&mut self, path: &PlacePath, access: Access, span: Span) -> R<()> {
        // §4.1's first check, before the aliasing one: a place is used only
        // where it is certainly initialized. A move out of a local leaves
        // every projection of it uninitialized too, so `a.x` after `a` moved
        // is as dead as `a` is.
        let root = &path.root;
        let moved = match self.ownership(root) {
            Some(Owns::No) => Some("was moved out of and cannot be used again"),
            Some(Owns::Maybe) => Some("may have been moved out of on some path here"),
            _ => None,
        };
        if let Some(what) = moved
            && !path.projections.is_empty()
        {
            return err(span, format!("`{root}` {what} (Ch. 3 §4.1)"));
        }

        self.retire_loans();
        for loan in &self.loans {
            if !loan.place.conflicts(path) {
                continue;
            }
            let borrowed = describe(&loan.place);
            let since = loan.span;
            let kind = if loan.mutable {
                "exclusively"
            } else {
                "shared"
            };
            let complaint = match access {
                // While an exclusive reference is live, the place may not be
                // read or written except through it.
                Access::Read if loan.mutable => Some("read"),
                Access::Read => None,
                Access::Write => Some("written to"),
                Access::Move => Some("moved out of"),
                // One exclusive borrow, or any number of shared ones.
                Access::Borrow(true) => Some("borrowed exclusively"),
                Access::Borrow(false) if loan.mutable => Some("borrowed"),
                Access::Borrow(false) => None,
            };
            if let Some(what) = complaint {
                return err(
                    span,
                    format!(
                        "`{borrowed}` cannot be {what} here: it is {kind} borrowed \
                         on line {}, and that borrow is still live (Ch. 3 §2.2)",
                        since.line
                    ),
                );
            }
        }
        Ok(())
    }

    /// Record a borrow. It dies after the last use of whatever holds it,
    /// which the caller patches once the binding is known.
    fn add_loan(&mut self, path: PlacePath, mutable: bool, span: Span) {
        let dies = self.stmt;
        self.loans.push(Loan {
            place: path,
            mutable,
            dies,
            span,
        });
    }

    /// A returned reference must be rooted at a parameter. Rooted at a local
    /// it would dangle, which is what §4.1 exists to prevent.
    fn check_return_root(&mut self, e: &ast::Expr, ty: &Ty, span: Span) -> R<()> {
        if !contains_reference(ty) {
            return Ok(());
        }
        let Some(path) = self.borrow_root(e) else {
            return Ok(()); // not a place the checker can see through
        };
        // Under rule 3 the only lender is `self`, whatever else is in scope
        // (Ch. 4 §1.4).
        let ok = match self.params.first() {
            Some(first) if first == "self" && self.self_lends => path == "self",
            _ => self.params.contains(&path),
        };
        if !ok {
            return err(
                span,
                format!(
                    "cannot return a reference into `{path}`: it is local to this function \
                     and dies when the function returns (Ch. 3 §4.1)"
                ),
            );
        }
        Ok(())
    }

    /// The root local a borrow expression borrows from.
    fn borrow_root(&self, e: &ast::Expr) -> Option<String> {
        match e {
            ast::Expr::Borrow(inner, ..) => self.path_of(inner).map(|p| p.root),
            ast::Expr::Path(name, _) => Some(name.clone()),
            ast::Expr::Field(base, ..) | ast::Expr::Index(base, ..) => self.borrow_root(base),
            _ => None,
        }
    }

    /// Drop everything this function still owns — its locals and its
    /// parameters — on the way out.
    fn drop_all(&mut self, span: Span) -> R<()> {
        self.drop_scope(0, span)
    }

    /// The drops a `return` needs: everything the frame owns, along *this*
    /// path only.
    ///
    /// `drop_all` retires what it drops, which is right at the end of a
    /// function and wrong in the middle of one — a `return` in a `match` arm
    /// leaves by one path while the arms beside it still own the same values,
    /// and retiring them there means the value the other path owns is never
    /// dropped at all. `break` and `continue` were given `drop_through` for
    /// exactly this reason and `return` was not.
    fn drop_returning(&mut self, span: Span) -> R<()> {
        self.drop_through(0, span)
    }

    /// A value moved out of inside a loop would be moved again on the next
    /// iteration.
    fn check_no_move_in_loop(&mut self, before: &[Owned], span: Span) -> R<()> {
        for e in before {
            let (name, was) = (&e.name, e.owns);
            if was != Owns::Yes {
                continue;
            }
            if let Some(now) = self.ownership(name)
                && now != Owns::Yes
            {
                return err(
                    span,
                    format!("`{name}` is moved out of here, and the loop may reach this again"),
                );
            }
        }
        Ok(())
    }

    /// The drop glue of a type: its own destructor, then its fields'.
    fn emit_drop(&mut self, addr: &str, ty: &Ty, span: Span) -> R<()> {
        self.drop_at(Operand::Value(addr.to_string()), ty, span, 0)
    }

    fn drop_at(&mut self, addr: Operand, ty: &Ty, span: Span, depth: u32) -> R<()> {
        if depth > 8 {
            return err(span, "drop glue nested too deeply");
        }
        if !self.types.needs_drop(ty) {
            return Ok(());
        }
        // A `Vec` drops each element it holds and then frees the allocation
        // — the elements first, for the reason a `Box` gives.
        if let Ty::VecOf(elem) = ty {
            let elem = (**elem).clone();
            let l = self.types.layout(&elem);
            let len = {
                let at = self.offset(addr.clone(), 3);
                self.load_from(at, &Ty::TAddr)
            };
            if self.types.needs_drop(&elem) {
                let base = self.load_ptr(addr.clone());
                let i = self.temp_slot(&Ty::TAddr);
                self.store_at(&i, 0, &Ty::TAddr, konst_addr(0), span)?;
                let (head, body, out) = (
                    self.fresh("vdrop.head"),
                    self.fresh("vdrop.body"),
                    self.fresh("vdrop.done"),
                );
                self.jump(&head);
                self.start(head.clone());
                let at = self.load_slot(&i, &Ty::TAddr);
                let c = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: Type::Int(27),
                        a: at.clone(),
                        b: len.clone(),
                    },
                );
                self.br3(c, &body, &out, &out);
                self.start(body);
                let off = self.apply_binary(
                    "*",
                    at.clone(),
                    konst_addr(l.size as i128),
                    &Ty::TAddr,
                    span,
                )?;
                let e = self.emit(
                    "e",
                    Type::Ptr,
                    InstKind::Offset {
                        p: base.clone(),
                        d: off,
                    },
                );
                self.drop_at(e, &elem, span, depth + 1)?;
                let next = self.apply_binary("+", at, konst_addr(1), &Ty::TAddr, span)?;
                self.store_at(&i, 0, &Ty::TAddr, next, span)?;
                self.jump(&head);
                self.start(out);
            }
            let cap = {
                let at = self.offset(addr.clone(), 6);
                self.load_from(at, &Ty::TAddr)
            };
            let bytes =
                self.apply_binary("*", cap, konst_addr(l.size as i128), &Ty::TAddr, span)?;
            let p = self.load_ptr(addr);
            self.free_if_any(p, bytes, l.align as i128);
            return Ok(());
        }

        // A `Box` is dropped by dropping the `T` and *then* freeing, in that
        // order: the destructor needs the storage it is about to give back
        // (Ch. 5 §2.3).
        if let Ty::Boxed(inner) = ty {
            let p = self.load_ptr(addr);
            self.drop_at(p.clone(), inner, span, depth + 1)?;
            let l = self.types.layout(inner);
            self.needs_heap.set(true);
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Call {
                    callee: Callee::Direct(FREE.to_string()),
                    args: vec![
                        p,
                        Operand::Const(Type::Int(27), Bt::from_i128(l.size as i128)),
                        Operand::Const(Type::Int(27), Bt::from_i128(l.align as i128)),
                    ],
                    ret: None,
                },
            });
            return Ok(());
        }
        // A type with a destructor is dropped by calling it, and by nothing
        // else here: `drop.T` is the *complete* glue for T — its body runs,
        // then its fields are dropped, both inside that function (Ch. 3
        // §1.4). Emitting the field drops here as well would drop every
        // nested destructor twice, which is what draft 0.1 did.
        //
        // Completeness is also what Ch. 4 §3.3's vtable drop slot assumes: a
        // caller with only a pointer and that slot must be able to drop the
        // whole value.
        if let Ty::Struct(n) | Ty::Enum(n) = ty {
            self.ensure_glue(n, ty, span)?;
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Call {
                    callee: Callee::Direct(format!("drop.{n}")),
                    args: vec![addr.clone()],
                    ret: None,
                },
            });
            return Ok(());
        }
        self.drop_fields(addr, ty, span, depth)
    }

    /// Make sure `drop.T` exists, synthesizing it if the type has no
    /// destructor of its own.
    ///
    /// Drop glue used to be generated *inline*, field by field, at each place
    /// a value died — and inlining recursion does not terminate, so a
    /// recursive type stopped at the depth limit instead of compiling.
    ///
    /// The synthesis adds no mechanism. `drop.T` takes `self` by value and
    /// has an **empty body**, and its fields are dropped when its frame ends
    /// — which is what `fn drop(self) {}` already meant (Ch. 4 §5.2). The
    /// glue is a destructor that does nothing, and that was already the
    /// definition.
    ///
    /// It is done here, at the first drop, rather than once over the file,
    /// because an instantiation of a generic type does not exist until
    /// something asks for it: `List<t27>` is named while a body is being
    /// lowered, and a pass over the file would have missed it.
    fn ensure_glue(&mut self, name: &str, ty: &Ty, span: Span) -> R<()> {
        let key = format!("drop.{name}");
        if self.types.destructors.contains(name) || self.sigs.borrow().contains_key(&key) {
            return Ok(());
        }
        self.sigs
            .borrow_mut()
            .insert(key.clone(), (vec![ty.clone()], Ty::Unit));
        self.extra_fns.borrow_mut().push(ast::FnItem {
            public: true,
            requires: Vec::new(),
            name: key,
            name_span: span,
            generics: Vec::new(),
            params: vec![ast::Named::new(
                "self",
                ast::Ty::Name(name.to_string(), span),
            )],
            ret: None,
            body: Some(ast::Block {
                stmts: Vec::new(),
                tail: None,
                span,
            }),
            span,
        });
        Ok(())
    }

    /// The second half of dropping a value: its fields, without its own
    /// destructor. This is what a destructor's `self` gets.
    fn drop_fields(&mut self, addr: Operand, ty: &Ty, span: Span, depth: u32) -> R<()> {
        if depth > 8 {
            return err(span, "drop glue nested too deeply");
        }
        match ty {
            Ty::Struct(_) | Ty::Tuple(_) => {
                for (_, ft, off) in self.types.fields(ty) {
                    if self.types.needs_drop(&ft) {
                        let at = self.offset(addr.clone(), off);
                        self.drop_at(at, &ft, span, depth + 1)?;
                    }
                }
            }
            Ty::Array(elem, n) => {
                let size = self.types.size(elem);
                let (elem, n) = ((**elem).clone(), *n);
                if self.types.needs_drop(&elem) {
                    for i in 0..n {
                        let at = self.offset(addr.clone(), i * size);
                        self.drop_at(at, &elem, span, depth + 1)?;
                    }
                }
            }
            // An enum's payload varies by variant, so dropping it is a
            // dispatch on the discriminant (Ch. 3 §1.4 item 2: "payload order
            // for an enum variant").
            Ty::Enum(name) => self.drop_enum_payload(addr, &name.clone(), span, depth)?,
            _ => {}
        }
        Ok(())
    }

    /// Drop the payload of whichever variant an enum currently holds.
    ///
    /// The discriminant decides, so this is the same dispatch `match` emits,
    /// with each arm dropping that variant's fields. Variants whose payload
    /// needs no dropping are not tested at all, so the common case — one
    /// droppable variant, as in `Option<Buffer>` — is one comparison.
    fn drop_enum_payload(&mut self, addr: Operand, name: &str, span: Span, depth: u32) -> R<()> {
        let ty = Ty::Enum(name.to_string());
        let l = self.types.layout(&ty);
        let e = l.enum_layout.clone().expect("an enum");
        let variants = self.types.enums.borrow()[name].clone();

        let droppable: Vec<usize> = (0..variants.len())
            .filter(|i| {
                variants[*i]
                    .fields
                    .iter()
                    .any(|(_, t)| self.types.needs_drop(t))
            })
            .collect();
        if droppable.is_empty() {
            return Ok(());
        }

        let (tag, tag_ty) = self.read_tag(addr.clone(), &e);
        let join = self.fresh("drop.join");

        // A niche-encoded enum's untagged variant has no discriminant value
        // to test, so it is recognized by elimination and handled last —
        // which needs something to eliminate. Every variant that *has* a
        // discriminant is tested, droppable or not: one with nothing to drop
        // jumps straight to the join, and that jump is the elimination.
        //
        // Without it, an enum whose only droppable variant is the untagged
        // one dropped it unconditionally. `enum Tree { Leaf, Node(Box<Tree>,
        // …) }` is exactly that shape — `Leaf` lives in the `Box`'s niche and
        // has nothing to drop — so dropping a `Leaf` freed whatever its
        // storage happened to hold.
        let droppable: std::collections::BTreeSet<usize> = droppable.into_iter().collect();
        let mut untagged: Option<usize> = None;
        for i in 0..variants.len() {
            let Some(value) = tag_value(&e, i) else {
                if droppable.contains(&i) {
                    untagged = Some(i);
                }
                continue;
            };
            if !droppable.contains(&i) {
                let body = self.fresh("drop.none");
                let next = self.fresh("drop.next");
                let c = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: tag_ty,
                        a: tag.clone(),
                        b: Operand::Const(tag_ty, Bt::from_i128(value)),
                    },
                );
                self.br3(c, &next, &body, &next);
                self.start(body);
                self.jump(&join);
                self.start(next);
                continue;
            }
            let body = self.fresh("drop.arm");
            let next = self.fresh("drop.next");
            let c = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: tag_ty,
                    a: tag.clone(),
                    b: Operand::Const(tag_ty, Bt::from_i128(value)),
                },
            );
            self.br3(c, &next, &body, &next);
            self.start(body);
            self.drop_variant_fields(addr.clone(), name, i, span, depth)?;
            self.jump(&join);
            self.start(next);
        }
        if let Some(i) = untagged {
            self.drop_variant_fields(addr, name, i, span, depth)?;
        }
        self.jump(&join);
        self.start(join);
        Ok(())
    }

    /// Drop one variant's payload fields, in payload order (Ch. 3 §1.4).
    fn drop_variant_fields(
        &mut self,
        addr: Operand,
        name: &str,
        variant: usize,
        span: Span,
        depth: u32,
    ) -> R<()> {
        for (_, ft, off) in self.types.variant_fields(name, variant) {
            if self.types.needs_drop(&ft) {
                let at = self.offset(addr.clone(), off);
                self.drop_at(at, &ft, span, depth + 1)?;
            }
        }
        Ok(())
    }

    fn stmt(&mut self, s: &ast::Stmt) -> R<()> {
        match s {
            ast::Stmt::Let {
                mutable,
                name,
                name_span,
                ty,
                value,
                span,
            } => {
                let declared = match ty {
                    Some(t) => {
                        let d = self.resolve(t)?;
                        // A dynamically sized type is legal only behind a
                        // reference (Ch. 3 §5.1, Ch. 4 §3.1).
                        check_sized(&d, *span, &format!("the local `{name}`"))?;
                        Some(d)
                    }
                    None => None,
                };
                let loans_before = self.loans.len();
                // Asked before lowering, since lowering a place moves it.
                let was_place = self.type_of_place(value)?.is_some();
                let (v, vt) = self.expr(value, declared.as_ref())?;
                let ty = match declared {
                    Some(d) => {
                        // The initializer is what has the wrong type, so that
                        // is what the complaint points at rather than the
                        // `let` the whole statement starts with.
                        self.check(&vt, &d, value.span(), "initializer")?;
                        d
                    }
                    None if vt == Ty::Never || vt == Ty::Unit => {
                        return err(*span, format!("cannot bind a value of type {vt}"));
                    }
                    None => vt,
                };
                // The binding's own type, which is the thing `let n = 1;`
                // does not say and an editor most wants told.
                if let Some(sink) = self.noted {
                    sink.borrow_mut().note(*name_span, &ty);
                }
                // Every loan the initializer created is held by this
                // binding, and lives until its last use (Ch. 3 §4.2). That
                // covers a borrow written here and a reference returned from
                // a call, which by elision borrows from that call's argument.
                let is_closure = nominal_name(&ty)
                    .is_some_and(|n| self.types.closures.borrow().contains_key(&n));
                if contains_reference(&ty) || is_closure {
                    let dies = self.last_use.get(name).copied().unwrap_or(self.stmt);
                    for loan in self.loans[loans_before..].iter_mut() {
                        loan.dies = loan.dies.max(dies);
                    }
                }
                check_sized(&ty, *span, &format!("the binding `{name}`"))?;
                if !ty.is_scalar() && !ty.is_aggregate() {
                    return err(*span, format!("cannot bind a value of type {ty}"));
                }
                // The initializer's storage becomes the binding's, where it
                // was made for this value: a computed aggregate lives in a
                // temporary that nothing else names, and copying out of it
                // is a copy for its own sake.
                if ty.is_aggregate()
                    && !was_place
                    && let Operand::Value(slot) = &v
                {
                    // Renamed as it is adopted, so that reading the TIR still
                    // shows which binding a slot belongs to. Half of what was
                    // found this way was found by reading it.
                    let old = slot.clone();
                    let new = self.fresh(&format!("{name}.slot"));
                    self.rename_value(&old, &new);
                    self.declare_at(name, new, ty.clone(), *mutable);
                    return Ok(());
                }
                let local = self.declare(name, ty.clone(), *mutable);
                // An aggregate is copied into the binding's own storage, so
                // that writing to one does not write through to another.
                let slot = local.slot.clone();
                self.store_at(&slot, 0, &ty, v, *span)?;
                Ok(())
            }
            ast::Stmt::Expr(e) => {
                self.expr(e, None)?;
                Ok(())
            }
        }
    }

    // --------------------------------------------------------- expressions

    fn expr(&mut self, e: &ast::Expr, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr_inner(e, expected)?;
        // Held apart from `self`, because the coercions below need it
        // mutably and this only needs the sink.
        let sink = self.noted;
        let note = |ty: &Ty| {
            if let Some(n) = sink {
                n.borrow_mut().note(e.span(), ty);
            }
        };
        // Two implicit conversions, and both convert a *representation*
        // rather than a value: `&Concrete` to `&dyn Trait` (Ch. 4 §3.2) and
        // `&Vec<T>` to `&[T]` (Ch. 5 §2.6). Nothing else in this language is
        // implicit.
        if let Some(want) = expected
            && ty != *want
        {
            if let Some(fat) = self.coerce_dyn(v.clone(), &ty, want, e.span())? {
                note(&fat.1);
                return Ok(fat);
            }
            if let Some(fat) = self.coerce_vec(v.clone(), &ty, want, e.span())? {
                note(&fat.1);
                return Ok(fat);
            }
        }
        note(&ty);
        Ok((v, ty))
    }

    fn expr_inner(&mut self, e: &ast::Expr, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        use ast::Expr as E;
        // Taken here, so that every arm below starts with nothing and only
        // the ones that say so put it back.
        let dest = self.dest.take();
        match e {
            // A character literal is a `char` and nothing else: unlike an
            // integer literal it takes no type from context, because there
            // is only one type it could have (Ch. 5 §1.2).
            E::Char(v, _) => Ok((Operand::Const(Type::Int(27), Bt::from_i128(*v)), Ty::Char)),

            // `e?` — Ch. 5 §4.1. Two rules and no trait, and each is a
            // `match` and a `return` spelled shorter, so it is lowered as
            // exactly that: the arms below are the ones the chapter writes
            // out, built here because only here is the function's own result
            // type known.
            E::Try(inner, span) => self.try_expr(inner, expected, *span),

            // Calling something that is not a name. The only callable value
            // in this language is a closure, and a closure is a place: its
            // captures are the receiver its body takes (Ch. 4 §4.2).
            E::CallExpr(callee, args, span) => {
                let ty = match self.peek_ty(callee)? {
                    Some(t) => t,
                    None => self.expr(callee, None)?.1,
                };
                let Some(info) =
                    nominal_name(&ty).and_then(|n| self.types.closures.borrow().get(&n).cloned())
                else {
                    return err(
                        *span,
                        format!("{ty} is not callable; only a closure is (Ch. 4 §4.3)"),
                    );
                };
                let recv = ast::Expr::Borrow(callee.clone(), false, *span);
                let mut full = vec![recv];
                full.extend(args.iter().cloned());
                self.call_key(&info.call, Vec::new(), &full, *span)
            }

            // A string literal is a fat pointer to storage that outlives
            // every frame — an address and a length in characters, which is
            // what `&[char]` is anywhere else (Ch. 3 §5.2, Ch. 5 §1.4).
            E::Str(chars, span) => {
                let symbol = self.string_data(chars);
                let ty = Ty::Ref(Box::new(Ty::Slice(Box::new(Ty::Char))), false);
                let slot = self.temp_slot(&ty);
                let at = Operand::Value(slot.clone());
                self.store_ptr(at, Operand::Global(symbol));
                let len = Operand::Const(Type::Int(27), Bt::from_i128(chars.len() as i128));
                self.store_at(&slot, 3, &Ty::TAddr, len, *span)?;
                Ok((Operand::Value(slot), ty))
            }

            // An unconstrained integer literal is `t27` (Ch. 1 §3), and one
            // that does not fit its type is an error, never a wrap.
            E::Int(v, span) => {
                let ty = match expected {
                    Some(t) if t.is_arithmetic() => t.clone(),
                    _ => Ty::T27,
                };
                let width = ty.width().unwrap_or(27);
                if !v.fits_width(width) {
                    return err(*span, format!("{v} does not fit in {ty}"));
                }
                Ok((Operand::Const(ty.tir(), v.clone()), ty))
            }

            E::Trit(t, _) => Ok((Operand::Const(Type::Int(1), Bt::from(*t)), Ty::Trit)),

            // false = 0, true = 1 in a full tryte (Ch. 1 §2).
            E::Bool(b, _) => Ok((
                Operand::Const(Type::Int(1), Bt::from_i128(i128::from(*b))),
                Ty::Bool,
            )),

            E::Unit(_) => Ok((unit(), Ty::Unit)),

            E::Path(name, span) => {
                if let Some(local) = self.lookup(name) {
                    // Reading a value that is not copyable moves it, and a
                    // moved-out place may not be read (Ch. 3 §1.2).
                    if !self.types.is_copyable(&local.ty) {
                        // A binding taken through a reference names storage
                        // the referent still owns, so there is nothing here
                        // to move. It can be read through and borrowed from;
                        // taking it would make a second owner of one value.
                        if local.borrowed {
                            return err(
                                *span,
                                format!(
                                    "`{name}` names part of a value this `match` borrowed, \
                                     so it cannot be moved out of; borrow it instead \
                                     (Ch. 3 §1.2)"
                                ),
                            );
                        }
                        match self.ownership(name) {
                            Some(Owns::No) => {
                                return err(
                                    *span,
                                    format!("`{name}` was moved out of and cannot be used again"),
                                );
                            }
                            Some(Owns::Maybe) => {
                                return err(
                                    *span,
                                    format!(
                                        "`{name}` may have been moved out of on some path here"
                                    ),
                                );
                            }
                            _ => {}
                        }
                        let path = PlacePath {
                            root: name.clone(),
                            projections: Vec::new(),
                        };
                        self.check_access(&path, Access::Move, *span)?;
                        self.mark_moved(name);
                    }
                    let path = PlacePath {
                        root: name.clone(),
                        projections: Vec::new(),
                    };
                    self.check_access(&path, Access::Read, *span)?;
                    if !local.ty.is_scalar() {
                        // An array's value is its address.
                        return Ok((Operand::Value(local.slot), local.ty));
                    }
                    let ty = local.ty.clone();
                    let v = self.emit(
                        "v",
                        ty.tir(),
                        InstKind::Load {
                            ty: ty.tir(),
                            p: Operand::Value(local.slot),
                        },
                    );
                    return Ok((v, ty));
                }
                match self.globals.get(name) {
                    Some(Global::Const(v, ty)) => {
                        Ok((Operand::Const(ty.tir(), v.clone()), ty.clone()))
                    }
                    Some(Global::Array(sym, ty)) => Ok((Operand::Global(sym.clone()), ty.clone())),
                    None => err(*span, format!("`{name}` is not in scope")),
                }
            }

            E::Unary(op, inner, span) => self.unary(op, inner, expected, *span),
            E::Binary(op, a, b, span) => self.binary(op, a, b, expected, *span),
            E::Assign(op, target, value, span) => self.assign(op, target, value, *span),
            E::Cast(inner, ty, span) => self.cast(inner, ty, *span),
            // A call *writes* its aggregate result, so it consumes a
            // destination rather than passing one on.
            E::Call(name, _, targs, args, span) => {
                self.dest = dest;
                self.call(name, targs, args, expected, *span)
            }
            E::Method(recv, name, _, args, span) => {
                self.dest = dest;
                self.method(recv, name, args, expected, *span)
            }

            // Transparent to a destination: their value is one of their
            // arms' values, and an arm may build where the whole is going.
            E::Block(b) => {
                self.dest = dest;
                self.block(b, expected)
            }
            E::If(cond, then, els, span) => {
                self.dest = dest;
                self.if_expr(cond, then, els.as_deref(), expected, *span)
            }
            E::Match(scrutinee, arms, span) => {
                self.dest = dest;
                self.match_expr(scrutinee, arms, expected, *span)
            }
            E::While(cond, body, span) => self.while_expr(cond, body, *span),
            E::Loop(body, span) => self.loop_expr(body, expected, *span),

            E::Break(value, span) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*span, "`break` outside a loop");
                };
                self.loops.last_mut().expect("just read").broke = true;
                match (value, &ctx.result) {
                    (Some(v), Some((slot, ty))) => {
                        let (val, vt) = self.expr(v, Some(ty))?;
                        self.check(&vt, ty, *span, "`break` value")?;
                        let (slot, ty) = (slot.clone(), ty.clone());
                        self.push(Inst {
                            results: Vec::new(),
                            kind: InstKind::Store {
                                ty: ty.tir(),
                                v: val,
                                p: Operand::Value(slot),
                            },
                        });
                    }
                    (Some(_), None) => {
                        return err(*span, "this loop's `break` cannot carry a value");
                    }
                    (None, _) => {}
                }
                // The scopes between here and the loop are being left, and
                // what they own dies with them (Ch. 3 §1.1). Emitted rather
                // than retired: the loop's other paths still own the same
                // values.
                self.drop_through(ctx.depth, *span)?;
                self.jump(&ctx.exit);
                Ok((unit(), Ty::Never))
            }

            E::Continue(span) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*span, "`continue` outside a loop");
                };
                self.drop_through(ctx.depth, *span)?;
                self.jump(&ctx.head);
                Ok((unit(), Ty::Never))
            }

            E::Return(value, span) => {
                let ret = self.ret.clone();
                match value {
                    Some(v) => {
                        self.check_return_root(v, &self.ret.clone(), *span)?;
                        if ret.is_aggregate() {
                            self.dest = Some(SRET.to_string());
                        }
                        let (val, vt) = self.expr(v, Some(&ret))?;
                        self.dest = None;
                        self.check(&vt, &ret, *span, "returned value")?;
                        if ret.is_aggregate() {
                            // Already there, if the value took the
                            // destination.
                            if val != Operand::Value(SRET.to_string()) {
                                let dst = Operand::Value(SRET.to_string());
                                self.copy_typed(dst, val.clone(), &ret, *span)?;
                            }
                            self.drop_returning(*span)?;
                            self.finish(Terminator::Ret(None));
                        } else {
                            self.drop_returning(*span)?;
                            self.finish(Terminator::Ret(Some(val)));
                        }
                    }
                    None => {
                        self.check(&Ty::Unit, &ret, *span, "`return` with no value")?;
                        self.drop_returning(*span)?;
                        self.finish(Terminator::Ret(None));
                    }
                }
                Ok((unit(), Ty::Never))
            }

            // An array literal builds its storage and fills it, like any
            // other aggregate.
            E::Array(items, span) => {
                let hint = match expected {
                    Some(Ty::Array(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let mut values = Vec::new();
                let mut elem = hint.clone();
                for item in items {
                    let (v, t) = self.expr(item, elem.as_ref())?;
                    if let Some(want) = &elem {
                        self.check(&t, want, item.span(), "array element")?;
                    } else {
                        elem = Some(t);
                    }
                    values.push(v);
                }
                let Some(elem) = elem else {
                    return err(*span, "an empty array literal needs a written type");
                };
                let ty = Ty::Array(Box::new(elem.clone()), values.len() as i128);
                let slot = self.temp_slot(&ty);
                let size = self.types.size(&elem);
                for (i, v) in values.into_iter().enumerate() {
                    self.store_at(&slot, i as i128 * size, &elem, v, *span)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Repeat(value, count, span) => {
                let hint = match expected {
                    Some(Ty::Array(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let n = const_int_in(count, &self.types.consts)?;
                if n < 0 {
                    return err(*span, format!("array length {n} is negative"));
                }
                let (v, elem) = self.expr(value, hint.as_ref())?;
                let ty = Ty::Array(Box::new(elem.clone()), n);
                let slot = self.temp_slot(&ty);
                let size = self.types.size(&elem);
                for i in 0..n {
                    self.store_at(&slot, i * size, &elem, v.clone(), *span)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Tuple(items, span) => {
                let mut tys = Vec::new();
                let mut values = Vec::new();
                let hint = match expected {
                    Some(Ty::Tuple(ts)) if ts.len() == items.len() => Some(ts.clone()),
                    _ => None,
                };
                for (i, item) in items.iter().enumerate() {
                    let want = hint.as_ref().map(|ts| ts[i].clone());
                    let (v, t) = self.expr(item, want.as_ref())?;
                    values.push(v);
                    tys.push(t);
                }
                let ty = Ty::Tuple(tys);
                let slot = self.temp_slot(&ty);
                let fields = self.types.fields(&ty);
                for ((_, ft, off), v) in fields.into_iter().zip(values) {
                    self.store_at(&slot, off, &ft, v, *span)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Aggregate(path, fields, span) => {
                self.dest = dest;
                self.aggregate(path, fields, expected, *span)
            }

            E::Closure(params, ret, body, span) => self.closure(params, ret, body, None, *span),

            // A borrow is the address of a place — which every local already
            // has, since every local lives in a slot.
            E::Borrow(place, mutable, span) => {
                if let Some(path) = self.path_of(place) {
                    self.check_access(&path, Access::Borrow(*mutable), *span)?;
                    self.add_loan(path, *mutable, *span);
                }
                let (addr, ty) = self.place(place, *span)?;
                // An array reference coerces to a slice reference, which is a
                // pointer and a length (Ch. 3 §5.3).
                if let Ty::Array(elem, n) = &ty {
                    let slice = Ty::Ref(Box::new(Ty::Slice(elem.clone())), *mutable);
                    let slot = self.temp_slot(&slice);
                    let len = Operand::Const(Type::Int(27), Bt::from_i128(*n));
                    // A fat pointer is a pointer and a length (Ch. 3 §5.2).
                    let at = Operand::Value(slot.clone());
                    self.store_ptr(at, addr);
                    self.store_at(&slot, 3, &Ty::TAddr, len, *span)?;
                    return Ok((Operand::Value(slot), slice));
                }
                Ok((addr, Ty::Ref(Box::new(ty), *mutable)))
            }

            E::Deref(inner, span) => {
                let (v, ty) = self.expr(inner, None)?;
                // `*b` on a `Box` reads what it owns, which is the same
                // operation on the same representation as `*r` on a
                // reference (Ch. 5 §2.3).
                let borrowed = matches!(ty, Ty::Ref(..));
                let (Ty::Ref(target, _) | Ty::Boxed(target)) = ty else {
                    return err(*span, format!("`*` applies to a reference, not {ty}"));
                };
                if target.is_unsized() {
                    return err(*span, format!("cannot read a value of type {target}"));
                }
                // Reading a place of non-copyable type moves it (Ch. 3
                // §1.2) — and there is nothing here to move *from*: the
                // value belongs to whatever the reference points at, which
                // will drop it whether or not this did.
                //
                // `E::Field` and `E::Index` beside this one have always
                // checked; this arm did not, so `take(*r)` compiled and the
                // value was dropped twice. The drop ledger found it.
                //
                // A `Box` is not a reference: reading through one moves the
                // box itself, which is the owner, so there is exactly one
                // drop and it happens here.
                if borrowed && !self.types.is_copyable(&target) {
                    return err(
                        *span,
                        format!(
                            "cannot move out of a reference: reading a {target} moves it, \
                             and this one belongs to whatever the reference points at, which \
                             will drop it either way (Ch. 3 §1.2)"
                        ),
                    );
                }
                Ok((self.load_from(v, &target), *target))
            }

            E::Field(..) | E::Index(..) => {
                let span = e.span();
                let path = self.path_of(e);
                let (addr, ty) = self.place(e, span)?;
                // Reading a place of non-copyable type *moves* it (Ch. 3
                // §1.2), and that is as true of a field as of a whole local.
                // Draft 0.1 tracks ownership per local rather than per place,
                // so a move out of any part moves the whole: conservative,
                // where doing nothing — which is what it did — was unsound.
                match (&path, self.types.is_copyable(&ty)) {
                    (Some(p), true) => self.check_access(p, Access::Read, span)?,
                    (Some(p), false) => {
                        self.check_access(p, Access::Move, span)?;
                        let root = p.root.clone();
                        if self.lookup(&root).is_some() {
                            self.mark_moved(&root);
                        }
                    }
                    (None, _) => {}
                }
                Ok((self.load_from(addr, &ty), ty))
            }
        }
    }

    fn unary(
        &mut self,
        op: &str,
        inner: &ast::Expr,
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(inner, expected)?;
        match op {
            // Negation is total at every width (Ch. 1 §1, P1).
            "-" if ty.is_arithmetic() || ty == Ty::Trit => {
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }
            "-" => err(span, format!("`-` does not apply to {ty}")),
            "!" if ty == Ty::Bool => {
                // `false` is 0 and `true` is 1, so negation is `1 − b`.
                let one = Operand::Const(Type::Int(1), Bt::from_i128(1));
                let r = self.emit(
                    "n",
                    Type::Int(1),
                    InstKind::Flavored {
                        op: FlavoredOp::Sub,
                        flavor: Flavor::Wrap,
                        ty: Type::Int(1),
                        a: one,
                        b: v,
                    },
                );
                Ok((r, Ty::Bool))
            }
            "!" => err(span, format!("`!` applies to bool, not {ty}")),
            _ => err(span, format!("unknown unary operator `{op}`")),
        }
    }

    fn binary(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // Short-circuit operators are control flow, not arithmetic.
        if op == "&&" || op == "||" {
            return self.short_circuit(op, a, b, span);
        }

        let arith_hint = expected.filter(|t| t.is_arithmetic());
        let (va, ta) = self.expr(a, arith_hint)?;
        let (vb, tb) = self.expr(b, Some(&ta))?;
        // Mixed-width arithmetic is a compile-time error (Ch. 1, P2).
        self.check(&tb, &ta, span, "right operand")?;

        // A comparison of two nominal values goes through `Ord` and `Eq`
        // (Ch. 4 §5.3), which is the only place an operator on a user type
        // means a call. Both operands are aggregates, so both are already
        // the addresses the comparison wants.
        if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "<=>") && ta.is_aggregate() {
            let Some(name) = nominal_name(&ta) else {
                return err(span, format!("`{op}` does not apply to {ta}"));
            };
            return self.compare_nominal(op, &name, va, vb, span);
        }

        let tir = ta.tir();
        match op {
            "+" | "-" | "*" | "<<" => {
                if !ta.is_arithmetic() {
                    return err(span, format!("`{op}` does not apply to {ta}"));
                }
                let fop = match op {
                    "+" => FlavoredOp::Add,
                    "-" => FlavoredOp::Sub,
                    "*" => FlavoredOp::Mul,
                    _ => FlavoredOp::Shl,
                };
                // Ch. 1, P4: the default profile traps on overflow. A release
                // profile would select `.wrap` here, and the `wrapping_*`
                // methods are always available.
                let r = self.emit(
                    "a",
                    tir,
                    InstKind::Flavored {
                        op: fop,
                        flavor: Flavor::Trap,
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, ta))
            }

            "/" | "%" | ">>" => {
                if !ta.is_arithmetic() {
                    return err(span, format!("`{op}` does not apply to {ta}"));
                }
                let pop = match op {
                    "/" => PlainOp::Div,
                    "%" => PlainOp::Rem,
                    _ => PlainOp::Shr,
                };
                let r = self.emit(
                    "a",
                    tir,
                    InstKind::Plain {
                        op: pop,
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, ta))
            }

            // The primitive comparison (Ch. 1 §5).
            "<=>" => {
                let r = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                Ok((r, Ty::Trit))
            }

            // The two-way predicates are projections of it: one comparison,
            // then a three-way select of 0 or 1.
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                let c = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: tir,
                        a: va,
                        b: vb,
                    },
                );
                let (n, z, p) = match op {
                    "==" => (0, 1, 0),
                    "!=" => (1, 0, 1),
                    "<" => (1, 0, 0),
                    "<=" => (1, 1, 0),
                    ">" => (0, 0, 1),
                    _ => (0, 1, 1),
                };
                let k = |v: i128| Operand::Const(Type::Int(1), Bt::from_i128(v));
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: c,
                        ty: Type::Int(1),
                        neg: k(n),
                        zero: k(z),
                        pos: k(p),
                    },
                );
                Ok((r, Ty::Bool))
            }

            _ => err(span, format!("unknown operator `{op}`")),
        }
    }

    /// `&&` and `||` short-circuit, so they are branches (Ch. 0 §2.4).
    fn short_circuit(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let (va, ta) = self.expr(a, Some(&Ty::Bool))?;
        self.check(&ta, &Ty::Bool, span, "left operand")?;

        let slot = self.temp_slot(&Ty::Bool);
        let rhs = self.fresh("sc.rhs");
        let join = self.fresh("sc.join");
        let short = self.fresh("sc.short");

        // Store the short-circuit answer, then test.
        self.store_slot(&slot, &Ty::Bool, va.clone(), span);
        if op == "&&" {
            self.br3(va, &short, &short, &rhs);
        } else {
            self.br3(va, &rhs, &rhs, &short);
        }

        self.start(short);
        self.jump(&join);

        self.start(rhs);
        let (vb, tb) = self.expr(b, Some(&Ty::Bool))?;
        self.check(&tb, &Ty::Bool, span, "right operand")?;
        self.store_slot(&slot, &Ty::Bool, vb, span);
        self.jump(&join);

        self.start(join);
        let v = self.load_slot(&slot, &Ty::Bool);
        Ok((v, Ty::Bool))
    }

    fn assign(
        &mut self,
        op: &str,
        target: &ast::Expr,
        value: &ast::Expr,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // Writing to a local needs `mut`; writing through a reference needs
        // that reference to be exclusive (Ch. 3 §2.1).
        self.check_writable(target, span)?;
        if let Some(path) = self.path_of(target) {
            self.check_access(&path, Access::Write, span)?;
        }
        let (addr, ty) = self.place(target, span)?;

        let v = if op == "=" {
            let (v, vt) = self.expr(value, Some(&ty))?;
            self.check(&vt, &ty, span, "assigned value")?;

            // The value being overwritten is going away, and Ch. 3 §1.1 gives
            // it exactly one owner and one drop. Draft 0.1 stored over it,
            // which leaked whatever it owned. Emitted after the right-hand
            // side, so that `a = f(a)` still reads `a` before it dies.
            if self.types.needs_drop(&ty) {
                let live = self
                    .path_of(target)
                    .map(|p| self.ownership(&p.root) != Some(Owns::No))
                    .unwrap_or(true);
                if live {
                    self.drop_at(addr.clone(), &ty, span, 0)?;
                }
            }
            v
        } else {
            // `a op= b` is `a = a op b`, with `a` evaluated once (Ch. 0 §2.2).
            let binop = &op[..op.len() - 1];
            let current = self.load_from(addr.clone(), &ty);
            let (rhs, rt) = self.expr(value, Some(&ty))?;
            self.check(&rt, &ty, span, "assigned value")?;
            self.apply_binary(binop, current, rhs, &ty, span)?
        };

        if ty.is_aggregate() {
            self.copy_typed(addr, v, &ty, span)?;
        } else {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: ty.tir(),
                    v,
                    p: addr,
                },
            });
        }
        // A whole local that was moved out of owns again once something has
        // been put back in it.
        if op == "="
            && let Some(path) = self.path_of(target)
            && path.projections.is_empty()
        {
            self.mark_initialized(&path.root);
        }
        Ok((unit(), Ty::Unit))
    }

    /// The root of a place decides whether it may be written to.
    fn check_writable(&mut self, target: &ast::Expr, span: Span) -> R<()> {
        match target {
            ast::Expr::Path(name, l) => match self.lookup(name) {
                None => err(*l, format!("`{name}` is not in scope")),
                Some(local) if !local.mutable => err(
                    *l,
                    format!("`{name}` is not mutable; declare it `let mut {name}`"),
                ),
                Some(_) => Ok(()),
            },
            ast::Expr::Field(base, ..) | ast::Expr::Index(base, ..) => {
                // Indexing or projecting through a reference is writable iff
                // that reference is exclusive; otherwise the root decides.
                let base_ty = self.type_of_place(base)?;
                match base_ty {
                    Some(Ty::Ref(_, false)) => err(
                        span,
                        "cannot write through a shared reference; it would need `&mut`",
                    ),
                    Some(Ty::Ref(_, true)) => Ok(()),
                    _ => self.check_writable(base, span),
                }
            }
            ast::Expr::Deref(inner, l) => {
                let ty = self.type_of_place(inner)?;
                match ty {
                    // A `Box` is owned, so writing through it needs only that
                    // the box itself be writable (Ch. 5 §2.3).
                    Some(Ty::Boxed(_)) => Ok(()),
                    Some(Ty::Ref(_, true)) => Ok(()),
                    Some(Ty::Ref(_, false)) => err(
                        *l,
                        "cannot write through a shared reference; it would need `&mut`",
                    ),
                    _ => err(*l, "`*` applies to a reference"),
                }
            }
            _ => err(span, "this expression is not a place"),
        }
    }

    /// The type of a place, without emitting anything. `None` means the
    /// expression is not a place, not that it has no type.
    fn type_of_place(&mut self, e: &ast::Expr) -> R<Option<Ty>> {
        let through_refs = |mut t: Ty| {
            while let Ty::Ref(inner, _) | Ty::Boxed(inner) = t.clone() {
                if inner.is_unsized() {
                    break;
                }
                t = *inner;
            }
            t
        };
        Ok(match e {
            ast::Expr::Path(name, _) => match self.lookup(name) {
                Some(l) => Some(l.ty),
                None => match self.globals.get(name) {
                    Some(Global::Array(_, ty)) => Some(ty.clone()),
                    _ => None,
                },
            },
            ast::Expr::Field(base, field, _) => {
                let Some(bt) = self.type_of_place(base)? else {
                    return Ok(None);
                };
                self.types
                    .fields(&through_refs(bt))
                    .into_iter()
                    .find(|(n, _, _)| n == field)
                    .map(|(_, t, _)| t)
            }
            ast::Expr::Index(base, _, _) => {
                let Some(bt) = self.type_of_place(base)? else {
                    return Ok(None);
                };
                match through_refs(bt) {
                    Ty::Array(elem, _) => Some(*elem),
                    Ty::Ref(inner, _) => match *inner {
                        Ty::Slice(elem) => Some(*elem),
                        _ => None,
                    },
                    _ => None,
                }
            }
            ast::Expr::Deref(inner, _) => match self.type_of_place(inner)? {
                Some(Ty::Ref(target, _) | Ty::Boxed(target)) => Some(*target),
                _ => None,
            },
            _ => None,
        })
    }

    /// The arithmetic half of `binary`, for compound assignment.
    fn apply_binary(
        &mut self,
        op: &str,
        a: Operand,
        b: Operand,
        ty: &Ty,
        span: Span,
    ) -> R<Operand> {
        if !ty.is_arithmetic() {
            return err(span, format!("`{op}=` does not apply to {ty}"));
        }
        let tir = ty.tir();
        Ok(match op {
            "+" | "-" | "*" | "<<" => {
                let fop = match op {
                    "+" => FlavoredOp::Add,
                    "-" => FlavoredOp::Sub,
                    "*" => FlavoredOp::Mul,
                    _ => FlavoredOp::Shl,
                };
                self.emit(
                    "a",
                    tir,
                    InstKind::Flavored {
                        op: fop,
                        flavor: Flavor::Trap,
                        ty: tir,
                        a,
                        b,
                    },
                )
            }
            "/" | "%" | ">>" => {
                let pop = match op {
                    "/" => PlainOp::Div,
                    "%" => PlainOp::Rem,
                    _ => PlainOp::Shr,
                };
                self.emit(
                    "a",
                    tir,
                    InstKind::Plain {
                        op: pop,
                        ty: tir,
                        a,
                        b,
                    },
                )
            }
            _ => return err(span, format!("unknown operator `{op}=`")),
        })
    }

    /// `x as T` (Ch. 1 §6).
    fn cast(&mut self, inner: &ast::Expr, to: &ast::Ty, span: Span) -> R<(Operand, Ty)> {
        let to = self.resolve(to)?;
        let (v, from) = self.expr(inner, None)?;

        // A fieldless enum may be cast to an integer, yielding its
        // discriminant. There is no cast in the reverse direction — that is
        // fallible, and library `try_from` territory (Ch. 2 §5.3).
        if let Ty::Enum(name) = &from {
            if !to.is_arithmetic() && to != Ty::Trit {
                return err(span, format!("an enum casts only to an integer, not {to}"));
            }
            if self.types.enums.borrow()[name]
                .iter()
                .any(|v| !v.fields.is_empty())
            {
                return err(
                    span,
                    format!("`{name}` has variants with payloads, so it has no integer value"),
                );
            }
            let l = self.types.layout(&from);
            let e = l.enum_layout.clone().expect("an enum");
            let (tag, tag_ty) = self.read_tag(v, &e);
            let want = to.tir();
            let value = match (tag_ty.width(), want.width()) {
                (Some(a), Some(b)) if a < b => self.emit(
                    "w",
                    want,
                    InstKind::Widen {
                        from: tag_ty,
                        a: tag,
                        to: want,
                    },
                ),
                (Some(a), Some(b)) if a > b => self.emit(
                    "t",
                    want,
                    InstKind::Trunc {
                        from: tag_ty,
                        a: tag,
                        to: want,
                    },
                ),
                _ => tag,
            };
            return Ok((value, to));
        }

        // `char` converts one way and to one type. Downward there is nothing
        // to check — a scalar value always fits a word — and upward there is:
        // most words are not characters, and Ch. 1 P2 does not let a
        // conversion that can be wrong be silent (Ch. 5 §1.2).
        if from == Ty::Char {
            if to != Ty::T27 {
                return err(
                    span,
                    format!(
                        "a `char` converts only to `t27`, not to {to}: the scalar value is \
                         a word, and narrowing it would be a conversion that can be wrong \
                         (Ch. 5 §1.2)"
                    ),
                );
            }
            return Ok((v, Ty::T27));
        }
        if to == Ty::Char {
            return err(
                span,
                format!(
                    "there is no `as` from {from} to `char`: most words are not scalar \
                     values, so the conversion can fail. `char::try_from` is the checked \
                     form (Ch. 5 §1.2)"
                ),
            );
        }

        // `trit` ↔ `bool` has no `as` path by design: both mappings are
        // plausible, so the language refuses to pick (Ch. 1 §6).
        if (from == Ty::Trit && to == Ty::Bool) || (from == Ty::Bool && to == Ty::Trit) {
            return err(
                span,
                "there is no `as` between `trit` and `bool`: use `is_pos`/`is_zero`/`is_neg` \
                 or `to_trit` (Ch. 1 §6)",
            );
        }
        if let Ty::Enum(name) = &to {
            return err(
                span,
                format!(
                    "there is no cast from {from} to `{name}`: an integer need not name a \
                     variant, so the conversion is fallible and belongs to a library \
                     `try_from` (Ch. 2 §5.3)"
                ),
            );
        }
        let (Some(fw), Some(tw)) = (from.width(), to.width()) else {
            return err(span, format!("cannot cast {from} to {to}"));
        };
        let value = match fw.cmp(&tw) {
            std::cmp::Ordering::Less => self.emit(
                "w",
                to.tir(),
                InstKind::Widen {
                    from: from.tir(),
                    a: v,
                    to: to.tir(),
                },
            ),
            std::cmp::Ordering::Greater => self.emit(
                "t",
                to.tir(),
                InstKind::Trunc {
                    from: from.tir(),
                    a: v,
                    to: to.tir(),
                },
            ),
            std::cmp::Ordering::Equal => v,
        };
        Ok((value, to))
    }

    fn call(
        &mut self,
        name: &str,
        targs: &[ast::Ty],
        args: &[ast::Expr],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // `trap()` — stop the program (Ch. 1 §6). A fault has no handler
        // (AM §4), so nothing after it runs and its type is `!`: it is what
        // lets a library function say "this cannot go on" in the language,
        // which nothing else could.
        if name == "trap" {
            if !args.is_empty() {
                return err(span, "`trap` takes no arguments");
            }
            self.finish(Terminator::Trap(FaultCode::Trap));
            return Ok((unit(), Ty::Never));
        }

        // `sign(x)` is a function, not a method (Ch. 1 §6).
        if name == "sign" {
            if args.len() != 1 {
                return err(span, "`sign` takes one argument");
            }
            let (v, ty) = self.expr(&args[0], None)?;
            if !ty.is_arithmetic() && ty != Ty::Trit {
                return err(span, format!("`sign` does not apply to {ty}"));
            }
            let zero = Operand::Const(ty.tir(), Bt::ZERO);
            let r = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: ty.tir(),
                    a: v,
                    b: zero,
                },
            );
            return Ok((r, Ty::Trit));
        }

        // `Name(args)` is a tuple-struct literal when the name is a type.
        // The parser cannot tell the two apart; here the names are known.
        if self.types.structs.borrow().contains_key(name) {
            let fields: Vec<(String, ast::Expr)> = args
                .iter()
                .enumerate()
                .map(|(i, e)| (i.to_string(), e.clone()))
                .collect();
            let path = ast::Path {
                segments: vec![name.to_string()],
                targs: Vec::new(),
                span,
            };
            return self.aggregate(&path, &fields, None, span);
        }

        // `f(args)` where `f` is a local holding a closure: the call goes to
        // the function its body became, with the captures as the receiver
        // (Ch. 4 §4.2).
        if let Some(local) = self.lookup(name)
            && let Some(cname) = nominal_name(&local.ty)
            && let Some(info) = self.types.closures.borrow().get(&cname).cloned()
        {
            let recv = ast::Expr::Borrow(
                Box::new(ast::Expr::Path(name.to_string(), span)),
                false,
                span,
            );
            let mut full = vec![recv];
            full.extend(args.iter().cloned());
            return self.call_key(&info.call, Vec::new(), &full, span);
        }

        // A generic callee is instantiated here, at the call site, which is
        // also where its bounds are checked (Ch. 4 §2.2).
        if self.generic_fns.contains_key(name) || self.specials.borrow().contains_key(name) {
            let (key, args) = self.instantiate_fn(name, targs, args, expected, span)?;
            return self.call_key(&key, Vec::new(), &args, span);
        }

        if !targs.is_empty() {
            return err(span, format!("`{name}` takes no type arguments"));
        }
        self.call_key(name, Vec::new(), args, span)
    }

    /// Instantiate a generic function for this call, and return the name the
    /// instantiation is known by (Ch. 4 §2.7).
    fn instantiate_fn(
        &mut self,
        name: &str,
        targs: &[ast::Ty],
        args: &[ast::Expr],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(String, Vec<ast::Expr>)> {
        let def = self.generic_def(name).expect("a generic function");
        if def.params.len() != args.len() {
            return err(
                span,
                format!(
                    "`{name}` takes {} argument(s), {} given",
                    def.params.len(),
                    args.len()
                ),
            );
        }

        // Infer each parameter from the arguments it appears in, and from
        // the type the result is expected to have — which is the only way to
        // tell `id(2)` bound to a `t9` from the same call bound to a `t27`.
        // `f::<T>(…)` gives the arguments outright, in declaration order,
        // and any it omits are inferred (Ch. 4 §2.3).
        // A specialized method arrives with the impl's half already settled.
        let mut env: HashMap<String, Ty> = self.special_env(name);
        if targs.len() > def.generics.len() {
            return err(
                span,
                format!(
                    "`{name}` takes {} type argument(s), {} given",
                    def.generics.len(),
                    targs.len()
                ),
            );
        }
        for (p, t) in def.generics.iter().zip(targs) {
            env.insert(p.name().to_string(), self.resolve(t)?);
        }
        if let (Some(want), Some(ret)) = (expected, &def.ret) {
            unify(ret, want, &def.generics, &mut env);
        }
        let mut args = args.to_vec();
        for (want, arg) in def.params.iter().map(|p| &p.ty).zip(&mut args) {
            if let Some(got) = self.peek_ty(arg)? {
                unify(want, &got, &def.generics, &mut env);
                continue;
            }
            // A closure has no type until it is lowered, so lower it now and
            // give the value a name the rest of the call can use.
            match arg.clone() {
                ast::Expr::Closure(cps, cret, body, cline) => {
                    // The bound says what signature is wanted, so a closure
                    // need not write its own types (Ch. 4 §4.1).
                    let hint = self.fn_hint(&def, want, &env)?;
                    let (v, got) = self.closure(&cps, &cret, &body, hint, cline)?;
                    let Operand::Value(slot) = v else {
                        return err(span, "a closure must have a slot");
                    };
                    let bound = self.bind_existing(slot, got.clone());
                    *arg = ast::Expr::Path(bound, span);
                    unify(want, &got, &def.generics, &mut env);
                }
                // And a literal *holding* a closure cannot be peeked either,
                // for the same reason and one level down: `f(Map { inner: c,
                // f: |x| x })` has to lower the whole literal to know what
                // `Map`'s parameters are, let alone `f`'s.
                ast::Expr::Aggregate(..) if holds_a_closure(arg) => {
                    let (v, got) = self.expr(arg, None)?;
                    let Operand::Value(slot) = v else {
                        return err(span, "a literal must have a slot");
                    };
                    let bound = self.bind_existing(slot, got.clone());
                    *arg = ast::Expr::Path(bound, span);
                    unify(want, &got, &def.generics, &mut env);
                }
                // And a method call, for the same reason two levels up:
                // `sum(it.map(f).filter(p))` cannot be told the type of its
                // argument without resolving the chain, and resolving it is
                // lowering it. The arguments are lowered left to right here
                // as they would be anyway, so nothing moves.
                ast::Expr::Method(..) => {
                    let (v, got) = self.expr(arg, None)?;
                    let Operand::Value(slot) = v else {
                        return err(span, "a method's result must have a slot");
                    };
                    let bound = self.bind_existing(slot, got.clone());
                    *arg = ast::Expr::Path(bound, span);
                    unify(want, &got, &def.generics, &mut env);
                }
                _ => {}
            }
        }

        let mut targs = Vec::new();
        for p in &def.generics {
            let ast::GenericParam::Type {
                name: pname,
                bounds,
            } = p
            else {
                return err(
                    span,
                    "a const generic argument cannot be inferred, and `::<>` is \
                     Ch. 4 §2.3, not implemented yet",
                );
            };
            // A parameter no argument's type mentions may still be settled
            // by another's `Fn` bound: `map<B, F: Fn(A) -> B>` learns `B`
            // from the closure it was handed (Ch. 4 §4.3).
            if !env.contains_key(pname)
                && let Some(t) = self.solve_from_fn_bounds(pname, &def.generics, &env)
            {
                env.insert(pname.clone(), t);
            }
            let Some(ty) = env.get(pname) else {
                return err(
                    span,
                    format!(
                        "cannot tell what `{pname}` is in this call to `{name}`; \
                         give an argument a written type (Ch. 4 §2.3)"
                    ),
                );
            };
            // §2.2: an instantiation that fails a bound is rejected here, at
            // the call site, and not inside the body.
            for b in bounds {
                let ty = ty.clone();
                self.check_bound_in(&ty, b, &env, name, pname, span)?;
            }
            targs.push(ty.clone());
        }

        let _ = targs;
        Ok((self.instantiate_with(name, env, span)?, args))
    }

    /// Instantiate a generic function with an environment already worked out,
    /// queueing its body if this is the first time (Ch. 4 §2.7).
    fn instantiate_with(&mut self, name: &str, env: HashMap<String, Ty>, span: Span) -> R<String> {
        let def = self.generic_def(name).expect("a generic function");
        let targs: Vec<Ty> = def.generics.iter().map(|p| env[p.name()].clone()).collect();
        let key = mangle(name, &targs);
        if self.sigs.borrow().contains_key(&key) {
            return Ok(key);
        }
        if self.pending.borrow().len() as u32 > INSTANTIATION_LIMIT {
            return err(
                span,
                format!(
                    "instantiating `{name}` does not terminate: more than \
                     {INSTANTIATION_LIMIT} instantiations are outstanding (Ch. 4 §2.7)"
                ),
            );
        }

        // The signature is built now, so this call can be checked against it
        // before the body it belongs to has been lowered.
        let params: Vec<Ty> = def
            .params
            .iter()
            .map(|p| {
                let ty = resolve_ty_env(&p.ty, self.types, &env)?;
                check_sized(&ty, p.ty.span(), &format!("the parameter `{}`", p.name))?;
                Ok(ty)
            })
            .collect::<R<_>>()?;
        let ret = match &def.ret {
            None => Ty::Unit,
            Some(t) => resolve_ty_env(t, self.types, &env)?,
        };
        let self_ref = matches!(def.params.first(), Some(p) if p.name == "self" && matches!(p.ty, ast::Ty::Ref(..)));
        check_returned_reference(&params, &ret, self_ref, span)?;
        self.sigs
            .borrow_mut()
            .insert(key.clone(), (params, ret.clone()));
        self.pending.borrow_mut().push(Job {
            key: key.clone(),
            from: name.to_string(),
            env,
            depth: 0,
        });
        Ok(key)
    }

    /// Check that a type argument satisfies a bound (Ch. 4 §2.2).
    /// Whether `ty` satisfies one bound.
    ///
    /// A bound with arguments — `U: From<T>` — is a different requirement for
    /// every argument, so the arguments are resolved in the caller's
    /// environment and appended to the trait's name, which is exactly how the
    /// impl recorded itself.
    fn check_bound_in(
        &mut self,
        ty: &Ty,
        bound: &ast::Bound,
        env: &HashMap<String, Ty>,
        callee: &str,
        param: &str,
        span: Span,
    ) -> R<()> {
        if bound.args.is_empty() {
            self.check_bound(ty, &bound.name, env, callee, param, span)?;
            return self.check_assoc_bindings(ty, bound, env, param, span);
        }
        let args: Vec<Ty> = bound
            .args
            .iter()
            .map(|a| resolve_ty_env(a, self.types, env))
            .collect::<R<_>>()?;
        let shown = format!(
            "{}<{}>",
            bound.name,
            args.iter()
                .map(Ty::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        // A rule that holds for every type satisfying a bound satisfies this
        // one too, wherever its own conditions hold (Ch. 4 §5.6).
        if !self.by_rule(ty, &bound.name, &args, 0) {
            self.check_bound_named(
                ty,
                &mangle(&bound.name, &args),
                &shown,
                env,
                callee,
                param,
                span,
            )?;
        }
        self.check_assoc_bindings(ty, bound, env, param, span)
    }

    /// Whether a blanket impl gives `ty` this trait with these arguments.
    ///
    /// The rule's parameters are bound from the type and the arguments asked
    /// about, and the rule's own bounds are then the question again — so this
    /// recurses, and a depth limit stands in for the termination argument a
    /// coherence checker would give (Ch. 4 §5.6).
    fn by_rule(&self, ty: &Ty, trait_name: &str, args: &[Ty], depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        for rule in &self.impls.blankets {
            if rule.trait_name != trait_name || rule.trait_args.len() != args.len() {
                continue;
            }
            let mut env: HashMap<String, Ty> = HashMap::new();
            env.insert(rule.self_param.clone(), ty.clone());
            for (written, got) in rule.trait_args.iter().zip(args) {
                unify(written, got, &rule.generics, &mut env);
            }
            if rule.generics.iter().any(|p| !env.contains_key(p.name())) {
                continue;
            }
            let holds = rule.generics.iter().all(|p| {
                let ast::GenericParam::Type { name, bounds } = p else {
                    return true;
                };
                let ty = &env[name];
                bounds.iter().all(|b| {
                    let Ok(bargs) = b
                        .args
                        .iter()
                        .map(|a| resolve_ty_env(a, self.types, &env))
                        .collect::<R<Vec<Ty>>>()
                    else {
                        return false;
                    };
                    self.by_rule(ty, &b.name, &bargs, depth + 1)
                        || self
                            .check_bound_named(
                                ty,
                                &mangle(&b.name, &bargs),
                                "",
                                &HashMap::new(),
                                "",
                                "",
                                rule.span,
                            )
                            .is_ok()
                })
            });
            if holds {
                return true;
            }
        }
        false
    }

    fn check_bound(
        &self,
        ty: &Ty,
        bound: &str,
        env: &HashMap<String, Ty>,
        callee: &str,
        param: &str,
        span: Span,
    ) -> R<()> {
        self.check_bound_named(ty, bound, bound, env, callee, param, span)
    }

    /// `Iterator<Item = t27>`: the implementation exists *and* chose this.
    ///
    /// A binding constrains what the implementor picked, where an argument
    /// picks which implementation is meant (Ch. 4 §1.7). Nothing else about
    /// the bound changes.
    fn check_assoc_bindings(
        &self,
        ty: &Ty,
        bound: &ast::Bound,
        env: &HashMap<String, Ty>,
        param: &str,
        span: Span,
    ) -> R<()> {
        for (name, written) in &bound.assoc {
            let want = resolve_ty_env(written, self.types, env)?;
            let chose = match nominal_name(ty)
                .and_then(|n| self.types.assoc.borrow().get(&(n, name.clone())).cloned())
            {
                Some(t) => Some(t),
                None => self.types.assoc_of_instantiation(ty, name, span)?,
            };
            match chose {
                Some(got) if got == want => {}
                Some(got) => {
                    return err(
                        span,
                        format!(
                            "`{param}` is `{ty}`, whose `{}::{name}` is {got} and not {want} \
                             (Ch. 4 §1.7)",
                            bound.name
                        ),
                    );
                }
                None => {
                    return err(
                        span,
                        format!("`{ty}` chooses no type for `{name}` (Ch. 4 §1.7)"),
                    );
                }
            }
        }
        Ok(())
    }

    /// The same, with the requirement spelled for a reader rather than for
    /// the lookup: `From<t27>` names what `From.t27` finds.
    #[allow(clippy::too_many_arguments)]
    fn check_bound_named(
        &self,
        ty: &Ty,
        bound: &str,
        shown: &str,
        env: &HashMap<String, Ty>,
        callee: &str,
        param: &str,
        span: Span,
    ) -> R<()> {
        // `Copy` is structural and automatic (Ch. 4 §5.1); `Sized` is a fact
        // about the type, not a claim about it (§2.5).
        // `Fn@…` is the bound an `impl Fn(…)` parameter was given: satisfied
        // by a closure whose signature is the written one (Ch. 4 §§2.2, 4.3).
        if let Some(key) = bound.strip_prefix("Fn@") {
            return self.check_fn_bound(ty, key, env, callee, param, span);
        }
        // A trait object implements its own trait, and its supertraits
        // (Ch. 4 §3.1): dispatch through it is what the vtable is for.
        if let Ty::Dyn(name) = ty {
            let mut chain = vec![name.clone()];
            let mut i = 0;
            while i < chain.len() {
                if let Some(decl) = self.traits.get(&chain[i]) {
                    for s in &decl.supertraits {
                        if !chain.contains(s) {
                            chain.push(s.clone());
                        }
                    }
                }
                i += 1;
            }
            if chain.iter().any(|t| t == bound) {
                return Ok(());
            }
        }
        let ok = match bound {
            "Copy" => self.types.is_copyable(ty),
            "Sized" => !ty.is_unsized(),
            // A reference's impls are keyed under its referent, which is
            // where `impl Trait for &T` puts them too (Ch. 4 §2.1).
            _ => match nominal_name(ty).or_else(|| match ty {
                Ty::Ref(inner, _) => nominal_name(inner),
                _ => None,
            }) {
                Some(n) => {
                    let base = self
                        .types
                        .instantiations
                        .borrow()
                        .get(&n)
                        .map(|(b, _)| b.clone());
                    self.impls.pairs.contains(&(n, bound.to_string()))
                        || base.is_some_and(|b| self.impls.pairs.contains(&(b, bound.to_string())))
                }
                None => false,
            },
        };
        // `impl<T> Make<T> for Pair<T>` gives `Pair<X>` the trait `Make<X>`,
        // and which arguments that is depends on the instantiation asking.
        let ok = ok || self.parameterized_gives(ty, bound);
        // A blanket impl gives the trait to every type meeting its bounds
        // (Ch. 4 §5.6), and a bound is one of the places that has to know:
        // `fn f<T: IntoIterator>` accepts an iterator because a rule says so
        // and not because a pair was written out.
        let ok = ok
            || self
                .impls
                .blankets
                .iter()
                .filter(|r| r.trait_name == bound)
                .any(|r| self.rule_applies(r, ty));
        if ok {
            return Ok(());
        }
        err(
            span,
            format!(
                "`{ty}` does not implement `{shown}`, which `{callee}` requires of \
                 `{param}` (Ch. 4 §2.2)"
            ),
        )
    }

    /// Whether a generic impl of a parameterized trait gives `ty` this bound.
    ///
    /// The impl wrote its trait arguments as its own parameters, and those
    /// are the self type's arguments — so the answer is "resolve them for
    /// this instantiation and see" (Ch. 4 §1.7).
    fn parameterized_gives(&self, ty: &Ty, bound: &str) -> bool {
        let Some(name) = nominal_name(ty) else {
            return false;
        };
        let Some((base, args)) = self.types.instantiations.borrow().get(&name).cloned() else {
            return false;
        };
        self.impls.parameterized.iter().any(|p| {
            if p.base != base || p.params.len() != args.len() {
                return false;
            }
            let env: HashMap<String, Ty> =
                p.params.iter().cloned().zip(args.iter().cloned()).collect();
            let Ok(resolved) = p
                .args
                .iter()
                .map(|a| resolve_ty_env(a, self.types, &env))
                .collect::<R<Vec<_>>>()
            else {
                return false;
            };
            mangle(&p.trait_name, &resolved) == bound
        })
    }

    /// Whether a blanket rule's own bounds hold for this type.
    ///
    /// Only the self parameter's bounds are checked: the rule's other
    /// parameters are settled by the call, and this question is asked before
    /// there is a call (Ch. 4 §5.6).
    fn rule_applies(&self, rule: &Blanket, ty: &Ty) -> bool {
        rule.generics.iter().all(|g| {
            let ast::GenericParam::Type { name, bounds } = g else {
                return true;
            };
            if *name != rule.self_param {
                return true;
            }
            bounds.iter().all(|b| {
                self.check_bound_named(ty, &b.name, &b.name, &HashMap::new(), "", "", rule.span)
                    .is_ok()
            })
        })
    }

    /// A call to a known key, with any number of already-evaluated leading
    /// arguments. Methods use the prefix for a receiver that is not a place
    /// (Ch. 4 §1.3); ordinary calls pass none.
    fn call_key(
        &mut self,
        name: &str,
        pre: Vec<(Operand, Ty)>,
        args: &[ast::Expr],
        span: Span,
    ) -> R<(Operand, Ty)> {
        let Some((params, ret)) = self.sigs.borrow().get(name).cloned() else {
            return err(span, format!("`{name}` is not a function in scope"));
        };
        if params.len() != args.len() + pre.len() {
            return err(
                span,
                format!(
                    "`{name}` takes {} argument(s), {} given",
                    params.len(),
                    args.len() + pre.len()
                ),
            );
        }
        let mut values = Vec::new();
        // An aggregate result is written through a pointer the caller
        // supplies, since TIR has no aggregate values (TIR §2) — and the
        // pointer is where the value was going, when that is known, so
        // `let v = f()` writes into `v` rather than into a temporary `v` is
        // copied from. Taken before the arguments are lowered, so an
        // argument cannot take it.
        let out = ret.is_aggregate().then(|| self.dest_or_temp(&ret));
        if let Some(slot) = &out {
            values.push(Operand::Value(slot.clone()));
        }
        for ((v, got), want) in pre.into_iter().zip(&params) {
            self.check(&got, want, span, "receiver")?;
            values.push(v);
        }
        for (arg, want) in args
            .iter()
            .zip(params.iter().skip(values.len() - out.is_some() as usize))
        {
            let (v, got) = self.expr(arg, Some(want))?;
            self.check(&got, want, arg.span(), "argument")?;
            values.push(v);
        }
        let kind = InstKind::Call {
            callee: Callee::Direct(name.to_string()),
            args: values,
            ret: if ret == Ty::Unit || ret.is_aggregate() {
                None
            } else {
                Some(ret.tir())
            },
        };
        match out {
            Some(slot) => {
                self.push(Inst {
                    results: Vec::new(),
                    kind,
                });
                Ok((Operand::Value(slot), ret))
            }
            None if ret == Ty::Unit => {
                self.push(Inst {
                    results: Vec::new(),
                    kind,
                });
                Ok((unit(), Ty::Unit))
            }
            None => {
                let v = self.emit("r", ret.tir(), kind);
                Ok((v, ret))
            }
        }
    }

    /// The method forms Ch. 1 fixes: the trit-wise operations and the
    /// `trit` ↔ `bool` projections (Ch. 1 §4, §6).
    fn method(
        &mut self,
        recv: &ast::Expr,
        name: &str,
        args: &[ast::Expr],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // Ch. 1's own methods are the language's, and are not overridable;
        // everything else resolves through impl blocks (Ch. 4 §1.3).
        if !is_builtin_method(name) {
            return self.user_method(recv, name, args, expected, span);
        }
        // A `Vec` is mutated in place and never read as a value here, so its
        // receiver is a *place*: reading it would move it, and `v.push(x)` in
        // a loop would then move the same `v` twice (Ch. 3 §1.2).
        let (v, ty) = match self.type_of_place(recv)? {
            Some(Ty::VecOf(_)) => self.place(recv, span)?,
            _ => self.expr(recv, None)?,
        };
        let one_arg = |this: &mut Self, want: &Ty| -> R<Operand> {
            if args.len() != 1 {
                return err(span, format!("`{name}` takes one argument"));
            }
            let (a, at) = this.expr(&args[0], Some(want))?;
            this.check(&at, want, span, "argument")?;
            Ok(a)
        };

        match name {
            "tmin" | "tmax" | "tmul" => {
                if !ty.is_arithmetic() && ty != Ty::Trit {
                    return err(span, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name {
                    "tmin" => PlainOp::TMin,
                    "tmax" => PlainOp::TMax,
                    _ => PlainOp::TMul,
                };
                let r = self.emit(
                    "a",
                    ty.tir(),
                    InstKind::Plain {
                        op,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                );
                Ok((r, ty))
            }

            // `tneg` is identical to unary `-`; provided for symmetry when
            // writing trit-manipulation code (Ch. 1 §4).
            "tneg" => {
                if !args.is_empty() {
                    return err(span, "`tneg` takes no arguments");
                }
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }

            "is_pos" | "is_zero" | "is_neg" => {
                if ty != Ty::Trit {
                    return err(span, format!("`{name}` applies to trit, not {ty}"));
                }
                if !args.is_empty() {
                    return err(span, format!("`{name}` takes no arguments"));
                }
                let k = |v: i128| Operand::Const(Type::Int(1), Bt::from_i128(v));
                let (n, z, p) = match name {
                    "is_neg" => (1, 0, 0),
                    "is_zero" => (0, 1, 0),
                    _ => (0, 0, 1),
                };
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: v,
                        ty: Type::Int(1),
                        neg: k(n),
                        zero: k(z),
                        pos: k(p),
                    },
                );
                Ok((r, Ty::Bool))
            }

            // false → 0t, true → 1t. The representations already agree, so
            // this is a retype and not an operation.
            "to_trit" => {
                if ty != Ty::Bool {
                    return err(span, format!("`to_trit` applies to bool, not {ty}"));
                }
                Ok((v, Ty::Trit))
            }

            // Ch. 1 §4: the high half of the product, so that
            // `a.mulh(b)·3^N + a.wrapping_mul(b)` is the exact product. Total,
            // hence flavorless: the high half always fits.
            "mulh" => {
                if !ty.is_arithmetic() {
                    return err(span, format!("`mulh` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let r = self.emit(
                    "h",
                    ty.tir(),
                    InstKind::Plain {
                        op: PlainOp::MulH,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                );
                Ok((r, ty))
            }

            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => {
                if !ty.is_arithmetic() {
                    return err(span, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name {
                    "wrapping_add" => FlavoredOp::Add,
                    "wrapping_sub" => FlavoredOp::Sub,
                    _ => FlavoredOp::Mul,
                };
                let r = self.emit(
                    "a",
                    ty.tir(),
                    InstKind::Flavored {
                        op,
                        flavor: Flavor::Wrap,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                );
                Ok((r, ty))
            }

            // Ch. 1 §4 carries the whole Rust family over. `saturating_*`
            // clamps to the range end the overflow ran past; `overflowing_*`
            // hands back the wrapped value and whether it wrapped.
            "saturating_add" | "saturating_sub" | "saturating_mul" | "overflowing_add"
            | "overflowing_sub" | "overflowing_mul" => {
                if !ty.is_arithmetic() {
                    return err(span, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name.rsplit('_').next().expect("a suffix") {
                    "add" => FlavoredOp::Add,
                    "sub" => FlavoredOp::Sub,
                    _ => FlavoredOp::Mul,
                };
                let width = ty.width().expect("arithmetic");
                let value = self.fresh("a");
                let flag = self.fresh("o");
                self.push(Inst {
                    results: vec![value.clone(), flag.clone()],
                    kind: InstKind::Flavored {
                        op,
                        flavor: Flavor::Flag,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                });
                let (value, flag) = (Operand::Value(value), Operand::Value(flag));

                if name.starts_with("saturating") {
                    // The overflow trit *is* the direction of the overflow,
                    // so the clamp is one three-way select and no comparison.
                    let max = Operand::Const(ty.tir(), Bt::max_of_width(width));
                    let min = Operand::Const(ty.tir(), Bt::min_of_width(width));
                    let r = self.emit(
                        "s",
                        ty.tir(),
                        InstKind::Select3 {
                            t: flag,
                            ty: ty.tir(),
                            neg: min,
                            zero: value,
                            pos: max,
                        },
                    );
                    return Ok((r, ty));
                }

                let k = |x: i128| Operand::Const(Type::Int(1), Bt::from_i128(x));
                let overflowed = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: flag,
                        ty: Type::Int(1),
                        neg: k(1),
                        zero: k(0),
                        pos: k(1),
                    },
                );
                let result = Ty::Tuple(vec![ty.clone(), Ty::Bool]);
                let slot = self.temp_slot(&result);
                let fields = self.types.fields(&result);
                self.store_at(&slot, fields[0].2, &ty, value, span)?;
                self.store_at(&slot, fields[1].2, &Ty::Bool, overflowed, span)?;
                Ok((Operand::Value(slot), result))
            }

            // `v.push(x)` — Ch. 5 §2.6. Growth **doubles** from four, and
            // the factor is specified rather than left open because a program
            // that pushes n elements is entitled to know it did O(n) work in
            // total, and the amortized argument is a property of the factor.
            "push" => {
                let elem = vec_elem(&ty, "push", span)?;
                let x = one_arg(self, &elem)?;
                self.vec_push(v, &elem, x, span)?;
                Ok((unit(), Ty::Unit))
            }

            // `v.pop()` — Ch. 5 §2.6.
            "pop" => {
                let elem = vec_elem(&ty, "pop", span)?;
                if !args.is_empty() {
                    return err(span, "`pop` takes no arguments");
                }
                self.vec_pop(v, &elem, span)
            }

            // `v.clear()` — the elements go, the allocation stays.
            "clear" if matches!(peel(&ty), Ty::VecOf(_)) => {
                let elem = vec_elem(&ty, "clear", span)?;
                if !args.is_empty() {
                    return err(span, "`clear` takes no arguments");
                }
                self.vec_clear(v, &elem, span)?;
                Ok((unit(), Ty::Unit))
            }

            // `v.reserve(n)` — room for `n` more.
            // `v.insert(i, x)` and `v.remove(i)` — Ch. 5 §2.6.
            "insert" if matches!(peel(&ty), Ty::VecOf(_)) => {
                let elem = vec_elem(&ty, "insert", span)?;
                let [at, x] = args else {
                    return err(span, "`insert` takes an index and a value");
                };
                let (at, _) = self.expr(at, Some(&Ty::TAddr))?;
                let (x, xt) = self.expr(x, Some(&elem))?;
                self.check(&xt, &elem, span, "`insert`'s value")?;
                self.vec_insert(v, &elem, at, x, span)?;
                Ok((unit(), Ty::Unit))
            }

            "remove" if matches!(peel(&ty), Ty::VecOf(_)) => {
                let elem = vec_elem(&ty, "remove", span)?;
                let [at] = args else {
                    return err(span, "`remove` takes an index");
                };
                let (at, _) = self.expr(at, Some(&Ty::TAddr))?;
                self.vec_remove(v, &elem, at, span)
            }

            "reserve" => {
                let elem = vec_elem(&ty, "reserve", span)?;
                let n = one_arg(self, &Ty::TAddr)?;
                self.vec_reserve(v, &elem, n, span)?;
                Ok((unit(), Ty::Unit))
            }

            // `v.capacity()` — the third word, and the only method that can
            // see the room beyond the length.
            "capacity" if matches!(peel(&ty), Ty::VecOf(_)) => {
                if !args.is_empty() {
                    return err(span, "`capacity` takes no arguments");
                }
                let at = self.offset(v, 6);
                Ok((self.load_from(at, &Ty::TAddr), Ty::TAddr))
            }

            // `v.is_empty()` — `len() == 0`, said once.
            "is_empty" if matches!(peel(&ty), Ty::VecOf(_)) => {
                if !args.is_empty() {
                    return err(span, "`is_empty` takes no arguments");
                }
                let at = self.offset(v, 3);
                let len = self.load_from(at, &Ty::TAddr);
                let c = self.emit(
                    "c",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: Type::Int(27),
                        a: len,
                        b: konst_addr(0),
                    },
                );
                let k = |x: i128| Operand::Const(Type::Int(1), Bt::from_i128(x));
                let b = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: c,
                        ty: Type::Int(1),
                        neg: k(0),
                        zero: k(1),
                        pos: k(0),
                    },
                );
                Ok((b, Ty::Bool))
            }

            // Ch. 3 §5.4: a slice's length is the second word of its fat
            // reference; an array's is in its type.
            "len" => {
                if !args.is_empty() {
                    return err(span, "`len` takes no arguments");
                }
                match &ty {
                    Ty::Array(_, n) => {
                        Ok((Operand::Const(Type::Int(27), Bt::from_i128(*n)), Ty::TAddr))
                    }
                    // A `Vec`'s length is its second word (Ch. 5 §2.6).
                    Ty::VecOf(_) => {
                        let at = self.offset(v, 3);
                        Ok((self.load_from(at, &Ty::TAddr), Ty::TAddr))
                    }
                    Ty::Ref(inner, _) if matches!(**inner, Ty::VecOf(_)) => {
                        let at = self.offset(v, 3);
                        Ok((self.load_from(at, &Ty::TAddr), Ty::TAddr))
                    }
                    Ty::Ref(inner, _) if matches!(**inner, Ty::Slice(_)) => {
                        let at = self.offset(v, 3);
                        Ok((self.load_from(at, &Ty::TAddr), Ty::TAddr))
                    }
                    Ty::Ref(inner, _) if matches!(**inner, Ty::Array(..)) => {
                        let Ty::Array(_, n) = &**inner else {
                            unreachable!()
                        };
                        Ok((Operand::Const(Type::Int(27), Bt::from_i128(*n)), Ty::TAddr))
                    }
                    other => err(
                        span,
                        format!("`len` applies to a slice or an array, not {other}"),
                    ),
                }
            }

            // Ch. 1 §4: the Rust family, carried over with identical naming.
            // The overflow trit already says whether the exact result fitted,
            // so this is one `.flag` operation and a three-way branch — no
            // second computation and no comparison against a bound.
            "checked_add" | "checked_sub" | "checked_mul" => {
                if !ty.is_arithmetic() {
                    return err(span, format!("`{name}` does not apply to {ty}"));
                }
                let b = one_arg(self, &ty)?;
                let op = match name.rsplit('_').next().expect("a suffix") {
                    "add" => FlavoredOp::Add,
                    "sub" => FlavoredOp::Sub,
                    _ => FlavoredOp::Mul,
                };
                let value = self.fresh("a");
                let flag = self.fresh("o");
                self.push(Inst {
                    results: vec![value.clone(), flag.clone()],
                    kind: InstKind::Flavored {
                        op,
                        flavor: Flavor::Flag,
                        ty: ty.tir(),
                        a: v,
                        b,
                    },
                });

                let opt = self
                    .types
                    .instantiate("Option", std::slice::from_ref(&ty), span)?;
                let ename = nominal_name(&opt).expect("an instantiation is nominal");
                let (some, none) = (
                    self.types
                        .variant(&ename, "Some")
                        .ok_or_else(|| one_err(span, "`Option` has no `Some`".into()))?,
                    self.types
                        .variant(&ename, "None")
                        .ok_or_else(|| one_err(span, "`Option` has no `None`".into()))?,
                );
                let slot = self.temp_slot(&opt);
                let (fits, over, join) = (
                    self.fresh("chk.some"),
                    self.fresh("chk.none"),
                    self.fresh("chk.join"),
                );
                // The overflow trit is the *direction* of the overflow, so
                // both nonzero arms mean the same thing here.
                self.br3(Operand::Value(flag), &over, &fits, &over);

                self.start(fits);
                let payload = [("0".to_string(), Operand::Value(value))];
                self.build_variant_into(&slot, &ename, some, &payload, span)?;
                self.jump(&join);

                self.start(over);
                self.build_variant_into(&slot, &ename, none, &[], span)?;
                self.jump(&join);

                self.start(join);
                Ok((Operand::Value(slot), opt))
            }

            other => err(span, format!("`{other}` is not a method in this milestone")),
        }
    }

    /// `e?` — propagate a failure (Ch. 5 §4.1).
    ///
    /// The two rules do not mix: `?` on an `Option` in a function returning
    /// `Result` is an error, and so is the reverse. Converting between them
    /// is `ok_or` and `ok`, written where it happens.
    ///
    /// Desugaring to `match` and `return` rather than emitting branches by
    /// hand is not laziness: it is what makes the drops right. Leaving a
    /// function early has to drop everything the frame owns, and `return`
    /// already does.
    fn try_expr(
        &mut self,
        inner: &ast::Expr,
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(inner, None)?;
        let Some((kind, args)) = self.carrier(&ty) else {
            return err(
                span,
                format!("`?` applies to `Result` and `Option`, not to {ty} (Ch. 5 §4.1)"),
            );
        };
        let ret = self.ret.clone();
        let Some((rkind, rargs)) = self.carrier(&ret) else {
            return err(
                span,
                format!(
                    "`?` needs a function that returns `Result` or `Option`, and this one \
                     returns {ret} (Ch. 5 §4.1)"
                ),
            );
        };
        if kind != rkind {
            return err(
                span,
                format!(
                    "`?` on a `{kind}` in a function returning `{rkind}` does not convert \
                     between them: write `ok_or(…)` or `ok()` where it happens \
                     (Ch. 5 §4.1)"
                ),
            );
        }

        // The value is matched twice over — once per arm — so it is bound
        // first and the arms name the binding.
        let Operand::Value(slot) = v else {
            return err(span, "`?` needs a place to match");
        };
        let bound = self.bind_existing(slot, ty.clone());
        let scrutinee = ast::Expr::Path(bound, span);
        let ok_bind = format!("#q{}", self.counter);
        let bad_bind = format!("#r{}", self.counter);

        let arm = |name: &str, payload: Vec<(String, ast::Pattern)>, body: ast::Expr| ast::Arm {
            patterns: vec![ast::Pattern::Aggregate(
                ast::Path {
                    segments: vec![kind.to_string(), name.to_string()],
                    targs: Vec::new(),
                    span,
                },
                payload,
                span,
            )],
            guard: None,
            body,
            span,
        };
        let path = |n: &str| ast::Expr::Path(n.to_string(), span);
        let variant = |name: &str, fields: Vec<(String, ast::Expr)>| {
            ast::Expr::Aggregate(
                ast::Path {
                    segments: vec![kind.to_string(), name.to_string()],
                    targs: Vec::new(),
                    span,
                },
                fields,
                span,
            )
        };

        let arms = if kind == "Option" {
            vec![
                arm(
                    "Some",
                    vec![("0".into(), ast::Pattern::Bind(ok_bind.clone(), span))],
                    path(&ok_bind),
                ),
                arm(
                    "None",
                    Vec::new(),
                    ast::Expr::Return(Some(Box::new(variant("None", Vec::new()))), span),
                ),
            ]
        } else {
            // `Err(e)` becomes `Err(F::from(e))`, and where the two error
            // types are the same there is nothing to convert (Ch. 4 §5.6).
            let (from_ty, to_ty) = (args[1].clone(), rargs[1].clone());
            let converted = if from_ty == to_ty {
                path(&bad_bind)
            } else {
                let Some(name) = nominal_name(&to_ty) else {
                    return err(
                        span,
                        format!("`?` cannot convert {from_ty} into {to_ty} (Ch. 5 §4.1)"),
                    );
                };
                ast::Expr::Aggregate(
                    ast::Path {
                        segments: vec![name, "from".to_string()],
                        targs: Vec::new(),
                        span,
                    },
                    vec![("0".to_string(), path(&bad_bind))],
                    span,
                )
            };
            vec![
                arm(
                    "Ok",
                    vec![("0".into(), ast::Pattern::Bind(ok_bind.clone(), span))],
                    path(&ok_bind),
                ),
                arm(
                    "Err",
                    vec![("0".into(), ast::Pattern::Bind(bad_bind.clone(), span))],
                    ast::Expr::Return(
                        Some(Box::new(variant("Err", vec![("0".to_string(), converted)]))),
                        span,
                    ),
                ),
            ]
        };
        self.match_expr(&scrutinee, &arms, expected, span)
    }

    /// Whether a type is one of the two `?` carries, and its arguments.
    fn carrier(&self, ty: &Ty) -> Option<(&'static str, Vec<Ty>)> {
        let name = nominal_name(ty)?;
        let (base, args) = self.types.instantiations.borrow().get(&name)?.clone();
        match base.as_str() {
            "Option" => Some(("Option", args)),
            "Result" => Some(("Result", args)),
            _ => None,
        }
    }

    /// `v.push(x)` — grow if the room has run out, then store and count.
    ///
    /// Growth **doubles**, from a first allocation of four. The factor is
    /// specified rather than left to the implementation because a program
    /// that pushes *n* elements is entitled to know it did O(*n*) work in
    /// total, and the amortized argument is a property of the factor
    /// (Ch. 5 §2.6). Relaxing it later costs nothing; tightening it would
    /// break something, so the reversible direction is the one taken.
    ///
    /// There is no `realloc`: growth allocates, copies and frees, which is
    /// what Ch. 5 §7 reserves the third target function for.
    fn vec_push(&mut self, at: Operand, elem: &Ty, x: Operand, span: Span) -> R<()> {
        let l = self.types.layout(elem);
        let (size, align) = (l.size as i128, l.align as i128);
        self.needs_heap.set(true);

        let ptr_at = at.clone();
        let len_at = self.offset(at.clone(), 3);
        let cap_at = self.offset(at.clone(), 6);
        let len = self.load_from(len_at.clone(), &Ty::TAddr);
        let cap = self.load_from(cap_at.clone(), &Ty::TAddr);

        let (grow, ready) = (self.fresh("vec.grow"), self.fresh("vec.ready"));
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: len.clone(),
                b: cap.clone(),
            },
        );
        // `len < cap` means there is room; anything else means there is not.
        self.br3(c, &ready, &grow, &grow);

        self.start(grow);
        // Four, or twice what there was.
        let doubled = self.apply_binary("*", cap.clone(), konst_addr(2), &Ty::TAddr, span)?;
        let want = self.emit(
            "n",
            Type::Int(27),
            InstKind::Plain {
                op: PlainOp::TMax,
                ty: Type::Int(27),
                a: doubled,
                b: konst_addr(4),
            },
        );
        self.vec_realloc(
            ptr_at.clone(),
            cap_at,
            size,
            align,
            len.clone(),
            cap,
            want,
            span,
        )?;
        self.jump(&ready);

        self.start(ready);
        let base = self.load_ptr(ptr_at);
        let off = self.apply_binary("*", len.clone(), konst_addr(size), &Ty::TAddr, span)?;
        let slot = self.emit("e", Type::Ptr, InstKind::Offset { p: base, d: off });
        if elem.is_aggregate() {
            self.copy_typed(slot, x, elem, span)?;
        } else {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: elem.tir(),
                    v: x,
                    p: slot,
                },
            });
        }
        let next = self.apply_binary("+", len, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at_operand(len_at, &Ty::TAddr, next, span)?;
        Ok(())
    }

    /// Move a `Vec`'s elements into an allocation of `want` elements and
    /// give the old one back.
    ///
    /// There is no `realloc` — Ch. 5 §7 reserves it — so growing is three
    /// steps, and the middle one is a copy of `len` elements' worth of
    /// trytes. A move *is* a copy of the storage, and nothing here owns
    /// anything twice (Ch. 3 §1.2).
    #[allow(clippy::too_many_arguments)]
    fn vec_realloc(
        &mut self,
        ptr_at: Operand,
        cap_at: Operand,
        size: i128,
        align: i128,
        len: Operand,
        cap: Operand,
        want: Operand,
        span: Span,
    ) -> R<()> {
        let bytes = self.apply_binary("*", want.clone(), konst_addr(size), &Ty::TAddr, span)?;
        let fresh = self.emit(
            "p",
            Type::Ptr,
            InstKind::Call {
                callee: Callee::Direct(ALLOC.to_string()),
                args: vec![bytes, konst_addr(align)],
                ret: Some(Type::Ptr),
            },
        );
        // The pointer word is read and written *as a pointer*, whatever the
        // layout calls it: TIR has no int-to-pointer cast, so an address that
        // is going to be offset has to have been a `ptr` all along (TIR §5).
        let old = self.load_ptr(ptr_at.clone());
        let copied = self.apply_binary("*", len, konst_addr(size), &Ty::TAddr, span)?;
        self.copy_n_trytes(fresh.clone(), old.clone(), copied, span)?;
        let oldcap = self.apply_binary("*", cap, konst_addr(size), &Ty::TAddr, span)?;
        self.free_if_any(old, oldcap, align);
        self.store_ptr(ptr_at, fresh);
        self.store_at_operand(cap_at, &Ty::TAddr, want, span)?;
        Ok(())
    }

    /// `v.reserve(n)` — make room for `n` more elements than there are.
    ///
    /// Exactly `len + n`, not the doubling `push` uses: a program that says
    /// how much it wants has said something `push`'s guess cannot improve on.
    fn vec_reserve(&mut self, at: Operand, elem: &Ty, n: Operand, span: Span) -> R<()> {
        let l = self.types.layout(elem);
        let (size, align) = (l.size as i128, l.align as i128);
        self.needs_heap.set(true);

        let len_at = self.offset(at.clone(), 3);
        let cap_at = self.offset(at.clone(), 6);
        let len = self.load_from(len_at, &Ty::TAddr);
        let cap = self.load_from(cap_at.clone(), &Ty::TAddr);
        let want = self.apply_binary("+", len.clone(), n, &Ty::TAddr, span)?;

        let (grow, ready) = (self.fresh("res.grow"), self.fresh("res.ready"));
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: want.clone(),
                b: cap.clone(),
            },
        );
        // `want > cap` is the only case that allocates; asking for room that
        // is already there is not an error, it is nothing.
        self.br3(c, &ready, &ready, &grow);
        self.start(grow);
        self.vec_realloc(at, cap_at, size, align, len, cap, want, span)?;
        self.jump(&ready);
        self.start(ready);
        Ok(())
    }

    /// Drop the elements in `[from, to)` of a `Vec`'s allocation.
    fn vec_drop_range(
        &mut self,
        base: Operand,
        from: Operand,
        to: Operand,
        elem: &Ty,
        span: Span,
        depth: u32,
    ) -> R<()> {
        if !self.types.needs_drop(elem) {
            return Ok(());
        }
        let size = self.types.layout(elem).size as i128;
        let i = self.temp_slot(&Ty::TAddr);
        self.store_at(&i, 0, &Ty::TAddr, from, span)?;
        let (head, body, out) = (
            self.fresh("vdrop.head"),
            self.fresh("vdrop.body"),
            self.fresh("vdrop.done"),
        );
        self.jump(&head);
        self.start(head.clone());
        let at = self.load_slot(&i, &Ty::TAddr);
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: at.clone(),
                b: to,
            },
        );
        self.br3(c, &body, &out, &out);
        self.start(body);
        let off = self.apply_binary("*", at.clone(), konst_addr(size), &Ty::TAddr, span)?;
        let e = self.emit("e", Type::Ptr, InstKind::Offset { p: base, d: off });
        self.drop_at(e, elem, span, depth + 1)?;
        let next = self.apply_binary("+", at, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at(&i, 0, &Ty::TAddr, next, span)?;
        self.jump(&head);
        self.start(out);
        Ok(())
    }

    /// Copy `n` trytes from `src` to `dst` **downwards**, so that the two may
    /// overlap with `dst` above `src`.
    ///
    /// `copy_n_trytes` walks up and would overwrite what it has yet to read.
    /// Which direction is safe is a property of which way the block moves,
    /// and `Vec::insert` moves one up.
    fn copy_n_trytes_back(&mut self, dst: Operand, src: Operand, n: Operand, span: Span) -> R<()> {
        let i = self.temp_slot(&Ty::TAddr);
        self.store_at(&i, 0, &Ty::TAddr, n, span)?;
        let (head, body, done) = (
            self.fresh("cpb.head"),
            self.fresh("cpb.body"),
            self.fresh("cpb.done"),
        );
        self.jump(&head);
        self.start(head.clone());
        let at = self.load_slot(&i, &Ty::TAddr);
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: at.clone(),
                b: konst_addr(0),
            },
        );
        self.br3(c, &done, &done, &body);
        self.start(body);
        let at = self.apply_binary("-", at, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at(&i, 0, &Ty::TAddr, at.clone(), span)?;
        let from = self.emit(
            "e",
            Type::Ptr,
            InstKind::Offset {
                p: src,
                d: at.clone(),
            },
        );
        let to = self.emit("e", Type::Ptr, InstKind::Offset { p: dst, d: at });
        let v = self.load_from(from, &Ty::T9);
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: Type::Int(9),
                v,
                p: to,
            },
        });
        self.jump(&head);
        self.start(done);
        Ok(())
    }

    /// The element count times the element size, as trytes.
    fn elems_in_trytes(&mut self, n: Operand, size: i128, span: Span) -> R<Operand> {
        self.apply_binary("*", n, konst_addr(size), &Ty::TAddr, span)
    }

    /// `v.insert(i, x)` and `v.remove(i)` — Ch. 5 §2.6.
    ///
    /// Both move a run of elements by one place, which is a copy of storage
    /// and therefore a move of every element in it (Ch. 3 §1.2). The shift is
    /// tryte-wise, so an element holding a reference loses its provenance in
    /// the interpreter for the reason an enum's payload does — G6.7.
    fn vec_insert(&mut self, at: Operand, elem: &Ty, i: Operand, x: Operand, span: Span) -> R<()> {
        let size = self.types.layout(elem).size as i128;
        let len_at = self.offset(at.clone(), 3);
        let len = self.load_from(len_at.clone(), &Ty::TAddr);
        // `i == len` is `push`, and is the one index past the end that is
        // not an error.
        self.bounds_check(i.clone(), len.clone(), true, span)?;

        // Make room, which may move the allocation — so the base is read
        // after, not before.
        self.vec_reserve(at.clone(), elem, konst_addr(1), span)?;
        let len = self.load_from(len_at.clone(), &Ty::TAddr);
        let base = self.load_ptr(at.clone());
        let moved = self.apply_binary("-", len.clone(), i.clone(), &Ty::TAddr, span)?;
        let bytes = self.elems_in_trytes(moved, size, span)?;
        let from_off = self.elems_in_trytes(i.clone(), size, span)?;
        let src = self.emit(
            "e",
            Type::Ptr,
            InstKind::Offset {
                p: base.clone(),
                d: from_off,
            },
        );
        let to_off = self.apply_binary("+", i.clone(), konst_addr(1), &Ty::TAddr, span)?;
        let to_off = self.elems_in_trytes(to_off, size, span)?;
        let dst = self.emit("e", Type::Ptr, InstKind::Offset { p: base, d: to_off });
        self.copy_n_trytes_back(dst, src, bytes, span)?;

        let slot = self.vec_elem_addr(at.clone(), i, size, span)?;
        if elem.is_aggregate() {
            self.copy_typed(slot, x, elem, span)?;
        } else {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: elem.tir(),
                    v: x,
                    p: slot,
                },
            });
        }
        let next = self.apply_binary("+", len, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at_operand(len_at, &Ty::TAddr, next, span)
    }

    fn vec_remove(&mut self, at: Operand, elem: &Ty, i: Operand, span: Span) -> R<(Operand, Ty)> {
        let size = self.types.layout(elem).size as i128;
        let len_at = self.offset(at.clone(), 3);
        let len = self.load_from(len_at.clone(), &Ty::TAddr);
        self.bounds_check(i.clone(), len.clone(), false, span)?;

        // The element leaves the `Vec`, so it is read before the shift and
        // is not dropped by it.
        let e = self.vec_elem_addr(at.clone(), i.clone(), size, span)?;
        let out = if elem.is_aggregate() {
            let tmp = self.temp_slot(elem);
            self.copy_typed(Operand::Value(tmp.clone()), e, elem, span)?;
            Operand::Value(tmp)
        } else {
            self.load_from(e, elem)
        };

        let base = self.load_ptr(at.clone());
        let after = self.apply_binary("-", len.clone(), i.clone(), &Ty::TAddr, span)?;
        let after = self.apply_binary("-", after, konst_addr(1), &Ty::TAddr, span)?;
        let bytes = self.elems_in_trytes(after, size, span)?;
        let from_off = self.apply_binary("+", i.clone(), konst_addr(1), &Ty::TAddr, span)?;
        let from_off = self.elems_in_trytes(from_off, size, span)?;
        let src = self.emit(
            "e",
            Type::Ptr,
            InstKind::Offset {
                p: base.clone(),
                d: from_off,
            },
        );
        let to_off = self.elems_in_trytes(i, size, span)?;
        let dst = self.emit("e", Type::Ptr, InstKind::Offset { p: base, d: to_off });
        self.copy_n_trytes(dst, src, bytes, span)?;

        let next = self.apply_binary("-", len, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at_operand(len_at, &Ty::TAddr, next, span)?;
        Ok((out, elem.clone()))
    }

    /// Trap unless `0 <= i < limit`, or `<=` when the end is allowed.
    fn bounds_check(&mut self, i: Operand, limit: Operand, inclusive: bool, _line: Span) -> R<()> {
        let (bad, lo, ok) = (
            self.fresh("ins.fault"),
            self.fresh("ins.lo"),
            self.fresh("ins.ok"),
        );
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: i.clone(),
                b: limit,
            },
        );
        if inclusive {
            self.br3(c, &lo, &lo, &bad);
        } else {
            self.br3(c, &lo, &bad, &bad);
        }
        self.start(bad);
        self.finish(Terminator::Trap(FaultCode::Trap));
        self.start(lo);
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: i,
                b: konst_addr(0),
            },
        );
        let bad2 = self.fresh("ins.neg");
        self.br3(c, &bad2, &ok, &ok);
        self.start(bad2);
        self.finish(Terminator::Trap(FaultCode::Trap));
        self.start(ok);
        Ok(())
    }

    /// `v.clear()` — drop every element and set the length to zero.
    ///
    /// The allocation stays, which is the whole difference between this and
    /// dropping the `Vec`: a cleared `Vec` has kept its room.
    fn vec_clear(&mut self, at: Operand, elem: &Ty, span: Span) -> R<()> {
        let len_at = self.offset(at.clone(), 3);
        if self.types.needs_drop(elem) {
            let len = self.load_from(len_at.clone(), &Ty::TAddr);
            let base = self.load_ptr(at);
            self.vec_drop_range(base, konst_addr(0), len, elem, span, 0)?;
        }
        self.store_at_operand(len_at, &Ty::TAddr, konst_addr(0), span)
    }

    /// The address of element `i` of the `Vec` at `at`.
    fn vec_elem_addr(&mut self, at: Operand, i: Operand, size: i128, span: Span) -> R<Operand> {
        let base = self.load_ptr(at);
        let off = self.apply_binary("*", i, konst_addr(size), &Ty::TAddr, span)?;
        Ok(self.emit("e", Type::Ptr, InstKind::Offset { p: base, d: off }))
    }

    /// `v.pop()` — `Option<T>`, and `None` exactly when the `Vec` is empty.
    ///
    /// The element is *moved out*: the length comes down first, so what is
    /// returned is no longer inside the `Vec` and will not be dropped twice.
    fn vec_pop(&mut self, at: Operand, elem: &Ty, span: Span) -> R<(Operand, Ty)> {
        let size = self.types.layout(elem).size as i128;
        let opt = self
            .types
            .instantiate("Option", std::slice::from_ref(elem), span)?;
        let ename = nominal_name(&opt).expect("an instantiation is nominal");
        let (some, none) = (
            self.types
                .variant(&ename, "Some")
                .ok_or_else(|| one_err(span, "`Option` has no `Some`".into()))?,
            self.types
                .variant(&ename, "None")
                .ok_or_else(|| one_err(span, "`Option` has no `None`".into()))?,
        );
        let slot = self.temp_slot(&opt);
        let len_at = self.offset(at.clone(), 3);
        let len = self.load_from(len_at.clone(), &Ty::TAddr);
        let (full, empty, join) = (
            self.fresh("pop.some"),
            self.fresh("pop.none"),
            self.fresh("pop.join"),
        );
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: len.clone(),
                b: konst_addr(0),
            },
        );
        self.br3(c, &empty, &empty, &full);

        self.start(full);
        let last = self.apply_binary("-", len, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at_operand(len_at, &Ty::TAddr, last.clone(), span)?;
        let e = self.vec_elem_addr(at, last, size, span)?;
        let v = if elem.is_aggregate() {
            let tmp = self.temp_slot(elem);
            self.copy_typed(Operand::Value(tmp.clone()), e, elem, span)?;
            Operand::Value(tmp)
        } else {
            self.load_from(e, elem)
        };
        self.build_variant_into(&slot, &ename, some, &[("0".to_string(), v)], span)?;
        self.jump(&join);

        self.start(empty);
        self.build_variant_into(&slot, &ename, none, &[], span)?;
        self.jump(&join);

        self.start(join);
        Ok((Operand::Value(slot), opt))
    }

    /// Store a scalar at an address that is already an operand.
    fn store_at_operand(&mut self, at: Operand, ty: &Ty, v: Operand, _line: Span) -> R<()> {
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: ty.tir(),
                v,
                p: at,
            },
        });
        Ok(())
    }

    /// Copy a *runtime* number of trytes from `src` to `dst`, in a loop.
    ///
    /// The other `copy_trytes` unrolls a size known at compile time; a
    /// `Vec`'s growth does not know one.
    fn copy_n_trytes(&mut self, dst: Operand, src: Operand, n: Operand, span: Span) -> R<()> {
        let i = self.temp_slot(&Ty::TAddr);
        self.store_at(
            &i,
            0,
            &Ty::TAddr,
            Operand::Const(Type::Int(27), Bt::ZERO),
            span,
        )?;
        let (head, body, done) = (
            self.fresh("cp.head"),
            self.fresh("cp.body"),
            self.fresh("cp.done"),
        );
        self.jump(&head);
        self.start(head.clone());
        let at = self.load_slot(&i, &Ty::TAddr);
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: at.clone(),
                b: n.clone(),
            },
        );
        self.br3(c, &body, &done, &done);
        self.start(body);
        let from = self.emit(
            "s",
            Type::Ptr,
            InstKind::Offset {
                p: src.clone(),
                d: at.clone(),
            },
        );
        let to = self.emit(
            "d",
            Type::Ptr,
            InstKind::Offset {
                p: dst.clone(),
                d: at.clone(),
            },
        );
        let v = self.load_from(from, &Ty::T9);
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: Type::Int(9),
                v,
                p: to,
            },
        });
        let next = self.apply_binary("+", at, konst_addr(1), &Ty::TAddr, span)?;
        self.store_at(&i, 0, &Ty::TAddr, next, span)?;
        self.jump(&head);
        self.start(done);
        Ok(())
    }

    /// Free an allocation, unless there was never one.
    fn free_if_any(&mut self, at: Operand, size: Operand, align: i128) {
        let (yes, no) = (self.fresh("free.yes"), self.fresh("free.no"));
        // Testing an address goes through a slot, because TIR's `cmp` takes
        // integers and there is no int-to-pointer cast either way (TIR §5).
        let cell = self.temp_slot(&Ty::TAddr);
        self.store_ptr(Operand::Value(cell.clone()), at.clone());
        let n = self.load_slot(&cell, &Ty::TAddr);
        let c = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: n,
                b: Operand::Const(Type::Int(27), Bt::ZERO),
            },
        );
        self.br3(c, &no, &no, &yes);
        self.start(yes);
        self.needs_heap.set(true);
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Call {
                callee: Callee::Direct(FREE.to_string()),
                args: vec![at, size, konst_addr(align)],
                ret: None,
            },
        });
        self.jump(&no);
        self.start(no);
    }

    /// `Box::new(v)`: one `T` on the heap.
    ///
    /// The allocator is the target's, declared here in TIR rather than in
    /// Trust because it returns a *pointer* and Trust has no way to name one
    /// (Ch. 5 §2.1). That is the whole of why `Box` is a language item: every
    /// other part of it — owning, moving, dropping, dereferencing — is what
    /// Ch. 3 already does to any value.
    fn box_new(&mut self, arg: &ast::Expr, fallible: bool, span: Span) -> R<(Operand, Ty)> {
        let (v, inner) = self.expr(arg, None)?;
        check_sized(&inner, span, "a `Box`'s contents")?;
        let l = self.types.layout(&inner);
        let size = Operand::Const(Type::Int(27), Bt::from_i128(l.size as i128));
        let align = Operand::Const(Type::Int(27), Bt::from_i128(l.align as i128));
        self.needs_heap.set(true);

        let p = self.emit(
            "box",
            Type::Ptr,
            InstKind::Call {
                callee: Callee::Direct(ALLOC.to_string()),
                args: vec![size, align],
                ret: Some(Type::Ptr),
            },
        );

        // 0 is not the address of anything (ISA §2.2 reserves the first
        // word), so it is what "could not" looks like.
        let boxed = Ty::Boxed(Box::new(inner.clone()));
        let (ok, bad, join) = (
            self.fresh("box.ok"),
            self.fresh("box.no"),
            self.fresh("box.join"),
        );
        let out = if fallible {
            let opt = self
                .types
                .instantiate("Option", std::slice::from_ref(&boxed), span)?;
            Some((self.temp_slot(&opt), opt))
        } else {
            None
        };
        // 0 is not the address of anything, so it is what "could not" looks
        // like — and reading an address *as an integer* to test it is the
        // same move the niche machinery makes for `Option<&T>` (Ch. 3 §2.5).
        let cell = self.temp_slot(&Ty::TAddr);
        self.store_ptr(Operand::Value(cell.clone()), p.clone());
        let n = self.load_slot(&cell, &Ty::TAddr);
        let got = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(27),
                a: n,
                b: Operand::Const(Type::Int(27), Bt::ZERO),
            },
        );
        self.br3(got, &bad, &bad, &ok);

        self.start(ok);
        // An aggregate's value *is* an address, so it is copied; a scalar is
        // stored (Ch. 2 §1).
        if inner.is_aggregate() {
            self.copy_typed(p.clone(), v, &inner, span)?;
        } else {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: inner.tir(),
                    v,
                    p: p.clone(),
                },
            });
        }
        match &out {
            Some((slot, opt)) => {
                let ename = nominal_name(opt).expect("an instantiation is nominal");
                let some = self
                    .types
                    .variant(&ename, "Some")
                    .ok_or_else(|| one_err(span, "`Option` has no `Some`".into()))?;
                let slot = slot.clone();
                self.build_variant_into(&slot, &ename, some, &[("0".into(), p.clone())], span)?;
                self.jump(&join);
            }
            None => self.jump(&join),
        }

        self.start(bad);
        match &out {
            Some((slot, opt)) => {
                let ename = nominal_name(opt).expect("an instantiation is nominal");
                let none = self
                    .types
                    .variant(&ename, "None")
                    .ok_or_else(|| one_err(span, "`Option` has no `None`".into()))?;
                let slot = slot.clone();
                self.build_variant_into(&slot, &ename, none, &[], span)?;
                self.jump(&join);
            }
            None => self.finish(Terminator::Trap(FaultCode::Trap)),
        }

        self.start(join);
        match out {
            Some((slot, opt)) => Ok((Operand::Value(slot), opt)),
            None => Ok((p, boxed)),
        }
    }

    /// `char::try_from(x)`: `Some(x)` when `x` is a Unicode scalar value.
    ///
    /// Four comparisons and four branches, which is what the definition has:
    /// non-negative, no greater than U+10FFFF, and outside the surrogate
    /// range — those are reserved for UTF-16 and are not characters
    /// (Ch. 5 §1.2).
    fn char_try_from(&mut self, v: Operand, span: Span) -> R<(Operand, Ty)> {
        let opt = self.types.instantiate("Option", &[Ty::Char], span)?;
        let ename = nominal_name(&opt).expect("an instantiation is nominal");
        let (some, none) = (
            self.types
                .variant(&ename, "Some")
                .ok_or_else(|| one_err(span, "`Option` has no `Some`".into()))?,
            self.types
                .variant(&ename, "None")
                .ok_or_else(|| one_err(span, "`Option` has no `None`".into()))?,
        );
        let slot = self.temp_slot(&opt);
        let (good, bad, join) = (
            self.fresh("char.ok"),
            self.fresh("char.no"),
            self.fresh("char.join"),
        );
        let (k1, k2, k3) = (
            self.fresh("char.hi"),
            self.fresh("char.sur"),
            self.fresh("char.sur2"),
        );

        let against = |me: &mut Self, k: i128| -> Operand {
            me.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: Type::Int(27),
                    a: v.clone(),
                    b: Operand::Const(Type::Int(27), Bt::from_i128(k)),
                },
            )
        };

        let c = against(self, 0);
        self.br3(c, &bad, &k1, &k1);

        self.start(k1);
        let c = against(self, 0x10FFFF);
        self.br3(c, &k2, &k2, &bad);

        // Below the surrogates is a character; at or above needs the second
        // bound.
        self.start(k2);
        let c = against(self, 0xD800);
        self.br3(c, &good, &k3, &k3);

        self.start(k3);
        let c = against(self, 0xDFFF);
        self.br3(c, &bad, &bad, &good);

        self.start(good);
        let payload = [("0".to_string(), v)];
        self.build_variant_into(&slot, &ename, some, &payload, span)?;
        self.jump(&join);

        self.start(bad);
        self.build_variant_into(&slot, &ename, none, &[], span)?;
        self.jump(&join);

        self.start(join);
        Ok((Operand::Value(slot), opt))
    }

    // ------------------------------------------------------------ places

    /// The concrete name a literal's path head refers to.
    ///
    /// For a generic type the arguments are not written — `Pair { … }`, not
    /// `Pair<t27, t9> { … }` — so they come from the type this literal is
    /// expected to have, or, failing that, from the types of the fields it is
    /// given. Anything else is an inference failure, and says so.
    fn instantiate_head(
        &mut self,
        path: &ast::Path,
        fields: &[(String, ast::Expr)],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<String> {
        let head = path.segments[0].clone();
        // A type parameter in scope stands for a concrete type here, exactly
        // as it does in a written type: `U::from(x)` inside a generic body is
        // the concrete `U`'s `from` (Ch. 4 §2.7).
        if let Some(bound) = self.env.get(&head)
            && let Some(name) = nominal_name(bound)
        {
            return Ok(name);
        }
        // `Option::<t27>::None` — the arguments are written, so nothing has
        // to be inferred (Ch. 4 §2.3).
        if !path.targs.is_empty() {
            let args: Vec<Ty> = path
                .targs
                .iter()
                .map(|t| self.resolve(t))
                .collect::<R<_>>()?;
            let ty = self.types.instantiate(&head, &args, span)?;
            return Ok(nominal_name(&ty).expect("an instantiation is nominal"));
        }
        // `Vec` is a language item and has no declaration to read fields
        // from, so its instantiation comes from the type the context expects
        // — which is the only thing that could say what it holds.
        if head == "Vec" || head == "String" {
            if let Some(want) = expected
                && matches!(want, Ty::VecOf(_))
            {
                return Ok(nominal_name(want).expect("a `Vec` is nominal"));
            }
            if head == "String" {
                return Ok(mangle("Vec", &[Ty::Char]));
            }
        }
        let generic = self.types.generic_structs.get(&head).map(|s| {
            (
                s.generics.clone(),
                s.fields.clone(),
                s.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
            )
        });
        let (params, decl) = match generic {
            Some((g, f, _)) => (g, f),
            None => match self.types.generic_enums.get(&head) {
                Some(e) => {
                    let variant = path.segments.get(1).cloned().unwrap_or_default();
                    let Some(v) = e.variants.iter().find(|v| v.name == variant) else {
                        return err(span, format!("`{head}` has no variant `{variant}`"));
                    };
                    (e.generics.clone(), v.fields.clone())
                }
                None => return Ok(head),
            },
        };

        // The expected type already names an instantiation of this generic.
        if let Some(want) = expected
            && let Some(name) = nominal_name(want)
            && name.starts_with(&format!("{head}."))
        {
            return Ok(name);
        }

        // Otherwise, match the written field types against what was given —
        // and consult the *literals* last. An unsuffixed integer literal
        // defaults to `t27` (Ch. 1 §3), and a default that goes first pins a
        // type parameter another field would have determined: `0..n` with an
        // `n: taddr` is a `Range<t27>` whose end does not fit.
        let mut env: HashMap<String, Ty> = HashMap::new();
        let literal = |e: &ast::Expr| matches!(e, ast::Expr::Int(..));
        for pass in [false, true] {
            for (fname, fty) in decl.iter().map(|f| (&f.name, &f.ty)) {
                let Some((_, e)) = fields.iter().find(|(n, _)| n == fname) else {
                    continue;
                };
                if literal(e) != pass {
                    continue;
                }
                if let Some(got) = self.peek_ty(e)? {
                    unify(fty, &got, &params, &mut env);
                }
            }
        }
        let mut args = Vec::new();
        for p in &params {
            match env.get(p.name()) {
                Some(t) => args.push(t.clone()),
                None => {
                    return err(
                        span,
                        format!(
                            "cannot tell what `{}` is here; write the type of this value \
                             (Ch. 4 §2.3)",
                            p.name()
                        ),
                    );
                }
            }
        }
        let ty = self.types.instantiate(&head, &args, span)?;
        Ok(nominal_name(&ty).expect("an instantiation is nominal"))
    }

    /// The type of an expression, where that can be told without lowering it.
    ///
    /// Inference for a generic call needs the argument types before it knows
    /// what the parameters are, and lowering an argument twice is not an
    /// option, so this answers for the forms that carry their type plainly
    /// and gives up on the rest.
    fn peek_ty(&mut self, e: &ast::Expr) -> R<Option<Ty>> {
        use ast::Expr as E;
        if let Some(t) = self.type_of_place(e)? {
            return Ok(Some(t));
        }
        Ok(match e {
            E::Int(..) => Some(Ty::T27),
            E::Trit(..) => Some(Ty::Trit),
            E::Char(..) => Some(Ty::Char),
            E::Str(..) => Some(Ty::Ref(Box::new(Ty::Slice(Box::new(Ty::Char))), false)),
            E::Bool(..) => Some(Ty::Bool),
            E::Unit(_) => Some(Ty::Unit),
            // A struct or variant literal names its own type, and a generic
            // one works its arguments out from the fields it was given —
            // which is what makes `f(Wrapper { inner: x })` infer `f`'s
            // parameter as well as `Wrapper`'s (Ch. 4 §2.3).
            // A literal of a concrete nominal type carries its type plainly;
            // a generic one works its arguments out from the fields it was
            // given, which is what lets `f(Wrapper { inner: x })` infer both
            // `f`'s parameter and `Wrapper`'s (Ch. 4 §2.3).
            E::Aggregate(path, fields, span) => {
                // A peek that cannot tell says so; it must never be the thing
                // that reports an inference failure, because the caller has
                // other ways to find the answer. `Option::None` on its own is
                // the case: nothing in the literal says what `T` is, and the
                // expected type at the *call* does.
                match self.instantiate_head(path, fields, None, *span) {
                    Ok(head) if self.types.structs.borrow().contains_key(&head) => {
                        Some(Ty::Struct(head))
                    }
                    Ok(head) if self.types.enums.borrow().contains_key(&head) => {
                        Some(Ty::Enum(head))
                    }
                    _ => None,
                }
            }
            E::Borrow(inner, mutable, _) => {
                self.peek_ty(inner)?.map(|t| Ty::Ref(Box::new(t), *mutable))
            }
            E::Cast(_, t, _) => Some(self.resolve(t)?),
            E::Call(name, _, _, _, _) => self.sigs.borrow().get(name).map(|(_, r)| r.clone()),
            // Arithmetic keeps its operands' type; a comparison answers a
            // bool, and `<=>` a trit (Ch. 1 §5).
            E::Binary(op, a, b, _) => match *op {
                "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" => Some(Ty::Bool),
                "<=>" => Some(Ty::Trit),
                _ => match self.peek_ty(a)? {
                    Some(t) => Some(t),
                    None => self.peek_ty(b)?,
                },
            },
            E::Unary("!", _, _) => Some(Ty::Bool),
            E::Unary(_, a, _) => self.peek_ty(a)?,
            E::Block(b) => match &b.tail {
                Some(t) => self.peek_ty(t)?,
                None => Some(Ty::Unit),
            },
            E::If(_, then, _, _) => match &then.tail {
                Some(t) => self.peek_ty(t)?,
                None => Some(Ty::Unit),
            },
            // A method's result needs resolution to know, and resolution is
            // lowering — except where Ch. 1 and Ch. 5 fix the answer whatever
            // the receiver is. `len` is the one that matters: `0..v.len()`
            // would otherwise let the literal pin the range's parameter to
            // `t27` and then fail against a `taddr` end.
            E::Method(_, name, _, args, _) if name == "len" && args.is_empty() => Some(Ty::TAddr),
            E::Method(_, name, _, args, _) if name == "capacity" && args.is_empty() => {
                Some(Ty::TAddr)
            }
            E::Method(..) => None,
            _ => None,
        })
    }

    /// Resolve a written type in this function's generic environment.
    fn resolve(&self, t: &ast::Ty) -> R<Ty> {
        resolve_ty_env(t, self.types, &self.env)
    }

    /// `a < b` and its relatives on a nominal type (Ch. 4 §5.3.1).
    ///
    /// `==` and `!=` call `eq`; the ordering forms and `<=>` call `cmp`, and
    /// the two-way ones are the same projection of it that Ch. 1 §5 requires
    /// of the built-in comparison.
    fn compare_nominal(
        &mut self,
        op: &str,
        name: &str,
        va: Operand,
        vb: Operand,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let (trait_name, method) = match op {
            "==" | "!=" => ("Eq", "eq"),
            _ => ("Ord", "cmp"),
        };
        let key = format!("{name}.{method}");
        if !self.sigs.borrow().contains_key(&key) {
            return err(
                span,
                format!(
                    "`{op}` on {name} needs `{trait_name}`, which it does not implement; \
                     write `impl {trait_name} for {name}` or `#[derive({trait_name})]` \
                     (Ch. 4 §5.3)"
                ),
            );
        }
        let ret = if op == "==" || op == "!=" {
            Ty::Bool
        } else {
            Ty::Trit
        };
        let v = self.emit(
            "c",
            Type::Int(1),
            InstKind::Call {
                callee: Callee::Direct(key),
                args: vec![va, vb],
                ret: Some(Type::Int(1)),
            },
        );
        match op {
            "==" => Ok((v, ret)),
            // `eq` answers with a bool, so `!=` is its negation and not a
            // projection of a comparison trit.
            "!=" => {
                let k = |x: i128| Operand::Const(Type::Int(1), Bt::from_i128(x));
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: v,
                        ty: Type::Int(1),
                        neg: k(1),
                        zero: k(1),
                        pos: k(0),
                    },
                );
                Ok((r, Ty::Bool))
            }
            "<=>" => Ok((v, Ty::Trit)),
            _ => {
                let k = |x: i128| Operand::Const(Type::Int(1), Bt::from_i128(x));
                let (n, z, p) = match op {
                    "<" => (1, 0, 0),
                    "<=" => (1, 1, 0),
                    ">" => (0, 0, 1),
                    _ => (0, 1, 1),
                };
                let r = self.emit(
                    "b",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: v,
                        ty: Type::Int(1),
                        neg: k(n),
                        zero: k(z),
                        pos: k(p),
                    },
                );
                Ok((r, Ty::Bool))
            }
        }
    }

    /// A closure satisfies `impl Fn(A) -> R` when its signature is that one
    /// and it captures no more strongly than the bound allows (Ch. 4 §4.3).
    fn check_fn_bound(
        &self,
        ty: &Ty,
        key: &str,
        env: &HashMap<String, Ty>,
        callee: &str,
        param: &str,
        span: Span,
    ) -> R<()> {
        let (kind, want_params, want_ret) = self.fn_bounds[key].clone();
        let Some(name) = nominal_name(ty) else {
            return err(span, format!("`{ty}` is not a closure"));
        };
        let Some(info) = self.types.closures.borrow().get(&name).cloned() else {
            return err(
                span,
                format!(
                    "`{ty}` is not a closure, and `{callee}` wants one for `{param}`; \
                     a named type implementing `{}` is Ch. 4 §4.3, not implemented",
                    kind.name()
                ),
            );
        };
        // `Fn` ⊂ `FnMut` ⊂ `FnOnce`: a closure that writes a capture cannot
        // be passed where one that only reads is wanted.
        let rank = |k: ast::FnKind| match k {
            ast::FnKind::Fn => 0,
            ast::FnKind::FnMut => 1,
            ast::FnKind::FnOnce => 2,
        };
        if rank(info.kind) > rank(kind) {
            return err(
                span,
                format!(
                    "this closure is `{}` because it writes a capture, and `{callee}` \
                     wants `{}` for `{param}` (Ch. 4 §4.3)",
                    info.kind.name(),
                    kind.name()
                ),
            );
        }
        // Under the *call's* environment: a bound may name the call's own
        // type parameters, and `B` in `Fn(A) -> B` is exactly one of those.
        let mut scope = self.env.clone();
        scope.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let want_params: Vec<Ty> = want_params
            .iter()
            .map(|t| resolve_ty_env(t, self.types, &scope))
            .collect::<R<_>>()?;
        let want_ret = match &want_ret {
            None => Ty::Unit,
            Some(t) => resolve_ty_env(t, self.types, &scope)?,
        };
        if info.params != want_params || info.ret != want_ret {
            return err(
                span,
                format!(
                    "this closure does not have the signature `{callee}` wants for \
                     `{param}` (Ch. 4 §4.3)"
                ),
            );
        }
        Ok(())
    }

    /// Lower a closure expression (Ch. 4 §§4.1–4.4).
    ///
    /// It becomes two things: an anonymous struct holding one reference per
    /// captured place, and an ordinary function whose body is the closure's
    /// with every capture rewritten to a field of that struct. Neither is
    /// visible to a program, and everything downstream sees a struct and a
    /// call.
    /// The signature a parameter's `Fn@…` bound asks for, if it has one.
    fn fn_hint(
        &self,
        def: &ast::FnItem,
        want: &ast::Ty,
        env: &HashMap<String, Ty>,
    ) -> R<Option<(Vec<Ty>, Option<Ty>)>> {
        let ast::Ty::Name(pname, _) = want else {
            return Ok(None);
        };
        let Some(ast::GenericParam::Type { bounds, .. }) =
            def.generics.iter().find(|g| g.name() == pname)
        else {
            return Ok(None);
        };
        let Some(key) = bounds.iter().find_map(|b| b.name.strip_prefix("Fn@")) else {
            return Ok(None);
        };
        let (_, ps, r) = self.fn_bounds[key].clone();
        // Under the call's environment, not the caller's: a specialized
        // method's bound is written in the impl's parameters, and those live
        // in what the specialization settled (Ch. 4 §4.3).
        let mut scope = self.env.clone();
        scope.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let ps: Vec<Ty> = ps
            .iter()
            .map(|t| resolve_ty_env(t, self.types, &scope))
            .collect::<R<_>>()?;
        // The bound may leave the result a type parameter of its own —
        // `fn map<B, F: Fn(A) -> B>` says nothing about `B` except that it is
        // whatever the closure returns. So the hint carries no result there,
        // and the closure's body decides (Ch. 4 §4.1).
        let r = match &r {
            None => Some(Ty::Unit),
            Some(ast::Ty::Name(n, _)) if def.generics.iter().any(|g| g.name() == n) => None,
            Some(t) => Some(resolve_ty_env(t, self.types, &scope)?),
        };
        Ok(Some((ps, r)))
    }

    fn closure(
        &mut self,
        params: &[(String, Option<ast::Ty>)],
        ret: &Option<ast::Ty>,
        body: &ast::Expr,
        hint: Option<(Vec<Ty>, Option<Ty>)>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // The free names that are locals here are the captures (§4.4).
        let mut bound: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
        let mut free = Vec::new();
        free_names(body, &mut bound, &mut free);

        let mut captures: Vec<(String, Ty, bool)> = Vec::new();
        for n in &free {
            let Some(local) = self.lookup(n) else {
                continue;
            };
            // By shared reference if it only reads, by exclusive reference if
            // it writes. Capture by value is §4.3's `FnOnce`, which needs the
            // move analysis this milestone does not do.
            let mutable = writes_name(body, n);
            captures.push((n.clone(), local.ty, mutable));
        }

        let mut kind = ast::FnKind::Fn;
        if captures.iter().any(|(_, _, m)| *m) {
            kind = ast::FnKind::FnMut;
        }

        // Parameter and result types are written or nothing: inference from
        // the position a closure appears in is not implemented, and the
        // diagnostic says so rather than guessing.
        let mut param_tys: Vec<Ty> = Vec::new();
        for (i, (n, t)) in params.iter().enumerate() {
            match (t, hint.as_ref().and_then(|(ps, _)| ps.get(i))) {
                (Some(t), _) => param_tys.push(self.resolve(t)?),
                (None, Some(h)) => param_tys.push(h.clone()),
                (None, None) => {
                    return err(
                        span,
                        format!(
                            "the type of `{n}` cannot be told from here; write it, as \
                             `|{n}: t27|` (Ch. 4 §4.1)"
                        ),
                    );
                }
            }
        }
        let ret_ty = match (ret, hint.as_ref()) {
            (Some(t), _) => self.resolve(t)?,
            (None, Some((_, Some(r)))) => r.clone(),
            // Nothing in the context says, so read it off the body — which
            // works for the forms that carry their type plainly. A hint whose
            // result is a type parameter says nothing either, so it lands
            // here too.
            (None, Some((_, None)) | None) => {
                let saved = self.scopes.len();
                self.scopes.push(HashMap::new());
                for ((n, _), t) in params.iter().zip(&param_tys) {
                    self.declare(n, t.clone(), false);
                }
                let guess = self.peek_ty(body)?;
                self.scopes.truncate(saved);
                match guess {
                    Some(t) => t,
                    None => {
                        return err(
                            span,
                            "the result type of this closure cannot be told from here; \
                             write it, as `|x: t27| -> t27 { … }` (Ch. 4 §4.1)",
                        );
                    }
                }
            }
        };
        // Written back out, because the synthesized function is an ordinary
        // one and its parameters are written types like any other.
        let ps: Vec<ast::Named> = params
            .iter()
            .zip(&param_tys)
            .map(|((n, _), t)| ast::Named::new(n.clone(), ast::Ty::Name(t.to_string(), span)))
            .collect();

        self.counter += 1;
        // A dot, not a `#`: this name reaches TIR and the assembler, and
        // both accept a dot in an identifier while Trust accepts neither in
        // one of its own, so it cannot collide with anything a program wrote.
        let name = format!("{}.closure{}", self.name, self.counter);

        // The capture struct: one reference per capture, in the order found.
        let fields: Vec<(String, Ty)> = captures
            .iter()
            .map(|(n, t, m)| (n.clone(), Ty::Ref(Box::new(t.clone()), *m)))
            .collect();
        let ty = self.types.register_struct(&name, fields.clone());

        // The body, as an ordinary function taking the captures by reference.
        let mut body = body.clone();
        let capture_names: Vec<String> = captures.iter().map(|(n, ..)| n.clone()).collect();
        rewrite_captures(&mut body, &capture_names);
        let mut call_params = vec![ast::Named::new(
            "self",
            ast::Ty::Ref(Box::new(ast::Ty::Name(name.clone(), span)), false, span),
        )];
        call_params.extend(ps.clone());
        let call = format!("{name}.call");
        let item = ast::FnItem {
            public: true,
            requires: Vec::new(),
            name: call.clone(),
            name_span: span,
            generics: Vec::new(),
            params: call_params,
            ret: Some(ast::Ty::Name(ret_ty.to_string(), span)),
            body: Some(ast::Block {
                stmts: Vec::new(),
                tail: Some(Box::new(body)),
                span,
            }),
            span,
        };

        let mut sig_params = vec![Ty::Ref(Box::new(ty.clone()), false)];
        sig_params.extend(param_tys.clone());
        self.sigs
            .borrow_mut()
            .insert(call.clone(), (sig_params, ret_ty.clone()));
        self.extra_fns.borrow_mut().push(item);
        self.types.closures.borrow_mut().insert(
            name.clone(),
            ClosureInfo {
                call,
                params: param_tys,
                ret: ret_ty,
                kind,
            },
        );

        // The value: a struct of borrows of the captured places.
        let slot = self.temp_slot(&ty);
        for ((n, _, mutable), (_, ft, off)) in captures.iter().zip(self.types.fields(&ty)) {
            let place = ast::Expr::Path(n.clone(), span);
            if let Some(path) = self.path_of(&place) {
                self.check_access(&path, Access::Borrow(*mutable), span)?;
                // The loan lives as long as the closure does, which for a
                // closure that cannot escape is the rest of the scope.
                // The loan lives as long as the closure does. A closure used
                // as an argument dies with the statement; one bound by `let`
                // is extended to its last use, below.
                self.add_loan(path, *mutable, span);
            }
            let (addr, _) = self.place(&place, span)?;
            self.store_at(&slot, off, &ft, addr, span)?;
        }
        Ok((Operand::Value(slot), ty))
    }

    /// Call a method through a trait object's vtable (Ch. 4 §§3.1, 3.3).
    fn dyn_call(
        &mut self,
        fat: Operand,
        trait_name: &str,
        name: &str,
        args: &[ast::Expr],
        span: Span,
    ) -> R<(Operand, Ty)> {
        if !self.traits.contains_key(trait_name) {
            return err(span, format!("`{trait_name}` is not a trait in scope"));
        }
        let methods = object_methods(self.traits, trait_name, &mut Vec::new());
        let Some(index) = methods.iter().position(|m| m.name == name) else {
            return err(
                span,
                format!(
                    "`dyn {trait_name}` has no method `{name}`; only the trait's are \
                         reachable through an object (Ch. 4 §3.1)"
                ),
            );
        };
        let m = &methods[index];
        object_safe(m, trait_name)?;

        // The signature, with `Self` erased to the data pointer.
        let params: Vec<Ty> = m.params[1..]
            .iter()
            .map(|p| self.resolve(&p.ty))
            .collect::<R<_>>()?;
        let ret = match &m.ret {
            None => Ty::Unit,
            Some(t) => self.resolve(t)?,
        };
        if params.len() != args.len() {
            return err(
                span,
                format!(
                    "`{trait_name}::{name}` takes {} argument(s), {} given",
                    params.len(),
                    args.len()
                ),
            );
        }

        // Data pointer, then vtable pointer (§3.2); the method slots start
        // after size, align and drop (§3.3).
        let data = self.load_ptr(fat.clone());
        let vt_at = self.offset(fat, 3);
        let vt = self.load_ptr(vt_at);
        let slot = self.offset(vt, 9 + 3 * index as i128);
        let f = self.load_ptr(slot);

        let mut values = vec![data];
        let out = ret.is_aggregate().then(|| self.temp_slot(&ret));
        if let Some(slot) = &out {
            values.insert(0, Operand::Value(slot.clone()));
        }
        for (arg, want) in args.iter().zip(&params) {
            let (v, got) = self.expr(arg, Some(want))?;
            self.check(&got, want, arg.span(), "argument")?;
            values.push(v);
        }
        let kind = InstKind::Call {
            callee: Callee::Indirect(f),
            args: values,
            ret: if ret == Ty::Unit || ret.is_aggregate() {
                None
            } else {
                Some(ret.tir())
            },
        };
        match out {
            Some(slot) => {
                self.push(Inst {
                    results: Vec::new(),
                    kind,
                });
                Ok((Operand::Value(slot), ret))
            }
            None if ret == Ty::Unit => {
                self.push(Inst {
                    results: Vec::new(),
                    kind,
                });
                Ok((unit(), Ty::Unit))
            }
            None => {
                let v = self.emit("r", ret.tir(), kind);
                Ok((v, ret))
            }
        }
    }

    /// The global holding a string literal's characters, one word each.
    ///
    /// Identical literals share one global. That is not an optimization the
    /// program can observe — a `&'static str` has no identity beyond what it
    /// points at, and nothing in this language compares addresses — so it is
    /// free.
    fn string_data(&mut self, chars: &[i128]) -> String {
        let key: Vec<i128> = chars.to_vec();
        if let Some(name) = self.strings.borrow().get(&key) {
            return name.clone();
        }
        let name = format!("str.{}", self.strings.borrow().len());
        let mut trytes = Vec::with_capacity(chars.len() * 3);
        for c in chars {
            let v = Bt::from_i128(*c);
            for i in 0..3 {
                trytes.push(InitItem::Tryte(v.shr(i * 9).wrap_to(9)));
            }
        }
        self.data.borrow_mut().push(ir::Global {
            name: name.clone(),
            trytes: trytes.len() as u32,
            init: Some(trytes),
        });
        self.strings.borrow_mut().insert(key, name.clone());
        name
    }

    /// The global holding the vtable for this (type, trait) pair, building
    /// it if this is the first coercion to it (Ch. 4 §3.3).
    ///
    /// Layout: size, align, drop, then one address per object-safe method in
    /// declaration order. `size` and `align` are here because a future
    /// `Box<dyn Trait>` needs them to free what it points at, and adding a
    /// slot later would change the layout of something programs will have
    /// been written against.
    fn vtable_for(&mut self, concrete: &Ty, trait_name: &str, span: Span) -> R<String> {
        let name = nominal_name(concrete)
            .ok_or_else(|| one_err(span, format!("{concrete} cannot be a trait object")))?;
        let symbol = format!("vt.{name}.{trait_name}");
        if self
            .vtables
            .borrow()
            .iter()
            .any(|(_, _, g)| g.name == symbol)
        {
            return Ok(symbol);
        }
        let Some(decl) = self.traits.get(trait_name).cloned() else {
            return err(span, format!("`{trait_name}` is not a trait in scope"));
        };
        let _ = &decl;
        if !self
            .impls
            .pairs
            .contains(&(name.clone(), trait_name.to_string()))
        {
            return err(
                span,
                format!(
                    "`{name}` does not implement `{trait_name}`, so it is not a `dyn {trait_name}`"
                ),
            );
        }

        let word = |v: i128| {
            let b = Bt::from_i128(v);
            (0..3)
                .map(|i| InitItem::Tryte(b.shr(i * 9).wrap_to(9)))
                .collect::<Vec<_>>()
        };
        let mut items = word(self.types.size(concrete));
        items.extend(word(self.types.layout(concrete).align as i128));
        // A drop slot of 0 is unambiguous: 0 is not the address of anything
        // (Ch. 3 §2.4).
        if self.types.needs_drop(concrete) {
            items.push(InitItem::Addr(format!("drop.{name}")));
        } else {
            items.extend(word(0));
        }
        for m in &object_methods(self.traits, trait_name, &mut Vec::new()) {
            object_safe(m, trait_name)?;
            items.push(InitItem::Addr(format!("{name}.{}", m.name)));
        }

        let trytes = items.iter().map(|i| i.trytes()).sum();
        self.vtables.borrow_mut().push((
            name,
            trait_name.to_string(),
            ir::Global {
                name: symbol.clone(),
                trytes,
                init: Some(items),
            },
        ));
        Ok(symbol)
    }

    /// Coerce `&Concrete` to `&dyn Trait`: a fat pointer of the data address
    /// and the vtable's (Ch. 4 §3.2).
    /// Coerce `&Vec<T>` to `&[T]`: the allocation and the length, which are
    /// the `Vec`'s first two words and a slice's only two (Ch. 5 §2.6).
    ///
    /// The capacity is left behind, which is the whole of the conversion: a
    /// slice may read what is there and a `Vec` may grow, and only one of
    /// those needs to know how much room is left.
    fn coerce_vec(
        &mut self,
        v: Operand,
        from: &Ty,
        to: &Ty,
        span: Span,
    ) -> R<Option<(Operand, Ty)>> {
        let (Ty::Ref(have, m), Ty::Ref(want, wm)) = (from, to) else {
            return Ok(None);
        };
        let (Ty::VecOf(elem), Ty::Slice(wanted)) = (&**have, &**want) else {
            return Ok(None);
        };
        if elem != wanted || (*wm && !*m) {
            return Ok(None);
        }
        let fat = Ty::Ref(Box::new(Ty::Slice(elem.clone())), *wm);
        let slot = self.temp_slot(&fat);
        let p = self.load_ptr(v.clone());
        self.store_ptr(Operand::Value(slot.clone()), p);
        let len_at = self.offset(v, 3);
        let len = self.load_from(len_at, &Ty::TAddr);
        self.store_at(&slot, 3, &Ty::TAddr, len, span)?;
        Ok(Some((Operand::Value(slot), fat)))
    }

    fn coerce_dyn(
        &mut self,
        v: Operand,
        from: &Ty,
        to: &Ty,
        span: Span,
    ) -> R<Option<(Operand, Ty)>> {
        let (Ty::Ref(target, m), Ty::Ref(want, _)) = (from, to) else {
            return Ok(None);
        };
        let Ty::Dyn(trait_name) = &**want else {
            return Ok(None);
        };
        if target.is_unsized() {
            return Ok(None);
        }
        let symbol = self.vtable_for(target, trait_name, span)?;
        let fat = Ty::Ref(Box::new(Ty::Dyn(trait_name.clone())), *m);
        let slot = self.temp_slot(&fat);
        let at = Operand::Value(slot.clone());
        self.store_ptr(at, v);
        let second = self.offset(Operand::Value(slot.clone()), 3);
        self.store_ptr(second, Operand::Global(symbol));
        Ok(Some((Operand::Value(slot), fat)))
    }

    /// The function a method call resolves to.
    ///
    /// A concrete impl gives the name directly. A generic impl gives a
    /// generic function keyed by the base type, which is instantiated here
    /// with the arguments the receiver's own instantiation was made with —
    /// recovered from the table, since a mangled name cannot be read back.
    /// Apply a blanket impl, if one provides this method for this type.
    ///
    /// The rule's parameters are bound the way any generic call binds them:
    /// the receiver's type against the `self` parameter as written, and the
    /// expected type against the result. So `c.into()` with `c: Celsius` and
    /// a `t27` wanted binds `T` from the receiver and `U` from the context,
    /// and the rule's own bound — `U: From<T>` — is then an ordinary check.
    ///
    /// The instantiated body is a generic function like any other, which is
    /// why this needs no new machinery: a blanket impl differs from every
    /// other impl only in being *found* by checking a condition rather than
    /// by looking up a name.
    fn blanket_method(
        &mut self,
        recv_ty: &Ty,
        method: &str,
        expected: Option<&Ty>,
        span: Span,
    ) -> R<Option<String>> {
        let rules: Vec<Blanket> = self
            .impls
            .blankets
            .iter()
            .filter(|b| b.methods.contains_key(method))
            .cloned()
            .collect();
        let mut found: Vec<String> = Vec::new();
        let mut why: Vec<Error> = Vec::new();
        for rule in &rules {
            let key = rule.methods[method].clone();
            let Some(def) = self.generic_fns.get(&key).cloned() else {
                continue;
            };
            let mut env: HashMap<String, Ty> = HashMap::new();
            env.insert(rule.self_param.clone(), recv_ty.clone());
            if let (Some(written), Some(want)) = (&def.ret, expected) {
                unify(written, want, &def.generics, &mut env);
            }
            // Every parameter has to come out bound, or the rule says
            // nothing about this call.
            if def.generics.iter().any(|p| !env.contains_key(p.name())) {
                why.push(SyntaxError {
                    span,
                    message: format!(
                        "`{}` applies to `{recv_ty}` only where the result type is \
                         known; write it (Ch. 4 §5.6)",
                        rule.trait_name
                    ),
                });
                continue;
            }
            let mut ok = true;
            for p in &def.generics {
                let ast::GenericParam::Type {
                    name: pname,
                    bounds,
                } = p
                else {
                    continue;
                };
                let ty = env[pname].clone();
                for b in bounds {
                    if let Err(e) = self.check_bound_in(&ty, b, &env, &rule.trait_name, pname, span)
                    {
                        why.push(e);
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    break;
                }
            }
            if ok {
                found.push(self.instantiate_with(&key, env, span)?);
            }
        }
        match found.len() {
            0 => match why.pop() {
                Some(e) => Err(e),
                None => Ok(None),
            },
            1 => Ok(Some(found.remove(0))),
            _ => err(
                span,
                format!("more than one rule gives `{recv_ty}` a `{method}` (Ch. 4 §1.8)"),
            ),
        }
    }

    /// Which function a method of a *parameterized* trait means.
    ///
    /// One type may implement such a trait many times, so the name alone does
    /// not say which — `t27::from` could be `From<t9>`'s or `From<bool>`'s.
    /// The arguments say: each candidate's parameter types are compared
    /// against what the call gives, and exactly one must fit.
    ///
    /// `None` means no parameterized trait provides this method, and the
    /// ordinary `Type.method` lookup is the answer.
    fn trait_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[ast::Expr],
        span: Span,
    ) -> R<Option<String>> {
        let Some(keys) = self
            .impls
            .by_method
            .get(&(type_name.to_string(), method.to_string()))
            .cloned()
        else {
            return Ok(None);
        };
        if keys.len() == 1 {
            return Ok(Some(keys[0].clone()));
        }
        // What the call actually passes, as far as it can be known without
        // lowering it — which for a written value is its type.
        let mut given = Vec::new();
        for a in args {
            given.push(self.peek_ty(a)?);
        }
        let mut fits: Vec<String> = Vec::new();
        for key in &keys {
            let Some((params, _)) = self.sigs.borrow().get(key).cloned() else {
                continue;
            };
            if params.len() != given.len() {
                continue;
            }
            if params
                .iter()
                .zip(&given)
                .all(|(want, got)| got.as_ref().is_none_or(|g| g == want))
            {
                fits.push(key.clone());
            }
        }
        match fits.len() {
            1 => Ok(Some(fits[0].clone())),
            0 => err(
                span,
                format!(
                    "no implementation of `{method}` for `{type_name}` takes these \
                     arguments; there {} (Ch. 4 §1.7)",
                    describe_candidates(&keys)
                ),
            ),
            _ => err(
                span,
                format!(
                    "which `{type_name}::{method}` is meant is not decided by these \
                     arguments; there {} (Ch. 4 §1.7)",
                    describe_candidates(&fits)
                ),
            ),
        }
    }

    fn method_key(&mut self, type_name: &str, name: &str, span: Span) -> R<String> {
        let concrete = format!("{type_name}.{name}");
        if self.sigs.borrow().contains_key(&concrete) {
            return Ok(concrete);
        }
        let Some((base, args)) = self.types.instantiations.borrow().get(type_name).cloned() else {
            return Ok(concrete);
        };
        let generic = format!("{base}.{name}");
        let Some(def) = self.generic_fns.get(&generic).cloned() else {
            return Ok(concrete);
        };
        if def.generics.len() < args.len() {
            return err(
                span,
                format!("`{base}::{name}` does not apply to `{type_name}`"),
            );
        }
        let mut env: HashMap<String, Ty> = def
            .generics
            .iter()
            .map(|p| p.name().to_string())
            .zip(args.iter().cloned())
            .collect();
        // The parameters the self type did not name. Each has to be settled
        // by a bound, and the only bound that settles one is `Fn`: given the
        // closure that was passed, its result type *is* the parameter its
        // signature named (Ch. 4 §4.3). This is what lets `Map`'s `Item` be
        // the closure's result instead of a fixed type.
        // The parameters after the self type's arguments come in two kinds.
        // A `Fn` bound over the impl's own settles the first kind — `B` in
        // `impl<I, B, F: Fn(I::Item) -> B>`. What is left belongs to the
        // *method*, and only the call's arguments can settle that.
        let mut own: Vec<ast::GenericParam> = Vec::new();
        for p in def.generics.iter().skip(args.len()) {
            let want = p.name().to_string();
            if env.contains_key(&want) {
                continue;
            }
            if own.is_empty()
                && let Some(t) = self.solve_from_fn_bounds(&want, &def.generics, &env)
            {
                env.insert(want, t);
                continue;
            }
            own.push(p.clone());
        }
        if own.is_empty() {
            return self.instantiate_with(&generic, env, span);
        }
        // Two stages, because instantiation is one step and this needs two:
        // the impl's half is settled here and the method goes back into the
        // queue as an ordinary generic function of what is left.
        let spec = mangle(&generic, &args);
        if !self.specials.borrow().contains_key(&spec) {
            let mut d = def.clone();
            d.generics = own;
            d.name = spec.clone();
            self.specials
                .borrow_mut()
                .insert(spec.clone(), Special { def: d, env });
        }
        Ok(spec)
    }

    /// A generic function's definition, whether written or specialized.
    fn generic_def(&self, name: &str) -> Option<ast::FnItem> {
        self.generic_fns
            .get(name)
            .cloned()
            .or_else(|| self.specials.borrow().get(name).map(|s| s.def.clone()))
    }

    /// What a specialized method's impl already settled, or nothing.
    fn special_env(&self, name: &str) -> HashMap<String, Ty> {
        self.specials
            .borrow()
            .get(name)
            .map(|s| s.env.clone())
            .unwrap_or_default()
    }

    /// Settle a type parameter from another parameter's `Fn` bound.
    ///
    /// `F: Fn(A) -> B` with a known `F` says what `B` is, because a closure
    /// has one signature and it is recorded (Ch. 4 §4.3).
    fn solve_from_fn_bounds(
        &self,
        want: &str,
        generics: &[ast::GenericParam],
        env: &HashMap<String, Ty>,
    ) -> Option<Ty> {
        for g in generics {
            let ast::GenericParam::Type { name, bounds } = g else {
                continue;
            };
            let Some(cname) = env.get(name).and_then(nominal_name) else {
                continue;
            };
            let Some(info) = self.types.closures.borrow().get(&cname).cloned() else {
                continue;
            };
            for b in bounds {
                let Some(key) = b.name.strip_prefix("Fn@") else {
                    continue;
                };
                let Some((_, params, ret)) = self.fn_bounds.get(key).cloned() else {
                    continue;
                };
                if let Some(ast::Ty::Name(n, _)) = &ret
                    && n == want
                {
                    return Some(info.ret.clone());
                }
                for (i, pt) in params.iter().enumerate() {
                    if let ast::Ty::Name(n, _) = pt
                        && n == want
                        && let Some(t) = info.params.get(i)
                    {
                        return Some(t.clone());
                    }
                }
            }
        }
        None
    }

    /// A method call that resolves to an impl block (Ch. 4 §1.3).
    ///
    /// Resolution is a desugaring: `p.area()` becomes `Point.area(&p)`, with
    /// as many dereferences inserted as the receiver's type needs (Ch. 3
    /// §2.3) and the borrow the receiver form asks for. Argument checking,
    /// loans, moves and drops then all come from the ordinary call path,
    /// which is the whole reason for doing it this way.
    fn user_method(
        &mut self,
        recv: &ast::Expr,
        name: &str,
        args: &[ast::Expr],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // A place receiver keeps its identity, so `&mut self` writes through
        // to the caller's value rather than to a copy.
        // A receiver that is not a place has to be lowered to be typed, and
        // it is lowered again below to be passed. That is one evaluation too
        // many the moment it *contains* a closure — `c.map(f).count()` would
        // make two closure types and then complain that the receiver is
        // neither — so a lowered non-place receiver is bound to a name and
        // the rest of this refers to that.
        let mut recv = recv.clone();
        let (recv_ty, place) = match self.type_of_place(&recv)? {
            Some(t) => (t, true),
            None => {
                let (v, ty) = self.expr(&recv, None)?;
                if let Operand::Value(slot) = v {
                    let bound = self.bind_existing(slot, ty.clone());
                    recv = ast::Expr::Path(bound, span);
                }
                (ty, false)
            }
        };
        let recv = &recv;

        // A call on a trait object is one indirect call through its vtable
        // (Ch. 4 §3.1). Nothing else about the receiver is known.
        if let Ty::Ref(inner, _) = &recv_ty
            && let Ty::Dyn(trait_name) = &**inner
        {
            let trait_name = trait_name.clone();
            let (fat, _) = self.expr(recv, None)?;
            return self.dyn_call(fat, &trait_name, name, args, span);
        }

        let mut base = recv_ty.clone();
        let mut derefs = 0;
        while let Ty::Ref(inner, _) = base.clone() {
            if inner.is_unsized() {
                break;
            }
            base = *inner;
            derefs += 1;
        }
        // A method on an unsized type takes `&self`, and the receiver is
        // already that reference: there is nothing to dereference and nothing
        // to borrow, because the fat pointer *is* the value (Ch. 5 §1.3).
        let by_reference = matches!(&base, Ty::Ref(inner, _) if inner.is_unsized());
        let Some(type_name) = nominal_name(&base).or_else(|| match &base {
            Ty::Ref(inner, _) => nominal_name(inner),
            _ => None,
        }) else {
            return err(span, format!("{base} has no methods"));
        };
        let mut key = self.method_key(&type_name, name, span)?;
        // A rule that holds for every type satisfying a bound is the last
        // thing tried, so a type's own method always wins (Ch. 4 §5.6).
        if !self.sigs.borrow().contains_key(&key)
            && let Some(k) = self.blanket_method(&base, name, expected, span)?
        {
            key = k;
        }
        // A method with type parameters of its own — one taking `impl Fn(…)`
        // is the common case — is a generic function, and generic functions
        // are instantiated at the call site rather than looked up.
        let generic = self.generic_def(&key);
        // A specialized method's receiver is written in the *impl's*
        // parameters, which the specialization already settled.
        let mut scope = self.env.clone();
        scope.extend(self.special_env(&key));
        let Some((params, _)) = self.sigs.borrow().get(&key).cloned().or_else(|| {
            // Only the receiver's type is wanted here, to decide whether
            // to borrow it. The rest may name the method's own
            // parameters — `impl Fn(…)` becomes one — which nothing can
            // resolve until the call site says what they are.
            let def = generic.as_ref()?;
            let written = &def.params.first()?.ty;
            let recv = resolve_ty_env(written, self.types, &scope).ok()?;
            Some((vec![recv], Ty::Unit))
        }) else {
            return err(
                span,
                format!("{base} has no method `{name}`, and neither does Ch. 1"),
            );
        };
        let Some(self_ty) = params.first().cloned() else {
            return err(
                span,
                format!(
                    "`{type_name}::{name}` takes no `self`, so it is called \
                     `{type_name}::{name}(…)` and not on a receiver (Ch. 4 §1.4)"
                ),
            );
        };

        if !place {
            // The receiver is a value, so it lives in a temporary and there
            // is nothing to borrow-check.
            let (v, ty) = self.expr(recv, None)?;
            let mut v = v;
            for _ in 0..derefs {
                v = self.load_ptr(v);
            }
            let arg = match &self_ty {
                Ty::Ref(..) if !base.is_aggregate() => {
                    let slot = self.temp_slot(&base);
                    self.store_at(&slot, 0, &base, v, span)?;
                    (Operand::Value(slot), self_ty.clone())
                }
                // An aggregate is already an address, which is what a
                // reference to it is.
                Ty::Ref(..) => (v, self_ty.clone()),
                _ => (v, ty),
            };
            // A generic method is instantiated from the *written* arguments,
            // so a receiver that is already a value has to become a name the
            // instantiation can read — the same binding a closure argument
            // gets (Ch. 4 §2.3). It is bound at its own type and borrowed
            // afterwards if the method wants a reference; binding it at the
            // *reference's* type would say the slot holds an address when it
            // holds the value.
            if generic.is_some() {
                let Operand::Value(slot) = arg.0 else {
                    return err(span, "a receiver must have a slot");
                };
                let bound = self.bind_existing(slot, base.clone());
                let named = ast::Expr::Path(bound, span);
                let receiver = match &self_ty {
                    Ty::Ref(_, mutable) => ast::Expr::Borrow(Box::new(named), *mutable, span),
                    _ => named,
                };
                let mut full = vec![receiver];
                full.extend(args.iter().cloned());
                let (k, full) = self.instantiate_fn(&key, &[], &full, expected, span)?;
                return self.call_key(&k, Vec::new(), &full, span);
            }
            return self.call_key(&key, vec![arg], args, span);
        }

        let mut receiver = recv.clone();
        for _ in 0..derefs {
            receiver = ast::Expr::Deref(Box::new(receiver), span);
        }
        let receiver = match &self_ty {
            Ty::Ref(..) if by_reference => receiver,
            Ty::Ref(_, mutable) => ast::Expr::Borrow(Box::new(receiver), *mutable, span),
            _ => receiver,
        };
        let mut full = vec![receiver];
        full.extend(args.iter().cloned());
        if generic.is_some() {
            let (k, full) = self.instantiate_fn(&key, &[], &full, expected, span)?;
            return self.call_key(&k, Vec::new(), &full, span);
        }
        self.call_key(&key, Vec::new(), &full, span)
    }

    /// The address of a place, and its type (Ch. 3 §1.3). A place is a local,
    /// a field of a place, an element of a place, or a dereference.
    /// A place, and its type — recorded like an expression's, because it is
    /// one. `p` in `p.x` never reaches `expr`, and an editor asking what `p`
    /// is has to be told the same answer either way.
    fn place(&mut self, e: &ast::Expr, span: Span) -> R<(Operand, Ty)> {
        let out = self.place_inner(e, span)?;
        if let Some(sink) = self.noted {
            sink.borrow_mut().note(e.span(), &out.1);
        }
        Ok(out)
    }

    fn place_inner(&mut self, e: &ast::Expr, span: Span) -> R<(Operand, Ty)> {
        match e {
            ast::Expr::Path(name, l) => {
                if let Some(local) = self.lookup(name) {
                    return Ok((Operand::Value(local.slot), local.ty));
                }
                if let Some(Global::Array(sym, ty)) = self.globals.get(name) {
                    return Ok((Operand::Global(sym.clone()), ty.clone()));
                }
                err(*l, format!("`{name}` is not a place"))
            }

            ast::Expr::Field(base, name, l) => {
                let (addr, bt) = self.place_or_deref(base, *l)?;
                let fields = self.types.fields(&bt);
                let Some((_, ft, off)) = fields.into_iter().find(|(n, _, _)| n == name) else {
                    return err(*l, format!("{bt} has no field `{name}`"));
                };
                Ok((self.offset(addr, off), ft))
            }

            ast::Expr::Index(base, index, l) => {
                let (addr, bt) = self.place_or_deref(base, *l)?;
                let (elem, len) = match &bt {
                    Ty::Array(elem, n) => ((**elem).clone(), Length::Fixed(*n)),
                    Ty::Ref(inner, _) if matches!(**inner, Ty::Slice(_)) => {
                        let Ty::Slice(elem) = &**inner else {
                            unreachable!()
                        };
                        ((**elem).clone(), Length::Dynamic)
                    }
                    // A `Vec` is a pointer and a length like a fat reference,
                    // with a capacity after them that indexing never sees
                    // (Ch. 5 §2.6).
                    Ty::VecOf(elem) => ((**elem).clone(), Length::Dynamic),
                    Ty::Ref(inner, _) if matches!(**inner, Ty::VecOf(_)) => {
                        let Ty::VecOf(elem) = &**inner else {
                            unreachable!()
                        };
                        ((**elem).clone(), Length::Dynamic)
                    }
                    other => return err(*l, format!("{other} cannot be indexed")),
                };
                let (base_addr, len_value) = match len {
                    Length::Fixed(n) => (addr, Operand::Const(Type::Int(27), Bt::from_i128(n))),
                    // A fat reference: the pointer, then the length.
                    Length::Dynamic => {
                        let p = self.load_ptr(addr.clone());
                        let l2 = self.offset(addr, 3);
                        let n = self.load_from(l2, &Ty::TAddr);
                        (p, n)
                    }
                };
                let (idx, it) = self.expr(index, Some(&Ty::TAddr))?;
                self.check(&it, &Ty::TAddr, *l, "index")?;
                let addr = self.checked_element(base_addr, idx, len_value, &elem, *l)?;
                Ok((addr, elem))
            }

            ast::Expr::Deref(inner, l) => {
                // Dereferencing a `Box` reads the pointer, not the value it
                // owns: `*b` does not move `b`, any more than `*r` moves `r`.
                // A reference is `Copy` and so this never mattered before.
                if let Some(Ty::Boxed(target)) = self.type_of_place(inner)? {
                    let (at, _) = self.place(inner, *l)?;
                    let p = self.load_ptr(at);
                    return Ok((p, *target));
                }
                let (v, ty) = self.expr(inner, None)?;
                let (Ty::Ref(target, _) | Ty::Boxed(target)) = ty else {
                    return err(*l, format!("`*` applies to a reference, not {ty}"));
                };
                Ok((v, *target))
            }

            _ => err(span, "this expression is not a place"),
        }
    }

    /// A place, dereferencing automatically through a reference — which is
    /// what makes `r.x` mean `(*r).x` (Ch. 3 §2.3).
    fn place_or_deref(&mut self, e: &ast::Expr, span: Span) -> R<(Operand, Ty)> {
        let (mut addr, mut ty) = self.place(e, span)?;
        while let Ty::Ref(target, _) | Ty::Boxed(target) = ty.clone() {
            if target.is_unsized() {
                break; // a fat reference is indexed, not dereferenced
            }
            addr = self.load_ptr(addr);
            ty = *target;
        }
        Ok((addr, ty))
    }

    /// Bounds-check an index and produce the element's address (Ch. 3 §5.5).
    ///
    /// Two comparisons rather than the fused single branch: the fusion needs
    /// `len − 1 − i`, which can overflow for an extreme index, and Ch. 2 §3's
    /// suggested fusion is incorrect outright.
    fn checked_element(
        &mut self,
        base: Operand,
        idx: Operand,
        len: Operand,
        elem: &Ty,
        span: Span,
    ) -> R<Operand> {
        let word = Type::Int(27);
        let zero = Operand::Const(word, Bt::ZERO);

        let low = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: word,
                a: idx.clone(),
                b: zero,
            },
        );
        let (ok_low, fault) = (self.fresh("idx.lo"), self.fresh("idx.fault"));
        self.br3(low, &fault, &ok_low, &ok_low);

        self.start(fault.clone());
        self.finish(Terminator::Trap(FaultCode::Trap));

        self.start(ok_low);
        let high = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: word,
                a: idx.clone(),
                b: len,
            },
        );
        let ok = self.fresh("idx.ok");
        self.br3(high, &ok, &fault, &fault);

        self.start(ok);
        let size = self.types.size(elem);
        let scale = self.emit(
            "a",
            word,
            InstKind::Flavored {
                op: FlavoredOp::Mul,
                flavor: Flavor::Trap,
                ty: word,
                a: idx,
                b: Operand::Const(word, Bt::from_i128(size)),
            },
        );
        let _ = span;
        Ok(self.emit("p", Type::Ptr, InstKind::Offset { p: base, d: scale }))
    }

    // -------------------------------------------------------- aggregates

    /// `base + offset` as an address.
    fn offset(&mut self, base: Operand, off: i128) -> Operand {
        if off == 0 {
            return base;
        }
        let d = Operand::Const(Type::Int(27), Bt::from_i128(off));
        self.emit("p", Type::Ptr, InstKind::Offset { p: base, d })
    }

    /// Load a pointer, keeping its provenance (TIR §5).
    fn load_ptr(&mut self, p: Operand) -> Operand {
        self.emit("p", Type::Ptr, InstKind::Load { ty: Type::Ptr, p })
    }

    /// Store a pointer.
    fn store_ptr(&mut self, at: Operand, v: Operand) {
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: Type::Ptr,
                v,
                p: at,
            },
        });
    }

    /// Read a value of type `ty` from an address. An aggregate is *named* by
    /// its address, so reading one is the address itself.
    fn load_from(&mut self, p: Operand, ty: &Ty) -> Operand {
        if ty.is_aggregate() {
            return p;
        }
        self.emit("v", ty.tir(), InstKind::Load { ty: ty.tir(), p })
    }

    /// Write a value into `slot + off`. An aggregate is copied.
    fn store_at(&mut self, slot: &str, off: i128, ty: &Ty, v: Operand, span: Span) -> R<()> {
        let dst = self.offset(Operand::Value(slot.to_string()), off);
        if ty.is_aggregate() {
            self.copy_typed(dst, v, ty, span)?;
        } else {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: ty.tir(),
                    v,
                    p: dst,
                },
            });
        }
        Ok(())
    }

    /// Copy a value of known type from one address to another.
    ///
    /// Field by field rather than tryte by tryte, because a pointer is not a
    /// number: it carries provenance (TIR §5), and a byte-wise copy would
    /// deliver the address without it. The type says where the pointers are,
    /// so the copy follows the type.
    fn copy_typed(&mut self, dst: Operand, src: Operand, ty: &Ty, span: Span) -> R<()> {
        match ty {
            // A thin reference is a pointer, and is copied as one.
            Ty::Ref(t, _) if !t.is_unsized() => {
                let v = self.load_ptr(src);
                self.store_ptr(dst, v);
                Ok(())
            }

            // A fat reference is a pointer and a length (Ch. 3 §5.2).
            Ty::Ref(..) => {
                let p = self.load_ptr(src.clone());
                self.store_ptr(dst.clone(), p);
                let from = self.offset(src, 3);
                let to = self.offset(dst, 3);
                let n = self.load_from(from, &Ty::TAddr);
                self.store_scalar(to, &Ty::TAddr, n);
                Ok(())
            }

            Ty::Array(elem, n) => {
                let size = self.types.size(elem);
                let (elem, n) = ((**elem).clone(), *n);
                for i in 0..n {
                    let from = self.offset(src.clone(), i * size);
                    let to = self.offset(dst.clone(), i * size);
                    self.copy_typed(to, from, &elem, span)?;
                }
                Ok(())
            }

            // A `Vec` is an allocation, a length and a capacity, and the
            // first of those is a pointer — so it is copied as one, for the
            // reason this function exists at all (Ch. 5 §2.6). Without this
            // arm a `Vec` fell through to the scalar case and one word was
            // copied where three were meant; a `Vec` returned from a function
            // arrived as the *address* of the buffer it was written into.
            Ty::VecOf(_) => {
                let p = self.load_ptr(src.clone());
                self.store_ptr(dst.clone(), p);
                for off in [3, 6] {
                    let from = self.offset(src.clone(), off);
                    let to = self.offset(dst.clone(), off);
                    let n = self.load_from(from, &Ty::TAddr);
                    self.store_scalar(to, &Ty::TAddr, n);
                }
                Ok(())
            }

            Ty::Tuple(_) | Ty::Struct(_) => {
                for (_, ft, off) in self.types.fields(ty) {
                    let from = self.offset(src.clone(), off);
                    let to = self.offset(dst.clone(), off);
                    self.copy_typed(to, from, &ft, span)?;
                }
                Ok(())
            }

            // An enum's payload varies by variant, so its storage is copied
            // as raw trytes. A reference inside a payload therefore loses its
            // provenance in the interpreter — see docs/spec-gaps.md G6.7.
            Ty::Enum(_) => {
                let l = self.types.layout(ty);
                self.copy_trytes(dst, src, l.size as i128, l.align as i128, span)
            }

            _ => {
                let v = self.load_from(src, ty);
                self.store_scalar(dst, ty, v);
                Ok(())
            }
        }
    }

    /// Store a scalar at an address, as a pointer when it is one.
    fn store_scalar(&mut self, at: Operand, ty: &Ty, v: Operand) {
        if matches!(ty, Ty::Ref(t, _) if !t.is_unsized()) {
            self.store_ptr(at, v);
            return;
        }
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: ty.tir(),
                v,
                p: at,
            },
        });
    }

    /// Copy raw storage, a tryte at a time.
    fn copy_trytes(
        &mut self,
        dst: Operand,
        src: Operand,
        size: i128,
        align: i128,
        span: Span,
    ) -> R<()> {
        if size > 243 {
            return err(
                span,
                "this milestone copies aggregates of at most 243 trytes",
            );
        }
        // Whole words first where both ends are word-aligned, which for an
        // enum they are: AM §2.3 caps alignment at a word and an enum holding
        // one is aligned to it. A tryte-at-a-time copy of an `Option<t27>` is
        // twelve instructions where two loads and two stores would do, and an
        // iterator pays it twice per item.
        let mut i = 0;
        if align >= WORD {
            while i + WORD <= size {
                let from = self.offset(src.clone(), i);
                let v = self.emit(
                    "v",
                    Type::Int(27),
                    InstKind::Load {
                        ty: Type::Int(27),
                        p: from,
                    },
                );
                let to = self.offset(dst.clone(), i);
                self.push(Inst {
                    results: Vec::new(),
                    kind: InstKind::Store {
                        ty: Type::Int(27),
                        v,
                        p: to,
                    },
                });
                i += WORD;
            }
        }
        for i in i..size {
            let from = self.offset(src.clone(), i);
            let v = self.emit(
                "v",
                Type::Int(9),
                InstKind::Load {
                    ty: Type::Int(9),
                    p: from,
                },
            );
            let to = self.offset(dst.clone(), i);
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: Type::Int(9),
                    v,
                    p: to,
                },
            });
        }
        Ok(())
    }

    /// A struct literal, a tuple-struct literal, or an enum variant.
    fn aggregate(
        &mut self,
        path: &ast::Path,
        fields: &[(String, ast::Expr)],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // A closure has no type until it is lowered, so a generic literal
        // holding one cannot infer that parameter from a peek. Lower it here
        // and give the value a name the rest of the literal uses — the same
        // move `instantiate_fn` makes for a closure *argument*, for the same
        // reason (Ch. 4 §4.2).
        let mut owned;
        let fields = if fields
            .iter()
            .any(|(_, e)| matches!(e, ast::Expr::Closure(..)))
        {
            owned = fields.to_vec();
            for (_, e) in &mut owned {
                let ast::Expr::Closure(cps, cret, body, cline) = e.clone() else {
                    continue;
                };
                let (v, got) = self.closure(&cps, &cret, &body, None, cline)?;
                let Operand::Value(slot) = v else {
                    return err(span, "a closure must have a slot");
                };
                let bound = self.bind_existing(slot, got);
                *e = ast::Expr::Path(bound, cline);
            }
            &owned[..]
        } else {
            fields
        };
        let head = self.instantiate_head(path, fields, expected, span)?;

        // `Type::NAME` — an associated constant, which is a constant under a
        // qualified name (Ch. 4 §1.7).
        if path.segments.len() == 2 && fields.is_empty() {
            let key = format!("{head}.{}", path.segments[1]);
            if self.globals.contains_key(&key) {
                return self.expr(&ast::Expr::Path(key, span), None);
            }
        }

        // `Vec::new()` — three words, all zero: no allocation, no elements,
        // no room. An empty `Vec` allocates nothing, which is why the
        // pointer's zero is a value it takes rather than a niche it offers
        // (Ch. 5 §2.6).
        // `Vec::with_capacity(n)` — an empty `Vec` with room for `n`, which
        // is `new` and then `reserve` and is spelled once because that is
        // what a program means by it (Ch. 5 §2.6).
        if path.segments.len() == 2
            && (head == "Vec" || head == "String" || head.starts_with("Vec."))
            && path.segments[1] == "with_capacity"
        {
            let [(_, arg)] = fields else {
                return err(span, "`with_capacity` takes one argument");
            };
            let elem = match expected {
                Some(Ty::VecOf(e)) => (**e).clone(),
                _ if head == "String" => Ty::Char,
                _ => {
                    return err(
                        span,
                        "cannot tell what `Vec::with_capacity` holds; write the type of \
                         this value (Ch. 4 §2.3)",
                    );
                }
            };
            let ty = Ty::VecOf(Box::new(elem.clone()));
            let slot = self.temp_slot(&ty);
            for off in [0, 3, 6] {
                self.store_at(
                    &slot,
                    off,
                    &Ty::TAddr,
                    Operand::Const(Type::Int(27), Bt::ZERO),
                    span,
                )?;
            }
            let (n, _) = self.expr(arg, Some(&Ty::TAddr))?;
            self.vec_reserve(Operand::Value(slot.clone()), &elem, n, span)?;
            return Ok((Operand::Value(slot), ty));
        }

        if path.segments.len() == 2
            && (head == "Vec" || head == "String" || head.starts_with("Vec."))
            && path.segments[1] == "new"
        {
            if !fields.is_empty() {
                return err(span, "`Vec::new` takes no arguments");
            }
            // `String::new` needs no annotation: `String` *is* `Vec<char>`,
            // so the element type is in the name.
            let elem = match expected.cloned() {
                Some(Ty::VecOf(elem)) => elem,
                _ if head == "String" => Box::new(Ty::Char),
                _ => {
                    return err(
                        span,
                        "cannot tell what `Vec::new` holds; write the type of this value \
                         (Ch. 4 §2.3)",
                    );
                }
            };
            let ty = Ty::VecOf(elem);
            let slot = self.temp_slot(&ty);
            for off in [0, 3, 6] {
                self.store_at(
                    &slot,
                    off,
                    &Ty::TAddr,
                    Operand::Const(Type::Int(27), Bt::ZERO),
                    span,
                )?;
            }
            return Ok((Operand::Value(slot), ty));
        }

        // `Box::new(v)` and `Box::try_new(v)` — the language item's two
        // constructors (Ch. 5 §2.3). `new` traps if it cannot, which is the
        // decision Ch. 2 §3 makes for an out-of-bounds index and Ch. 1 §4 for
        // a trapping overflow: a failure the program did not say what to do
        // about stops the program.
        if path.segments.len() == 2
            && head == "Box"
            && matches!(path.segments[1].as_str(), "new" | "try_new")
        {
            let [(_, arg)] = fields else {
                return err(
                    span,
                    format!("`Box::{}` takes one argument", path.segments[1]),
                );
            };
            let fallible = path.segments[1] == "try_new";
            return self.box_new(arg, fallible, span);
        }

        // `char::try_from(x)` — the one conversion into `char`, and the one
        // thing in Ch. 5 §1 that cannot be written in this language: every
        // other library function here is ordinary Trust, and this one has to
        // produce a `char` from a word, which is exactly what no `as` does.
        if path.segments.len() == 2 && head == "char" && path.segments[1] == "try_from" {
            let [(_, arg)] = fields else {
                return err(span, "`char::try_from` takes one argument");
            };
            let (v, vt) = self.expr(arg, Some(&Ty::T27))?;
            self.check(&vt, &Ty::T27, span, "`char::try_from`'s argument")?;
            return self.char_try_from(v, span);
        }

        // `Type::function(args)` — an associated function, which is written
        // like a variant and told apart by what the names are (Ch. 4 §1.4).
        if path.segments.len() == 2 {
            let key = format!("{head}.{}", path.segments[1]);
            if self.sigs.borrow().contains_key(&key) {
                let args: Vec<ast::Expr> = fields.iter().map(|(_, e)| e.clone()).collect();
                return self.call_key(&key, Vec::new(), &args, span);
            }
            // A method of a trait that takes arguments is not under
            // `Type.method`: there may be several, and which one is meant is
            // decided by the arguments given (Ch. 4 §1.7).
            let args: Vec<ast::Expr> = fields.iter().map(|(_, e)| e.clone()).collect();
            if let Some(key) = self.trait_method(&head, &path.segments[1], &args, span)? {
                return self.call_key(&key, Vec::new(), &args, span);
            }
            // An associated function of a *generic* type. The head is already
            // the instantiation's mangled name — the context said which one —
            // and its methods live under the base name until one is asked
            // for, which is what `method_key` asks (Ch. 4 §§1.4, 2.7).
            if self.types.instantiations.borrow().contains_key(&head) {
                let key = self.method_key(&head, &path.segments[1], span)?;
                if self.sigs.borrow().contains_key(&key) {
                    return self.call_key(&key, Vec::new(), &args, span);
                }
                // A method that still has parameters of its own goes through
                // the ordinary generic path, which is where the call's
                // arguments settle them (Ch. 4 §2.7).
                if self.generic_fns.contains_key(&key) || self.specials.borrow().contains_key(&key)
                {
                    return self.call(&key, &[], &args, expected, span);
                }
            }
        }

        // `Enum::Variant`, with or without a payload.
        if path.segments.len() == 2 {
            let (enum_name, variant) = (head, path.segments[1].clone());
            if !self.types.enums.borrow().contains_key(&enum_name) {
                return err(span, format!("`{enum_name}` is not an enum in scope"));
            }
            let Some(index) = self.types.variant(&enum_name, &variant) else {
                return err(span, format!("`{enum_name}` has no variant `{variant}`"));
            };
            return self.build_variant(&enum_name, index, fields, span);
        }

        // A struct literal.
        if !self.types.structs.borrow().contains_key(&head) {
            return err(span, format!("`{head}` is not a struct in scope"));
        }
        let ty = Ty::Struct(head.clone());
        let declared = self.types.fields(&ty);
        if declared.len() != fields.len() {
            return err(
                span,
                format!(
                    "`{head}` has {} field(s), {} given",
                    declared.len(),
                    fields.len()
                ),
            );
        }
        let slot = self.dest_or_temp(&ty);
        for (name, value) in fields {
            let Some((_, ft, off)) = declared.iter().find(|(n, _, _)| n == name).cloned() else {
                return err(span, format!("`{head}` has no field `{name}`"));
            };
            let (v, vt) = self.expr(value, Some(&ft))?;
            self.check(&vt, &ft, value.span(), "field")?;
            self.store_at(&slot, off, &ft, v, span)?;
        }
        Ok((Operand::Value(slot), ty))
    }

    /// Build one variant of an enum, writing the discriminant however this
    /// enum's layout encodes it (Ch. 2 §5.1, §6).
    fn build_variant(
        &mut self,
        enum_name: &str,
        index: usize,
        fields: &[(String, ast::Expr)],
        span: Span,
    ) -> R<(Operand, Ty)> {
        let ty = Ty::Enum(enum_name.to_string());
        let declared = self.types.variant_fields(enum_name, index);
        if declared.len() != fields.len() {
            return err(
                span,
                format!(
                    "this variant has {} field(s), {} given",
                    declared.len(),
                    fields.len()
                ),
            );
        }

        let slot = self.dest_or_temp(&ty);
        let mut values = Vec::new();
        for (name, value) in fields {
            let Some((_, ft, _)) = declared.iter().find(|(n, _, _)| n == name).cloned() else {
                return err(span, format!("this variant has no field `{name}`"));
            };
            let (v, vt) = self.expr(value, Some(&ft))?;
            self.check(&vt, &ft, value.span(), "field")?;
            values.push((name.clone(), v));
        }
        self.build_variant_into(&slot, enum_name, index, &values, span)?;
        Ok((Operand::Value(slot), ty))
    }

    /// The destination this value was told to go to, or a new temporary.
    ///
    /// A destination is used only when it is *this* value's: `dest` is taken
    /// at the top of `expr_inner`, so by the time a literal asks, anything
    /// meant for an enclosing expression is already gone.
    fn dest_or_temp(&mut self, ty: &Ty) -> String {
        match self.dest.take() {
            Some(slot) => slot,
            None => self.temp_slot(ty),
        }
    }

    /// Write a variant into storage that already exists, from values rather
    /// than from expressions — what a built-in method has to hand.
    fn build_variant_into(
        &mut self,
        slot: &str,
        enum_name: &str,
        index: usize,
        fields: &[(String, Operand)],
        span: Span,
    ) -> R<()> {
        let ty = Ty::Enum(enum_name.to_string());
        let l = self.types.layout(&ty);
        let e = l.enum_layout.clone().expect("an enum");
        let declared = self.types.variant_fields(enum_name, index);
        let slot = slot.to_string();

        // The storage is *not* zeroed first. Ch. 2 §1 says padding trytes have
        // unspecified contents, and G7.4 reads that as every pattern being
        // acceptable — so zeroing bought a determinism the specification does
        // not ask for, at one store per tryte of the whole enum, on every
        // construction of every variant. `Option<t27>` paid six.
        for (name, v) in fields {
            let Some((_, ft, off)) = declared.iter().find(|(n, _, _)| n == name).cloned() else {
                return err(span, format!("this variant has no field `{name}`"));
            };
            self.store_at(&slot, off, &ft, v.clone(), span)?;
        }

        self.write_tag(&slot, &e, index, span)
    }

    /// Store the discriminant of variant `index`.
    fn write_tag(&mut self, slot: &str, e: &layout::EnumLayout, index: usize, span: Span) -> R<()> {
        match &e.tag {
            layout::Tag::None => Ok(()),

            // Representation-identical to `trit` (Ch. 2 §5.2).
            layout::Tag::TritShaped => {
                let v = Operand::Const(Type::Int(1), Bt::from_i128(e.discriminants[index]));
                self.push(Inst {
                    results: Vec::new(),
                    kind: InstKind::Store {
                        ty: Type::Int(1),
                        v,
                        p: Operand::Value(slot.to_string()),
                    },
                });
                Ok(())
            }

            layout::Tag::Direct { ty, offset } => {
                let tir = Type::Int(ty.trits());
                let v = Operand::Const(tir, Bt::from_i128(e.discriminants[index]));
                let p = self.offset(Operand::Value(slot.to_string()), *offset as i128);
                self.push(Inst {
                    results: Vec::new(),
                    kind: InstKind::Store { ty: tir, v, p },
                });
                Ok(())
            }

            // The discriminant costs no space: it lives in an invalid
            // representation of the payload (Ch. 2 §6).
            layout::Tag::Niche {
                untagged,
                offset,
                spot,
                ..
            } => {
                if index == *untagged {
                    return Ok(());
                }
                let which = (0..e.discriminants.len())
                    .filter(|i| i != untagged)
                    .position(|i| i == index)
                    .expect("a tagged variant") as u128;
                let Some(value) = spot.nth(which) else {
                    return err(span, "this enum has more variants than niches");
                };
                let tir = Type::Int(spot.trits);
                let p = self.offset(Operand::Value(slot.to_string()), *offset as i128);
                self.push(Inst {
                    results: Vec::new(),
                    kind: InstKind::Store {
                        ty: tir,
                        v: Operand::Const(tir, Bt::from_i128(value)),
                        p,
                    },
                });
                Ok(())
            }
        }
    }

    /// Read an enum's discriminant as a value comparable with the
    /// discriminants the layout assigned.
    fn read_tag(&mut self, addr: Operand, e: &layout::EnumLayout) -> (Operand, Type) {
        match &e.tag {
            layout::Tag::None => (
                Operand::Const(Type::Int(9), Bt::from_i128(e.discriminants[0])),
                Type::Int(9),
            ),
            // Representation-identical to `trit`, so the discriminant is a
            // trit and feeds `br3` with nothing in between (Ch. 2 §5.2).
            layout::Tag::TritShaped => (
                self.emit(
                    "d",
                    Type::Int(1),
                    InstKind::Load {
                        ty: Type::Int(1),
                        p: addr,
                    },
                ),
                Type::Int(1),
            ),
            layout::Tag::Direct { ty, offset } => {
                let tir = Type::Int(ty.trits());
                let p = self.offset(addr, *offset as i128);
                (self.emit("d", tir, InstKind::Load { ty: tir, p }), tir)
            }
            layout::Tag::Niche { offset, spot, .. } => {
                let tir = Type::Int(spot.trits);
                let p = self.offset(addr, *offset as i128);
                (self.emit("d", tir, InstKind::Load { ty: tir, p }), tir)
            }
        }
    }

    // ------------------------------------------------------ control flow

    fn temp_slot(&mut self, ty: &Ty) -> String {
        let slot = self.fresh("tmp.slot");
        let trytes = self.types.size(ty).max(1) as u32;
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot { trytes },
        });
        slot
    }

    /// Write a value into a join slot. An aggregate is copied: the arms
    /// produce addresses, and the join needs one storage location they all
    /// agree on.
    ///
    /// `span` is only ever reached by the aggregate path, and a drop flag is a
    /// `bool`, which is why the three that write one pass `Span::NONE`.
    fn store_slot(&mut self, slot: &str, ty: &Ty, v: Operand, span: Span) {
        if ty.is_aggregate() {
            let dst = Operand::Value(slot.to_string());
            let ty = ty.clone();
            let _ = self.copy_typed(dst, v, &ty, span);
            return;
        }
        self.push(Inst {
            results: Vec::new(),
            kind: InstKind::Store {
                ty: ty.tir(),
                v,
                p: Operand::Value(slot.to_string()),
            },
        });
    }

    /// Read a join slot back. An aggregate *is* its address.
    fn load_slot(&mut self, slot: &str, ty: &Ty) -> Operand {
        if ty.is_aggregate() {
            return Operand::Value(slot.to_string());
        }
        self.emit(
            "v",
            ty.tir(),
            InstKind::Load {
                ty: ty.tir(),
                p: Operand::Value(slot.to_string()),
            },
        )
    }

    fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then: &ast::Block,
        els: Option<&ast::Expr>,
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        // Where this `if`'s value is going, if it was told. Both arms are
        // given it, so each may build its value there rather than somewhere
        // the join copies from.
        let dest = self.dest.take();
        // The condition is a `bool`, not a `trit`, and not "anything
        // nonzero" (Ch. 1 §2).
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, span, "condition")?;

        let (then_l, else_l, join_l) = (self.fresh("then"), self.fresh("else"), self.fresh("join"));
        self.br3(c, &else_l, &else_l, &then_l);

        // The two arms' values meet in a slot, so nothing crosses a block
        // edge in a register.
        let mut result: Option<(String, Ty)> = None;

        let before = self.owned_snapshot();
        self.start(then_l);
        self.dest.clone_from(&dest);
        let (tv, tt) = self.block(then, expected)?;
        self.dest = None;
        if tt != Ty::Never && tt != Ty::Unit {
            let slot = match &dest {
                Some(d) => d.clone(),
                None => self.temp_slot(&tt),
            };
            // A value that was built in the join slot is already there.
            if tv != Operand::Value(slot.clone()) {
                self.store_slot(&slot, &tt, tv, span);
            }
            result = Some((slot, tt.clone()));
        }
        self.jump(&join_l);
        let after_then = self.owned_snapshot();

        self.owned = before;
        self.start(else_l);
        let et = match els {
            None => Ty::Unit,
            Some(e) => {
                self.dest.clone_from(&dest);
                let (ev, et) = self.expr(e, expected)?;
                self.dest = None;
                if let Some((slot, ty)) = &result
                    && et != Ty::Never
                {
                    let (slot, ty) = (slot.clone(), ty.clone());
                    self.check(&et, &ty, span, "`else` branch")?;
                    if ev != Operand::Value(slot.clone()) {
                        self.store_slot(&slot, &ty, ev, span);
                    }
                }
                et
            }
        };
        self.jump(&join_l);
        let after_else = self.owned_snapshot();
        self.owned_join(after_then, after_else);

        self.start(join_l);
        match result {
            // An `if` used for its value must have an `else` (§5.3).
            Some((slot, ty)) if els.is_some() => {
                let v = self.load_slot(&slot, &ty);
                Ok((v, ty))
            }
            Some((_, ty)) if ty != Ty::Unit && expected.is_some_and(|e| *e != Ty::Unit) => {
                err(span, "an `if` used for its value needs an `else` branch")
            }
            _ => {
                if tt == Ty::Never && et == Ty::Never {
                    // Both arms diverge; so does the `if`.
                    return Ok((unit(), Ty::Never));
                }
                Ok((unit(), Ty::Unit))
            }
        }
    }

    fn while_expr(&mut self, cond: &ast::Expr, body: &ast::Block, span: Span) -> R<(Operand, Ty)> {
        let (head, body_l, exit) = (self.fresh("while"), self.fresh("body"), self.fresh("done"));
        // The **body first**, and the test after it.
        //
        // A loop laid out as test-then-body ends its body with a jump back,
        // and a backward jump is an instruction: the test, the branch, and
        // the jump are three per iteration where two will do. Emitting the
        // body first makes its jump to the test a fall-through, and the
        // branch back into the body is the only transfer left. On HPL that
        // jump was 5.1% of everything executed.
        self.jump(&head);

        self.start(body_l.clone());
        let before = self.owned_snapshot();
        self.loops.push(LoopCtx {
            depth: self.scopes.len(),
            exit: exit.clone(),
            head: head.clone(),
            result: None,
            // A `while` always has an exit: the condition being false.
            broke: true,
        });
        self.block(body, None)?;
        self.loops.pop();
        self.check_no_move_in_loop(&before, span)?;
        self.jump(&head);

        self.start(head);
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, span, "condition")?;
        self.br3(c, &exit, &exit, &body_l);

        self.start(exit);
        Ok((unit(), Ty::Unit))
    }

    fn loop_expr(
        &mut self,
        body: &ast::Block,
        expected: Option<&Ty>,
        _line: Span,
    ) -> R<(Operand, Ty)> {
        let (head, exit) = (self.fresh("loop"), self.fresh("done"));
        // A `loop` yields a value only if something expects one; `break expr`
        // then writes to this slot.
        let result = expected
            .filter(|t| t.is_scalar() || t.is_aggregate())
            .map(|t| (self.temp_slot(t), t.clone()));

        self.jump(&head);
        self.start(head.clone());
        let before = self.owned_snapshot();
        self.loops.push(LoopCtx {
            depth: self.scopes.len(),
            exit: exit.clone(),
            head: head.clone(),
            result: result.clone(),
            broke: false,
        });
        self.block(body, None)?;
        let ctx = self.loops.pop().expect("just pushed");
        self.check_no_move_in_loop(&before, _line)?;
        self.jump(&head);

        // Nothing leaves this loop, so there is nowhere after it: its type is
        // `!`, and the block that would have followed is emitted no more than
        // the block after a `return` is.
        if !ctx.broke {
            return Ok((unit(), Ty::Never));
        }

        self.start(exit);
        match result {
            Some((slot, ty)) => {
                let v = self.load_slot(&slot, &ty);
                Ok((v, ty))
            }
            None => Ok((unit(), Ty::Unit)),
        }
    }

    /// `match` (§5.4). Over a `trit` with the three trit patterns this is one
    /// `br3` and nothing else — which is the whole point.
    fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::Arm],
        expected: Option<&Ty>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let dest = self.dest.take();
        let (mut v, mut ty) = self.expr(scrutinee, None)?;
        // A scrutinee behind a reference is dereferenced, exactly as `.` is
        // (Ch. 3 §2.3). Bindings then copy out of the referent, which the
        // copy rule of §1.2 already governs.
        //
        // Whether anything was stripped is what decides who owns an arm's
        // bindings: matching a value *moves* it, so the bindings receive what
        // it held; matching through a reference moves nothing, so they are
        // copies of storage the referent still owns, and dropping one would
        // free a tree somebody is still holding.
        let mut borrowed = false;
        while let Ty::Ref(target, _) | Ty::Boxed(target) = ty.clone() {
            if target.is_unsized() {
                break;
            }
            borrowed = true;
            // `v` is the reference itself, which is an address. An
            // aggregate's value *is* its address, so dereferencing one is a
            // retype; a scalar has to be loaded.
            if !target.is_aggregate() {
                v = self.load_from(v, &target);
            }
            ty = *target;
        }
        if let Ty::Enum(name) = ty.clone() {
            return self.match_enum(&name, v, arms, expected, borrowed, dest, span);
        }
        if !ty.is_scalar() {
            return err(span, format!("cannot match on {ty}"));
        }
        check_exhaustive(&ty, arms, span)?;

        let join = self.fresh("match.join");
        let mut result: Option<(String, Ty)> = None;

        // A trit scrutinee whose arms are the three trit literals is exactly
        // a three-way branch (Ch. 1 §5).
        if ty == Ty::Trit
            && let Some(order) = trit_dispatch(arms)
        {
            let labels: Vec<String> = (0..arms.len()).map(|_| self.fresh("arm")).collect();
            let pick = |i: Option<usize>| match i {
                Some(i) => labels[i].clone(),
                None => join.clone(),
            };
            self.br3(v, &pick(order[0]), &pick(order[1]), &pick(order[2]));
            let before = self.owned_snapshot();
            let mut merged: Option<Vec<Owned>> = None;
            for (arm, label) in arms.iter().zip(&labels) {
                self.owned = before.clone();
                self.start(label.clone());
                self.arm_body(arm, expected, &mut result, &join, span, None, &dest)?;
                merged = Some(self.join_arm(merged));
            }
            if let Some(m) = merged {
                self.owned = m;
            }
            self.start(join);
            return Ok(self.match_result(result));
        }

        // Otherwise: test the arms in order.
        let mut fell_through = true;
        let before = self.owned_snapshot();
        let mut merged: Option<Vec<Owned>> = None;
        for arm in arms {
            let next = self.fresh("arm.next");
            let body = self.fresh("arm");
            self.owned = before.clone();
            let unconditional = self.arm_test(arm, &v, &ty, &body, &next, span)?;
            self.start(body);
            self.arm_body(arm, expected, &mut result, &join, span, None, &dest)?;
            merged = Some(self.join_arm(merged));
            if unconditional {
                // A wildcard or binding matches everything, so no later arm
                // and no fallthrough block is reachable — and an unreachable
                // block is one the verifier rejects.
                fell_through = false;
                break;
            }
            self.start(next);
        }
        if fell_through {
            // Exhaustiveness was checked, so control cannot arrive here in a
            // well-typed program — but TIR wants a terminator.
            self.finish(Terminator::Trap(FaultCode::Trap));
        }

        self.start(join);
        Ok(self.match_result(result))
    }

    /// `match` over an enum: read the discriminant once, then dispatch.
    ///
    /// A three-variant fieldless enum with discriminants −1, 0, +1 is
    /// representation-identical to `trit`, and this is where Ch. 2 §5.2's
    /// promise is kept: the dispatch is one `br3`.
    #[allow(clippy::too_many_arguments)]
    fn match_enum(
        &mut self,
        name: &str,
        addr: Operand,
        arms: &[ast::Arm],
        expected: Option<&Ty>,
        borrowed: bool,
        dest: Option<String>,
        span: Span,
    ) -> R<(Operand, Ty)> {
        let ty = Ty::Enum(name.to_string());
        let l = self.types.layout(&ty);
        let e = l.enum_layout.clone().expect("an enum");
        let variants = self.types.enums.borrow()[name].clone();

        // Which variant each arm selects, and whether an arm catches all.
        let mut selects: Vec<Option<usize>> = Vec::new();
        for arm in arms {
            if arm.guard.is_some() {
                return err(arm.span, "match guards are not lowered yet");
            }
            if arm.patterns.len() != 1 {
                return err(arm.span, "or-patterns over an enum are not lowered yet");
            }
            selects.push(match &arm.patterns[0] {
                ast::Pattern::Wild(_) | ast::Pattern::Bind(..) => None,
                ast::Pattern::Aggregate(path, _, l2) => {
                    let variant = match path.segments.len() {
                        2 if heads_match(&path.segments[0], name) => path.segments[1].clone(),
                        1 => path.segments[0].clone(),
                        _ => {
                            return err(
                                *l2,
                                format!("`{}` is not a variant of `{name}`", path.last()),
                            );
                        }
                    };
                    match self.types.variant(name, &variant) {
                        Some(i) => Some(i),
                        None => return err(*l2, format!("`{name}` has no variant `{variant}`")),
                    }
                }
                other => {
                    return err(
                        other.span(),
                        format!("this pattern does not match `{name}`"),
                    );
                }
            });
        }

        let covered: Vec<usize> = selects.iter().flatten().copied().collect();
        let catchall = selects.iter().any(Option::is_none);
        if !catchall && covered.len() < variants.len() {
            return err(
                span,
                format!(
                    "this `match` is not exhaustive: `{name}` has {} variant(s), {} covered",
                    variants.len(),
                    covered.len()
                ),
            );
        }

        let (tag, tag_ty) = self.read_tag(addr.clone(), &e);
        let join = self.fresh("match.join");
        let mut result: Option<(String, Ty)> = None;

        // The trit-shaped case: one `br3`, no comparison at all.
        if e.tag == layout::Tag::TritShaped
            && !catchall
            && let Some(order) = trit_variant_dispatch(&e.discriminants, &selects)
        {
            let labels: Vec<String> = (0..arms.len()).map(|_| self.fresh("arm")).collect();
            let pick = |i: usize| labels[i].clone();
            self.br3(tag, &pick(order[0]), &pick(order[1]), &pick(order[2]));
            let before = self.owned_snapshot();
            let mut merged: Option<Vec<Owned>> = None;
            for (i, arm) in arms.iter().enumerate() {
                self.owned = before.clone();
                self.start(labels[i].clone());
                self.enum_arm(
                    arm,
                    name,
                    selects[i],
                    &addr,
                    expected,
                    &mut result,
                    &join,
                    borrowed,
                    span,
                    &dest,
                )?;
                merged = Some(self.join_arm(merged));
            }
            if let Some(m) = merged {
                self.owned = m;
            }
            self.start(join);
            return Ok(self.match_result(result));
        }

        // Each arm tests one value of the discriminant — except the arm for a
        // niche-encoded enum's *untagged* variant, whose storage holds an
        // ordinary payload and which is therefore recognized by elimination.
        // Those, and a wildcard, are emitted last, since the variant patterns
        // are disjoint and only order relative to the catch-all matters.
        let before = self.owned_snapshot();
        let mut merged: Option<Vec<Owned>> = None;
        let mut tested: Vec<(usize, usize, i128)> = Vec::new();
        let mut default: Option<usize> = None;
        for (i, select) in selects.iter().enumerate() {
            match select {
                None => {
                    if default.is_none() {
                        default = Some(i);
                    }
                }
                Some(index) => match tag_value(&e, *index) {
                    Some(v) => tested.push((i, *index, v)),
                    None if default.is_none() => default = Some(i),
                    None => {}
                },
            }
        }

        for (arm_index, variant, value) in tested {
            let body = self.fresh("arm");
            let next = self.fresh("arm.next");
            let k = Operand::Const(tag_ty, Bt::from_i128(value));
            let c = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: tag_ty,
                    a: tag.clone(),
                    b: k,
                },
            );
            self.br3(c, &next, &body, &next);
            self.owned = before.clone();
            self.start(body);
            self.enum_arm(
                &arms[arm_index],
                name,
                Some(variant),
                &addr,
                expected,
                &mut result,
                &join,
                borrowed,
                span,
                &dest,
            )?;
            merged = Some(self.join_arm(merged));
            self.owned = before.clone();
            self.start(next);
        }

        match default {
            Some(i) => {
                let variant = selects[i];
                self.owned = before.clone();
                self.enum_arm(
                    &arms[i],
                    name,
                    variant,
                    &addr,
                    expected,
                    &mut result,
                    &join,
                    borrowed,
                    span,
                    &dest,
                )?;
                merged = Some(self.join_arm(merged));
            }
            None => self.finish(Terminator::Trap(FaultCode::Trap)),
        }
        if let Some(m) = merged {
            self.owned = m;
        }
        self.start(join);
        Ok(self.match_result(result))
    }

    /// One arm of an enum `match`: bind whatever the pattern names, then
    /// lower the body.
    #[allow(clippy::too_many_arguments)]
    fn enum_arm(
        &mut self,
        arm: &ast::Arm,
        enum_name: &str,
        variant: Option<usize>,
        addr: &Operand,
        expected: Option<&Ty>,
        result: &mut Option<(String, Ty)>,
        join: &str,
        borrowed: bool,
        span: Span,
        dest: &Option<String>,
    ) -> R<()> {
        let depth = self.scopes.len();
        self.scopes.push(HashMap::new());

        if let ast::Pattern::Bind(name, _) = &arm.patterns[0] {
            let ty = Ty::Enum(enum_name.to_string());
            let local = if borrowed {
                self.declare_borrowed(name, ty.clone())
            } else {
                self.declare(name, ty.clone(), false)
            };
            self.store_at(&local.slot, 0, &ty, addr.clone(), span)?;
        }
        if let (Some(index), ast::Pattern::Aggregate(_, fields, _)) = (variant, &arm.patterns[0]) {
            let declared = self.types.variant_fields(enum_name, index);
            if !fields.is_empty() && fields.len() != declared.len() {
                self.scopes.pop();
                return err(
                    arm.span,
                    format!(
                        "this variant has {} field(s), the pattern names {}",
                        declared.len(),
                        fields.len()
                    ),
                );
            }
            for (name, pat) in fields {
                let Some((_, ft, off)) = declared.iter().find(|(n, _, _)| n == name).cloned()
                else {
                    self.scopes.pop();
                    return err(arm.span, format!("this variant has no field `{name}`"));
                };
                let ast::Pattern::Bind(bound, _) = pat else {
                    self.scopes.pop();
                    return err(arm.span, "nested patterns are not lowered yet");
                };
                let p = self.offset(addr.clone(), off);
                let v = self.load_from(p, &ft);
                // A binding read out of borrowed storage never owns: the
                // referent still does, and two owners of one allocation is
                // the bug this whole chapter exists to make impossible.
                let local = if borrowed {
                    self.declare_borrowed(bound, ft.clone())
                } else {
                    self.declare(bound, ft.clone(), false)
                };
                self.store_at(&local.slot, 0, &ft, v, arm.span)?;
            }
        }

        let r = self.arm_body(arm, expected, result, join, span, Some(depth), dest);
        self.scopes.pop();
        r
    }

    /// Fold one arm's ownership state into what the arms before it left.
    ///
    /// Arms are alternatives, not a sequence: a value moved in one is not
    /// moved in the next, and a value moved in *some* of them is only
    /// maybe-owned afterwards, which is what the drop flag is for (Ch. 3
    /// §1.2). `if`/`else` did this from the start and `match` did not, so a
    /// move in the first arm made every arm after it complain.
    fn join_arm(&mut self, merged: Option<Vec<Owned>>) -> Vec<Owned> {
        let here = self.owned_snapshot();
        match merged {
            None => here,
            Some(prev) => {
                self.owned_join(prev, here);
                self.owned_snapshot()
            }
        }
    }

    fn match_result(&mut self, result: Option<(String, Ty)>) -> (Operand, Ty) {
        match result {
            Some((slot, ty)) => {
                let v = self.load_slot(&slot, &ty);
                (v, ty)
            }
            None => (unit(), Ty::Unit),
        }
    }

    /// Lower one arm's body, store its value, and leave for the join.
    ///
    /// `scope` is the depth an arm's *pattern bindings* live at, when it has
    /// any. They are dropped here — after the arm's value is stored and
    /// before the jump, because a drop emitted after a terminator is in no
    /// block at all. An arm is a scope like any other (Ch. 3 §1.4); it did
    /// not used to be, and a binding that outlived its arm was swept up by
    /// the next scope to end at the same depth.
    #[allow(clippy::too_many_arguments)]
    fn arm_body(
        &mut self,
        arm: &ast::Arm,
        expected: Option<&Ty>,
        result: &mut Option<(String, Ty)>,
        join: &str,
        span: Span,
        scope: Option<usize>,
        dest: &Option<String>,
    ) -> R<()> {
        self.dest.clone_from(dest);
        let (v, ty) = self.expr(&arm.body, expected)?;
        self.dest = None;
        if ty != Ty::Never && ty != Ty::Unit {
            if result.is_none() {
                let slot = match dest {
                    Some(d) => d.clone(),
                    None => self.temp_slot(&ty),
                };
                *result = Some((slot, ty.clone()));
            }
            let (slot, want) = result.clone().expect("just set");
            self.check(&ty, &want, span, "match arm")?;
            // A value built in the join slot is already there.
            if v != Operand::Value(slot.clone()) {
                self.store_slot(&slot, &want, v, span);
            }
        }
        if let Some(depth) = scope {
            self.drop_scope(depth, arm.span)?;
        }
        self.jump(join);
        Ok(())
    }

    /// Emit the test for one arm, branching to `body` or `next`. Returns
    /// true if the arm matches unconditionally.
    fn arm_test(
        &mut self,
        arm: &ast::Arm,
        v: &Operand,
        ty: &Ty,
        body: &str,
        next: &str,
        span: Span,
    ) -> R<bool> {
        if arm.guard.is_some() {
            return err(span, "match guards are not lowered yet");
        }
        // A wildcard or binding matches everything.
        if arm
            .patterns
            .iter()
            .any(|p| matches!(p, ast::Pattern::Wild(_) | ast::Pattern::Bind(..)))
        {
            self.jump(body);
            return Ok(true);
        }
        let mut remaining = arm.patterns.len();
        for p in &arm.patterns {
            let value = pattern_value(p, ty, span)?;
            let k = Operand::Const(ty.tir(), value);
            let c = self.emit(
                "c",
                Type::Int(1),
                InstKind::Cmp {
                    ty: ty.tir(),
                    a: v.clone(),
                    b: k,
                },
            );
            remaining -= 1;
            let fail = if remaining == 0 {
                next.to_string()
            } else {
                self.fresh("alt")
            };
            self.br3(c, &fail, body, &fail);
            self.start(fail);
        }
        Ok(false)
    }
}

/// A place, spelled for a diagnostic.
fn describe(p: &PlacePath) -> String {
    let mut s = p.root.clone();
    for proj in &p.projections {
        match proj {
            Proj::Field(f) => {
                s.push('.');
                s.push_str(f);
            }
            Proj::Index => s.push_str("[…]"),
        }
    }
    s
}

/// The last statement at which each name is used.
///
/// Statements are numbered in the order lowering visits them, and lowering
/// keeps the same count — so the two agree without either knowing about the
/// other's internals. A borrow is dead after its holder's last use, which is
/// what makes the checking non-lexical (Ch. 3 §4.2).
fn last_use_of(body: &ast::Block) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let mut index = 0;
    walk_block(body, &mut index, &mut out);
    out
}

fn walk_block(b: &ast::Block, index: &mut u32, out: &mut HashMap<String, u32>) {
    for stmt in &b.stmts {
        *index += 1;
        match stmt {
            ast::Stmt::Let { value, .. } => walk_expr(value, index, out),
            ast::Stmt::Expr(e) => walk_expr(e, index, out),
        }
    }
    if let Some(tail) = &b.tail {
        *index += 1;
        walk_expr(tail, index, out);
    }
}

fn walk_expr(e: &ast::Expr, index: &mut u32, out: &mut HashMap<String, u32>) {
    use ast::Expr as E;
    let go = |x: &ast::Expr, i: &mut u32, o: &mut HashMap<String, u32>| walk_expr(x, i, o);
    match e {
        E::Char(..) | E::Str(..) => {}
        E::Try(a, _) => go(a, index, out),
        E::CallExpr(c, args, _) => {
            go(c, index, out);
            for a in args {
                go(a, index, out);
            }
        }
        // A closure's body is walked in place: its uses of a capture count
        // as uses at the point the closure is written, and the closure's own
        // binding extends them to its last use.
        E::Closure(_, _, body, _) => go(body, index, out),
        E::Path(name, _) => {
            let at = *index;
            out.entry(name.clone())
                .and_modify(|v| *v = (*v).max(at))
                .or_insert(at);
        }
        E::Int(..) | E::Trit(..) | E::Bool(..) | E::Unit(_) | E::Continue(_) => {}
        E::Array(items, _) | E::Tuple(items, _) => {
            for i in items {
                go(i, index, out);
            }
        }
        E::Aggregate(_, fields, _) => {
            for (_, v) in fields {
                go(v, index, out);
            }
        }
        E::Repeat(a, b, _) | E::Binary(_, a, b, _) | E::Assign(_, a, b, _) | E::Index(a, b, _) => {
            go(a, index, out);
            go(b, index, out);
        }
        E::Unary(_, a, _)
        | E::Cast(a, _, _)
        | E::Field(a, _, _)
        | E::Borrow(a, _, _)
        | E::Deref(a, _) => go(a, index, out),
        E::Call(_, _, _, args, _) => {
            for a in args {
                go(a, index, out);
            }
        }
        E::Method(recv, _, _, args, _) => {
            go(recv, index, out);
            for a in args {
                go(a, index, out);
            }
        }
        E::Block(b) => walk_block(b, index, out),
        E::If(c, then, els, _) => {
            go(c, index, out);
            walk_block(then, index, out);
            if let Some(e) = els {
                go(e, index, out);
            }
        }
        E::Match(scrutinee, arms, _) => {
            go(scrutinee, index, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    go(g, index, out);
                }
                go(&arm.body, index, out);
            }
        }
        E::Loop(b, _) => walk_block(b, index, out),
        E::While(c, b, _) => {
            go(c, index, out);
            walk_block(b, index, out);
        }
        E::Break(v, _) | E::Return(v, _) => {
            if let Some(v) = v {
                go(v, index, out);
            }
        }
    }
}

/// Where an indexable thing's length comes from.
enum Length {
    /// An array: from the type.
    Fixed(i128),
    /// A slice reference: from the second word of the fat pointer.
    Dynamic,
}

/// A unit value: never read, but TIR operands are always typed.
fn unit() -> Operand {
    Operand::Const(Type::Int(27), Bt::ZERO)
}

fn pattern_value(p: &ast::Pattern, ty: &Ty, span: Span) -> R<Bt> {
    match p {
        ast::Pattern::Int(v, l) => {
            if !ty.is_arithmetic() {
                return err(*l, format!("an integer pattern does not match {ty}"));
            }
            Ok(v.clone())
        }
        ast::Pattern::Trit(t, l) => {
            if *ty != Ty::Trit {
                return err(*l, format!("a trit pattern does not match {ty}"));
            }
            Ok(Bt::from(*t))
        }
        ast::Pattern::Bool(b, l) => {
            if *ty != Ty::Bool {
                return err(*l, format!("a bool pattern does not match {ty}"));
            }
            Ok(Bt::from_i128(i128::from(*b)))
        }
        ast::Pattern::Char(v, l) => {
            if *ty != Ty::Char {
                return err(*l, format!("a character pattern does not match {ty}"));
            }
            Ok(Bt::from_i128(*v))
        }
        _ => err(span, "unsupported pattern"),
    }
}

/// The value the discriminant takes for a variant, or `None` when the variant
/// is recognized by elimination — the untagged variant of a niche encoding,
/// whose storage holds an ordinary payload (Ch. 2 §6).
fn tag_value(e: &layout::EnumLayout, index: usize) -> Option<i128> {
    match &e.tag {
        layout::Tag::None => None,
        layout::Tag::TritShaped | layout::Tag::Direct { .. } => Some(e.discriminants[index]),
        layout::Tag::Niche { untagged, spot, .. } => {
            if index == *untagged {
                return None;
            }
            let which = (0..e.discriminants.len())
                .filter(|i| i != untagged)
                .position(|i| i == index)? as u128;
            spot.nth(which)
        }
    }
}

/// For a trit-shaped enum, the arm that handles each of −1, 0, +1.
fn trit_variant_dispatch(discs: &[i128], selects: &[Option<usize>]) -> Option<[usize; 3]> {
    let mut order = [None; 3];
    for (arm, select) in selects.iter().enumerate() {
        let variant = (*select)?;
        let slot = (discs.get(variant)? + 1) as usize;
        if slot > 2 || order[slot].is_some() {
            return None;
        }
        order[slot] = Some(arm);
    }
    Some([order[0]?, order[1]?, order[2]?])
}

/// If the arms are exactly the three trit literals, in some order, return for
/// each of −1, 0, +1 the arm that handles it.
fn trit_dispatch(arms: &[ast::Arm]) -> Option<[Option<usize>; 3]> {
    let mut order = [None, None, None];
    for (i, arm) in arms.iter().enumerate() {
        if arm.guard.is_some() {
            return None;
        }
        for p in &arm.patterns {
            let ast::Pattern::Trit(t, _) = p else {
                return None;
            };
            let slot = (t.to_i8() + 1) as usize;
            if order[slot].is_some() {
                return None;
            }
            order[slot] = Some(i);
        }
    }
    order.iter().all(Option::is_some).then_some(order)
}

/// A `match` must be exhaustive (Ch. 2 §5). Over a `trit` that means three
/// arms or a wildcard; over `bool`, two; over an integer, a wildcard.
fn check_exhaustive(ty: &Ty, arms: &[ast::Arm], span: Span) -> R<()> {
    let mut seen: Vec<Bt> = Vec::new();
    for arm in arms {
        if arm.guard.is_some() {
            continue; // a guarded arm never counts (§5.4)
        }
        for p in &arm.patterns {
            match p {
                ast::Pattern::Wild(_) | ast::Pattern::Bind(..) => return Ok(()),
                _ => {
                    let v = pattern_value(p, ty, span)?;
                    if !seen.contains(&v) {
                        seen.push(v);
                    }
                }
            }
        }
    }
    let needed = match ty {
        Ty::Trit => 3,
        Ty::Bool => 2,
        _ => usize::MAX,
    };
    if seen.len() >= needed {
        Ok(())
    } else {
        err(
            span,
            format!(
                "this `match` is not exhaustive: {ty} needs {} more case(s) or a `_` arm",
                if needed == usize::MAX {
                    "some".to_string()
                } else {
                    (needed - seen.len()).to_string()
                }
            ),
        )
    }
}
