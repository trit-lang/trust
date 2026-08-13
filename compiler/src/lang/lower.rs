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
        matches!(self, Ty::Slice(_))
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
            // A slice has no layout of its own; only `&[T]` does.
            Ty::Slice(_) => layout::Ty::Unit,
        }
    }
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
    db: layout::TypeDb,
    /// Types with a destructor of their own.
    destructors: std::collections::BTreeSet<String>,
    /// Field names and semantic types of each struct, in declaration order.
    structs: HashMap<String, Vec<(String, Ty)>>,
    /// Variants of each enum, in declaration order.
    enums: HashMap<String, Vec<VariantInfo>>,
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
        layout::layout_of(&self.db, &ty.layout_ty())
            .unwrap_or_else(|e| panic!("layout of {ty} failed after checking: {e}"))
    }

    fn size(&self, ty: &Ty) -> i128 {
        self.layout(ty).size as i128
    }

    /// The fields of a struct or a tuple, with their offsets.
    fn fields(&self, ty: &Ty) -> Vec<(String, Ty, i128)> {
        let l = self.layout(ty);
        match ty {
            Ty::Struct(name) => self.structs[name]
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
                self.destructors.contains(n)
                    || self.structs[n].iter().any(|(_, t)| self.needs_drop(t))
            }
            Ty::Enum(n) => {
                self.destructors.contains(n)
                    || self.enums[n]
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
            .get(enum_name)?
            .iter()
            .position(|v| v.name == variant)
    }

    /// A variant's payload fields, with their offsets.
    fn variant_fields(&self, enum_name: &str, index: usize) -> Vec<(String, Ty, i128)> {
        let ty = Ty::Enum(enum_name.to_string());
        let l = self.layout(&ty);
        let e = l.enum_layout.expect("an enum");
        self.enums[enum_name][index]
            .fields
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

    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    for item in &file.items {
        if let ast::Item::Fn(f) = item {
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
            if let Ok(r) = &ret
                && let Err(e) = check_no_returned_reference(r, f.line)
            {
                errs.push(e);
                continue;
            }
            match (params, ret) {
                (Ok(p), Ok(r)) => {
                    if sigs.insert(fn_key(f), (p, r)).is_some() {
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

    for item in &file.items {
        let ast::Item::Fn(f) = item else { continue };
        if !sigs.contains_key(&fn_key(f)) {
            continue; // its signature was already reported
        }
        let signature = signature_of(f, &sigs);
        match &f.body {
            // A function without a body is a declaration, and lowers to TIR's
            // own declaration form — one mechanism, spelled twice.
            None => module.decls.push(signature),
            Some(body) => match function(f, signature, body, &sigs, &globals, &types) {
                Ok(func) => module.funcs.push(func),
                Err(e) => errs.push(e),
            },
        }
    }

    if errs.is_empty() {
        Ok(module)
    } else {
        Err(errs)
    }
}

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
/// A reference in a return position needs the region check of Ch. 3 §4.1,
/// which is not implemented. Accepting it would let a dangling reference
/// through, and this language's whole claim is that it does not.
fn check_no_returned_reference(ty: &Ty, line: Line) -> R<()> {
    if contains_reference(ty) {
        return err(
            line,
            format!(
                "returning {ty} needs the region check for a returned reference, which is \
                 not implemented yet: a reference may be a parameter, a local, or a field \
                 of a local (docs/spec-gaps.md G0.5)"
            ),
        );
    }
    Ok(())
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

fn signature_of(f: &ast::FnItem, sigs: &HashMap<String, (Vec<Ty>, Ty)>) -> Signature {
    let key = fn_key(f);
    let (params, ret) = sigs
        .get(&key)
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
        name: key,
        params: tir_params,
        ret: if ret == Ty::Unit || ret.is_aggregate() {
            None
        } else {
            Some(ret.tir())
        },
    }
}

/// Resolve every nominal type in the file and hand them to the layout engine.
fn build_types(file: &ast::File) -> R<Types> {
    let mut types = Types {
        db: layout::TypeDb::new(),
        destructors: std::collections::BTreeSet::new(),
        structs: HashMap::new(),
        enums: HashMap::new(),
    };

    // Names first, so that a type may mention another declared later.
    for item in &file.items {
        match item {
            ast::Item::Struct(s) => {
                types.structs.insert(s.name.clone(), Vec::new());
            }
            ast::Item::Enum(e) => {
                types.enums.insert(e.name.clone(), Vec::new());
            }
            _ => {}
        }
    }

    for item in &file.items {
        match item {
            ast::Item::Struct(st) => {
                let fields: Vec<(String, Ty)> = st
                    .fields
                    .iter()
                    .map(|(n, t)| {
                        let ty = resolve_ty(t, &types)?;
                        check_sized(&ty, t.line(), &format!("the field `{n}`"))?;
                        Ok((n.clone(), ty))
                    })
                    .collect::<R<_>>()?;
                types.db.struct_(
                    &st.name,
                    repr_of(st.repr),
                    fields
                        .iter()
                        .map(|(n, t)| (n.as_str(), t.layout_ty()))
                        .collect(),
                );
                types.structs.insert(st.name.clone(), fields);
            }
            ast::Item::Enum(en) => {
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
                types.db.enum_(&en.name, repr_of(en.repr), variants);
                types.enums.insert(en.name.clone(), infos);
            }
            _ => {}
        }
    }

    // A function named `drop` whose one parameter is named `self` is that
    // parameter's type's destructor (Ch. 3 §1.4).
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
            ast::Item::Struct(s) => (&s.name, s.line),
            ast::Item::Enum(e) => (&e.name, e.line),
            _ => continue,
        };
        if let Err(e) = layout::layout_of(&types.db, &layout::Ty::named(name)) {
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
    match t {
        ast::Ty::Unit(_) => Ok(Ty::Unit),
        ast::Ty::Tuple(ts, _) => Ok(Ty::Tuple(
            ts.iter().map(|t| resolve_ty(t, types)).collect::<R<_>>()?,
        )),
        ast::Ty::Ref(t, mutable, _) => Ok(Ty::Ref(Box::new(resolve_ty(t, types)?), *mutable)),
        ast::Ty::Slice(t, line) => {
            let elem = resolve_ty(t, types)?;
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
            other if types.structs.contains_key(other) => Ok(Ty::Struct(other.to_string())),
            other if types.enums.contains_key(other) => Ok(Ty::Enum(other.to_string())),
            other => err(*line, format!("`{other}` is not a type in scope")),
        },
        ast::Ty::Array(elem, count, line) => {
            let elem = resolve_ty(elem, types)?;
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
                    trytes.push(value.shr(i as u32 * 9).wrap_to(9));
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
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
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

fn function(
    f: &ast::FnItem,
    sig: Signature,
    body: &ast::Block,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    globals: &HashMap<String, Global>,
    types: &Types,
) -> R<Function> {
    let (param_tys, ret) = sigs.get(&fn_key(f)).cloned().unwrap();
    let destructor_of = fn_key(f).strip_prefix("drop.").map(|t| t.to_string());
    let mut fx = Fn {
        sigs,
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
            self.stmt(stmt)?;
        }
        let result = match &b.tail {
            Some(e) => self.expr(e, expected)?,
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
                    callee: format!("drop.{n}"),
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
                    Some(t) => Some(resolve_ty(t, self.types)?),
                    None => None,
                };
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
                        self.mark_moved(name);
                    }
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
            E::Call(name, args, line) => self.call(name, args, *line),
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

            E::Aggregate(path, fields, line) => self.aggregate(path, fields, *line),

            // A borrow is the address of a place — which every local already
            // has, since every local lives in a slot.
            E::Borrow(place, mutable, line) => {
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

    /// The type of a place, without emitting anything.
    fn type_of_place(&mut self, e: &ast::Expr) -> R<Option<Ty>> {
        Ok(match e {
            ast::Expr::Path(name, _) => self.lookup(name).map(|l| l.ty),
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
        let to = resolve_ty(to, self.types)?;
        let (v, from) = self.expr(inner, None)?;

        // A fieldless enum may be cast to an integer, yielding its
        // discriminant. There is no cast in the reverse direction — that is
        // fallible, and library `try_from` territory (Ch. 2 §5.3).
        if let Ty::Enum(name) = &from {
            if !to.is_arithmetic() && to != Ty::Trit {
                return err(line, format!("an enum casts only to an integer, not {to}"));
            }
            if self.types.enums[name].iter().any(|v| !v.fields.is_empty()) {
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

    fn call(&mut self, name: &str, args: &[ast::Expr], line: Line) -> R<(Operand, Ty)> {
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
        if self.types.structs.contains_key(name) {
            let fields: Vec<(String, ast::Expr)> = args
                .iter()
                .enumerate()
                .map(|(i, e)| (i.to_string(), e.clone()))
                .collect();
            let path = ast::Path {
                segments: vec![name.to_string()],
                line,
            };
            return self.aggregate(&path, &fields, line);
        }

        let Some((params, ret)) = self.sigs.get(name).cloned() else {
            return err(line, format!("`{name}` is not a function in scope"));
        };
        if params.len() != args.len() {
            return err(
                line,
                format!(
                    "`{name}` takes {} argument(s), {} given",
                    params.len(),
                    args.len()
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
        for (arg, want) in args.iter().zip(&params) {
            let (v, got) = self.expr(arg, Some(want))?;
            self.check(&got, want, arg.line(), "argument")?;
            values.push(v);
        }
        let kind = InstKind::Call {
            callee: name.to_string(),
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
        line: Line,
    ) -> R<(Operand, Ty)> {
        let head = path.segments[0].clone();

        // `Enum::Variant`, with or without a payload.
        if path.segments.len() == 2 {
            let (enum_name, variant) = (head, path.segments[1].clone());
            if !self.types.enums.contains_key(&enum_name) {
                return err(line, format!("`{enum_name}` is not an enum in scope"));
            }
            let Some(index) = self.types.variant(&enum_name, &variant) else {
                return err(line, format!("`{enum_name}` has no variant `{variant}`"));
            };
            return self.build_variant(&enum_name, index, fields, line);
        }

        // A struct literal.
        if !self.types.structs.contains_key(&head) {
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
        let (v, ty) = self.expr(scrutinee, None)?;
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
        let variants = self.types.enums[name].clone();

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
                        2 if path.segments[0] == name => path.segments[1].clone(),
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
