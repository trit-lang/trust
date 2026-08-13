//! Legalization (TIR §6) — the mandatory pass between target-independent
//! optimization and instruction selection.
//!
//! Contract: take any well-formed TIR and produce TIR whose every value type
//! and every operation width appears in the target's legal set. Backends may
//! assume legalized input and are not required to handle anything else.
//!
//! # What this pass does today
//!
//! **Promotion** — widths below the smallest legal width are widened,
//! operated on, and renormalized. This is the direction the reference target
//! needs and the one a frontend hits constantly, because `cmp` yields `t1`.
//!
//! **Expansion** — widths above the widest legal width become several
//! legal-width parts with the carry chained between them (see `expand`).
//! `add`, `sub`, `neg`, `cmp`, the trit-wise set, `select3`, the conversions
//! and memory accesses are expanded; `mul` is blocked on a primitive TIR does
//! not define, and `div` and the shifts are unwritten. Both are reported
//! rather than mis-compiled — see `docs/spec-gaps.md` G6.5, G6.6.
//!
//! # Renormalization
//!
//! A promoted value is kept **normalized**: a value of logical width `w`
//! living in a legal `tL` always holds a number inside `tw`'s symmetric
//! range. That invariant is what makes the promoted forms of `div`, `cmp`,
//! `neg` and the trit-wise operations exact with no fixup at all.
//!
//! Renormalizing after a wrapping operation cannot go through `trunc`,
//! because `trunc tL -> tw` would put an *illegal* width back into the
//! output. It is done at the legal width instead, with `tmul` against a mask
//! of `w` ones: `tmul(x, 1)` is `x` and `tmul(x, 0)` is `0`, so the mask
//! keeps the low `w` trits and clears the rest — which is exactly the
//! symmetric residue mod 3^w (AM §3.1), computed trit-wise with no carries.
//!
//! # `t1` is the condition type, not an arithmetic width
//!
//! `cmp` yields `t1` by definition (TIR §3.3) and `br3` consumes one, so a
//! target whose legal set omits 1 still has `t1` values. The invariant this
//! pass establishes is therefore: every *arithmetic* operand and result has a
//! width in the legal set, and `t1` survives only as a comparison result, a
//! `.flag` overflow trit, or a branch/select selector. Conversions between
//! the two worlds are explicit — `widen` on the way up, and `cmp x, 0` on the
//! way down, which reads the sign rather than a residue and so is exact for
//! any value.

pub(crate) mod expand;

use super::ir::*;
use super::target::TargetDesc;
use expand::Wide;
use std::collections::HashMap;
use trit_core::{Bt, FaultCode, Flavor};

/// Why a module could not be legalized.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LegalizeError {
    /// The function involved, if any.
    pub function: Option<String>,
    /// What could not be done.
    pub message: String,
}

impl std::fmt::Display for LegalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.function {
            Some(n) => write!(f, "@{n}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for LegalizeError {}

/// Legalize a module for a target.
pub fn legalize_module(m: &Module, target: &TargetDesc) -> Result<Module, Vec<LegalizeError>> {
    let mut errs = Vec::new();
    let widths = WidthMap { target };

    let mut out = Module {
        version: m.version.clone(),
        target: target.name.clone(),
        globals: m.globals.clone(),
        decls: Vec::new(),
        funcs: Vec::new(),
    };

    for d in &m.decls {
        match widths.signature(d) {
            Ok(s) => out.decls.push(s),
            Err(e) => errs.push(LegalizeError {
                function: Some(d.name.clone()),
                message: e,
            }),
        }
    }

    // Signatures first: a call has to agree with the callee's legalized shape.
    let mut sigs: HashMap<String, Signature> = HashMap::new();
    for s in m.funcs.iter().map(|f| &f.sig).chain(m.decls.iter()) {
        match widths.signature(s) {
            Ok(l) => {
                sigs.insert(s.name.clone(), l);
            }
            Err(e) => errs.push(LegalizeError {
                function: Some(s.name.clone()),
                message: e,
            }),
        }
    }

    for f in &m.funcs {
        match legalize_function(f, &widths, &sigs) {
            Ok(func) => out.funcs.push(func),
            Err(msgs) => errs.extend(msgs.into_iter().map(|message| LegalizeError {
                function: Some(f.sig.name.clone()),
                message,
            })),
        }
    }

    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// Maps logical widths onto the target's legal set.
struct WidthMap<'a> {
    target: &'a TargetDesc,
}

/// How a logical width is represented after legalization.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// One value, at this legal width — promotion, or nothing to do.
    Legal(u32),
    /// Several values, one per part — expansion.
    Wide(Wide),
}

impl WidthMap<'_> {
    /// How a value of logical width `w` is represented.
    fn classify(&self, w: u32) -> Class {
        match self.target.legal_at_least(w) {
            Some(l) => Class::Legal(l),
            None => Class::Wide(Wide::new(w, self.target.widest_legal())),
        }
    }

    /// The legal width an operation of logical width `w` is performed at.
    /// Only valid where expansion is not an option.
    fn arith(&self, w: u32) -> Result<u32, String> {
        match self.classify(w) {
            Class::Legal(l) => Ok(l),
            Class::Wide(_) => Err(format!(
                "t{w} is wider than the widest legal width t{} for target \"{}\", \
                 and this position cannot be expanded into parts",
                self.target.widest_legal(),
                self.target.name
            )),
        }
    }

    fn ty(&self, t: Type) -> Result<Type, String> {
        match t {
            Type::Ptr => Ok(Type::Ptr),
            Type::Int(w) => Ok(Type::Int(self.arith(w)?)),
        }
    }

    /// Legalize a signature.
    ///
    /// A parameter or result too wide for the target would have to be passed
    /// in several registers or through a hidden pointer, and TIR has neither
    /// multiple return values nor an `sret` convention — the calling
    /// convention is a target-description property (TIR §7) and the reference
    /// target's is `"tritium0"`, "defined in the target's own doc", which does
    /// not exist. So this is reported rather than invented.
    fn signature(&self, s: &Signature) -> Result<Signature, String> {
        let across_the_boundary = |t: Type| -> Result<Type, String> {
            if let Type::Int(w) = t
                && matches!(self.classify(w), Class::Wide(_))
            {
                return Err(format!(
                    "`t{w}` crosses a function boundary but is wider than the \
                     widest legal width t{}; passing it needs a calling \
                     convention this repository does not have \
                     (see docs/spec-gaps.md G6.5)",
                    self.target.widest_legal()
                ));
            }
            self.ty(t)
        };
        Ok(Signature {
            name: s.name.clone(),
            params: s
                .params
                .iter()
                .map(|(n, t)| Ok((n.clone(), across_the_boundary(*t)?)))
                .collect::<Result<_, String>>()?,
            ret: s.ret.map(across_the_boundary).transpose()?,
        })
    }
}

/// What one block parameter became after legalization: itself, or one slot
/// per part.
struct ParamShape {
    slots: Vec<(String, Type)>,
    wide: Option<Wide>,
}

/// The mask that renormalizes a promoted value: `w` ones, zeros above.
fn mask(w: u32, at: u32) -> Operand {
    Operand::Const(Type::Int(at), Bt::max_of_width(w))
}

fn konst(at: u32, v: i128) -> Operand {
    Operand::Const(Type::Int(at), Bt::from_i128(v))
}

/// Builds one legalized function, splitting blocks where a promoted
/// operation needs a conditional fault.
struct Emit<'a> {
    widths: &'a WidthMap<'a>,
    sigs: &'a HashMap<String, Signature>,
    /// Finished blocks, in order.
    blocks: Vec<Block>,
    /// The block being built.
    label: String,
    params: Vec<(String, Type)>,
    insts: Vec<Inst>,
    /// Actual type of every value defined so far.
    actual: HashMap<String, Type>,
    /// Values replaced by another operand (identity conversions).
    subst: HashMap<String, Operand>,
    /// Parts of every expanded value, least significant first.
    parts: HashMap<String, Vec<Operand>>,
    /// The shape each block's parameters legalized to, so a branch can be
    /// converted to the destination's types rather than to its own
    /// arguments'.
    block_shapes: HashMap<String, Vec<ParamShape>>,
    prefix: String,
    counter: u32,
    errs: Vec<String>,
    /// Set when the rest of the block provably cannot execute — a shift whose
    /// constant amount is out of range always faults, so everything after it
    /// is dead and the block ends there.
    halted: Option<Terminator>,
}

impl Emit<'_> {
    fn fresh(&mut self, what: &str) -> String {
        self.counter += 1;
        format!("{}{what}{}", self.prefix, self.counter)
    }

    fn push(&mut self, results: Vec<String>, kind: InstKind) {
        self.insts.push(Inst { results, kind });
    }

    /// Emit an instruction with one fresh result of the given type.
    fn emit(&mut self, what: &str, ty: Type, kind: InstKind) -> Operand {
        let name = self.fresh(what);
        self.actual.insert(name.clone(), ty);
        self.push(vec![name.clone()], kind);
        Operand::Value(name)
    }

    /// Emit an instruction with two fresh results — the `.flag` form.
    fn emit2(&mut self, what: &str, t0: Type, t1: Type, kind: InstKind) -> (Operand, Operand) {
        let (a, b) = (self.fresh(what), self.fresh(what));
        self.actual.insert(a.clone(), t0);
        self.actual.insert(b.clone(), t1);
        self.push(vec![a.clone(), b.clone()], kind);
        (Operand::Value(a), Operand::Value(b))
    }

    fn finish_block(&mut self, term: Terminator) {
        let label = std::mem::take(&mut self.label);
        let params = std::mem::take(&mut self.params);
        let insts = std::mem::take(&mut self.insts);
        self.blocks.push(Block {
            label,
            params,
            insts,
            term,
        });
    }

    fn start_block(&mut self, label: String) {
        self.label = label;
        self.params = Vec::new();
        self.insts = Vec::new();
    }

    /// End the current block with a three-way test that faults on the arms
    /// marked `true`, and continue emitting into a fresh block.
    ///
    /// The continuation has exactly one predecessor, so every value defined
    /// before the split still dominates every use after it and no block
    /// parameters are needed.
    fn trap_if(&mut self, cond: Operand, trap_on: [bool; 3], code: FaultCode) {
        let fault_label = self.fresh("fault");
        let cont_label = self.fresh("cont");
        let arm = |t: bool| Target {
            label: if t {
                fault_label.clone()
            } else {
                cont_label.clone()
            },
            args: Vec::new(),
        };
        self.finish_block(Terminator::Br3 {
            t: cond,
            neg: arm(trap_on[0]),
            zero: arm(trap_on[1]),
            pos: arm(trap_on[2]),
        });
        self.blocks.push(Block {
            label: fault_label,
            params: Vec::new(),
            insts: Vec::new(),
            term: Terminator::Trap(code),
        });
        self.start_block(cont_label);
    }

    /// The type an operand currently has.
    fn type_of(&self, o: &Operand) -> Option<Type> {
        match o {
            Operand::Const(t, _) => Some(*t),
            Operand::Global(_) => Some(Type::Ptr),
            Operand::Value(v) => self.actual.get(v).copied(),
        }
    }

    /// Resolve substitutions introduced by identity conversions.
    fn resolve(&self, o: &Operand) -> Operand {
        match o {
            Operand::Value(v) => self.subst.get(v).cloned().unwrap_or_else(|| o.clone()),
            other => other.clone(),
        }
    }

    /// Convert an operand to the type a use site requires.
    ///
    /// Only two conversions are ever needed, and each is exact: `widen` lifts
    /// a `t1` condition into the arithmetic width, and `cmp x, 0` projects an
    /// arithmetic value back down to a condition trit by *sign* — never by
    /// residue, which `trunc` would give and which would be wrong for any
    /// value outside −1…1.
    fn coerce(&mut self, o: &Operand, want: Type) -> Operand {
        let o = self.resolve(o);
        let have = match self.type_of(&o) {
            Some(t) => t,
            None => return o,
        };
        if have == want {
            return o;
        }
        match (have, want, &o) {
            // Retyping a constant is free as long as it still fits.
            (Type::Int(_), Type::Int(n), Operand::Const(_, v)) if v.fits_width(n) => {
                Operand::Const(want, v.clone())
            }
            (Type::Int(from), Type::Int(to), _) if from < to => self.emit(
                "w",
                want,
                InstKind::Widen {
                    from: have,
                    a: o,
                    to: want,
                },
            ),
            (Type::Int(_), Type::Int(1), _) => {
                let zero = konst(have.width().unwrap(), 0);
                self.emit(
                    "s",
                    Type::Int(1),
                    InstKind::Cmp {
                        ty: have,
                        a: o,
                        b: zero,
                    },
                )
            }
            _ => {
                self.errs.push(format!(
                    "cannot convert a value of type {have} to {want} during legalization"
                ));
                o
            }
        }
    }

    /// Renormalize a promoted value to the symmetric range of `w` trits.
    fn renormalize(&mut self, v: Operand, w: u32, at: u32) -> Operand {
        if w == at {
            return v;
        }
        let m = mask(w, at);
        self.emit(
            "n",
            Type::Int(at),
            InstKind::Plain {
                op: PlainOp::TMul,
                ty: Type::Int(at),
                a: v,
                b: m,
            },
        )
    }

    /// The trits at or above position `w` — zero exactly when the value fits
    /// in `w` trits, and otherwise carrying the direction of the overflow.
    fn high_part(&mut self, v: Operand, w: u32, at: u32) -> Operand {
        let k = konst(at, w as i128);
        self.emit(
            "h",
            Type::Int(at),
            InstKind::Plain {
                op: PlainOp::Shr,
                ty: Type::Int(at),
                a: v,
                b: k,
            },
        )
    }

    fn sign_of(&mut self, v: Operand, at: u32) -> Operand {
        let zero = konst(at, 0);
        self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(at),
                a: v,
                b: zero,
            },
        )
    }

    /// Guard a promoted shift: at the promoted width the machine would only
    /// fault for `k ≥ L`, but the operation is logically `tw` and must fault
    /// for `k ≥ w` and for negative `k` (AM §3.3 — not masked, not
    /// undefined).
    fn guard_shift(&mut self, k: &Operand, w: u32, at: u32) {
        if let Operand::Const(_, v) = k {
            let out_of_range = match v.to_i128() {
                Some(n) => n < 0 || n >= w as i128,
                None => true,
            };
            if out_of_range {
                // Statically always a fault: the block ends here.
                self.halted = Some(Terminator::Trap(FaultCode::Shift));
            }
            return;
        }
        let neg = self.sign_of(k.clone(), at);
        self.trap_if(neg, [true, false, false], FaultCode::Shift);
        let limit = konst(at, w as i128 - 1);
        let over = self.emit(
            "c",
            Type::Int(1),
            InstKind::Cmp {
                ty: Type::Int(at),
                a: k.clone(),
                b: limit,
            },
        );
        self.trap_if(over, [false, false, true], FaultCode::Shift);
    }
}

fn legalize_function(
    f: &Function,
    widths: &WidthMap,
    sigs: &HashMap<String, Signature>,
) -> Result<Function, Vec<String>> {
    let sig = match sigs.get(&f.sig.name) {
        Some(s) => s.clone(),
        None => return Err(vec!["signature could not be legalized".into()]),
    };

    // A prefix no existing name starts with, so fresh names cannot collide.
    let mut prefix = "lz".to_string();
    let mut names: Vec<&str> = Vec::new();
    for b in &f.blocks {
        names.push(&b.label);
        names.extend(b.params.iter().map(|(n, _)| n.as_str()));
        for i in &b.insts {
            names.extend(i.results.iter().map(String::as_str));
        }
    }
    names.extend(f.sig.params.iter().map(|(n, _)| n.as_str()));
    while names.iter().any(|n| n.starts_with(&prefix)) {
        prefix.push('z');
    }
    prefix.push('.');

    let mut e = Emit {
        widths,
        sigs,
        blocks: Vec::new(),
        label: String::new(),
        params: Vec::new(),
        insts: Vec::new(),
        actual: HashMap::new(),
        subst: HashMap::new(),
        parts: HashMap::new(),
        block_shapes: HashMap::new(),
        prefix,
        counter: 0,
        errs: Vec::new(),
        halted: None,
    };

    for (n, t) in &sig.params {
        e.actual.insert(n.clone(), *t);
    }

    // Block parameter shapes are fixed up front: a branch has to agree with
    // its destination before that destination has been walked. A parameter
    // too wide for the target becomes one parameter per part.
    for b in &f.blocks {
        let shapes: Vec<ParamShape> = b
            .params
            .iter()
            .map(|(n, t)| {
                let w = t.width().unwrap_or(e.widths.target.word);
                match e.widths.classify(w) {
                    Class::Legal(l) => {
                        let ty = if matches!(t, Type::Ptr) {
                            Type::Ptr
                        } else {
                            Type::Int(l)
                        };
                        e.actual.insert(n.clone(), ty);
                        ParamShape {
                            slots: vec![(n.clone(), ty)],
                            wide: None,
                        }
                    }
                    Class::Wide(wide) => {
                        let slots: Vec<(String, Type)> = (0..wide.k)
                            .map(|i| (format!("{}{n}.{i}", e.prefix), wide.ty()))
                            .collect();
                        for (name, ty) in &slots {
                            e.actual.insert(name.clone(), *ty);
                        }
                        e.parts.insert(
                            n.clone(),
                            slots
                                .iter()
                                .map(|(name, _)| Operand::Value(name.clone()))
                                .collect(),
                        );
                        ParamShape {
                            slots,
                            wide: Some(wide),
                        }
                    }
                }
            })
            .collect();
        e.block_shapes.insert(b.label.clone(), shapes);
    }

    for b in &f.blocks {
        e.start_block(b.label.clone());
        e.params = e.block_shapes[&b.label]
            .iter()
            .flat_map(|s| s.slots.clone())
            .collect();
        e.halted = None;

        for inst in &b.insts {
            legalize_inst(&mut e, inst);
        }
        let term = match e.halted.take() {
            Some(halt) => halt,
            None => legalize_terminator(&mut e, &b.term, &sig),
        };
        e.finish_block(term);
    }

    if e.errs.is_empty() {
        Ok(Function {
            sig,
            blocks: e.blocks,
        })
    } else {
        Err(e.errs)
    }
}

fn legalize_terminator(e: &mut Emit, t: &Terminator, sig: &Signature) -> Terminator {
    match t {
        Terminator::Br3 {
            t: cond,
            neg,
            zero,
            pos,
        } => {
            let cond = e.coerce(cond, Type::Int(1));
            Terminator::Br3 {
                t: cond,
                neg: legalize_target(e, neg),
                zero: legalize_target(e, zero),
                pos: legalize_target(e, pos),
            }
        }
        Terminator::Br(d) => Terminator::Br(legalize_target(e, d)),
        Terminator::Ret(None) => Terminator::Ret(None),
        Terminator::Ret(Some(v)) => {
            let want = sig
                .ret
                .expect("verified: value returned from a () function");
            Terminator::Ret(Some(e.coerce(v, want)))
        }
        other => other.clone(),
    }
}

fn legalize_target(e: &mut Emit, t: &Target) -> Target {
    // Arguments are converted to the *destination's* parameter shapes, which
    // were fixed before any block was emitted. A wide parameter consumes one
    // argument here and produces one argument per part.
    let shapes: Vec<(Option<Wide>, Type)> = e
        .block_shapes
        .get(&t.label)
        .map(|shapes| shapes.iter().map(|s| (s.wide, s.slots[0].1)).collect())
        .unwrap_or_default();

    let mut args = Vec::new();
    for (i, a) in t.args.iter().enumerate() {
        match shapes.get(i) {
            Some((Some(wide), _)) => args.extend(expand::parts_of(e, a, *wide)),
            Some((None, ty)) => args.push(e.coerce(a, *ty)),
            None => args.push(e.resolve(a)),
        }
    }
    Target {
        label: t.label.clone(),
        args,
    }
}

fn legalize_inst(e: &mut Emit, inst: &Inst) {
    if e.halted.is_some() {
        return;
    }
    // A value too wide for the target takes the expansion path; everything
    // else takes the promotion path below.
    if let Some(wide) = wide_operation(e, inst)
        && expand_inst(e, inst, wide)
    {
        return;
    }
    match &inst.kind {
        InstKind::Flavored {
            op,
            flavor,
            ty,
            a,
            b,
        } => flavored(e, inst, *op, *flavor, *ty, a, b),

        InstKind::Plain { op, ty, a, b } => {
            let w = ty.width().expect("verified");
            let Ok(at) = width(e, w) else { return };
            let (a, b) = (e.coerce(a, Type::Int(at)), e.coerce(b, Type::Int(at)));
            if matches!(op, PlainOp::Shr) && at != w {
                e.guard_shift(&b, w, at);
                if e.halted.is_some() {
                    return;
                }
            }
            // div, rem, shr and the trit-wise operations all preserve
            // normalization: their results already lie inside tw's range
            // whenever their operands do, so no mask is needed.
            define(
                e,
                inst,
                0,
                Type::Int(at),
                InstKind::Plain {
                    op: *op,
                    ty: Type::Int(at),
                    a,
                    b,
                },
            );
        }

        InstKind::Neg { ty, a } => {
            let w = ty.width().expect("verified");
            let Ok(at) = width(e, w) else { return };
            let a = e.coerce(a, Type::Int(at));
            define(
                e,
                inst,
                0,
                Type::Int(at),
                InstKind::Neg {
                    ty: Type::Int(at),
                    a,
                },
            );
        }

        InstKind::Cmp { ty, a, b } => {
            let w = ty.width().expect("verified");
            let Ok(at) = width(e, w) else { return };
            let (a, b) = (e.coerce(a, Type::Int(at)), e.coerce(b, Type::Int(at)));
            define(
                e,
                inst,
                0,
                Type::Int(1),
                InstKind::Cmp {
                    ty: Type::Int(at),
                    a,
                    b,
                },
            );
        }

        InstKind::Select3 {
            t,
            ty,
            neg,
            zero,
            pos,
        } => {
            let w = ty.width().expect("verified");
            let Ok(at) = width(e, w) else { return };
            let t = e.coerce(t, Type::Int(1));
            let neg = e.coerce(neg, Type::Int(at));
            let zero = e.coerce(zero, Type::Int(at));
            let pos = e.coerce(pos, Type::Int(at));
            define(
                e,
                inst,
                0,
                Type::Int(at),
                InstKind::Select3 {
                    t,
                    ty: Type::Int(at),
                    neg,
                    zero,
                    pos,
                },
            );
        }

        InstKind::Slot { trytes } => {
            define(e, inst, 0, Type::Ptr, InstKind::Slot { trytes: *trytes });
        }

        InstKind::Load { ty, p } => {
            let w = ty.width().expect("verified");
            if !memory_width_ok(e, w) {
                return;
            }
            let p = e.coerce(p, Type::Ptr);
            define(e, inst, 0, *ty, InstKind::Load { ty: *ty, p });
        }

        InstKind::Store { ty, v, p } => {
            let w = ty.width().expect("verified");
            if !memory_width_ok(e, w) {
                return;
            }
            let v = e.coerce(v, *ty);
            let p = e.coerce(p, Type::Ptr);
            e.push(Vec::new(), InstKind::Store { ty: *ty, v, p });
        }

        InstKind::Offset { p, d } => {
            let Ok(at) = width(e, e.widths.target.ptr_width) else {
                return;
            };
            let p = e.coerce(p, Type::Ptr);
            let d = e.coerce(d, Type::Int(at));
            define(e, inst, 0, Type::Ptr, InstKind::Offset { p, d });
        }

        InstKind::Widen { from, a, to } => {
            let (Ok(lf), Ok(lt)) = (
                width(e, from.width().expect("verified")),
                width(e, to.width().expect("verified")),
            ) else {
                return;
            };
            let a = e.coerce(a, Type::Int(lf));
            if lf == lt {
                // The promoted value is already exact at this width: the
                // widen is an identity, so its uses read the source directly.
                substitute(e, inst, a);
            } else {
                define(
                    e,
                    inst,
                    0,
                    Type::Int(lt),
                    InstKind::Widen {
                        from: Type::Int(lf),
                        a,
                        to: Type::Int(lt),
                    },
                );
            }
        }

        InstKind::Trunc { from, a, to } => {
            let w = to.width().expect("verified");
            let (Ok(lf), Ok(lt)) = (width(e, from.width().expect("verified")), width(e, w)) else {
                return;
            };
            let a = e.coerce(a, Type::Int(lf));
            let narrowed = if lt < lf {
                e.emit(
                    "t",
                    Type::Int(lt),
                    InstKind::Trunc {
                        from: Type::Int(lf),
                        a,
                        to: Type::Int(lt),
                    },
                )
            } else {
                a
            };
            // Whatever legal width it now lives in, the value has to end up
            // wrapped into tw's range.
            let v = e.renormalize(narrowed, w, lt);
            substitute(e, inst, v);
        }

        InstKind::Call { callee, args, ret } => {
            let Some(sig) = e.sigs.get(callee).cloned() else {
                e.errs
                    .push(format!("`@{callee}` has no legalized signature"));
                return;
            };
            let args = args
                .iter()
                .zip(&sig.params)
                .map(|(a, (_, want))| e.coerce(a, *want))
                .collect();
            let kind = InstKind::Call {
                callee: callee.clone(),
                args,
                ret: sig.ret,
            };
            match (ret, sig.ret) {
                (Some(_), Some(t)) => define(e, inst, 0, t, kind),
                _ => e.push(Vec::new(), kind),
            }
        }
    }
}

fn flavored(
    e: &mut Emit,
    inst: &Inst,
    op: FlavoredOp,
    flavor: Flavor,
    ty: Type,
    a: &Operand,
    b: &Operand,
) {
    let w = ty.width().expect("verified");
    let Ok(at) = width(e, w) else { return };
    let (a, b) = (e.coerce(a, Type::Int(at)), e.coerce(b, Type::Int(at)));

    if at == w {
        let results = inst.results.clone();
        for (i, r) in results.iter().enumerate() {
            let t = if i == 0 { Type::Int(w) } else { Type::Int(1) };
            e.actual.insert(r.clone(), t);
        }
        e.push(
            results,
            InstKind::Flavored {
                op,
                flavor,
                ty,
                a,
                b,
            },
        );
        return;
    }

    if matches!(op, FlavoredOp::Shl) {
        e.guard_shift(&b, w, at);
        if e.halted.is_some() {
            return;
        }
    }

    // Detecting overflow of the *logical* width needs the exact result, and
    // for `mul` that is up to 2w trits wide. `.wrap` is fine regardless — the
    // low w trits of the exact product survive any wider wrap — but the
    // overflow-reporting flavors cannot be synthesized without room.
    if matches!(op, FlavoredOp::Mul | FlavoredOp::Shl)
        && flavor != Flavor::Wrap
        && !fits_exact(op, w, at)
    {
        e.errs.push(format!(
            "`{}{}` at t{w} promoted to t{at} cannot detect overflow: the exact \
             result needs more than t{at}; expansion is not implemented yet",
            op.name(),
            flavor.suffix()
        ));
        return;
    }

    // Compute wide and wrapping: at the promoted width the exact result fits,
    // so this wrap never actually wraps.
    let wide = e.emit(
        "r",
        Type::Int(at),
        InstKind::Flavored {
            op,
            flavor: Flavor::Wrap,
            ty: Type::Int(at),
            a,
            b,
        },
    );

    match flavor {
        Flavor::Wrap => {
            let v = e.renormalize(wide, w, at);
            substitute(e, inst, v);
        }
        Flavor::Trap => {
            let hi = e.high_part(wide.clone(), w, at);
            let c = e.sign_of(hi, at);
            e.trap_if(c, [true, false, true], FaultCode::Overflow);
            // Past the check the value provably fits tw, so it is already
            // normalized.
            substitute(e, inst, wide);
        }
        Flavor::Flag => {
            let hi = e.high_part(wide.clone(), w, at);
            // The overflow trit is the direction of the overflow, which is
            // the sign of the discarded high part.
            let f = e.sign_of(hi, at);
            let v = e.renormalize(wide, w, at);
            substitute(e, inst, v);
            if let Some(name) = inst.results.get(1) {
                match f {
                    Operand::Value(fv) => {
                        e.subst.insert(name.clone(), Operand::Value(fv));
                        e.actual.insert(name.clone(), Type::Int(1));
                    }
                    other => {
                        e.subst.insert(name.clone(), other);
                    }
                }
            }
        }
    }
}

/// The wide layout an instruction operates on, if any of the widths it names
/// is too wide for the target.
fn wide_operation(e: &Emit, inst: &Inst) -> Option<Wide> {
    let widest = |ts: [Option<u32>; 2]| -> Option<Wide> {
        ts.into_iter()
            .flatten()
            .filter_map(|w| match e.widths.classify(w) {
                Class::Wide(wide) => Some(wide),
                Class::Legal(_) => None,
            })
            .max_by_key(|w| w.logical())
    };
    match &inst.kind {
        InstKind::Flavored { ty, .. }
        | InstKind::Plain { ty, .. }
        | InstKind::Neg { ty, .. }
        | InstKind::Cmp { ty, .. }
        | InstKind::Select3 { ty, .. }
        | InstKind::Load { ty, .. }
        | InstKind::Store { ty, .. } => widest([ty.width(), None]),
        InstKind::Widen { from, to, .. } | InstKind::Trunc { from, to, .. } => {
            widest([from.width(), to.width()])
        }
        _ => None,
    }
}

/// Expand one instruction. Returns `false` if this instruction has no
/// expansion, in which case an error has been recorded.
fn expand_inst(e: &mut Emit, inst: &Inst, wide: Wide) -> bool {
    match &inst.kind {
        InstKind::Flavored {
            op: op @ (FlavoredOp::Add | FlavoredOp::Sub),
            flavor,
            a,
            b,
            ..
        } => {
            expand::add_sub(e, inst, *op, *flavor, wide, a, b);
            true
        }
        InstKind::Neg { a, .. } => {
            expand::positionwise(e, inst, wide, a, None, |a, _, ty| InstKind::Neg { ty, a });
            true
        }
        InstKind::Plain {
            op: op @ (PlainOp::TMin | PlainOp::TMax | PlainOp::TMul),
            a,
            b,
            ..
        } => {
            let op = *op;
            expand::positionwise(e, inst, wide, a, Some(b), move |a, b, ty| InstKind::Plain {
                op,
                ty,
                a,
                b: b.expect("binary"),
            });
            true
        }
        InstKind::Cmp { a, b, .. } => {
            expand::cmp(e, inst, wide, a, b);
            true
        }
        InstKind::Select3 {
            t, neg, zero, pos, ..
        } => {
            expand::select3(e, inst, wide, t, neg, zero, pos);
            true
        }
        InstKind::Load { p, .. } => {
            expand::load(e, inst, wide, p);
            true
        }
        InstKind::Store { v, p, .. } => {
            expand::store(e, wide, v, p);
            true
        }
        InstKind::Widen { from, a, to } => {
            let (from, to) = (
                e.widths.classify(from.width().expect("verified")),
                e.widths.classify(to.width().expect("verified")),
            );
            match to {
                Class::Wide(to) => expand::widen_into(e, inst, a, from, to),
                // Widening cannot narrow, so a wide source with a legal
                // destination is impossible in verified input.
                Class::Legal(_) => e.errs.push("`widen` narrows".into()),
            }
            true
        }
        InstKind::Trunc { from, a, to } => {
            let w = to.width().expect("verified");
            let from_class = e.widths.classify(from.width().expect("verified"));
            let to_class = e.widths.classify(w);
            match from_class {
                Class::Wide(from) => expand::trunc_from(e, inst, a, from, to_class, w),
                Class::Legal(_) => e.errs.push("`trunc` widens".into()),
            }
            true
        }
        // Everything else at a wide width needs a technique this pass does
        // not have; say which, rather than emit something plausible.
        InstKind::Flavored { op, .. } => {
            e.errs.push(format!(
                "`{}` at t{} needs expansion, which requires a widening multiply \
                 (a `mulhi`-style instruction TIR does not define): the product of \
                 two t{} parts does not fit in a legal width",
                op.name(),
                wide.logical(),
                wide.part
            ));
            true
        }
        InstKind::Plain { op, .. } => {
            e.errs.push(format!(
                "`{}` at t{} needs expansion, which is not implemented \
                 (multi-part division and shifts are still to be written)",
                op.name(),
                wide.logical()
            ));
            true
        }
        _ => false,
    }
}

/// Can the exact result of `op` at logical width `w` be held in `at` trits?
fn fits_exact(op: FlavoredOp, w: u32, at: u32) -> bool {
    match op {
        // A sum of two w-trit values needs w+1 trits.
        FlavoredOp::Add | FlavoredOp::Sub => at > w,
        FlavoredOp::Mul => at >= 2 * w,
        // A shift by up to w−1 needs 2w−1 trits to stay exact.
        FlavoredOp::Shl => at >= 2 * w - 1,
    }
}

fn width(e: &mut Emit, w: u32) -> Result<u32, ()> {
    match e.widths.arith(w) {
        Ok(at) => Ok(at),
        Err(msg) => {
            e.errs.push(msg);
            Err(())
        }
    }
}

/// Memory access widths are not promoted: a load of `tw` moves ⌈w/9⌉ trytes,
/// which is a property of the address space rather than of the arithmetic
/// unit (AM §2.1–2.3). Only widths the target cannot address at all are an
/// error — which needs expansion, not promotion.
fn memory_width_ok(e: &mut Emit, w: u32) -> bool {
    if e.widths.target.legal_at_least(w).is_some() {
        return true;
    }
    e.errs.push(format!(
        "memory access of t{w} exceeds the widest legal width t{}; \
         expansion is not implemented yet",
        e.widths.target.widest_legal()
    ));
    false
}

/// Bind one of the original instruction's results to a freshly emitted one.
fn define(e: &mut Emit, inst: &Inst, index: usize, ty: Type, kind: InstKind) {
    match inst.results.get(index) {
        Some(name) => {
            e.actual.insert(name.clone(), ty);
            e.push(vec![name.clone()], kind);
        }
        None => e.push(Vec::new(), kind),
    }
}

/// The original instruction's result is whatever `value` names.
fn substitute(e: &mut Emit, inst: &Inst, value: Operand) {
    if let Some(name) = inst.results.first() {
        if let Some(t) = e.type_of(&value) {
            e.actual.insert(name.clone(), t);
        }
        e.subst.insert(name.clone(), value);
    }
}
