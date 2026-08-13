//! A reference interpreter for TIR, executing directly against AM semantics.
//!
//! This is the oracle the rest of the pipeline is measured against: a
//! transformation is correct iff it preserves observable behavior here (TIR
//! preamble). It is written for clarity and for faithfully reproducing the
//! defined-fault / UB split of TIR §4, not for speed.
//!
//! Memory is tryte-addressed with provenance (TIR §5): a pointer remembers
//! the allocation it came from, so an `offset` that escapes its allocation is
//! caught rather than silently wandering into another object. Uninitialized
//! storage yields *poison*, which propagates through arithmetic and becomes
//! UB when branched on.

use super::ir::*;
use std::collections::HashMap;
use trit_core::{Bt, Fault, FaultCode, Flavor, Tint, Trit};

/// Why execution stopped short of returning.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Halt {
    /// A defined fault (AM §4) — a machine halt with a code.
    Fault(Fault),
    /// Undefined behavior. TIR has exactly four sources (TIR §4); an
    /// implementation is free to do anything, and this one reports it.
    Ub(String),
    /// Not a program property: the interpreter's own step budget ran out.
    OutOfFuel,
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Halt::Fault(x) => write!(f, "{x}"),
            Halt::Ub(m) => write!(f, "undefined behavior: {m}"),
            Halt::OutOfFuel => f.write_str("interpreter step budget exhausted"),
        }
    }
}

impl std::error::Error for Halt {}

impl From<Fault> for Halt {
    fn from(f: Fault) -> Halt {
        Halt::Fault(f)
    }
}

/// A pointer: an allocation plus a tryte displacement within it. The pair is
/// the provenance (TIR §5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ptr {
    /// Which allocation this pointer is derived from.
    pub alloc: usize,
    /// Displacement in trytes from the allocation's base. May equal the
    /// allocation's size — the one-past-the-end address, which is a legal
    /// pointer but not a legal access.
    pub offset: i128,
}

/// A runtime value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Val {
    /// An integer of a known width.
    Int(Tint),
    /// An address.
    Ptr(Ptr),
    /// Poison: the result of reading uninitialized storage. Propagates
    /// through every operation; branching on it is UB (TIR §4.4).
    Poison,
}

impl Val {
    /// The integer this value holds, if it is one.
    pub fn as_int(&self) -> Option<&Tint> {
        match self {
            Val::Int(i) => Some(i),
            _ => None,
        }
    }
}

impl std::fmt::Display for Val {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Ptr(p) => write!(f, "ptr(#{}+{})", p.alloc, p.offset),
            Val::Poison => f.write_str("poison"),
        }
    }
}

/// One allocation — a global or a `slot`.
struct Alloc {
    name: String,
    /// Set when this allocation *is* a function's address (TIR §1.2). It has
    /// no trytes, so every load and store through it is out of range — which
    /// is what §1.2 means by "loading or storing through one is UB".
    function: Option<String>,
    /// One entry per tryte; `None` is uninitialized.
    trytes: Vec<Option<Tint>>,
    /// Pointers stored in this allocation, by tryte offset.
    ///
    /// A pointer is not a number here: it carries the provenance of TIR §5.
    /// Storing one to memory therefore stores it beside the trytes rather
    /// than into them, so that loading it back yields the same provenance.
    /// A real machine keeps the address and nothing else; the interpreter
    /// keeps more so that it can *check* what the machine may assume.
    pointers: std::collections::HashMap<i128, Ptr>,
}

/// Trytes occupied by a `tN` access.
pub fn size_trytes(width: u32) -> u32 {
    width.div_ceil(9).max(1)
}

/// Natural alignment of a `tN` access, in trytes (AM §2.3).
///
/// The AM tabulates two rows — 1…9 trits align to 1 tryte, 10…27 trits to 3 —
/// and stops at the word. This is the smallest power of three at least the
/// access's tryte count, which reproduces both rows exactly and extends past
/// t27 for the widths legalization introduces (`docs/spec-gaps.md` G4.1).
pub fn align_trytes(width: u32) -> u32 {
    let size = size_trytes(width);
    let mut a = 1;
    while a < size {
        a *= 3;
    }
    a
}

/// The interpreter.
pub struct Interp<'m> {
    module: &'m Module,
    allocs: Vec<Alloc>,
    globals: HashMap<String, usize>,
    /// The allocation that is each function's address (TIR §1.2).
    functions: HashMap<String, usize>,
    fuel: u64,
    depth: u32,
}

/// How many instructions a run may execute before giving up.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// How deep calls may nest.
pub const MAX_DEPTH: u32 = 512;

impl<'m> Interp<'m> {
    /// Prepare a module for execution, allocating its globals.
    pub fn new(module: &'m Module) -> Interp<'m> {
        let mut interp = Interp {
            module,
            allocs: Vec::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            fuel: DEFAULT_FUEL,
            depth: 0,
        };
        // One allocation per function, so that `addr @f` has something with
        // provenance to point at (TIR §1.2).
        for name in module
            .funcs
            .iter()
            .map(|f| f.sig.name.clone())
            .chain(module.decls.iter().map(|d| d.name.clone()))
        {
            let id = interp.allocs.len();
            interp.allocs.push(Alloc {
                name: format!("@{name}"),
                function: Some(name.clone()),
                trytes: Vec::new(),
                pointers: std::collections::HashMap::new(),
            });
            interp.functions.insert(name, id);
        }

        for g in &module.globals {
            let mut trytes: Vec<Option<Tint>> = Vec::new();
            let mut pointers = std::collections::HashMap::new();
            match &g.init {
                None => trytes = vec![None; g.trytes as usize],
                Some(items) => {
                    for item in items {
                        match item {
                            InitItem::Tryte(v) => trytes.push(Some(Tint::wrapping(9, v.clone()))),
                            InitItem::Addr(name) => {
                                // The address itself is a relocation the
                                // target resolves, so the trytes hold nothing
                                // and the provenance is kept beside them.
                                if let Some(id) = interp.functions.get(name) {
                                    pointers.insert(
                                        trytes.len() as i128,
                                        Ptr {
                                            alloc: *id,
                                            offset: 0,
                                        },
                                    );
                                } else if let Some(id) = interp.globals.get(name) {
                                    pointers.insert(
                                        trytes.len() as i128,
                                        Ptr {
                                            alloc: *id,
                                            offset: 0,
                                        },
                                    );
                                }
                                trytes.extend([None, None, None]);
                            }
                        }
                    }
                }
            }
            let id = interp.allocs.len();
            interp.allocs.push(Alloc {
                name: format!("@{}", g.name),
                function: None,
                trytes,
                pointers,
            });
            interp.globals.insert(g.name.clone(), id);
        }
        interp
    }

    /// Set the step budget.
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    /// Call a function by name.
    pub fn call(&mut self, name: &str, args: &[Val]) -> Result<Option<Val>, Halt> {
        let f = self
            .module
            .function(name)
            .ok_or_else(|| Halt::Ub(format!("`@{name}` has no body in this module")))?;
        if f.sig.params.len() != args.len() {
            return Err(Halt::Ub(format!(
                "`@{name}` takes {} arguments, {} given",
                f.sig.params.len(),
                args.len()
            )));
        }
        if self.depth >= MAX_DEPTH {
            return Err(Halt::Ub(format!("call depth exceeded {MAX_DEPTH}")));
        }
        self.depth += 1;

        let mut env: HashMap<String, Val> = f
            .sig
            .params
            .iter()
            .map(|(n, _)| n.clone())
            .zip(args.iter().cloned())
            .collect();
        let frame_base = self.allocs.len();

        let mut block = &f.blocks[0];
        let result = loop {
            for inst in &block.insts {
                if self.fuel == 0 {
                    self.depth -= 1;
                    return Err(Halt::OutOfFuel);
                }
                self.fuel -= 1;
                match self.exec_inst(inst, &mut env) {
                    Ok(()) => {}
                    Err(e) => {
                        self.depth -= 1;
                        return Err(e);
                    }
                }
            }
            match self.exec_terminator(&block.term, &env) {
                Err(e) => {
                    self.depth -= 1;
                    return Err(e);
                }
                Ok(Flow::Return(v)) => break v,
                Ok(Flow::Jump(label, args)) => {
                    let Some(next) = f.block(&label) else {
                        self.depth -= 1;
                        return Err(Halt::Ub(format!("`^{label}` is not a block in `@{name}`")));
                    };
                    // Block parameters are the phi-equivalent: arguments are
                    // evaluated in the predecessor, then bound on entry. SSA
                    // makes every name unique, so rebinding on a back edge is
                    // the only shadowing that can occur.
                    for ((n, _), v) in next.params.iter().zip(args) {
                        env.insert(n.clone(), v);
                    }
                    block = next;
                }
            }
        };

        // `slot` storage has function lifetime (TIR §3.4): the frame's
        // allocations die here. They are kept in place, not popped, so that a
        // pointer that outlives them reports a dead allocation rather than
        // aliasing a later frame.
        for a in &mut self.allocs[frame_base..] {
            a.trytes.clear();
        }
        self.depth -= 1;
        Ok(result)
    }

    fn exec_inst(&mut self, inst: &Inst, env: &mut HashMap<String, Val>) -> Result<(), Halt> {
        let results = match &inst.kind {
            InstKind::Flavored {
                op,
                flavor,
                ty,
                a,
                b,
            } => {
                let (a, b) = (self.operand(a, env)?, self.operand(b, env)?);
                match (a, b) {
                    (Val::Int(a), Val::Int(b)) => {
                        let (v, over) = match op {
                            FlavoredOp::Add => a.add(&b, *flavor)?,
                            FlavoredOp::Sub => a.sub(&b, *flavor)?,
                            FlavoredOp::Mul => a.mul(&b, *flavor)?,
                            FlavoredOp::Shl => {
                                let k = shift_amount(&b)?;
                                a.shl(k, *flavor)?
                            }
                        };
                        if *flavor == Flavor::Flag {
                            vec![Val::Int(v), Val::Int(Tint::wrapping(1, Bt::from(over)))]
                        } else {
                            vec![Val::Int(v)]
                        }
                    }
                    _ => poison_results(if *flavor == Flavor::Flag { 2 } else { 1 }, *ty),
                }
            }
            InstKind::Plain { op, ty, a, b } => {
                let (a, b) = (self.operand(a, env)?, self.operand(b, env)?);
                match (a, b) {
                    (Val::Int(a), Val::Int(b)) => vec![Val::Int(match op {
                        PlainOp::Div => a.div(&b)?,
                        PlainOp::Rem => a.rem(&b)?,
                        PlainOp::Shr => a.shr(shift_amount(&b)?)?,
                        PlainOp::TMin => a.tmin(&b),
                        PlainOp::TMax => a.tmax(&b),
                        PlainOp::TMul => a.tmul(&b),
                    })],
                    _ => poison_results(1, *ty),
                }
            }
            InstKind::Neg { ty, a } => match self.operand(a, env)? {
                Val::Int(a) => vec![Val::Int(a.neg())],
                _ => poison_results(1, *ty),
            },
            InstKind::Cmp { a, b, .. } => {
                let (a, b) = (self.operand(a, env)?, self.operand(b, env)?);
                match (a, b) {
                    (Val::Int(a), Val::Int(b)) => {
                        vec![Val::Int(Tint::wrapping(1, Bt::from(a.cmp3(&b))))]
                    }
                    _ => vec![Val::Poison],
                }
            }
            InstKind::Select3 {
                t, neg, zero, pos, ..
            } => {
                let t = self.operand(t, env)?;
                match t {
                    Val::Int(t) => {
                        let arm = match t.sign() {
                            Trit::Neg => neg,
                            Trit::Zero => zero,
                            Trit::Pos => pos,
                        };
                        vec![self.operand(arm, env)?]
                    }
                    // Selecting on poison is not a branch, so it is not UB;
                    // the result is simply poison.
                    _ => vec![Val::Poison],
                }
            }
            InstKind::Slot { trytes } => {
                let id = self.allocs.len();
                self.allocs.push(Alloc {
                    name: format!("slot#{id}"),
                    function: None,
                    trytes: vec![None; *trytes as usize],
                    pointers: std::collections::HashMap::new(),
                });
                vec![Val::Ptr(Ptr {
                    alloc: id,
                    offset: 0,
                })]
            }
            InstKind::Load { ty, p } => {
                let p = self.operand(p, env)?;
                match ty.width() {
                    Some(width) => vec![self.load(&p, width)?],
                    None => vec![self.load_pointer(&p)?],
                }
            }
            InstKind::Store { ty, v, p } => {
                let v = self.operand(v, env)?;
                let p = self.operand(p, env)?;
                if ty.width().is_none() {
                    self.store_pointer(&p, &v)?;
                    return Ok(());
                }
                let width = ty.width().expect("checked just above");
                self.store(&p, width, &v)?;
                Vec::new()
            }
            InstKind::Offset { p, d } => {
                let p = self.operand(p, env)?;
                let d = self.operand(d, env)?;
                match (p, d) {
                    (Val::Ptr(p), Val::Int(d)) => {
                        let d = d
                            .to_i128()
                            .ok_or_else(|| Halt::Ub("offset displacement is absurd".into()))?;
                        let offset = p.offset + d;
                        // A pointer may address the allocation's trytes plus
                        // the one-past-the-end address, and nothing else.
                        let size = self.allocs[p.alloc].trytes.len() as i128;
                        if offset < 0 || offset > size {
                            return Err(Halt::Ub(format!(
                                "`offset` leaves the provenance of {}: {offset} is outside 0..={size}",
                                self.allocs[p.alloc].name
                            )));
                        }
                        vec![Val::Ptr(Ptr {
                            alloc: p.alloc,
                            offset,
                        })]
                    }
                    _ => vec![Val::Poison],
                }
            }
            InstKind::Widen { a, to, .. } => match self.operand(a, env)? {
                Val::Int(a) => vec![Val::Int(a.widen(to.width().unwrap()))],
                _ => vec![Val::Poison],
            },
            InstKind::Trunc { a, to, .. } => match self.operand(a, env)? {
                Val::Int(a) => vec![Val::Int(a.trunc(to.width().unwrap()))],
                _ => vec![Val::Poison],
            },
            InstKind::Call { callee, args, .. } => {
                let args: Vec<Val> = args
                    .iter()
                    .map(|a| self.operand(a, env))
                    .collect::<Result<_, _>>()?;
                // TIR §4's fifth UB source: a call through anything that is
                // not the address of a function.
                let name = match callee {
                    Callee::Direct(n) => n.clone(),
                    Callee::Indirect(p) => match self.operand(p, env)? {
                        Val::Ptr(ptr) => match &self.allocs[ptr.alloc].function {
                            Some(n) if ptr.offset == 0 => n.clone(),
                            _ => {
                                return Err(Halt::Ub(
                                    "indirect call through a pointer that is not a \
                                     function's address (TIR §3.7)"
                                        .into(),
                                ));
                            }
                        },
                        _ => {
                            return Err(Halt::Ub(
                                "indirect call through something that is not a pointer \
                                 (TIR §3.7)"
                                    .into(),
                            ));
                        }
                    },
                };
                match self.call(&name, &args)? {
                    Some(v) => vec![v],
                    None => Vec::new(),
                }
            }
        };

        for (name, value) in inst.results.iter().zip(results) {
            env.insert(name.clone(), value);
        }
        Ok(())
    }

    fn exec_terminator(&self, t: &Terminator, env: &HashMap<String, Val>) -> Result<Flow, Halt> {
        match t {
            Terminator::Br3 { t, neg, zero, pos } => {
                let sel = self.operand(t, env)?;
                let Val::Int(sel) = sel else {
                    // TIR §4.4: branching on poison is UB.
                    return Err(Halt::Ub("`br3` on a poison selector".into()));
                };
                let target = match sel.sign() {
                    Trit::Neg => neg,
                    Trit::Zero => zero,
                    Trit::Pos => pos,
                };
                self.jump(target, env)
            }
            Terminator::Br(d) => self.jump(d, env),
            Terminator::Ret(None) => Ok(Flow::Return(None)),
            Terminator::Ret(Some(v)) => Ok(Flow::Return(Some(self.operand(v, env)?))),
            Terminator::Trap(code) => Err(Halt::Fault(Fault::new(*code))),
            Terminator::Unreachable => {
                Err(Halt::Ub("control reached `unreachable` (TIR §4.1)".into()))
            }
        }
    }

    fn jump(&self, target: &Target, env: &HashMap<String, Val>) -> Result<Flow, Halt> {
        let args = target
            .args
            .iter()
            .map(|a| self.operand(a, env))
            .collect::<Result<_, _>>()?;
        Ok(Flow::Jump(target.label.clone(), args))
    }

    fn operand(&self, o: &Operand, env: &HashMap<String, Val>) -> Result<Val, Halt> {
        match o {
            Operand::Value(n) => env
                .get(n)
                .cloned()
                .ok_or_else(|| Halt::Ub(format!("`%{n}` is not live here"))),
            Operand::Const(Type::Int(w), v) => Ok(Val::Int(Tint::wrapping(*w, v.clone()))),
            Operand::Const(Type::Ptr, _) => {
                Err(Halt::Ub("there are no pointer constants in TIR".into()))
            }
            Operand::Global(g) => self
                .globals
                .get(g)
                .map(|&alloc| Val::Ptr(Ptr { alloc, offset: 0 }))
                .ok_or_else(|| Halt::Ub(format!("`@{g}` is not a global"))),
        }
    }

    fn check_access(&self, p: &Val, width: u32, what: &str) -> Result<Ptr, Halt> {
        let Val::Ptr(p) = p else {
            return Err(Halt::Ub(format!("`{what}` through a non-address value")));
        };
        let align = align_trytes(width) as i128;
        if p.offset % align != 0 {
            return Err(Halt::Ub(format!(
                "misaligned `{what}` of t{width}: offset {} is not a multiple of {align} trytes (TIR §4.2)",
                p.offset
            )));
        }
        let size = size_trytes(width) as i128;
        let alloc = &self.allocs[p.alloc];
        if p.offset < 0 || p.offset + size > alloc.trytes.len() as i128 {
            return Err(Halt::Ub(format!(
                "`{what}` of t{width} at offset {} escapes {} ({} trytes) (TIR §4.3)",
                p.offset,
                alloc.name,
                alloc.trytes.len()
            )));
        }
        Ok(*p)
    }

    fn load(&self, p: &Val, width: u32) -> Result<Val, Halt> {
        if matches!(p, Val::Poison) {
            return Err(Halt::Ub("`load` through a poison address".into()));
        }
        let p = self.check_access(p, width, "load")?;
        let alloc = &self.allocs[p.alloc];
        // Little-trytean: the least significant tryte lives at the lowest
        // address (AM §2.2).
        let mut v = Bt::ZERO;
        for i in (0..size_trytes(width)).rev() {
            match &alloc.trytes[(p.offset + i as i128) as usize] {
                None => return Ok(Val::Poison),
                Some(t) => v = v.shl(9).add(t.value()),
            }
        }
        Ok(Val::Int(Tint::wrapping(width, v)))
    }

    /// Load a pointer, with the provenance it was stored with.
    fn load_pointer(&self, p: &Val) -> Result<Val, Halt> {
        if matches!(p, Val::Poison) {
            return Err(Halt::Ub("`load` through a poison address".into()));
        }
        let p = self.check_access(p, 27, "load")?;
        match self.allocs[p.alloc].pointers.get(&p.offset) {
            Some(q) => Ok(Val::Ptr(*q)),
            // Nothing was stored here as a pointer, so whatever is here is
            // not one: reading it as a pointer yields poison rather than a
            // fabricated provenance.
            None => Ok(Val::Poison),
        }
    }

    /// Store a pointer, keeping its provenance.
    fn store_pointer(&mut self, p: &Val, v: &Val) -> Result<(), Halt> {
        if matches!(p, Val::Poison) {
            return Err(Halt::Ub("`store` through a poison address".into()));
        }
        let p = self.check_access(p, 27, "store")?;
        let alloc = &mut self.allocs[p.alloc];
        match v {
            Val::Ptr(q) => {
                alloc.pointers.insert(p.offset, *q);
                // The trytes are initialized too, so a later integer read
                // sees something defined rather than poison.
                for i in 0..3 {
                    alloc.trytes[(p.offset + i) as usize] = Some(Tint::zero(9));
                }
                Ok(())
            }
            Val::Poison => {
                alloc.pointers.remove(&p.offset);
                Ok(())
            }
            Val::Int(_) => Err(Halt::Ub(
                "storing an integer where a pointer is expected".into(),
            )),
        }
    }

    fn store(&mut self, p: &Val, width: u32, v: &Val) -> Result<(), Halt> {
        if matches!(p, Val::Poison) {
            return Err(Halt::Ub("`store` through a poison address".into()));
        }
        let p = self.check_access(p, width, "store")?;
        let size = size_trytes(width);
        let trytes: Vec<Option<Tint>> = match v {
            Val::Poison => vec![None; size as usize],
            Val::Ptr(_) => return Err(Halt::Ub("storing an address as an integer".into())),
            Val::Int(v) => (0..size)
                .map(|i| Some(Tint::wrapping(9, v.value().shr(i * 9))))
                .collect(),
        };
        let alloc = &mut self.allocs[p.alloc];
        for (i, t) in trytes.into_iter().enumerate() {
            alloc.trytes[p.offset as usize + i] = t;
        }
        // Writing bytes over a stored pointer destroys it.
        alloc.pointers.remove(&p.offset);
        Ok(())
    }
}

enum Flow {
    Jump(String, Vec<Val>),
    Return(Option<Val>),
}

fn poison_results(n: usize, _ty: Type) -> Vec<Val> {
    vec![Val::Poison; n]
}

/// A shift amount is a trit count, so it must be a small non-negative number;
/// anything else is out of range and faults `F_SHIFT` (TIR §3.1).
fn shift_amount(k: &Tint) -> Result<u32, Fault> {
    k.to_i128()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Fault::new(FaultCode::Shift))
}
