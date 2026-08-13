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
use super::lex::{Line, SyntaxError};
use crate::layout;
use crate::tir::ir::{self, *};
use std::cell::RefCell;
use std::collections::HashMap;
use trit_core::{Bt, FaultCode, Flavor};

/// A type-checking error.
pub type Error = SyntaxError;

type R<T> = Result<T, Error>;

fn err<T>(line: Line, message: impl Into<String>) -> R<T> {
    Err(SyntaxError {
        line,
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
            Ty::T27 | Ty::TAddr | Ty::Never | Ty::Unit => Type::Int(27),
            // Unsized on its own; only a reference to one is a value.
            Ty::Dyn(_) => Type::Ptr,
            // A thin reference is an address — a word-sized value.
            Ty::Ref(t, _) if !t.is_unsized() => Type::Ptr,
            // An aggregate is never an SSA value (TIR §2); it lives in
            // memory and its value is its address. A fat reference is two
            // words and travels the same way.
            Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Struct(_)
            | Ty::Enum(_)
            | Ty::Ref(..)
            | Ty::Slice(_) => Type::Ptr,
        }
    }

    /// Width in trits, for the numeric types.
    fn width(&self) -> Option<u32> {
        match self {
            Ty::Trit | Ty::Bool => Some(1),
            Ty::T9 => Some(9),
            Ty::T27 | Ty::TAddr => Some(27),
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
            Ty::Array(..) | Ty::Tuple(_) | Ty::Struct(_) | Ty::Enum(_) => true,
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
            Ty::Unit | Ty::Never => layout::Ty::Unit,
            Ty::Array(t, n) => layout::Ty::array(t.layout_ty(), *n),
            Ty::Tuple(ts) => layout::Ty::Tuple(ts.iter().map(Ty::layout_ty).collect()),
            Ty::Struct(n) | Ty::Enum(n) => layout::Ty::named(n),
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
    line: Line,
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
    /// Field names and semantic types of each struct, in declaration order.
    structs: RefCell<HashMap<String, Vec<(String, Ty)>>>,
    /// Variants of each enum, in declaration order.
    enums: RefCell<HashMap<String, Vec<VariantInfo>>>,
    /// Generic struct and enum definitions, un-instantiated.
    generic_structs: HashMap<String, ast::StructItem>,
    generic_enums: HashMap<String, ast::EnumItem>,
    /// Trait declarations, so that `dyn Trait` can be checked for object
    /// safety where it is written rather than where it is coerced to.
    traits: HashMap<String, ast::TraitItem>,
    /// What each mangled name was an instantiation of. A mangled name is not
    /// parseable back into its arguments, and a generic impl needs them.
    instantiations: RefCell<HashMap<String, (String, Vec<Ty>)>>,
}

/// One enum variant, resolved.
#[derive(Clone)]
struct VariantInfo {
    name: String,
    fields: Vec<(String, Ty)>,
}

impl Types {
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
            Ty::Struct(n) => {
                let fields = self.structs.borrow()[n].clone();
                self.destructors.contains(n) || fields.iter().any(|(_, t)| self.needs_drop(t))
            }
            Ty::Enum(n) => {
                let variants = self.enums.borrow()[n].clone();
                self.destructors.contains(n)
                    || variants
                        .iter()
                        .any(|v| v.fields.iter().any(|(_, t)| self.needs_drop(t)))
            }
            Ty::Array(t, n) => *n > 0 && self.needs_drop(t),
            Ty::Tuple(ts) => ts.iter().any(|t| self.needs_drop(t)),
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

    /// Instantiate a generic struct or enum, or return the one already made
    /// (Ch. 4 §2.7).
    ///
    /// The instantiation is an ordinary nominal type under a mangled name, so
    /// the layout engine, the drop machinery and code generation never learn
    /// that generics exist.
    fn instantiate(&self, name: &str, args: &[Ty], line: Line) -> R<Ty> {
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
                        line,
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
                line,
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
                .map(|(n, t)| {
                    let ty = resolve_ty_env(t, self, &env)?;
                    check_sized(&ty, t.line(), &format!("the field `{n}`"))?;
                    Ok((n.clone(), ty))
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
                    .map(|(n, t)| Ok((n.clone(), resolve_ty_env(t, self, &env)?)))
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
            return err(line, format!("`{name}` cannot be laid out here: {e}"));
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
pub fn lower(file: &ast::File) -> Result<Module, Vec<Error>> {
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
    let impls: std::collections::HashSet<(String, String)> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Impl(imp) => imp
                .trait_name
                .as_ref()
                .map(|t| (imp.self_ty.clone(), t.clone())),
            _ => None,
        })
        .collect();

    // Impl blocks become ordinary functions before anything else looks at
    // the file, so the rest of lowering never learns they existed.
    let (expanded, impl_errs) = expand_impls(file, &types);
    errs.extend(impl_errs);
    let fns: Vec<ast::FnItem> = file
        .items
        .iter()
        .filter_map(|i| match i {
            ast::Item::Fn(f) => Some(f.clone()),
            _ => None,
        })
        .chain(expanded)
        .collect();

    // A generic function is not code until it is instantiated (§2.7), so it
    // is set aside here and its body lowered once per instantiation.
    let mut generic_fns: HashMap<String, ast::FnItem> = HashMap::new();
    for f in &fns {
        if !f.generics.is_empty() {
            if f.body.is_none() {
                errs.push(SyntaxError {
                    line: f.line,
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
                    line: f.line,
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
                .map(|(n, t)| {
                    let ty = resolve_ty(t, &types)?;
                    check_sized(&ty, t.line(), &format!("the parameter `{n}`"))?;
                    Ok(ty)
                })
                .collect();
            let ret = match &f.ret {
                None => Ok(Ty::Unit),
                Some(t) => resolve_ty(t, &types),
            };
            let self_ref = matches!(
                f.params.first(),
                Some((n, ast::Ty::Ref(..))) if n == "self"
            );
            if let (Ok(p), Ok(r)) = (&params, &ret)
                && let Err(e) = check_returned_reference(p, r, self_ref, f.line)
            {
                errs.push(e);
                continue;
            }
            match (params, ret) {
                (Ok(p), Ok(r)) => {
                    if sigs.borrow_mut().insert(fn_key(f), (p, r)).is_some() {
                        errs.push(SyntaxError {
                            line: f.line,
                            message: format!("`{}` is defined more than once", f.name),
                        });
                    }
                }
                (Err(e), _) | (_, Err(e)) => errs.push(e),
            }
        }
    }

    // Constants next, since a function body may use one.
    let mut globals: HashMap<String, Global> = HashMap::new();
    for item in &file.items {
        if let ast::Item::Const(c) = item {
            match const_item(c, &mut module, &types) {
                Ok(g) => {
                    if globals.insert(c.name.clone(), g).is_some() {
                        errs.push(SyntaxError {
                            line: c.line,
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
    let world = World {
        traits: &types.traits.clone(),
        vtables: &vtables,
        sigs: &sigs,
        generic_fns: &generic_fns,
        impls: &impls,
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
        // Not `while let Some(job) = pending.borrow_mut().pop()`: that keeps
        // the mutable borrow alive for the whole body, and the body queues
        // more jobs.
        let Some(job) = pending.borrow_mut().pop() else {
            break;
        };
        let def = generic_fns[&job.from].clone();
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

    if errs.is_empty() {
        Ok(module)
    } else {
        Err(errs)
    }
}

/// One error, for the places that build one rather than return it.
fn one_err(line: Line, message: String) -> Error {
    SyntaxError { line, message }
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
            m.line,
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
        Some((n, _)) if n == "self" => {}
        _ => return complaint("it takes no `self`"),
    }
    for (i, (_, t)) in m.params.iter().enumerate() {
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

/// The methods Ch. 1 defines on the built-in types. A user method of the
/// same name on the same type would shadow a language rule, so these are
/// matched first and impl blocks never see them.
const BUILTIN_METHODS: &[&str] = &[
    "tmin",
    "tmax",
    "tmul",
    "tneg",
    "is_pos",
    "is_zero",
    "is_neg",
    "to_trit",
    "len",
    "wrapping_add",
    "wrapping_sub",
    "wrapping_mul",
    "saturating_add",
    "saturating_sub",
    "saturating_mul",
    "overflowing_add",
    "overflowing_sub",
    "overflowing_mul",
    "checked_add",
    "checked_sub",
    "checked_mul",
];

/// The name of the hidden out-pointer for an aggregate return.
const SRET: &str = "sret";

/// The name a function is known by. A destructor is keyed by its type, since
/// every type may have one and they would otherwise collide.
fn fn_key(f: &ast::FnItem) -> String {
    match (&f.name[..], f.params.first()) {
        ("drop", Some((p, ast::Ty::Name(ty, _)))) if p == "self" => format!("drop.{ty}"),
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
fn check_returned_reference(params: &[Ty], ret: &Ty, self_ref: bool, line: Line) -> R<()> {
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
            line,
            format!(
                "this function returns {ret} but borrows from nothing: the reference could \
                 only point into a local, which dies when the function does (Ch. 3 §4.1)"
            ),
        ),
        n => err(
            line,
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
fn check_sized(ty: &Ty, line: Line, what: &str) -> R<()> {
    if ty.is_unsized() {
        return err(
            line,
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
        _ => return None,
    })
}

/// Substitute `Self` for the implementing type throughout a method, in type
/// and in path position, so that nothing downstream ever sees `Self`.
fn subst_self(f: &ast::FnItem, self_ty: &SelfTy) -> ast::FnItem {
    let mut f = f.clone();
    for (_, t) in &mut f.params {
        subst_ty(t, self_ty);
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
        ast::Ty::SelfTy(_) => *t = self_ty.ty.clone(),
        ast::Ty::Array(e, _, _) | ast::Ty::Ref(e, _, _) | ast::Ty::Slice(e, _) => {
            subst_ty(e, self_ty)
        }
        ast::Ty::Tuple(ts, _) | ast::Ty::App(_, ts, _) => {
            ts.iter_mut().for_each(|t| subst_ty(t, self_ty))
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
        Call(_, args, _) | Array(args, _) | Tuple(args, _) => kids.extend(args.iter_mut()),
        Method(r, _, args, _) => {
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
        Int(..) | Trit(..) | Bool(..) | Unit(_) | Continue(_) => {}
    }
    for k in kids {
        subst_expr(k, self_ty);
    }
}

/// Expand every `trait` and `impl` in the file into ordinary functions.
///
/// A method becomes a function named `Type.method`, `Self` substituted away
/// and the receiver an ordinary leading parameter. Everything downstream —
/// signatures, calls, drops, the borrow checker — then works unchanged, which
/// is the point: Ch. 4 §1.2's impl block is a naming construct, not a new
/// kind of code.
fn expand_impls(file: &ast::File, types: &Types) -> (Vec<ast::FnItem>, Vec<Error>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let mut traits: HashMap<String, &ast::TraitItem> = HashMap::new();

    for item in &file.items {
        if let ast::Item::Trait(t) = item
            && traits.insert(t.name.clone(), t).is_some()
        {
            errs.push(SyntaxError {
                line: t.line,
                message: format!("`{}` is defined more than once", t.name),
            });
        }
    }
    for t in traits.values() {
        for s in &t.supertraits {
            if !traits.contains_key(s) && s != "Eq" && s != "Ord" {
                errs.push(SyntaxError {
                    line: t.line,
                    message: format!("`{s}` is not a trait in scope"),
                });
            }
        }
    }

    // Which methods each type already has, so a collision is reported rather
    // than silently resolved (§1.3).
    let mut defined: HashMap<String, Line> = HashMap::new();

    for item in &file.items {
        let ast::Item::Impl(imp) = item else { continue };
        let self_ty = &imp.self_ty;
        if !types.structs.borrow().contains_key(self_ty)
            && !types.enums.borrow().contains_key(self_ty)
            && !types.generic_structs.contains_key(self_ty)
            && !types.generic_enums.contains_key(self_ty)
            && !matches!(self_ty.as_str(), "trit" | "bool" | "t9" | "t27" | "taddr")
        {
            errs.push(SyntaxError {
                line: imp.line,
                message: format!("`{self_ty}` is not a type in scope"),
            });
            continue;
        }

        if !imp.generics.is_empty()
            && !types.generic_structs.contains_key(self_ty)
            && !types.generic_enums.contains_key(self_ty)
        {
            errs.push(SyntaxError {
                line: imp.line,
                message: format!(
                    "`{self_ty}` is not a generic type, so this impl has \
                                  type parameters nothing can determine (Ch. 4 §2.1)"
                ),
            });
            continue;
        }
        if !imp.generics.is_empty() && imp.trait_name.as_deref() == Some("Drop") {
            errs.push(SyntaxError {
                line: imp.line,
                message: "a destructor on a generic type is not implemented; whether a \
                          type needs dropping is decided before its instantiations exist"
                    .into(),
            });
            continue;
        }

        // What `Self` means here: the concrete type, or the applied generic.
        let self_repr = SelfTy {
            ty: if imp.self_args.is_empty() {
                ast::Ty::Name(self_ty.clone(), imp.line)
            } else {
                ast::Ty::App(self_ty.clone(), imp.self_args.clone(), imp.line)
            },
            name: self_ty.clone(),
        };

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
                        line: imp.line,
                        message: format!("`{trait_name}` is not a trait in scope"),
                    });
                    continue;
                };
                match check_trait_impl(decl, imp) {
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
                if !f.generics.is_empty() {
                    errs.push(SyntaxError {
                        line: f.line,
                        message: "a method with type parameters of its own, inside a \
                                  generic impl, is not implemented"
                            .into(),
                    });
                    continue;
                }
                f.generics.clone_from(&imp.generics);
            }
            // A destructor keeps its own name so that `fn_key` gives it the
            // `drop.Type` key the drop machinery already uses (Ch. 3 §1.4).
            let is_destructor = imp.trait_name.as_deref() == Some("Drop") && f.name == "drop";
            if !is_destructor && f.name == "drop" {
                errs.push(SyntaxError {
                    line: f.line,
                    message: "a destructor is written `impl Drop for T` (Ch. 4 §5.2); \
                              an inherent `drop` would never be called"
                        .into(),
                });
                continue;
            }
            let key = if is_destructor {
                format!("drop.{self_ty}")
            } else {
                format!("{self_ty}.{}", f.name)
            };
            if let Some(first) = defined.insert(key.clone(), f.line) {
                errs.push(SyntaxError {
                    line: f.line,
                    message: format!(
                        "`{self_ty}` already has a method `{}` (line {first}); \
                         §1.3 requires the ambiguity be written out",
                        f.name
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
    (out, errs)
}

/// `impl Drop for T` must supply exactly `fn drop(self)` (Ch. 4 §5.2).
fn check_drop_impl(imp: &ast::ImplItem) -> R<()> {
    let bad = |line| {
        err(
            line,
            "`impl Drop` supplies exactly one method, `fn drop(self)` (Ch. 4 §5.2)",
        )
    };
    if imp.methods.len() != 1 {
        return bad(imp.line);
    }
    let m = &imp.methods[0];
    if m.name != "drop" || m.ret.is_some() {
        return bad(m.line);
    }
    match m.params.as_slice() {
        [(n, ast::Ty::SelfTy(_))] if n == "self" => Ok(()),
        _ => err(
            m.line,
            "a destructor takes `self` by value, so that dropping its fields is not \
             a drop of `self` (Ch. 3 §1.4)",
        ),
    }
}

/// Check an impl against its trait, and return the provided methods it did
/// not override (Ch. 4 §§1.2, 1.5).
fn check_trait_impl(decl: &ast::TraitItem, imp: &ast::ImplItem) -> R<Vec<ast::FnItem>> {
    for m in &imp.methods {
        let Some(want) = decl.methods.iter().find(|d| d.name == m.name) else {
            return err(
                m.line,
                format!(
                    "`{}` has no method `{}`, and a trait impl may supply nothing else",
                    decl.name, m.name
                ),
            );
        };
        let same_ret = match (&want.ret, &m.ret) {
            (None, None) => true,
            (Some(a), Some(b)) => same_ast_ty(a, b),
            _ => false,
        };
        if want.params.len() != m.params.len() || !same_ret {
            return err(
                m.line,
                format!(
                    "`{}` does not match the signature `{}` declares",
                    m.name, decl.name
                ),
            );
        }
        for ((_, a), (_, b)) in want.params.iter().zip(&m.params) {
            if !same_ast_ty(a, b) {
                return err(
                    m.line,
                    format!(
                        "`{}` does not match the signature `{}` declares",
                        m.name, decl.name
                    ),
                );
            }
        }
        if m.body.is_none() {
            return err(m.line, format!("`{}` needs a body here", m.name));
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
                    imp.line,
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
            .map(|((n, _), t)| (n.clone(), t.tir())),
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
        let (name, derives, line, generics) = match item {
            ast::Item::Struct(s) => (&s.name, &s.derives, s.line, &s.generics),
            ast::Item::Enum(e) => (&e.name, &e.derives, e.line, &e.generics),
            _ => continue,
        };
        if derives.is_empty() {
            continue;
        }
        if !generics.is_empty() {
            errs.push(SyntaxError {
                line,
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
                        line,
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
                    Err(e) => errs.push(SyntaxError { line, message: e }),
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
                .replace([' ', ',', '&', '[', ']', '(', ')'], "_"),
        );
    }
    out
}

/// Resolve every nominal type in the file and hand them to the layout engine.
fn build_types(file: &ast::File) -> R<Types> {
    let mut types = Types {
        db: RefCell::new(layout::TypeDb::new()),
        destructors: std::collections::BTreeSet::new(),
        structs: RefCell::new(HashMap::new()),
        enums: RefCell::new(HashMap::new()),
        generic_structs: HashMap::new(),
        generic_enums: HashMap::new(),
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
                    .map(|(n, t)| {
                        let ty = resolve_ty(t, &types)?;
                        check_sized(&ty, t.line(), &format!("the field `{n}`"))?;
                        Ok((n.clone(), ty))
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
                        .map(|(n, t)| Ok((n.clone(), resolve_ty(t, &types)?)))
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
                imp.line,
                format!(
                    "`{}` cannot have a destructor: it is not declared in this file",
                    imp.self_ty
                ),
            );
        }
        if !types.destructors.insert(imp.self_ty.clone()) {
            return err(
                imp.line,
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
        let [(param, ty)] = &f.params[..] else {
            return err(f.line, "`drop` takes exactly one parameter, named `self`");
        };
        if param != "self" {
            return err(f.line, "a destructor's parameter must be named `self`");
        }
        let ty = resolve_ty(ty, &types)?;
        let name = match &ty {
            Ty::Struct(n) | Ty::Enum(n) => n.clone(),
            other => {
                return err(
                    f.line,
                    format!("`{other}` cannot have a destructor: it is not declared in this file"),
                );
            }
        };
        if f.ret.is_some() {
            return err(f.line, "a destructor returns nothing");
        }
        if !types.destructors.insert(name.clone()) {
            return err(f.line, format!("`{name}` has more than one destructor"));
        }
    }

    // Ask the layout engine about every nominal type now, so that an
    // ill-formed one — an infinite type, a duplicate discriminant — is
    // reported here rather than at its first use.
    for item in &file.items {
        let (name, line) = match item {
            ast::Item::Struct(s) if s.generics.is_empty() => (&s.name, s.line),
            ast::Item::Enum(e) if e.generics.is_empty() => (&e.name, e.line),
            _ => continue,
        };
        if let Err(e) = layout::layout_of(&types.db.borrow(), &layout::Ty::named(name)) {
            return err(line, e.to_string());
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
        // Substitution replaces every `Self` before lowering (Ch. 4 §1.2),
        // so one surviving here was written where there is no impl.
        ast::Ty::SelfTy(l) => err(
            *l,
            "`Self` names the implementing type, and there is none here",
        ),
        // `Name<T, U>` — instantiate now, so that everything downstream sees
        // an ordinary nominal type (Ch. 4 §2.7).
        ast::Ty::App(name, args, line) => {
            let args: Vec<Ty> = args
                .iter()
                .map(|a| resolve_ty_env(a, types, env))
                .collect::<R<_>>()?;
            types.instantiate(name, &args, *line)
        }
        // §3.4: a trait may be used as an object only if every method is
        // object-safe, and this is where "used as" happens.
        ast::Ty::Dyn(name, line) => {
            if !types.traits.contains_key(name) {
                return err(*line, format!("`{name}` is not a trait in scope"));
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
        ast::Ty::Slice(t, line) => {
            let elem = resolve_ty_env(t, types, env)?;
            if elem.is_unsized() {
                return err(*line, "a slice element must have a size");
            }
            Ok(Ty::Slice(Box::new(elem)))
        }
        ast::Ty::Name(name, line) => match name.as_str() {
            "trit" => Ok(Ty::Trit),
            "bool" => Ok(Ty::Bool),
            "t9" => Ok(Ty::T9),
            "t27" => Ok(Ty::T27),
            "taddr" => Ok(Ty::TAddr),
            // Ch. 1 §8 claims these so no user identifier can take them.
            "t3" | "t81" | "f27" => err(
                *line,
                format!("`{name}` is a reserved type name (Ch. 1 §8)"),
            ),
            // A type parameter in scope.
            other if env.contains_key(other) => Ok(env[other].clone()),
            other
                if types.generic_structs.contains_key(other)
                    || types.generic_enums.contains_key(other) =>
            {
                err(
                    *line,
                    format!("`{other}` is generic and needs its arguments written: `{other}<…>`"),
                )
            }
            other if types.structs.borrow().contains_key(other) => {
                Ok(Ty::Struct(other.to_string()))
            }
            other if types.enums.borrow().contains_key(other) => Ok(Ty::Enum(other.to_string())),
            other => err(*line, format!("`{other}` is not a type in scope")),
        },
        ast::Ty::Array(elem, count, line) => {
            let elem = resolve_ty_env(elem, types, env)?;
            let n = const_int(count)?;
            if n < 0 {
                // Ch. 2 §3: the type-level face of the signed-taddr decision.
                return err(*line, format!("array length {n} is negative"));
            }
            Ok(Ty::Array(Box::new(elem), n))
        }
    }
}

/// Evaluate a constant expression. Ch. 0 §3.2: exactly, in balanced ternary.
fn const_int(e: &ast::Expr) -> R<i128> {
    match e {
        ast::Expr::Int(v, line) => v
            .to_i128()
            .ok_or(())
            .or_else(|()| err(*line, format!("{v} is too large"))),
        other => err(
            other.line(),
            "this milestone evaluates only integer literals in constant position",
        ),
    }
}

fn const_item(c: &ast::ConstItem, module: &mut Module, types: &Types) -> R<Global> {
    let ty = resolve_ty(&c.ty, types)?;
    match (&ty, &c.value) {
        (Ty::Array(elem, n), ast::Expr::Array(items, line)) => {
            if items.len() as i128 != *n {
                return err(
                    *line,
                    format!("expected {n} elements, found {}", items.len()),
                );
            }
            let mut trytes = Vec::new();
            for item in items {
                let v = const_int(item)?;
                let width = elem.width().unwrap_or(27);
                if !Bt::from_i128(v).fits_width(width) {
                    return err(item.line(), format!("{v} does not fit in {elem}"));
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
                return err(c.value.line(), format!("{v} does not fit in {t}"));
            }
            Ok(Global::Const(Bt::from_i128(v), ty))
        }
        _ => err(c.line, "this constant's form is not supported yet"),
    }
}

// --------------------------------------------------------------- lowering

struct Fn<'a> {
    /// Signatures, shared and mutable: instantiating a generic function adds
    /// one, and the call site that caused it must be able to check its
    /// arguments against it immediately (Ch. 4 §2.7).
    sigs: &'a RefCell<HashMap<String, (Vec<Ty>, Ty)>>,
    /// Trait declarations, for `dyn Trait` (Ch. 4 §3).
    traits: &'a HashMap<String, ast::TraitItem>,
    /// Vtables built so far (Ch. 4 §3.3).
    vtables: &'a RefCell<Vec<(String, String, ir::Global)>>,
    /// Generic function definitions, un-instantiated.
    generic_fns: &'a HashMap<String, ast::FnItem>,
    /// Every (type, trait) pair the file implements, for bounds (§2.2).
    impls: &'a std::collections::HashSet<(String, String)>,
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
    owned: Vec<(String, Owns, usize)>,
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

#[derive(Clone)]
struct LoopCtx {
    /// Where `break` goes.
    exit: String,
    /// Where `continue` goes.
    head: String,
    /// The slot a `break` with a value writes to.
    result: Option<(String, Ty)>,
}

/// Everything a function body is lowered against, which is the same for
/// every function in the module and is therefore passed as one thing.
struct World<'a> {
    /// Trait declarations, for `dyn Trait` (Ch. 4 §3).
    traits: &'a HashMap<String, ast::TraitItem>,
    /// Vtables built so far, keyed by (concrete type, trait), and the
    /// globals they became.
    vtables: &'a RefCell<Vec<(String, String, ir::Global)>>,
    sigs: &'a RefCell<HashMap<String, (Vec<Ty>, Ty)>>,
    generic_fns: &'a HashMap<String, ast::FnItem>,
    impls: &'a std::collections::HashSet<(String, String)>,
    pending: &'a RefCell<Vec<Job>>,
    globals: &'a HashMap<String, Global>,
    types: &'a Types,
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
        traits,
        vtables,
        sigs,
        generic_fns,
        impls,
        pending,
        globals,
        types,
    } = *w;
    let (param_tys, ret) = sigs.borrow().get(key).cloned().unwrap();
    let destructor_of = key.strip_prefix("drop.").map(|t| t.to_string());
    let mut fx = Fn {
        traits,
        vtables,
        sigs,
        generic_fns,
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
        params: f.params.iter().map(|(n, _)| n.clone()).collect(),
        self_lends: matches!(
            f.params.first(),
            Some((n, ast::Ty::Ref(..))) if n == "self"
        ),
        destructor_of,
        counter: 0,
        done: false,
    };

    // Parameters arrive as SSA values and are spilled into slots at once, so
    // that they read and write like any other local.
    for ((name, _), ty) in f.params.iter().zip(&param_tys) {
        // The parameter arrives as an SSA value; give the local its own
        // storage so that writing to it is local, and copy an aggregate in.
        let incoming = Operand::Value(name.clone());
        let local = fx.declare(name, ty.clone(), true);
        let slot = local.slot.clone();
        fx.store_at(&slot, 0, ty, incoming, f.line)?;
    }

    if let Some(tail) = &body.tail {
        fx.check_return_root(tail, &ret, body.line)?;
    }
    let (value, ty) = fx.block(body, Some(&ret))?;
    if !fx.done {
        // The parameters are this function's to drop too (Ch. 3 §1.1).
        if ret == Ty::Unit {
            fx.drop_all(f.line)?;
            fx.finish(Terminator::Ret(None));
        } else {
            fx.check(&ty, &ret, body.line, "function body")?;
            if ret.is_aggregate() {
                let dst = Operand::Value(SRET.to_string());
                fx.copy_typed(dst, value, &ret, body.line)?;
                fx.drop_all(f.line)?;
                fx.finish(Terminator::Ret(None));
            } else {
                fx.drop_all(f.line)?;
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

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool) -> Local {
        let slot = self.fresh(&format!("{name}.slot"));
        let trytes = self.types.size(&ty).max(1) as u32;
        self.slots.push(Inst {
            results: vec![slot.clone()],
            kind: InstKind::Slot { trytes },
        });
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
        };
        let scope = self.scopes.last_mut().expect("a scope");
        scope.insert(name.to_string(), local.clone());
        if let Some(flag) = &drop_flag {
            let one = Operand::Const(Type::Int(1), Bt::from_i128(1));
            let flag = flag.clone();
            self.store_slot(&flag, &Ty::Bool, one);
            self.owned
                .push((name.to_string(), Owns::Yes, self.scopes.len() - 1));
        }
        local
    }

    /// Record that a place was moved out of (Ch. 3 §1.2).
    fn mark_moved(&mut self, name: &str) {
        if let Some(entry) = self.owned.iter_mut().rev().find(|(n, _, _)| n == name) {
            entry.1 = Owns::No;
        }
        if let Some(local) = self.lookup(name)
            && let Some(flag) = local.drop_flag
        {
            let zero = Operand::Const(Type::Int(1), Bt::ZERO);
            self.store_slot(&flag, &Ty::Bool, zero);
        }
    }

    /// The ownership state, for saving across a branch.
    fn owned_snapshot(&self) -> Vec<(String, Owns, usize)> {
        self.owned.clone()
    }

    /// Join two paths' ownership: a value moved on either is not certainly
    /// owned afterwards, and its drop is decided by its flag.
    fn owned_join(&mut self, a: Vec<(String, Owns, usize)>, b: Vec<(String, Owns, usize)>) {
        self.owned = a
            .into_iter()
            .map(|(n, o, d)| {
                let other = b
                    .iter()
                    .find(|(m, _, _)| *m == n)
                    .map(|(_, o, _)| *o)
                    .unwrap_or(o);
                (n, o.join(other), d)
            })
            .collect();
    }

    /// Whether a local is known to still own its value.
    fn ownership(&self, name: &str) -> Option<Owns> {
        self.owned
            .iter()
            .rev()
            .find(|(n, _, _)| n == name)
            .map(|(_, o, _)| *o)
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
    }

    // -------------------------------------------------------------- checks

    fn check(&self, got: &Ty, want: &Ty, line: Line, what: &str) -> R<()> {
        if got == want || *got == Ty::Never {
            return Ok(());
        }
        err(
            line,
            format!("{what} has type {got}, expected {want} (there are no implicit conversions)"),
        )
    }

    // --------------------------------------------------------------- items

    fn block(&mut self, b: &ast::Block, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        let depth = self.scopes.len();
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
                self.expr(e, expected)?
            }
            None => (unit(), Ty::Unit),
        };
        self.drop_scope(depth, b.line)?;
        self.scopes.pop();
        Ok(result)
    }

    /// Drop every local of this scope that still owns its value, in reverse
    /// order of declaration (Ch. 3 §1.4).
    fn drop_scope(&mut self, depth: usize, line: Line) -> R<()> {
        if self.done {
            // Control left by another path; whatever terminated it is
            // responsible for its own drops.
            self.owned.retain(|(_, _, d)| *d < depth);
            return Ok(());
        }
        let mine: Vec<(String, Owns)> = self
            .owned
            .iter()
            .filter(|(_, _, d)| *d >= depth)
            .map(|(n, o, _)| (n.clone(), *o))
            .collect();
        self.owned.retain(|(_, _, d)| *d < depth);

        for (name, owns) in mine.into_iter().rev() {
            if owns == Owns::No {
                continue;
            }
            let Some(local) = self.lookup(&name) else {
                continue;
            };
            // Inside a destructor, `self` is not dropped as a whole.
            let fields_only = name == "self" && self.destructor_of.is_some();
            match (owns, local.drop_flag.clone()) {
                (Owns::Yes, _) if fields_only => {
                    let addr = Operand::Value(local.slot.clone());
                    self.drop_fields(addr, &local.ty, line, 0)?;
                }
                (Owns::Yes, _) => self.emit_drop(&local.slot, &local.ty, line)?,
                // Ownership depends on the path taken, so the flag decides.
                (_, Some(flag)) => {
                    let f = self.load_slot(&flag, &Ty::Bool);
                    let (yes, join) = (self.fresh("drop.yes"), self.fresh("drop.join"));
                    self.br3(f, &join, &join, &yes);
                    self.start(yes);
                    self.emit_drop(&local.slot, &local.ty, line)?;
                    self.jump(&join);
                    self.start(join);
                }
                _ => self.emit_drop(&local.slot, &local.ty, line)?,
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
    fn check_access(&mut self, path: &PlacePath, access: Access, line: Line) -> R<()> {
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
            return err(line, format!("`{root}` {what} (Ch. 3 §4.1)"));
        }

        self.retire_loans();
        for loan in &self.loans {
            if !loan.place.conflicts(path) {
                continue;
            }
            let borrowed = describe(&loan.place);
            let since = loan.line;
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
                    line,
                    format!(
                        "`{borrowed}` cannot be {what} here: it is {kind} borrowed \
                         on line {since}, and that borrow is still live (Ch. 3 §2.2)"
                    ),
                );
            }
        }
        Ok(())
    }

    /// Record a borrow. It dies after the last use of whatever holds it,
    /// which the caller patches once the binding is known.
    fn add_loan(&mut self, path: PlacePath, mutable: bool, line: Line) {
        let dies = self.stmt;
        self.loans.push(Loan {
            place: path,
            mutable,
            dies,
            line,
        });
    }

    /// A returned reference must be rooted at a parameter. Rooted at a local
    /// it would dangle, which is what §4.1 exists to prevent.
    fn check_return_root(&mut self, e: &ast::Expr, ty: &Ty, line: Line) -> R<()> {
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
                line,
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
    fn drop_all(&mut self, line: Line) -> R<()> {
        self.drop_scope(0, line)
    }

    /// A value moved out of inside a loop would be moved again on the next
    /// iteration.
    fn check_no_move_in_loop(&mut self, before: &[(String, Owns, usize)], line: Line) -> R<()> {
        for (name, was, _) in before {
            if *was != Owns::Yes {
                continue;
            }
            if let Some(now) = self.ownership(name)
                && now != Owns::Yes
            {
                return err(
                    line,
                    format!("`{name}` is moved out of here, and the loop may reach this again"),
                );
            }
        }
        Ok(())
    }

    /// The drop glue of a type: its own destructor, then its fields'.
    fn emit_drop(&mut self, addr: &str, ty: &Ty, line: Line) -> R<()> {
        self.drop_at(Operand::Value(addr.to_string()), ty, line, 0)
    }

    fn drop_at(&mut self, addr: Operand, ty: &Ty, line: Line, depth: u32) -> R<()> {
        if depth > 8 {
            return err(line, "drop glue nested too deeply");
        }
        if !self.types.needs_drop(ty) {
            return Ok(());
        }
        // The destructor runs first, then the fields it did not move out of
        // (Ch. 3 §1.4).
        if let Ty::Struct(n) | Ty::Enum(n) = ty
            && self.types.destructors.contains(n)
        {
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Call {
                    callee: Callee::Direct(format!("drop.{n}")),
                    args: vec![addr.clone()],
                    ret: None,
                },
            });
        }
        self.drop_fields(addr, ty, line, depth)
    }

    /// The second half of dropping a value: its fields, without its own
    /// destructor. This is what a destructor's `self` gets.
    fn drop_fields(&mut self, addr: Operand, ty: &Ty, line: Line, depth: u32) -> R<()> {
        if depth > 8 {
            return err(line, "drop glue nested too deeply");
        }
        match ty {
            Ty::Struct(_) | Ty::Tuple(_) => {
                for (_, ft, off) in self.types.fields(ty) {
                    if self.types.needs_drop(&ft) {
                        let at = self.offset(addr.clone(), off);
                        self.drop_at(at, &ft, line, depth + 1)?;
                    }
                }
            }
            Ty::Array(elem, n) => {
                let size = self.types.size(elem);
                let (elem, n) = ((**elem).clone(), *n);
                if self.types.needs_drop(&elem) {
                    for i in 0..n {
                        let at = self.offset(addr.clone(), i * size);
                        self.drop_at(at, &elem, line, depth + 1)?;
                    }
                }
            }
            // An enum's payload varies by variant, so dropping it needs a
            // dispatch this milestone does not emit; its own destructor ran.
            _ => {}
        }
        Ok(())
    }

    fn stmt(&mut self, s: &ast::Stmt) -> R<()> {
        match s {
            ast::Stmt::Let {
                mutable,
                name,
                ty,
                value,
                line,
            } => {
                let declared = match ty {
                    Some(t) => {
                        let d = self.resolve(t)?;
                        // A dynamically sized type is legal only behind a
                        // reference (Ch. 3 §5.1, Ch. 4 §3.1).
                        check_sized(&d, *line, &format!("the local `{name}`"))?;
                        Some(d)
                    }
                    None => None,
                };
                let loans_before = self.loans.len();
                let (v, vt) = self.expr(value, declared.as_ref())?;
                let ty = match declared {
                    Some(d) => {
                        self.check(&vt, &d, *line, "initializer")?;
                        d
                    }
                    None if vt == Ty::Never || vt == Ty::Unit => {
                        return err(*line, format!("cannot bind a value of type {vt}"));
                    }
                    None => vt,
                };
                // Every loan the initializer created is held by this
                // binding, and lives until its last use (Ch. 3 §4.2). That
                // covers a borrow written here and a reference returned from
                // a call, which by elision borrows from that call's argument.
                if contains_reference(&ty) {
                    let dies = self.last_use.get(name).copied().unwrap_or(self.stmt);
                    for loan in self.loans[loans_before..].iter_mut() {
                        loan.dies = loan.dies.max(dies);
                    }
                }
                check_sized(&ty, *line, &format!("the binding `{name}`"))?;
                if !ty.is_scalar() && !ty.is_aggregate() {
                    return err(*line, format!("cannot bind a value of type {ty}"));
                }
                let local = self.declare(name, ty.clone(), *mutable);
                // An aggregate is copied into the binding's own storage, so
                // that writing to one does not write through to another.
                let slot = local.slot.clone();
                self.store_at(&slot, 0, &ty, v, *line)?;
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
        // `&Concrete` becomes `&dyn Trait` wherever one is expected — the
        // only implicit conversion the language has, and it converts a
        // representation rather than a value (Ch. 4 §3.2).
        if let Some(want) = expected
            && ty != *want
            && let Some(fat) = self.coerce_dyn(v.clone(), &ty, want, e.line())?
        {
            return Ok(fat);
        }
        Ok((v, ty))
    }

    fn expr_inner(&mut self, e: &ast::Expr, expected: Option<&Ty>) -> R<(Operand, Ty)> {
        use ast::Expr as E;
        match e {
            // An unconstrained integer literal is `t27` (Ch. 1 §3), and one
            // that does not fit its type is an error, never a wrap.
            E::Int(v, line) => {
                let ty = match expected {
                    Some(t) if t.is_arithmetic() => t.clone(),
                    _ => Ty::T27,
                };
                let width = ty.width().unwrap_or(27);
                if !v.fits_width(width) {
                    return err(*line, format!("{v} does not fit in {ty}"));
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

            E::Path(name, line) => {
                if let Some(local) = self.lookup(name) {
                    // Reading a value that is not copyable moves it, and a
                    // moved-out place may not be read (Ch. 3 §1.2).
                    if !self.types.is_copyable(&local.ty) {
                        match self.ownership(name) {
                            Some(Owns::No) => {
                                return err(
                                    *line,
                                    format!("`{name}` was moved out of and cannot be used again"),
                                );
                            }
                            Some(Owns::Maybe) => {
                                return err(
                                    *line,
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
                        self.check_access(&path, Access::Move, *line)?;
                        self.mark_moved(name);
                    }
                    let path = PlacePath {
                        root: name.clone(),
                        projections: Vec::new(),
                    };
                    self.check_access(&path, Access::Read, *line)?;
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
                    None => err(*line, format!("`{name}` is not in scope")),
                }
            }

            E::Unary(op, inner, line) => self.unary(op, inner, expected, *line),
            E::Binary(op, a, b, line) => self.binary(op, a, b, expected, *line),
            E::Assign(op, target, value, line) => self.assign(op, target, value, *line),
            E::Cast(inner, ty, line) => self.cast(inner, ty, *line),
            E::Call(name, args, line) => self.call(name, args, expected, *line),
            E::Method(recv, name, args, line) => self.method(recv, name, args, *line),

            E::Block(b) => self.block(b, expected),
            E::If(cond, then, els, line) => {
                self.if_expr(cond, then, els.as_deref(), expected, *line)
            }
            E::Match(scrutinee, arms, line) => self.match_expr(scrutinee, arms, expected, *line),
            E::While(cond, body, line) => self.while_expr(cond, body, *line),
            E::Loop(body, line) => self.loop_expr(body, expected, *line),

            E::Break(value, line) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*line, "`break` outside a loop");
                };
                match (value, &ctx.result) {
                    (Some(v), Some((slot, ty))) => {
                        let (val, vt) = self.expr(v, Some(ty))?;
                        self.check(&vt, ty, *line, "`break` value")?;
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
                        return err(*line, "this loop's `break` cannot carry a value");
                    }
                    (None, _) => {}
                }
                self.jump(&ctx.exit);
                Ok((unit(), Ty::Never))
            }

            E::Continue(line) => {
                let Some(ctx) = self.loops.last().cloned() else {
                    return err(*line, "`continue` outside a loop");
                };
                self.jump(&ctx.head);
                Ok((unit(), Ty::Never))
            }

            E::Return(value, line) => {
                let ret = self.ret.clone();
                match value {
                    Some(v) => {
                        self.check_return_root(v, &self.ret.clone(), *line)?;
                        let (val, vt) = self.expr(v, Some(&ret))?;
                        self.check(&vt, &ret, *line, "returned value")?;
                        if ret.is_aggregate() {
                            let dst = Operand::Value(SRET.to_string());
                            self.copy_typed(dst, val, &ret, *line)?;
                            self.drop_all(*line)?;
                            self.finish(Terminator::Ret(None));
                        } else {
                            self.drop_all(*line)?;
                            self.finish(Terminator::Ret(Some(val)));
                        }
                    }
                    None => {
                        self.check(&Ty::Unit, &ret, *line, "`return` with no value")?;
                        self.drop_all(*line)?;
                        self.finish(Terminator::Ret(None));
                    }
                }
                Ok((unit(), Ty::Never))
            }

            // An array literal builds its storage and fills it, like any
            // other aggregate.
            E::Array(items, line) => {
                let hint = match expected {
                    Some(Ty::Array(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let mut values = Vec::new();
                let mut elem = hint.clone();
                for item in items {
                    let (v, t) = self.expr(item, elem.as_ref())?;
                    if let Some(want) = &elem {
                        self.check(&t, want, item.line(), "array element")?;
                    } else {
                        elem = Some(t);
                    }
                    values.push(v);
                }
                let Some(elem) = elem else {
                    return err(*line, "an empty array literal needs a written type");
                };
                let ty = Ty::Array(Box::new(elem.clone()), values.len() as i128);
                let slot = self.temp_slot(&ty);
                let size = self.types.size(&elem);
                for (i, v) in values.into_iter().enumerate() {
                    self.store_at(&slot, i as i128 * size, &elem, v, *line)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Repeat(value, count, line) => {
                let hint = match expected {
                    Some(Ty::Array(t, _)) => Some((**t).clone()),
                    _ => None,
                };
                let n = const_int(count)?;
                if n < 0 {
                    return err(*line, format!("array length {n} is negative"));
                }
                let (v, elem) = self.expr(value, hint.as_ref())?;
                let ty = Ty::Array(Box::new(elem.clone()), n);
                let slot = self.temp_slot(&ty);
                let size = self.types.size(&elem);
                for i in 0..n {
                    self.store_at(&slot, i * size, &elem, v.clone(), *line)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Tuple(items, line) => {
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
                    self.store_at(&slot, off, &ft, v, *line)?;
                }
                Ok((Operand::Value(slot), ty))
            }

            E::Aggregate(path, fields, line) => self.aggregate(path, fields, expected, *line),

            // A borrow is the address of a place — which every local already
            // has, since every local lives in a slot.
            E::Borrow(place, mutable, line) => {
                if let Some(path) = self.path_of(place) {
                    self.check_access(&path, Access::Borrow(*mutable), *line)?;
                    self.add_loan(path, *mutable, *line);
                }
                let (addr, ty) = self.place(place, *line)?;
                // An array reference coerces to a slice reference, which is a
                // pointer and a length (Ch. 3 §5.3).
                if let Ty::Array(elem, n) = &ty {
                    let slice = Ty::Ref(Box::new(Ty::Slice(elem.clone())), *mutable);
                    let slot = self.temp_slot(&slice);
                    let len = Operand::Const(Type::Int(27), Bt::from_i128(*n));
                    // A fat pointer is a pointer and a length (Ch. 3 §5.2).
                    let at = Operand::Value(slot.clone());
                    self.store_ptr(at, addr);
                    self.store_at(&slot, 3, &Ty::TAddr, len, *line)?;
                    return Ok((Operand::Value(slot), slice));
                }
                Ok((addr, Ty::Ref(Box::new(ty), *mutable)))
            }

            E::Deref(inner, line) => {
                let (v, ty) = self.expr(inner, None)?;
                let Ty::Ref(target, _) = ty else {
                    return err(*line, format!("`*` applies to a reference, not {ty}"));
                };
                if target.is_unsized() {
                    return err(*line, format!("cannot read a value of type {target}"));
                }
                Ok((self.load_from(v, &target), *target))
            }

            E::Field(..) | E::Index(..) => {
                let line = e.line();
                if let Some(path) = self.path_of(e) {
                    self.check_access(&path, Access::Read, line)?;
                }
                let (addr, ty) = self.place(e, line)?;
                Ok((self.load_from(addr, &ty), ty))
            }
        }
    }

    fn unary(
        &mut self,
        op: &str,
        inner: &ast::Expr,
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (v, ty) = self.expr(inner, expected)?;
        match op {
            // Negation is total at every width (Ch. 1 §1, P1).
            "-" if ty.is_arithmetic() || ty == Ty::Trit => {
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }
            "-" => err(line, format!("`-` does not apply to {ty}")),
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
            "!" => err(line, format!("`!` applies to bool, not {ty}")),
            _ => err(line, format!("unknown unary operator `{op}`")),
        }
    }

    fn binary(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        // Short-circuit operators are control flow, not arithmetic.
        if op == "&&" || op == "||" {
            return self.short_circuit(op, a, b, line);
        }

        let arith_hint = expected.filter(|t| t.is_arithmetic());
        let (va, ta) = self.expr(a, arith_hint)?;
        let (vb, tb) = self.expr(b, Some(&ta))?;
        // Mixed-width arithmetic is a compile-time error (Ch. 1, P2).
        self.check(&tb, &ta, line, "right operand")?;

        // A comparison of two nominal values goes through `Ord` and `Eq`
        // (Ch. 4 §5.3), which is the only place an operator on a user type
        // means a call. Both operands are aggregates, so both are already
        // the addresses the comparison wants.
        if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "<=>") && ta.is_aggregate() {
            let Some(name) = nominal_name(&ta) else {
                return err(line, format!("`{op}` does not apply to {ta}"));
            };
            return self.compare_nominal(op, &name, va, vb, line);
        }

        let tir = ta.tir();
        match op {
            "+" | "-" | "*" | "<<" => {
                if !ta.is_arithmetic() {
                    return err(line, format!("`{op}` does not apply to {ta}"));
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
                    return err(line, format!("`{op}` does not apply to {ta}"));
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

            _ => err(line, format!("unknown operator `{op}`")),
        }
    }

    /// `&&` and `||` short-circuit, so they are branches (Ch. 0 §2.4).
    fn short_circuit(
        &mut self,
        op: &str,
        a: &ast::Expr,
        b: &ast::Expr,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (va, ta) = self.expr(a, Some(&Ty::Bool))?;
        self.check(&ta, &Ty::Bool, line, "left operand")?;

        let slot = self.temp_slot(&Ty::Bool);
        let rhs = self.fresh("sc.rhs");
        let join = self.fresh("sc.join");
        let short = self.fresh("sc.short");

        // Store the short-circuit answer, then test.
        self.store_slot(&slot, &Ty::Bool, va.clone());
        if op == "&&" {
            self.br3(va, &short, &short, &rhs);
        } else {
            self.br3(va, &rhs, &rhs, &short);
        }

        self.start(short);
        self.jump(&join);

        self.start(rhs);
        let (vb, tb) = self.expr(b, Some(&Ty::Bool))?;
        self.check(&tb, &Ty::Bool, line, "right operand")?;
        self.store_slot(&slot, &Ty::Bool, vb);
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        // Writing to a local needs `mut`; writing through a reference needs
        // that reference to be exclusive (Ch. 3 §2.1).
        self.check_writable(target, line)?;
        if let Some(path) = self.path_of(target) {
            self.check_access(&path, Access::Write, line)?;
        }
        let (addr, ty) = self.place(target, line)?;

        let v = if op == "=" {
            let (v, vt) = self.expr(value, Some(&ty))?;
            self.check(&vt, &ty, line, "assigned value")?;
            v
        } else {
            // `a op= b` is `a = a op b`, with `a` evaluated once (Ch. 0 §2.2).
            let binop = &op[..op.len() - 1];
            let current = self.load_from(addr.clone(), &ty);
            let (rhs, rt) = self.expr(value, Some(&ty))?;
            self.check(&rt, &ty, line, "assigned value")?;
            self.apply_binary(binop, current, rhs, &ty, line)?
        };

        if ty.is_aggregate() {
            self.copy_typed(addr, v, &ty, line)?;
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
        Ok((unit(), Ty::Unit))
    }

    /// The root of a place decides whether it may be written to.
    fn check_writable(&mut self, target: &ast::Expr, line: Line) -> R<()> {
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
                        line,
                        "cannot write through a shared reference; it would need `&mut`",
                    ),
                    Some(Ty::Ref(_, true)) => Ok(()),
                    _ => self.check_writable(base, line),
                }
            }
            ast::Expr::Deref(inner, l) => {
                let ty = self.type_of_place(inner)?;
                match ty {
                    Some(Ty::Ref(_, true)) => Ok(()),
                    Some(Ty::Ref(_, false)) => err(
                        *l,
                        "cannot write through a shared reference; it would need `&mut`",
                    ),
                    _ => err(*l, "`*` applies to a reference"),
                }
            }
            _ => err(line, "this expression is not a place"),
        }
    }

    /// The type of a place, without emitting anything. `None` means the
    /// expression is not a place, not that it has no type.
    fn type_of_place(&mut self, e: &ast::Expr) -> R<Option<Ty>> {
        let through_refs = |mut t: Ty| {
            while let Ty::Ref(inner, _) = t.clone() {
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
                Some(Ty::Ref(target, _)) => Some(*target),
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
        line: Line,
    ) -> R<Operand> {
        if !ty.is_arithmetic() {
            return err(line, format!("`{op}=` does not apply to {ty}"));
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
            _ => return err(line, format!("unknown operator `{op}=`")),
        })
    }

    /// `x as T` (Ch. 1 §6).
    fn cast(&mut self, inner: &ast::Expr, to: &ast::Ty, line: Line) -> R<(Operand, Ty)> {
        let to = self.resolve(to)?;
        let (v, from) = self.expr(inner, None)?;

        // A fieldless enum may be cast to an integer, yielding its
        // discriminant. There is no cast in the reverse direction — that is
        // fallible, and library `try_from` territory (Ch. 2 §5.3).
        if let Ty::Enum(name) = &from {
            if !to.is_arithmetic() && to != Ty::Trit {
                return err(line, format!("an enum casts only to an integer, not {to}"));
            }
            if self.types.enums.borrow()[name]
                .iter()
                .any(|v| !v.fields.is_empty())
            {
                return err(
                    line,
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

        // `trit` ↔ `bool` has no `as` path by design: both mappings are
        // plausible, so the language refuses to pick (Ch. 1 §6).
        if (from == Ty::Trit && to == Ty::Bool) || (from == Ty::Bool && to == Ty::Trit) {
            return err(
                line,
                "there is no `as` between `trit` and `bool`: use `is_pos`/`is_zero`/`is_neg` \
                 or `to_trit` (Ch. 1 §6)",
            );
        }
        if let Ty::Enum(name) = &to {
            return err(
                line,
                format!(
                    "there is no cast from {from} to `{name}`: an integer need not name a \
                     variant, so the conversion is fallible and belongs to a library \
                     `try_from` (Ch. 2 §5.3)"
                ),
            );
        }
        let (Some(fw), Some(tw)) = (from.width(), to.width()) else {
            return err(line, format!("cannot cast {from} to {to}"));
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
        args: &[ast::Expr],
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        // `sign(x)` is a function, not a method (Ch. 1 §6).
        if name == "sign" {
            if args.len() != 1 {
                return err(line, "`sign` takes one argument");
            }
            let (v, ty) = self.expr(&args[0], None)?;
            if !ty.is_arithmetic() && ty != Ty::Trit {
                return err(line, format!("`sign` does not apply to {ty}"));
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
                line,
            };
            return self.aggregate(&path, &fields, None, line);
        }

        // A generic callee is instantiated here, at the call site, which is
        // also where its bounds are checked (Ch. 4 §2.2).
        if self.generic_fns.contains_key(name) {
            let key = self.instantiate_fn(name, args, expected, line)?;
            return self.call_key(&key, Vec::new(), args, line);
        }

        self.call_key(name, Vec::new(), args, line)
    }

    /// Instantiate a generic function for this call, and return the name the
    /// instantiation is known by (Ch. 4 §2.7).
    fn instantiate_fn(
        &mut self,
        name: &str,
        args: &[ast::Expr],
        expected: Option<&Ty>,
        line: Line,
    ) -> R<String> {
        let def = self.generic_fns[name].clone();
        if def.params.len() != args.len() {
            return err(
                line,
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
        let mut env: HashMap<String, Ty> = HashMap::new();
        if let (Some(want), Some(ret)) = (expected, &def.ret) {
            unify(ret, want, &def.generics, &mut env);
        }
        for ((_, want), arg) in def.params.iter().zip(args) {
            if let Some(got) = self.peek_ty(arg)? {
                unify(want, &got, &def.generics, &mut env);
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
                    line,
                    "a const generic argument cannot be inferred, and `::<>` is \
                     Ch. 4 §2.3, not implemented yet",
                );
            };
            let Some(ty) = env.get(pname) else {
                return err(
                    line,
                    format!(
                        "cannot tell what `{pname}` is in this call to `{name}`; \
                         give an argument a written type (Ch. 4 §2.3)"
                    ),
                );
            };
            // §2.2: an instantiation that fails a bound is rejected here, at
            // the call site, and not inside the body.
            for b in bounds {
                self.check_bound(ty, b, name, pname, line)?;
            }
            targs.push(ty.clone());
        }

        let _ = targs;
        self.instantiate_with(name, env, line)
    }

    /// Instantiate a generic function with an environment already worked out,
    /// queueing its body if this is the first time (Ch. 4 §2.7).
    fn instantiate_with(&mut self, name: &str, env: HashMap<String, Ty>, line: Line) -> R<String> {
        let def = self.generic_fns[name].clone();
        let targs: Vec<Ty> = def.generics.iter().map(|p| env[p.name()].clone()).collect();
        let key = mangle(name, &targs);
        if self.sigs.borrow().contains_key(&key) {
            return Ok(key);
        }
        if self.pending.borrow().len() as u32 > INSTANTIATION_LIMIT {
            return err(
                line,
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
            .map(|(n, t)| {
                let ty = resolve_ty_env(t, self.types, &env)?;
                check_sized(&ty, t.line(), &format!("the parameter `{n}`"))?;
                Ok(ty)
            })
            .collect::<R<_>>()?;
        let ret = match &def.ret {
            None => Ty::Unit,
            Some(t) => resolve_ty_env(t, self.types, &env)?,
        };
        let self_ref = matches!(def.params.first(), Some((n, ast::Ty::Ref(..))) if n == "self");
        check_returned_reference(&params, &ret, self_ref, line)?;
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
    fn check_bound(&self, ty: &Ty, bound: &str, callee: &str, param: &str, line: Line) -> R<()> {
        // `Copy` is structural and automatic (Ch. 4 §5.1); `Sized` is a fact
        // about the type, not a claim about it (§2.5).
        let ok = match bound {
            "Copy" => self.types.is_copyable(ty),
            "Sized" => !ty.is_unsized(),
            _ => match nominal_name(ty) {
                Some(n) => {
                    let base = self
                        .types
                        .instantiations
                        .borrow()
                        .get(&n)
                        .map(|(b, _)| b.clone());
                    self.impls.contains(&(n, bound.to_string()))
                        || base.is_some_and(|b| self.impls.contains(&(b, bound.to_string())))
                }
                None => false,
            },
        };
        if ok {
            return Ok(());
        }
        err(
            line,
            format!(
                "`{ty}` does not implement `{bound}`, which `{callee}` requires of \
                 `{param}` (Ch. 4 §2.2)"
            ),
        )
    }

    /// A call to a known key, with any number of already-evaluated leading
    /// arguments. Methods use the prefix for a receiver that is not a place
    /// (Ch. 4 §1.3); ordinary calls pass none.
    fn call_key(
        &mut self,
        name: &str,
        pre: Vec<(Operand, Ty)>,
        args: &[ast::Expr],
        line: Line,
    ) -> R<(Operand, Ty)> {
        let Some((params, ret)) = self.sigs.borrow().get(name).cloned() else {
            return err(line, format!("`{name}` is not a function in scope"));
        };
        if params.len() != args.len() + pre.len() {
            return err(
                line,
                format!(
                    "`{name}` takes {} argument(s), {} given",
                    params.len(),
                    args.len() + pre.len()
                ),
            );
        }
        let mut values = Vec::new();
        // An aggregate result is written through a pointer the caller
        // supplies, since TIR has no aggregate values (TIR §2).
        let out = ret.is_aggregate().then(|| self.temp_slot(&ret));
        if let Some(slot) = &out {
            values.push(Operand::Value(slot.clone()));
        }
        for ((v, got), want) in pre.into_iter().zip(&params) {
            self.check(&got, want, line, "receiver")?;
            values.push(v);
        }
        for (arg, want) in args
            .iter()
            .zip(params.iter().skip(values.len() - out.is_some() as usize))
        {
            let (v, got) = self.expr(arg, Some(want))?;
            self.check(&got, want, arg.line(), "argument")?;
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        // Ch. 1's own methods are the language's, and are not overridable;
        // everything else resolves through impl blocks (Ch. 4 §1.3).
        if !BUILTIN_METHODS.contains(&name) {
            return self.user_method(recv, name, args, line);
        }
        let (v, ty) = self.expr(recv, None)?;
        let one_arg = |this: &mut Self, want: &Ty| -> R<Operand> {
            if args.len() != 1 {
                return err(line, format!("`{name}` takes one argument"));
            }
            let (a, at) = this.expr(&args[0], Some(want))?;
            this.check(&at, want, line, "argument")?;
            Ok(a)
        };

        match name {
            "tmin" | "tmax" | "tmul" => {
                if !ty.is_arithmetic() && ty != Ty::Trit {
                    return err(line, format!("`{name}` does not apply to {ty}"));
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
                    return err(line, "`tneg` takes no arguments");
                }
                let r = self.emit("n", ty.tir(), InstKind::Neg { ty: ty.tir(), a: v });
                Ok((r, ty))
            }

            "is_pos" | "is_zero" | "is_neg" => {
                if ty != Ty::Trit {
                    return err(line, format!("`{name}` applies to trit, not {ty}"));
                }
                if !args.is_empty() {
                    return err(line, format!("`{name}` takes no arguments"));
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
                    return err(line, format!("`to_trit` applies to bool, not {ty}"));
                }
                Ok((v, Ty::Trit))
            }

            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => {
                if !ty.is_arithmetic() {
                    return err(line, format!("`{name}` does not apply to {ty}"));
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
                    return err(line, format!("`{name}` does not apply to {ty}"));
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
                self.store_at(&slot, fields[0].2, &ty, value, line)?;
                self.store_at(&slot, fields[1].2, &Ty::Bool, overflowed, line)?;
                Ok((Operand::Value(slot), result))
            }

            // Ch. 3 §5.4: a slice's length is the second word of its fat
            // reference; an array's is in its type.
            "len" => {
                if !args.is_empty() {
                    return err(line, "`len` takes no arguments");
                }
                match &ty {
                    Ty::Array(_, n) => {
                        Ok((Operand::Const(Type::Int(27), Bt::from_i128(*n)), Ty::TAddr))
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
                        line,
                        format!("`len` applies to a slice or an array, not {other}"),
                    ),
                }
            }

            "checked_add" | "checked_sub" | "checked_mul" => err(
                line,
                format!(
                    "`{name}` returns `Option<{ty}>`, and generics are Chapter 4, \
                     which is not written yet"
                ),
            ),

            other => err(line, format!("`{other}` is not a method in this milestone")),
        }
    }

    // ------------------------------------------------------------ places

    /// The address of a place, and its type (Ch. 3 §1.3). A place is a local,
    /// a field of a place, an element of a place, or a dereference.
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
        line: Line,
    ) -> R<String> {
        let head = path.segments[0].clone();
        let generic = self.types.generic_structs.get(&head).map(|s| {
            (
                s.generics.clone(),
                s.fields.clone(),
                s.fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            )
        });
        let (params, decl) = match generic {
            Some((g, f, _)) => (g, f),
            None => match self.types.generic_enums.get(&head) {
                Some(e) => {
                    let variant = path.segments.get(1).cloned().unwrap_or_default();
                    let Some(v) = e.variants.iter().find(|v| v.name == variant) else {
                        return err(line, format!("`{head}` has no variant `{variant}`"));
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

        // Otherwise, match the written field types against what was given.
        let mut env: HashMap<String, Ty> = HashMap::new();
        for (fname, fty) in &decl {
            let Some((_, e)) = fields.iter().find(|(n, _)| n == fname) else {
                continue;
            };
            if let Some(got) = self.peek_ty(e)? {
                unify(fty, &got, &params, &mut env);
            }
        }
        let mut args = Vec::new();
        for p in &params {
            match env.get(p.name()) {
                Some(t) => args.push(t.clone()),
                None => {
                    return err(
                        line,
                        format!(
                            "cannot tell what `{}` is here; write the type of this value \
                             (Ch. 4 §2.3)",
                            p.name()
                        ),
                    );
                }
            }
        }
        let ty = self.types.instantiate(&head, &args, line)?;
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
            E::Bool(..) => Some(Ty::Bool),
            E::Unit(_) => Some(Ty::Unit),
            E::Borrow(inner, mutable, _) => {
                self.peek_ty(inner)?.map(|t| Ty::Ref(Box::new(t), *mutable))
            }
            E::Cast(_, t, _) => Some(self.resolve(t)?),
            E::Call(name, _, _) => self.sigs.borrow().get(name).map(|(_, r)| r.clone()),
            E::Method(..) | E::Aggregate(..) => None,
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (trait_name, method) = match op {
            "==" | "!=" => ("Eq", "eq"),
            _ => ("Ord", "cmp"),
        };
        let key = format!("{name}.{method}");
        if !self.sigs.borrow().contains_key(&key) {
            return err(
                line,
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

    /// Call a method through a trait object's vtable (Ch. 4 §§3.1, 3.3).
    fn dyn_call(
        &mut self,
        fat: Operand,
        trait_name: &str,
        name: &str,
        args: &[ast::Expr],
        line: Line,
    ) -> R<(Operand, Ty)> {
        if !self.traits.contains_key(trait_name) {
            return err(line, format!("`{trait_name}` is not a trait in scope"));
        }
        let methods = object_methods(self.traits, trait_name, &mut Vec::new());
        let Some(index) = methods.iter().position(|m| m.name == name) else {
            return err(
                line,
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
            .map(|(_, t)| self.resolve(t))
            .collect::<R<_>>()?;
        let ret = match &m.ret {
            None => Ty::Unit,
            Some(t) => self.resolve(t)?,
        };
        if params.len() != args.len() {
            return err(
                line,
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
            self.check(&got, want, arg.line(), "argument")?;
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

    /// The global holding the vtable for this (type, trait) pair, building
    /// it if this is the first coercion to it (Ch. 4 §3.3).
    ///
    /// Layout: size, align, drop, then one address per object-safe method in
    /// declaration order. `size` and `align` are here because a future
    /// `Box<dyn Trait>` needs them to free what it points at, and adding a
    /// slot later would change the layout of something programs will have
    /// been written against.
    fn vtable_for(&mut self, concrete: &Ty, trait_name: &str, line: Line) -> R<String> {
        let name = nominal_name(concrete)
            .ok_or_else(|| one_err(line, format!("{concrete} cannot be a trait object")))?;
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
            return err(line, format!("`{trait_name}` is not a trait in scope"));
        };
        let _ = &decl;
        if !self.impls.contains(&(name.clone(), trait_name.to_string())) {
            return err(
                line,
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
    fn coerce_dyn(
        &mut self,
        v: Operand,
        from: &Ty,
        to: &Ty,
        line: Line,
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
        let symbol = self.vtable_for(target, trait_name, line)?;
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
    fn method_key(&mut self, type_name: &str, name: &str, line: Line) -> R<String> {
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
        if def.generics.len() != args.len() {
            return err(
                line,
                format!("`{base}::{name}` does not apply to `{type_name}`"),
            );
        }
        let env: HashMap<String, Ty> = def
            .generics
            .iter()
            .map(|p| p.name().to_string())
            .zip(args.iter().cloned())
            .collect();
        self.instantiate_with(&generic, env, line)
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        // A place receiver keeps its identity, so `&mut self` writes through
        // to the caller's value rather than to a copy.
        let (recv_ty, place) = match self.type_of_place(recv)? {
            Some(t) => (t, true),
            None => (self.expr(recv, None)?.1, false),
        };

        // A call on a trait object is one indirect call through its vtable
        // (Ch. 4 §3.1). Nothing else about the receiver is known.
        if let Ty::Ref(inner, _) = &recv_ty
            && let Ty::Dyn(trait_name) = &**inner
        {
            let trait_name = trait_name.clone();
            let (fat, _) = self.expr(recv, None)?;
            return self.dyn_call(fat, &trait_name, name, args, line);
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
        let Some(type_name) = nominal_name(&base) else {
            return err(line, format!("{base} has no methods"));
        };
        let key = self.method_key(&type_name, name, line)?;
        let Some((params, _)) = self.sigs.borrow().get(&key).cloned() else {
            return err(
                line,
                format!("{base} has no method `{name}`, and neither does Ch. 1"),
            );
        };
        let Some(self_ty) = params.first().cloned() else {
            return err(
                line,
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
                    self.store_at(&slot, 0, &base, v, line)?;
                    (Operand::Value(slot), self_ty.clone())
                }
                // An aggregate is already an address, which is what a
                // reference to it is.
                Ty::Ref(..) => (v, self_ty.clone()),
                _ => (v, ty),
            };
            return self.call_key(&key, vec![arg], args, line);
        }

        let mut receiver = recv.clone();
        for _ in 0..derefs {
            receiver = ast::Expr::Deref(Box::new(receiver), line);
        }
        let receiver = match &self_ty {
            Ty::Ref(_, mutable) => ast::Expr::Borrow(Box::new(receiver), *mutable, line),
            _ => receiver,
        };
        let mut full = vec![receiver];
        full.extend(args.iter().cloned());
        self.call_key(&key, Vec::new(), &full, line)
    }

    fn place(&mut self, e: &ast::Expr, line: Line) -> R<(Operand, Ty)> {
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
                let (v, ty) = self.expr(inner, None)?;
                let Ty::Ref(target, _) = ty else {
                    return err(*l, format!("`*` applies to a reference, not {ty}"));
                };
                Ok((v, *target))
            }

            _ => err(line, "this expression is not a place"),
        }
    }

    /// A place, dereferencing automatically through a reference — which is
    /// what makes `r.x` mean `(*r).x` (Ch. 3 §2.3).
    fn place_or_deref(&mut self, e: &ast::Expr, line: Line) -> R<(Operand, Ty)> {
        let (mut addr, mut ty) = self.place(e, line)?;
        while let Ty::Ref(target, _) = ty.clone() {
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
        line: Line,
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
        let _ = line;
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
    fn store_at(&mut self, slot: &str, off: i128, ty: &Ty, v: Operand, line: Line) -> R<()> {
        let dst = self.offset(Operand::Value(slot.to_string()), off);
        if ty.is_aggregate() {
            self.copy_typed(dst, v, ty, line)?;
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
    fn copy_typed(&mut self, dst: Operand, src: Operand, ty: &Ty, line: Line) -> R<()> {
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
                    self.copy_typed(to, from, &elem, line)?;
                }
                Ok(())
            }

            Ty::Tuple(_) | Ty::Struct(_) => {
                for (_, ft, off) in self.types.fields(ty) {
                    let from = self.offset(src.clone(), off);
                    let to = self.offset(dst.clone(), off);
                    self.copy_typed(to, from, &ft, line)?;
                }
                Ok(())
            }

            // An enum's payload varies by variant, so its storage is copied
            // as raw trytes. A reference inside a payload therefore loses its
            // provenance in the interpreter — see docs/spec-gaps.md G6.7.
            Ty::Enum(_) => {
                let size = self.types.size(ty);
                self.copy_trytes(dst, src, size, line)
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
    fn copy_trytes(&mut self, dst: Operand, src: Operand, size: i128, line: Line) -> R<()> {
        if size > 243 {
            return err(
                line,
                "this milestone copies aggregates of at most 243 trytes",
            );
        }
        for i in 0..size {
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        let head = self.instantiate_head(path, fields, expected, line)?;

        // `Type::function(args)` — an associated function, which is written
        // like a variant and told apart by what the names are (Ch. 4 §1.4).
        if path.segments.len() == 2 {
            let key = format!("{head}.{}", path.segments[1]);
            if self.sigs.borrow().contains_key(&key) {
                let args: Vec<ast::Expr> = fields.iter().map(|(_, e)| e.clone()).collect();
                return self.call_key(&key, Vec::new(), &args, line);
            }
        }

        // `Enum::Variant`, with or without a payload.
        if path.segments.len() == 2 {
            let (enum_name, variant) = (head, path.segments[1].clone());
            if !self.types.enums.borrow().contains_key(&enum_name) {
                return err(line, format!("`{enum_name}` is not an enum in scope"));
            }
            let Some(index) = self.types.variant(&enum_name, &variant) else {
                return err(line, format!("`{enum_name}` has no variant `{variant}`"));
            };
            return self.build_variant(&enum_name, index, fields, line);
        }

        // A struct literal.
        if !self.types.structs.borrow().contains_key(&head) {
            return err(line, format!("`{head}` is not a struct in scope"));
        }
        let ty = Ty::Struct(head.clone());
        let declared = self.types.fields(&ty);
        if declared.len() != fields.len() {
            return err(
                line,
                format!(
                    "`{head}` has {} field(s), {} given",
                    declared.len(),
                    fields.len()
                ),
            );
        }
        let slot = self.temp_slot(&ty);
        for (name, value) in fields {
            let Some((_, ft, off)) = declared.iter().find(|(n, _, _)| n == name).cloned() else {
                return err(line, format!("`{head}` has no field `{name}`"));
            };
            let (v, vt) = self.expr(value, Some(&ft))?;
            self.check(&vt, &ft, value.line(), "field")?;
            self.store_at(&slot, off, &ft, v, line)?;
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        let ty = Ty::Enum(enum_name.to_string());
        let l = self.types.layout(&ty);
        let e = l.enum_layout.clone().expect("an enum");
        let declared = self.types.variant_fields(enum_name, index);
        if declared.len() != fields.len() {
            return err(
                line,
                format!(
                    "this variant has {} field(s), {} given",
                    declared.len(),
                    fields.len()
                ),
            );
        }

        let slot = self.temp_slot(&ty);
        // Zero the storage first, so padding and unwritten payload trytes are
        // deterministic.
        for i in 0..l.size as i128 {
            let p = self.offset(Operand::Value(slot.clone()), i);
            self.push(Inst {
                results: Vec::new(),
                kind: InstKind::Store {
                    ty: Type::Int(9),
                    v: Operand::Const(Type::Int(9), Bt::ZERO),
                    p,
                },
            });
        }

        for (name, value) in fields {
            let Some((_, ft, off)) = declared.iter().find(|(n, _, _)| n == name).cloned() else {
                return err(line, format!("this variant has no field `{name}`"));
            };
            let (v, vt) = self.expr(value, Some(&ft))?;
            self.check(&vt, &ft, value.line(), "field")?;
            self.store_at(&slot, off, &ft, v, line)?;
        }

        self.write_tag(&slot, &e, index, line)?;
        Ok((Operand::Value(slot), ty))
    }

    /// Store the discriminant of variant `index`.
    fn write_tag(&mut self, slot: &str, e: &layout::EnumLayout, index: usize, line: Line) -> R<()> {
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
                    return err(line, "this enum has more variants than niches");
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
    fn store_slot(&mut self, slot: &str, ty: &Ty, v: Operand) {
        if ty.is_aggregate() {
            let dst = Operand::Value(slot.to_string());
            let ty = ty.clone();
            let _ = self.copy_typed(dst, v, &ty, 0);
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        // The condition is a `bool`, not a `trit`, and not "anything
        // nonzero" (Ch. 1 §2).
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, line, "condition")?;

        let (then_l, else_l, join_l) = (self.fresh("then"), self.fresh("else"), self.fresh("join"));
        self.br3(c, &else_l, &else_l, &then_l);

        // The two arms' values meet in a slot, so nothing crosses a block
        // edge in a register.
        let mut result: Option<(String, Ty)> = None;

        let before = self.owned_snapshot();
        self.start(then_l);
        let (tv, tt) = self.block(then, expected)?;
        if tt != Ty::Never && tt != Ty::Unit {
            let slot = self.temp_slot(&tt);
            self.store_slot(&slot, &tt, tv);
            result = Some((slot, tt.clone()));
        }
        self.jump(&join_l);
        let after_then = self.owned_snapshot();

        self.owned = before;
        self.start(else_l);
        let et = match els {
            None => Ty::Unit,
            Some(e) => {
                let (ev, et) = self.expr(e, expected)?;
                if let Some((slot, ty)) = &result
                    && et != Ty::Never
                {
                    let (slot, ty) = (slot.clone(), ty.clone());
                    self.check(&et, &ty, line, "`else` branch")?;
                    self.store_slot(&slot, &ty, ev);
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
                err(line, "an `if` used for its value needs an `else` branch")
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

    fn while_expr(&mut self, cond: &ast::Expr, body: &ast::Block, line: Line) -> R<(Operand, Ty)> {
        let (head, body_l, exit) = (self.fresh("while"), self.fresh("body"), self.fresh("done"));
        self.jump(&head);

        self.start(head.clone());
        let (c, ct) = self.expr(cond, Some(&Ty::Bool))?;
        self.check(&ct, &Ty::Bool, line, "condition")?;
        self.br3(c, &exit, &exit, &body_l);

        self.start(body_l);
        let before = self.owned_snapshot();
        self.loops.push(LoopCtx {
            exit: exit.clone(),
            head: head.clone(),
            result: None,
        });
        self.block(body, None)?;
        self.loops.pop();
        self.check_no_move_in_loop(&before, line)?;
        self.jump(&head);

        self.start(exit);
        Ok((unit(), Ty::Unit))
    }

    fn loop_expr(
        &mut self,
        body: &ast::Block,
        expected: Option<&Ty>,
        _line: Line,
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
            exit: exit.clone(),
            head: head.clone(),
            result: result.clone(),
        });
        self.block(body, None)?;
        self.loops.pop();
        self.check_no_move_in_loop(&before, _line)?;
        self.jump(&head);

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
        line: Line,
    ) -> R<(Operand, Ty)> {
        let (mut v, mut ty) = self.expr(scrutinee, None)?;
        // A scrutinee behind a reference is dereferenced, exactly as `.` is
        // (Ch. 3 §2.3). Bindings then copy out of the referent, which the
        // copy rule of §1.2 already governs.
        while let Ty::Ref(target, _) = ty.clone() {
            if target.is_unsized() {
                break;
            }
            // `v` is the reference itself, which is an address. An
            // aggregate's value *is* its address, so dereferencing one is a
            // retype; a scalar has to be loaded.
            if !target.is_aggregate() {
                v = self.load_from(v, &target);
            }
            ty = *target;
        }
        if let Ty::Enum(name) = ty.clone() {
            return self.match_enum(&name, v, arms, expected, line);
        }
        if !ty.is_scalar() {
            return err(line, format!("cannot match on {ty}"));
        }
        check_exhaustive(&ty, arms, line)?;

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
            for (arm, label) in arms.iter().zip(&labels) {
                self.start(label.clone());
                self.arm_body(arm, expected, &mut result, &join, line)?;
            }
            self.start(join);
            return Ok(self.match_result(result));
        }

        // Otherwise: test the arms in order.
        let mut fell_through = true;
        for arm in arms {
            let next = self.fresh("arm.next");
            let body = self.fresh("arm");
            let unconditional = self.arm_test(arm, &v, &ty, &body, &next, line)?;
            self.start(body);
            self.arm_body(arm, expected, &mut result, &join, line)?;
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
    fn match_enum(
        &mut self,
        name: &str,
        addr: Operand,
        arms: &[ast::Arm],
        expected: Option<&Ty>,
        line: Line,
    ) -> R<(Operand, Ty)> {
        let ty = Ty::Enum(name.to_string());
        let l = self.types.layout(&ty);
        let e = l.enum_layout.clone().expect("an enum");
        let variants = self.types.enums.borrow()[name].clone();

        // Which variant each arm selects, and whether an arm catches all.
        let mut selects: Vec<Option<usize>> = Vec::new();
        for arm in arms {
            if arm.guard.is_some() {
                return err(arm.line, "match guards are not lowered yet");
            }
            if arm.patterns.len() != 1 {
                return err(arm.line, "or-patterns over an enum are not lowered yet");
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
                        other.line(),
                        format!("this pattern does not match `{name}`"),
                    );
                }
            });
        }

        let covered: Vec<usize> = selects.iter().flatten().copied().collect();
        let catchall = selects.iter().any(Option::is_none);
        if !catchall && covered.len() < variants.len() {
            return err(
                line,
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
            for (i, arm) in arms.iter().enumerate() {
                self.start(labels[i].clone());
                self.enum_arm(
                    arm,
                    name,
                    selects[i],
                    &addr,
                    expected,
                    &mut result,
                    &join,
                    line,
                )?;
            }
            self.start(join);
            return Ok(self.match_result(result));
        }

        // Each arm tests one value of the discriminant — except the arm for a
        // niche-encoded enum's *untagged* variant, whose storage holds an
        // ordinary payload and which is therefore recognized by elimination.
        // Those, and a wildcard, are emitted last, since the variant patterns
        // are disjoint and only order relative to the catch-all matters.
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
            self.start(body);
            self.enum_arm(
                &arms[arm_index],
                name,
                Some(variant),
                &addr,
                expected,
                &mut result,
                &join,
                line,
            )?;
            self.start(next);
        }

        match default {
            Some(i) => {
                let variant = selects[i];
                self.enum_arm(
                    &arms[i],
                    name,
                    variant,
                    &addr,
                    expected,
                    &mut result,
                    &join,
                    line,
                )?;
            }
            None => self.finish(Terminator::Trap(FaultCode::Trap)),
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
        line: Line,
    ) -> R<()> {
        self.scopes.push(HashMap::new());

        if let ast::Pattern::Bind(name, _) = &arm.patterns[0] {
            let ty = Ty::Enum(enum_name.to_string());
            let local = self.declare(name, ty.clone(), false);
            self.store_at(&local.slot, 0, &ty, addr.clone(), line)?;
        }
        if let (Some(index), ast::Pattern::Aggregate(_, fields, _)) = (variant, &arm.patterns[0]) {
            let declared = self.types.variant_fields(enum_name, index);
            if !fields.is_empty() && fields.len() != declared.len() {
                self.scopes.pop();
                return err(
                    arm.line,
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
                    return err(arm.line, format!("this variant has no field `{name}`"));
                };
                let ast::Pattern::Bind(bound, _) = pat else {
                    self.scopes.pop();
                    return err(arm.line, "nested patterns are not lowered yet");
                };
                let p = self.offset(addr.clone(), off);
                let v = self.load_from(p, &ft);
                let local = self.declare(bound, ft.clone(), false);
                self.store_at(&local.slot, 0, &ft, v, arm.line)?;
            }
        }

        let r = self.arm_body(arm, expected, result, join, line);
        self.scopes.pop();
        r
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

    fn arm_body(
        &mut self,
        arm: &ast::Arm,
        expected: Option<&Ty>,
        result: &mut Option<(String, Ty)>,
        join: &str,
        line: Line,
    ) -> R<()> {
        // A binding pattern names the scrutinee; this milestone's patterns
        // bind nothing else.
        let (v, ty) = self.expr(&arm.body, expected)?;
        if ty != Ty::Never && ty != Ty::Unit {
            if result.is_none() {
                *result = Some((self.temp_slot(&ty), ty.clone()));
            }
            let (slot, want) = result.clone().expect("just set");
            self.check(&ty, &want, line, "match arm")?;
            self.store_slot(&slot, &want, v);
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
        line: Line,
    ) -> R<bool> {
        if arm.guard.is_some() {
            return err(line, "match guards are not lowered yet");
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
            let value = pattern_value(p, ty, line)?;
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
        E::Call(_, args, _) => {
            for a in args {
                go(a, index, out);
            }
        }
        E::Method(recv, _, args, _) => {
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

fn pattern_value(p: &ast::Pattern, ty: &Ty, line: Line) -> R<Bt> {
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
        _ => err(line, "unsupported pattern"),
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
fn check_exhaustive(ty: &Ty, arms: &[ast::Arm], line: Line) -> R<()> {
    let mut seen: Vec<Bt> = Vec::new();
    for arm in arms {
        if arm.guard.is_some() {
            continue; // a guarded arm never counts (§5.4)
        }
        for p in &arm.patterns {
            match p {
                ast::Pattern::Wild(_) | ast::Pattern::Bind(..) => return Ok(()),
                _ => {
                    let v = pattern_value(p, ty, line)?;
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
            line,
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
