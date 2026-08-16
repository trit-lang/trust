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
use trit_core::{Bt, Flavor, Trit};

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
        fold_constants(f);
        elide_sign_checks(f);
        elide_dominated_checks(f);
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
/// Compute what is already known (TIR §6).
///
/// An instruction whose operands are all constants has a constant result, and
/// the arithmetic is the machine's — `trit_core` is where the AM's rounding,
/// wrapping and trit operations are defined, so folding here and executing
/// there cannot disagree without one of them being wrong.
///
/// A fold that would **fault** is left alone. `div` by zero and a `.trap`
/// that overflows are things the program does, at the point it does them; a
/// compiler that performed them early would be reporting a fault for code
/// that may never run.
pub fn fold_constants(f: &mut Function) {
    let mut known: BTreeMap<String, (Type, Bt)> = BTreeMap::new();
    for b in &mut f.blocks {
        for inst in &mut b.insts {
            // Operands the earlier folds settled.
            map_operands(&mut inst.kind, &mut |o| {
                if let Operand::Value(v) = o
                    && let Some((t, k)) = known.get(v)
                {
                    *o = Operand::Const(*t, k.clone());
                }
            });
            let Some((ty, value)) = folded(&inst.kind) else {
                continue;
            };
            if let [r] = &inst.results[..] {
                known.insert(r.clone(), (ty, value.clone()));
            }
            inst.kind = InstKind::Plain {
                op: PlainOp::TMax,
                ty,
                a: Operand::Const(ty, value.clone()),
                b: Operand::Const(ty, value),
            };
        }
        map_terminator(&mut b.term, &mut |o| {
            if let Operand::Value(v) = o
                && let Some((t, k)) = known.get(v)
            {
                *o = Operand::Const(*t, k.clone());
            }
        });
    }
    remove_dead(f);
}

/// The constant an instruction computes, if every operand is one and the
/// result cannot fault.
fn folded(k: &InstKind) -> Option<(Type, Bt)> {
    let konst = |o: &Operand| match o {
        Operand::Const(t, v) => Some((*t, v.clone())),
        _ => None,
    };
    match k {
        InstKind::Flavored {
            op,
            flavor,
            ty,
            a,
            b,
        } => {
            let (_, x) = konst(a)?;
            let (_, y) = konst(b)?;
            let width = ty.width()?;
            let exact = match op {
                FlavoredOp::Add => x.to_i128()? + y.to_i128()?,
                FlavoredOp::Sub => x.to_i128()? - y.to_i128()?,
                FlavoredOp::Mul => x.to_i128()?.checked_mul(y.to_i128()?)?,
                // A shift's amount is checked by the machine, so a fold that
                // would fault is not a fold.
                FlavoredOp::Shl => {
                    let n = y.to_i128()?;
                    if !(0..i128::from(width)).contains(&n) {
                        return None;
                    }
                    x.to_i128()?.checked_mul(3i128.checked_pow(n as u32)?)?
                }
            };
            let wrapped = Bt::from_i128(exact).wrap_to(width);
            match flavor {
                // `.wrap` is the only flavor that is a value and nothing
                // else; `.trap` may fault and `.flag` yields two results.
                Flavor::Wrap => Some((*ty, wrapped)),
                Flavor::Trap if Bt::from_i128(exact).fits_width(width) => Some((*ty, wrapped)),
                _ => None,
            }
        }
        InstKind::Cmp { a, b, .. } => {
            let (_, x) = konst(a)?;
            let (_, y) = konst(b)?;
            let (x, y) = (x.to_i128()?, y.to_i128()?);
            let t = match x.cmp(&y) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            Some((Type::Int(1), Bt::from_i128(t)))
        }
        InstKind::Select3 {
            t,
            ty,
            neg,
            zero,
            pos,
        } => {
            let (_, sel) = konst(t)?;
            let arm = match sel.to_i128()? {
                v if v < 0 => neg,
                0 => zero,
                _ => pos,
            };
            let (_, v) = konst(arm)?;
            Some((*ty, v))
        }
        _ => None,
    }
}

/// Drop a check that a value which cannot be negative is not negative
/// (TIR §6).
///
/// Every index pays two comparisons — `0 <= i` and `i < len` — and the first
/// is decidable far more often than the second, because an index is usually
/// a counter that starts at zero and goes up. After `mem2reg` that counter is
/// a **block parameter** whose incoming values are `const 0` and
/// `add.trap %k, 1`, which is the textbook shape:
///
/// ```text
/// br ^loop(const t27 0)
/// ^loop(%k: t27):
///     %c = cmp t27 %k, const t27 0
///     br3 %c, ^fault, ^ok, ^ok
/// ```
///
/// The proof is a **greatest** fixpoint: assume every value is non-negative
/// and refute, which is what lets `%k` depend on itself. `.trap` is what
/// makes the arithmetic sound — a `.wrap` addition can land below zero and is
/// refuted, and so is anything read from memory or returned by a call.
pub fn elide_sign_checks(f: &mut Function) {
    let ok = non_negative(f);
    for b in &mut f.blocks {
        // Only where the other two arms agree: then knowing the first is
        // impossible makes the branch unconditional, and the comparison goes
        // with it. Redirecting one arm of a three-way branch would save
        // nothing.
        let Terminator::Br3 { t, neg, zero, pos } = &b.term else {
            continue;
        };
        if zero != pos {
            continue;
        }
        let Operand::Value(c) = t else { continue };
        let against_zero = b.insts.iter().any(|i| {
            i.results.first().is_some_and(|r| r == c)
                && matches!(&i.kind, InstKind::Cmp { a: Operand::Value(x), b, .. }
                    if ok.contains(x) && is_zero(b))
        });
        if !against_zero {
            continue;
        }
        let _ = neg;
        b.term = Terminator::Br(zero.clone());
    }
    prune_unreachable(f);
    remove_dead(f);
}

/// Drop the blocks nothing can reach.
///
/// Proving a branch always goes one way is what makes a block unreachable,
/// and an unreachable block is one the verifier rejects — so removing it is
/// part of the transformation and not a tidy-up. It also removes what the
/// block used, which may be defined somewhere that no longer dominates it.
pub fn prune_unreachable(f: &mut Function) {
    let index: BTreeMap<&str, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.as_str(), i))
        .collect();
    let mut live = vec![false; f.blocks.len()];
    let mut stack = vec![0usize];
    live[0] = true;
    while let Some(b) = stack.pop() {
        for s in successors(&f.blocks[b].term) {
            if let Some(&j) = index.get(s.as_str())
                && !live[j]
            {
                live[j] = true;
                stack.push(j);
            }
        }
    }
    let mut keep = live.into_iter();
    f.blocks.retain(|_| keep.next().unwrap_or(true));
}

fn is_zero(o: &Operand) -> bool {
    matches!(o, Operand::Const(_, v) if v.to_i128() == Some(0))
}

/// The values that cannot be negative, by refutation from an optimistic
/// start — the only way a counter that is defined in terms of itself can be
/// proved anything at all.
fn non_negative(f: &Function) -> BTreeSet<String> {
    let mut ok: BTreeSet<String> = BTreeSet::new();
    for b in &f.blocks {
        for (n, _) in &b.params {
            ok.insert(n.clone());
        }
        for inst in &b.insts {
            for r in &inst.results {
                ok.insert(r.clone());
            }
        }
    }
    // A parameter arrives from outside and says nothing about itself.
    for (n, _) in &f.sig.params {
        ok.remove(n);
    }

    // What reaches each block parameter, by position.
    let mut incoming: BTreeMap<(String, usize), Vec<Operand>> = BTreeMap::new();
    for b in &f.blocks {
        for t in targets(&b.term) {
            for (i, a) in t.args.iter().enumerate() {
                incoming
                    .entry((t.label.clone(), i))
                    .or_default()
                    .push(a.clone());
            }
        }
    }

    let holds = |o: &Operand, ok: &BTreeSet<String>| match o {
        Operand::Const(_, v) => v.to_i128().is_some_and(|n| n >= 0),
        Operand::Value(v) => ok.contains(v),
        Operand::Global(_) => false,
    };

    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for (i, (n, _)) in b.params.iter().enumerate() {
                if !ok.contains(n) {
                    continue;
                }
                let all = incoming
                    .get(&(b.label.clone(), i))
                    .is_some_and(|args| !args.is_empty() && args.iter().all(|a| holds(a, &ok)));
                if !all {
                    ok.remove(n);
                    changed = true;
                }
            }
            for inst in &b.insts {
                let Some(r) = inst.results.first() else {
                    continue;
                };
                if !ok.contains(r) || inst.results.len() != 1 {
                    if inst.results.len() > 1 {
                        for r in &inst.results {
                            if ok.remove(r) {
                                changed = true;
                            }
                        }
                    }
                    continue;
                }
                // `.trap` is the whole of the argument: a wrapping add can
                // land below zero, and a trapping one faults instead.
                let sound = match &inst.kind {
                    InstKind::Flavored {
                        op: FlavoredOp::Add | FlavoredOp::Mul,
                        flavor: Flavor::Trap,
                        a,
                        b,
                        ..
                    } => holds(a, &ok) && holds(b, &ok),
                    InstKind::Slot { .. } => true,
                    _ => false,
                };
                if !sound {
                    ok.remove(r);
                    changed = true;
                }
            }
        }
    }
    ok
}

/// The targets of a terminator, with their arguments.
fn targets(t: &Terminator) -> Vec<&Target> {
    match t {
        Terminator::Br3 { neg, zero, pos, .. } => vec![neg, zero, pos],
        Terminator::Br(d) => vec![d],
        Terminator::Ret(_) | Terminator::Trap(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// Drop a comparison a branch above it already decided (TIR §6).
///
/// The upper half of an index check is usually the loop's own condition,
/// asked a second time:
///
/// ```text
/// ^while:   %c1 = cmp %k, %len1        ; %len1 = load (offset %v 3)
///           br3 %c1, ^body, ^done, ^done
/// ^body:    …
/// ^idx.lo:  %c2 = cmp %k, %len2        ; %len2 = the same load, again
///           br3 %c2, ^ok, ^fault, ^fault
/// ```
///
/// It does not *look* decided because every operand is a different SSA
/// value, and the obvious fix — number them and make them one value — was
/// measured and made every benchmark **slower**: merging two computations
/// extends a live range across the loop's back edge, and a linear-scan
/// allocator with 27 registers spills to pay for it (G8.18).
///
/// So nothing is rewritten. This asks whether the two comparisons are
/// *equal*, by their defining instructions rather than by their names, and
/// an oracle costs no registers. A `store` or a `call` anywhere on the path
/// gives up, which is what makes comparing two loads sound without any alias
/// analysis at all.
pub fn elide_dominated_checks(f: &mut Function) {
    let def: BTreeMap<String, InstKind> = f
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter_map(|i| match &i.results[..] {
            [r] => Some((r.clone(), i.kind.clone())),
            _ => None,
        })
        .collect();
    let preds = predecessors_by_label(f);
    let index: BTreeMap<String, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.clone(), i))
        .collect();

    let mut rewrite: Vec<(usize, Target)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let Terminator::Br3 { t, neg, zero, pos } = &b.term else {
            continue;
        };
        if zero != pos {
            continue;
        }
        let Operand::Value(c2) = t else { continue };
        let Some(InstKind::Cmp { a: x2, b: y2, .. }) = def.get(c2) else {
            continue;
        };

        // Walk up the chain of sole predecessors, giving up at a write.
        let mut at = bi;
        let mut clean = !writes(&f.blocks[bi]);
        let mut proved = false;
        for _ in 0..64 {
            if !clean {
                break;
            }
            let label = &f.blocks[at].label;
            let Some(ps) = preds.get(label) else { break };
            let [p] = &ps[..] else { break };
            let Some(&pi) = index.get(p) else { break };
            let d = &f.blocks[pi];
            if let Terminator::Br3 {
                t: dc,
                neg: dn,
                zero: dz,
                pos: dp,
            } = &d.term
                && dn.label == *label
                && dz.label != *label
                && dp.label != *label
                && let Operand::Value(c1) = dc
                && let Some(InstKind::Cmp { a: x1, b: y1, .. }) = def.get(c1)
                && equiv(x1, x2, &def, 0)
                && equiv(y1, y2, &def, 0)
            {
                proved = true;
                break;
            }
            clean &= !writes(d);
            at = pi;
        }
        if proved {
            rewrite.push((bi, neg.clone()));
        }
    }
    for (bi, to) in rewrite {
        f.blocks[bi].term = Terminator::Br(to);
    }
    prune_unreachable(f);
    remove_dead(f);
}

/// Whether a block writes anything a later read could see.
fn writes(b: &Block) -> bool {
    b.insts
        .iter()
        .any(|i| matches!(i.kind, InstKind::Store { .. } | InstKind::Call { .. }))
}

fn predecessors_by_label(f: &Function) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in &f.blocks {
        for s in successors(&b.term) {
            let e = out.entry(s).or_default();
            if !e.contains(&b.label) {
                e.push(b.label.clone());
            }
        }
    }
    out
}

/// Whether two operands must hold the same value — by what defines them, not
/// by what they are called.
fn equiv(a: &Operand, b: &Operand, def: &BTreeMap<String, InstKind>, depth: u32) -> bool {
    if a == b {
        return true;
    }
    if depth > 4 {
        return false;
    }
    let (Operand::Value(x), Operand::Value(y)) = (a, b) else {
        return false;
    };
    let (Some(dx), Some(dy)) = (def.get(x), def.get(y)) else {
        return false;
    };
    match (dx, dy) {
        (InstKind::Load { ty: t1, p: p1 }, InstKind::Load { ty: t2, p: p2 }) => {
            t1 == t2 && equiv(p1, p2, def, depth + 1)
        }
        (InstKind::Offset { p: p1, d: d1 }, InstKind::Offset { p: p2, d: d2 }) => {
            equiv(p1, p2, def, depth + 1) && equiv(d1, d2, def, depth + 1)
        }
        (
            InstKind::Flavored {
                op: o1,
                flavor: f1,
                ty: t1,
                a: a1,
                b: b1,
            },
            InstKind::Flavored {
                op: o2,
                flavor: f2,
                ty: t2,
                a: a2,
                b: b2,
            },
        ) => {
            o1 == o2
                && f1 == f2
                && t1 == t2
                && equiv(a1, a2, def, depth + 1)
                && equiv(b1, b2, def, depth + 1)
        }
        _ => false,
    }
}

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
