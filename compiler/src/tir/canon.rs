//! Canonicalization — the target-independent optimization TIR §6 names.
//!
//! TIR §6 places legalization "between target-independent optimization and
//! instruction selection". Legalization and instruction selection have both
//! existed for a while; this is the stage between them that did not.
//!
//! A canonicalizer's contract is narrow and the reason to want it is written
//! down in two other places. `lang/lower.rs` gives **every** local a `slot`,
//! so a read is a `load` and a write is a `store`, because TIR is SSA and a
//! mutable local is not — and that file says the cost "is paid back by the
//! optimizer that does not exist yet". `codegen.rs` cannot pay it back,
//! because by then the memory traffic is real instructions and the
//! information that it was a local is gone.
//!
//! # What this pass does
//!
//! `promote_slots`: a `slot` whose every use is a `load` or `store` **through
//! it**, all within one block, becomes an SSA value. Stores are deleted and
//! each load is replaced by whatever was last stored. `mem2reg::promote` then
//! does the same across blocks, where the answer at a block's entry may
//! depend on which path arrived and a block parameter is what says so.
//!
//! `branch_through_select`: a `br3` on a `select3` of constants is a `br3` on
//! the select's own selector, with the arms permuted. This is the shape the
//! frontend emits for every comparison in a condition — `i < n` is a `cmp`
//! and then a `select3` turning its trit into a `bool` — and the branch then
//! asks for the sign of the bool, which is the trit again.
//!
//! `remove_dead`: an instruction whose results nothing reads, and which
//! cannot fault or write, is not emitted. Mostly this is what the
//! transformation above leaves behind, and it is what makes that one worth
//! anything.
//!
//! # Why one block
//!
//! Promoting a slot read in one block and written in another is SSA
//! construction: it needs a value to arrive along each edge, which is what
//! block parameters are for and what placing them correctly is hard about.
//! Restricting to a single block makes the value at each load unambiguous —
//! it is the last thing stored above it — and no edge carries anything new.
//!
//! That restriction is not as narrow as it sounds, because the shape it
//! catches is the one the frontend emits most: a parameter, which arrives as
//! an SSA value already, is stored into its slot and read back from it.
//!
//! # What makes it safe
//!
//! A slot is promoted only when nothing can observe it as memory:
//!
//! - Every use is the pointer operand of a `load` or `store`. A slot passed
//!   to a call, offset into, or stored *as a value* escapes, and escaping is
//!   the whole question — TIR §5's provenance rules mean an address that got
//!   out may be read through later.
//! - Every access is in the block that defines the slot.
//! - Every access has the same type, so "the value in the slot" is one value
//!   and not a reinterpretation of some trytes.
//! - The first access is a store. Reading uninitialized `slot` storage is UB
//!   (TIR §4 item 4) and yields poison; a pass is not the place to decide
//!   what poison is.
//!
//! Note what is *not* required: that the slot be exactly the width of the
//! access. A `slot tryte[9]` accessed only as one `t27` at offset zero is
//! still only ever that one value, because reaching the rest of it needs an
//! `offset`, and an `offset` is a use that disqualifies the slot.

use crate::tir::ir::*;
use std::collections::{BTreeMap, BTreeSet};
use trit_core::{Flavor, Trit};

/// Canonicalize a module: the same program, in a form later passes handle
/// better.
///
/// This is a TIR → TIR transformation and preserves observable AM behavior,
/// which is TIR's own correctness criterion. The interpreter is the oracle
/// that says so — see `compiler/tests/canon.rs`.
pub fn canonicalize_module(m: &Module) -> Module {
    let mut out = m.clone();
    for f in &mut out.funcs {
        promote_slots(f);
        crate::tir::mem2reg::promote(f);
        branch_through_select(f);
        remove_dead(f);
    }
    out
}

/// Branch on a comparison rather than on what a `select3` made of it.
///
/// The frontend has no `bool`-producing comparison: `i < n` is
///
/// ```text
/// %c = cmp t27 %i, %n
/// %b = select3 %c, t1 const t1 1, const t1 0, const t1 0
/// br2 %b, ^body, ^done
/// ```
///
/// and `br2` is a `br3` on the *sign* of `%b` — which is the sign of whichever
/// constant `%c` chose. So the branch can read `%c` directly, with each arm
/// sent where the constant it would have produced points. The `select3` is
/// then usually dead, and with it the constants it named.
///
/// On the machine that is seven instructions where two will do: the three
/// constants have to be materialized into registers, since `sel3` is the one
/// instruction with four register sources and no immediate form, and
/// legalization then adds a `cmp` against zero to project the widened `bool`
/// back down to a condition trit. Both disappear.
pub fn branch_through_select(f: &mut Function) {
    // Every `select3` whose three arms are constants, by result name.
    let mut selects: BTreeMap<String, (Operand, [Trit; 3])> = BTreeMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            let InstKind::Select3 {
                t, neg, zero, pos, ..
            } = &inst.kind
            else {
                continue;
            };
            let (Some(r), Operand::Const(_, n), Operand::Const(_, z), Operand::Const(_, p)) =
                (inst.results.first(), neg, zero, pos)
            else {
                continue;
            };
            selects.insert(r.clone(), (t.clone(), [n.sign(), z.sign(), p.sign()]));
        }
    }
    if selects.is_empty() {
        return;
    }

    for b in &mut f.blocks {
        let Terminator::Br3 { t, neg, zero, pos } = &b.term else {
            continue;
        };
        let Operand::Value(name) = t else { continue };
        let Some((selector, signs)) = selects.get(name) else {
            continue;
        };
        // `%c` is an operand of the `select3`, which dominates this branch,
        // so `%c` dominates it too: reading it here is always in scope.
        let arms = [neg.clone(), zero.clone(), pos.clone()];
        let pick = |t: Trit| arms[(t.to_i8() + 1) as usize].clone();
        b.term = Terminator::Br3 {
            t: selector.clone(),
            neg: pick(signs[0]),
            zero: pick(signs[1]),
            pos: pick(signs[2]),
        };
    }
}

/// Delete instructions nothing reads.
///
/// Only those that can neither fault nor write: a `store`, a `call`, and
/// anything that can raise — a trapping flavor, a division, a shift — stays
/// whether its result is wanted or not. `load` stays too, because this pass
/// has no reason to reason about memory.
///
/// Iterated, because deleting one instruction is what makes the next one
/// dead.
pub fn remove_dead(f: &mut Function) {
    loop {
        let mut live: BTreeSet<String> = BTreeSet::new();
        for b in &f.blocks {
            for inst in &b.insts {
                visit_operands(&inst.kind, &mut |o| {
                    if let Operand::Value(n) = o {
                        live.insert(n.clone());
                    }
                });
            }
            visit_terminator(&b.term, &mut |o| {
                if let Operand::Value(n) = o {
                    live.insert(n.clone());
                }
            });
        }
        let mut removed = false;
        for b in &mut f.blocks {
            b.insts.retain(|inst| {
                let wanted =
                    inst.results.is_empty() || inst.results.iter().any(|r| live.contains(r));
                let keep = wanted || !is_removable(&inst.kind);
                removed |= !keep;
                keep
            });
        }
        if !removed {
            return;
        }
    }
}

/// Whether an instruction's only effect is its results.
fn is_removable(k: &InstKind) -> bool {
    match k {
        // `shl` faults on a shift amount outside 0…26 whatever its flavor
        // (TRISC-27 §4.1), and a trapping flavor faults on overflow.
        InstKind::Flavored { op, flavor, .. } => *op != FlavoredOp::Shl && *flavor != Flavor::Trap,
        // `div` and `rem` fault on a zero divisor; `shr` on the same amounts
        // `shl` does.
        InstKind::Plain { op, .. } => matches!(
            op,
            PlainOp::MulH | PlainOp::TMin | PlainOp::TMax | PlainOp::TMul
        ),
        InstKind::Cmp { .. }
        | InstKind::Neg { .. }
        | InstKind::Widen { .. }
        | InstKind::Trunc { .. }
        | InstKind::Select3 { .. }
        | InstKind::Slot { .. }
        | InstKind::Offset { .. } => true,
        InstKind::Load { .. } | InstKind::Store { .. } | InstKind::Call { .. } => false,
    }
}

/// Replace single-block, never-escaping slots with the values they hold.
pub fn promote_slots(f: &mut Function) {
    for i in 0..f.blocks.len() {
        let promotable = promotable_in(f, i);
        if promotable.is_empty() {
            continue;
        }
        let subst = rewrite_block(&mut f.blocks[i], &promotable);

        // A deleted load's *result* may be named in another block even though
        // the slot itself never leaves this one, so the renaming is
        // function-wide. It is sound wherever it applies: a block naming that
        // result is dominated by this one, and the value put into the slot is
        // defined at or above the store, so it dominates everything the
        // result did.
        if subst.is_empty() {
            continue;
        }
        for (j, b) in f.blocks.iter_mut().enumerate() {
            if j == i {
                continue;
            }
            for inst in &mut b.insts {
                map_operands(&mut inst.kind, &mut |o| {
                    if let Operand::Value(n) = o
                        && let Some(v) = subst.get(n)
                    {
                        *o = v.clone();
                    }
                });
            }
            map_terminator(&mut b.term, &mut |o| {
                if let Operand::Value(n) = o
                    && let Some(v) = subst.get(n)
                {
                    *o = v.clone();
                }
            });
        }
    }
}

/// The slots block `i` defines that this pass may promote.
fn promotable_in(f: &Function, i: usize) -> BTreeSet<String> {
    let block = &f.blocks[i];

    // Candidates: slots this block defines.
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for inst in &block.insts {
        if matches!(inst.kind, InstKind::Slot { .. })
            && let Some(r) = inst.results.first()
        {
            candidates.insert(r.clone());
        }
    }
    if candidates.is_empty() {
        return candidates;
    }

    // Any use that is not an access through the slot, or is not in this
    // block, disqualifies it.
    for (j, b) in f.blocks.iter().enumerate() {
        let here = j == i;
        for inst in &b.insts {
            // Only the *pointer* position of an access reads the slot as a
            // place. `store %s, %s` puts the address itself in memory, and
            // that is an escape however it is spelled.
            let mut disqualify = |o: &Operand| {
                if let Operand::Value(n) = o {
                    candidates.remove(n);
                }
            };
            match (&inst.kind, here) {
                (InstKind::Load { p, .. }, true) => {
                    if !matches!(p, Operand::Value(_)) {
                        visit_operands(&inst.kind, &mut disqualify);
                    }
                }
                (InstKind::Store { v, p, .. }, true) => {
                    disqualify(v);
                    if !matches!(p, Operand::Value(_)) {
                        disqualify(p);
                    }
                }
                _ => visit_operands(&inst.kind, &mut disqualify),
            }
        }
        visit_terminator(&b.term, &mut |o| {
            if let Operand::Value(n) = o {
                candidates.remove(n);
            }
        });
    }

    // What is left is accessed only through loads and stores in this block.
    // Two more conditions are about the accesses themselves.
    let mut ty_of: BTreeMap<String, Type> = BTreeMap::new();
    let mut first_is_store: BTreeMap<String, bool> = BTreeMap::new();
    for inst in &block.insts {
        let (p, ty, is_store) = match &inst.kind {
            InstKind::Load { p, ty } => (p, *ty, false),
            InstKind::Store { p, ty, .. } => (p, *ty, true),
            _ => continue,
        };
        let Operand::Value(n) = p else { continue };
        if !candidates.contains(n) {
            continue;
        }
        first_is_store.entry(n.clone()).or_insert(is_store);
        match ty_of.get(n) {
            Some(seen) if *seen != ty => {
                candidates.remove(n);
            }
            Some(_) => {}
            None => {
                ty_of.insert(n.clone(), ty);
            }
        }
    }

    candidates.retain(|n| first_is_store.get(n).copied().unwrap_or(false));
    candidates
}

/// Delete the promoted slots' allocations and accesses, forwarding each load
/// to the value last stored.
///
/// Returns what each deleted load's result now stands for, which the caller
/// applies to the rest of the function.
fn rewrite_block(block: &mut Block, promoted: &BTreeSet<String>) -> BTreeMap<String, Operand> {
    // What each promoted slot currently holds, and what each deleted load's
    // result should be read as.
    let mut held: BTreeMap<String, Operand> = BTreeMap::new();
    let mut subst: BTreeMap<String, Operand> = BTreeMap::new();

    let resolve = |o: &Operand, subst: &BTreeMap<String, Operand>| -> Operand {
        match o {
            Operand::Value(n) => subst.get(n).cloned().unwrap_or_else(|| o.clone()),
            _ => o.clone(),
        }
    };

    let mut kept: Vec<Inst> = Vec::with_capacity(block.insts.len());
    for inst in block.insts.drain(..) {
        // Every operand is rewritten first: a value forwarded out of a
        // deleted load may be named anywhere below it.
        let mut inst = inst;
        map_operands(&mut inst.kind, &mut |o| *o = resolve(o, &subst));

        match &inst.kind {
            InstKind::Slot { .. } if inst.results.first().is_some_and(|r| promoted.contains(r)) => {
                continue;
            }
            InstKind::Store { v, p, .. } => {
                if let Operand::Value(n) = p
                    && promoted.contains(n)
                {
                    held.insert(n.clone(), v.clone());
                    continue;
                }
            }
            InstKind::Load { p, .. } => {
                if let Operand::Value(n) = p
                    && promoted.contains(n)
                {
                    // The first access to a promoted slot is a store, so
                    // there is always something here.
                    let v = held
                        .get(n)
                        .expect("a promoted slot is stored before it is read");
                    if let Some(r) = inst.results.first() {
                        subst.insert(r.clone(), v.clone());
                    }
                    continue;
                }
            }
            _ => {}
        }
        kept.push(inst);
    }
    block.insts = kept;
    map_terminator(&mut block.term, &mut |o| *o = resolve(o, &subst));
    subst
}

// ------------------------------------------------------- operand traversal

/// Visit every operand an instruction reads.
pub(crate) fn visit_operands(k: &InstKind, f: &mut impl FnMut(&Operand)) {
    let mut k = k.clone();
    map_operands(&mut k, &mut |o| f(o));
}

/// Visit every operand a terminator reads.
pub(crate) fn visit_terminator(t: &Terminator, f: &mut impl FnMut(&Operand)) {
    let mut t = t.clone();
    map_terminator(&mut t, &mut |o| f(o));
}

/// Apply a function to every operand an instruction reads, in place.
pub(crate) fn map_operands(k: &mut InstKind, f: &mut impl FnMut(&mut Operand)) {
    match k {
        InstKind::Flavored { a, b, .. }
        | InstKind::Plain { a, b, .. }
        | InstKind::Cmp { a, b, .. } => {
            f(a);
            f(b);
        }
        InstKind::Neg { a, .. } | InstKind::Widen { a, .. } | InstKind::Trunc { a, .. } => f(a),
        InstKind::Select3 {
            t, neg, zero, pos, ..
        } => {
            f(t);
            f(neg);
            f(zero);
            f(pos);
        }
        InstKind::Slot { .. } => {}
        InstKind::Load { p, .. } => f(p),
        InstKind::Store { v, p, .. } => {
            f(v);
            f(p);
        }
        InstKind::Offset { p, d } => {
            f(p);
            f(d);
        }
        InstKind::Call { callee, args, .. } => {
            if let Callee::Indirect(p) = callee {
                f(p);
            }
            args.iter_mut().for_each(f);
        }
    }
}

/// Apply a function to every operand a terminator reads, in place.
pub(crate) fn map_terminator(t: &mut Terminator, f: &mut impl FnMut(&mut Operand)) {
    match t {
        Terminator::Br3 { t, neg, zero, pos } => {
            f(t);
            for d in [neg, zero, pos] {
                d.args.iter_mut().for_each(&mut *f);
            }
        }
        Terminator::Br(d) => d.args.iter_mut().for_each(f),
        Terminator::Ret(Some(v)) => f(v),
        Terminator::Ret(None) | Terminator::Trap(_) | Terminator::Unreachable => {}
    }
}

/// The blocks a terminator may transfer to.
pub(crate) fn successors(t: &Terminator) -> Vec<String> {
    match t {
        Terminator::Br3 { neg, zero, pos, .. } => {
            vec![neg.label.clone(), zero.label.clone(), pos.label.clone()]
        }
        Terminator::Br(d) => vec![d.label.clone()],
        Terminator::Ret(_) | Terminator::Trap(_) | Terminator::Unreachable => Vec::new(),
    }
}
