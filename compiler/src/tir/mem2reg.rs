//! Slots that never escape become SSA values, across the whole function.
//!
//! `canon::promote_slots` does this for a slot whose accesses are all in one
//! block, where the value at each load is unambiguously the last thing stored
//! above it. This does it everywhere, which means answering the same question
//! at a block's *entry*: what was stored into this slot along the path that
//! got here? When the predecessors disagree the answer is a new block
//! parameter, and that disagreement is what block parameters are for.
//!
//! # The algorithm
//!
//! Braun et al.'s on-demand SSA construction, which never computes a
//! dominance frontier. Three questions, memoized:
//!
//! - **What does block *b* store into this slot?** Purely local: the last
//!   `store` through it in *b*, or nothing.
//! - **What holds at the end of *b*?** What *b* stored, or what held at its
//!   entry.
//! - **What holds at the entry of *b*?** With no predecessors, the frame's
//!   own zero. With one, whatever held at its end. With several — or on
//!   re-entry, which is a loop — a **new block parameter**, recorded *before*
//!   the recursion so that a back edge finds it instead of running forever.
//!
//! A parameter whose arguments all turn out to be one operand (or the
//! parameter itself) says nothing: it is removed and its uses replaced, which
//! may make another trivial. That is what stops a loop from collecting a
//! parameter for every variable it does not touch.
//!
//! # What it will not touch
//!
//! The escape test `canon::promote_slots` uses, minus the single-block
//! condition: every use of the slot is the pointer of a `load` or `store`,
//! and every access is at one type. An address that reaches a call, an
//! `offset`, or memory could be read through later, and TIR §5's provenance
//! rules are what make "nothing else names it" the whole question.
//!
//! And a slot is promoted only when **every load has a store reaching it on
//! every path** (`definitely_assigned`). Reading uninitialized `slot` storage
//! is UB and yields poison (TIR §4 item 4); UB permits any answer, but a pass
//! that quietly chose one would be deciding what poison is, and that decision
//! belongs in the specification rather than in an optimizer. The single-block
//! pass declined it for the same reason.

use crate::tir::canon::{
    map_operands, map_terminator, successors, visit_operands, visit_terminator,
};
use crate::tir::ir::*;
use std::collections::{BTreeMap, BTreeSet};
use trit_core::Bt;

/// Promote every slot in the function that never escapes.
pub fn promote(f: &mut Function) {
    let mut slots = promotable(f);
    let assigned = definitely_assigned(f, &slots);
    slots.retain(|n, _| assigned.contains(n));
    if slots.is_empty() {
        return;
    }
    Builder::new(f, slots).run(f);
}

/// The slots every load of which has a store reaching it along every path.
///
/// A forward fixpoint over "definitely stored on entry": the intersection of
/// what the predecessors leave, plus what the block itself stores. A load
/// that arrives before its slot is in that set disqualifies the slot — not
/// because promoting it would be wrong (reading uninitialized storage is UB,
/// so any answer conforms) but because the answer would be this pass's
/// invention rather than the program's.
fn definitely_assigned(f: &Function, slots: &BTreeMap<String, Type>) -> BTreeSet<String> {
    let n = f.blocks.len();
    let index: BTreeMap<&str, usize> = f
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.as_str(), i))
        .collect();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, b) in f.blocks.iter().enumerate() {
        for label in successors(&b.term) {
            if let Some(&j) = index.get(label.as_str()) {
                preds[j].push(i);
            }
        }
    }
    let all: BTreeSet<String> = slots.keys().cloned().collect();

    // Start optimistic everywhere but the entry, which nothing precedes, and
    // shrink to a fixpoint.
    let mut on_entry: Vec<BTreeSet<String>> = vec![all.clone(); n];
    if n > 0 {
        on_entry[0] = BTreeSet::new();
    }
    loop {
        let mut changed = false;
        for bi in 0..n {
            if preds[bi].is_empty() {
                continue;
            }
            let mut into = all.clone();
            for &p in &preds[bi] {
                let mut out = on_entry[p].clone();
                for inst in &f.blocks[p].insts {
                    if let InstKind::Store {
                        p: Operand::Value(s),
                        ..
                    } = &inst.kind
                        && slots.contains_key(s)
                    {
                        out.insert(s.clone());
                    }
                }
                into = into.intersection(&out).cloned().collect();
            }
            if into != on_entry[bi] {
                on_entry[bi] = into;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut ok = all;
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut have = on_entry[bi].clone();
        for inst in &b.insts {
            match &inst.kind {
                InstKind::Load {
                    p: Operand::Value(s),
                    ..
                } if slots.contains_key(s) && !have.contains(s) => {
                    ok.remove(s);
                }
                InstKind::Store {
                    p: Operand::Value(s),
                    ..
                } if slots.contains_key(s) => {
                    have.insert(s.clone());
                }
                _ => {}
            }
        }
    }
    ok
}

/// The slots this pass may promote, with the type they are accessed at.
fn promotable(f: &Function) -> BTreeMap<String, Type> {
    let mut candidates: BTreeMap<String, Option<Type>> = BTreeMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if matches!(inst.kind, InstKind::Slot { .. })
                && let Some(r) = inst.results.first()
            {
                candidates.insert(r.clone(), None);
            }
        }
    }
    if candidates.is_empty() {
        return BTreeMap::new();
    }

    let mut stored: BTreeSet<String> = BTreeSet::new();
    for b in &f.blocks {
        for inst in &b.insts {
            match &inst.kind {
                InstKind::Load { p, ty } => access(&mut candidates, p, *ty, &mut stored, false),
                InstKind::Store { p, ty, v } => {
                    // Storing the address *is* letting it out.
                    if let Operand::Value(n) = v {
                        candidates.remove(n);
                    }
                    access(&mut candidates, p, *ty, &mut stored, true);
                }
                other => visit_operands(other, &mut |o| {
                    if let Operand::Value(n) = o {
                        candidates.remove(n);
                    }
                }),
            }
        }
        visit_terminator(&b.term, &mut |o| {
            if let Operand::Value(n) = o {
                candidates.remove(n);
            }
        });
    }

    candidates
        .into_iter()
        .filter(|(n, _)| stored.contains(n))
        .filter_map(|(n, t)| t.map(|t| (n, t)))
        .collect()
}

/// Record an access through `p`, or disqualify whatever it names.
fn access(
    candidates: &mut BTreeMap<String, Option<Type>>,
    p: &Operand,
    ty: Type,
    stored: &mut BTreeSet<String>,
    is_store: bool,
) {
    let Operand::Value(n) = p else {
        return;
    };
    let Some(seen) = candidates.get_mut(n) else {
        return;
    };
    match seen {
        // Two access types means the storage is being reinterpreted, and
        // "the value in the slot" is then not one value.
        Some(t) if *t != ty => {
            candidates.remove(n);
            return;
        }
        Some(_) => {}
        None => *seen = Some(ty),
    }
    if is_store {
        stored.insert(n.clone());
    }
}

struct Builder {
    slots: BTreeMap<String, Type>,
    index: BTreeMap<String, usize>,
    preds: Vec<Vec<usize>>,
    /// What each block stores into each slot, last store winning.
    local: Vec<BTreeMap<String, Operand>>,
    /// Memoized answers, and the guard against recursing round a loop.
    entry: Vec<BTreeMap<String, Operand>>,
    asking: Vec<BTreeSet<String>>,
    /// Parameters this pass added: name → (block, slot).
    added: BTreeMap<String, (usize, String)>,
    /// Added parameters per block, in the order edges must supply them.
    params: Vec<Vec<(String, Type)>>,
    subst: BTreeMap<String, Operand>,
    counter: u32,
}

impl Builder {
    fn new(f: &Function, slots: BTreeMap<String, Type>) -> Builder {
        let n = f.blocks.len();
        let index: BTreeMap<String, usize> = f
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();
        let mut preds = vec![Vec::new(); n];
        for (i, b) in f.blocks.iter().enumerate() {
            for label in successors(&b.term) {
                if let Some(&j) = index.get(&label) {
                    preds[j].push(i);
                }
            }
        }
        let mut local = vec![BTreeMap::new(); n];
        for (i, b) in f.blocks.iter().enumerate() {
            for inst in &b.insts {
                if let InstKind::Store {
                    p: Operand::Value(p),
                    v,
                    ..
                } = &inst.kind
                    && slots.contains_key(p)
                {
                    local[i].insert(p.clone(), v.clone());
                }
            }
        }
        Builder {
            slots,
            index,
            preds,
            local,
            entry: vec![BTreeMap::new(); n],
            asking: vec![BTreeSet::new(); n],
            added: BTreeMap::new(),
            params: vec![Vec::new(); n],
            subst: BTreeMap::new(),
            counter: 0,
        }
    }

    /// What holds at the end of block `bi`.
    fn at_end(&mut self, bi: usize, slot: &str) -> Operand {
        match self.local[bi].get(slot) {
            Some(v) => v.clone(),
            None => self.at_entry(bi, slot),
        }
    }

    /// What holds at the entry of block `bi`.
    fn at_entry(&mut self, bi: usize, slot: &str) -> Operand {
        if let Some(v) = self.entry[bi].get(slot) {
            return v.clone();
        }
        // Re-entering the question is a loop, and the answer is a parameter
        // for the same reason a disagreement is.
        let looping = !self.asking[bi].insert(slot.to_string());
        let value = match (looping, self.preds[bi].as_slice()) {
            // Nothing reaches here: untouched frame storage, which is zero.
            (_, []) => Operand::Const(self.slots[slot], Bt::from_i128(0)),
            (false, [only]) => {
                let only = *only;
                self.at_end(only, slot)
            }
            _ => self.new_param(bi, slot),
        };
        self.asking[bi].remove(slot);
        self.entry[bi]
            .entry(slot.to_string())
            .or_insert(value)
            .clone()
    }

    fn new_param(&mut self, bi: usize, slot: &str) -> Operand {
        self.counter += 1;
        let name = format!("m2r.{}.{}", slot.replace('.', "_"), self.counter);
        self.params[bi].push((name.clone(), self.slots[slot]));
        self.added.insert(name.clone(), (bi, slot.to_string()));
        let param = Operand::Value(name.clone());
        // Recorded before the recursion that fills its edges, so a back edge
        // asking the same question finds it rather than asking again.
        self.entry[bi].insert(slot.to_string(), param.clone());
        param
    }

    fn run(mut self, f: &mut Function) {
        // Answer every load: what a store above it in the block left, or
        // what held at the block's entry.
        for bi in 0..f.blocks.len() {
            let mut above: BTreeMap<String, Operand> = BTreeMap::new();
            for inst in f.blocks[bi].insts.clone() {
                match &inst.kind {
                    InstKind::Store {
                        p: Operand::Value(p),
                        v,
                        ..
                    } if self.slots.contains_key(p) => {
                        above.insert(p.clone(), v.clone());
                    }
                    InstKind::Load {
                        p: Operand::Value(p),
                        ..
                    } if self.slots.contains_key(p) => {
                        let v = match above.get(p) {
                            Some(v) => v.clone(),
                            None => self.at_entry(bi, p),
                        };
                        if let Some(r) = inst.results.first() {
                            self.subst.insert(r.clone(), v);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Every added parameter needs an argument on each incoming edge, and
        // finding one may add further parameters.
        let mut args: BTreeMap<String, BTreeMap<usize, Operand>> = BTreeMap::new();
        let mut done: BTreeSet<String> = BTreeSet::new();
        loop {
            let pending: Vec<String> = self
                .added
                .keys()
                .filter(|n| !done.contains(*n))
                .cloned()
                .collect();
            if pending.is_empty() {
                break;
            }
            for name in pending {
                done.insert(name.clone());
                let (bi, slot) = self.added[&name].clone();
                for pi in self.preds[bi].clone() {
                    let v = self.at_end(pi, &slot);
                    args.entry(name.clone()).or_default().insert(pi, v);
                }
            }
        }

        self.remove_trivial(&mut args);
        self.install(f, &args);
    }

    /// A parameter whose arguments are all one operand says nothing.
    fn remove_trivial(&mut self, args: &mut BTreeMap<String, BTreeMap<usize, Operand>>) {
        loop {
            let mut found = None;
            for (name, by_pred) in args.iter() {
                let mut only: Option<Operand> = None;
                let mut same = true;
                for v in by_pred.values() {
                    let v = self.resolve(v);
                    if matches!(&v, Operand::Value(n) if n == name) {
                        continue; // the parameter itself says nothing new
                    }
                    match &only {
                        None => only = Some(v),
                        Some(o) if *o == v => {}
                        Some(_) => {
                            same = false;
                            break;
                        }
                    }
                }
                if same && let Some(o) = only {
                    found = Some((name.clone(), o));
                    break;
                }
            }
            let Some((name, with)) = found else { return };
            self.subst.insert(name.clone(), with);
            args.remove(&name);
            let (bi, _) = self.added.remove(&name).expect("added by this pass");
            self.params[bi].retain(|(n, _)| *n != name);
        }
    }

    /// Install the parameters, fill the edges, and drop what is now unread.
    fn install(&self, f: &mut Function, args: &BTreeMap<String, BTreeMap<usize, Operand>>) {
        for (bi, ps) in self.params.iter().enumerate() {
            f.blocks[bi].params.extend(ps.iter().cloned());
        }

        // A block's added parameters come after whatever it already had, in
        // the same order, so the arguments append in that order too.
        for pi in 0..f.blocks.len() {
            let mut extra: Vec<(String, Vec<Operand>)> = Vec::new();
            for label in successors(&f.blocks[pi].term) {
                let Some(&bj) = self.index.get(&label) else {
                    continue;
                };
                let vals = self.params[bj]
                    .iter()
                    .map(|(n, _)| self.resolve(&args[n][&pi]))
                    .collect();
                extra.push((label, vals));
            }
            for_each_target(&mut f.blocks[pi].term, &mut |t| {
                if let Some((_, vals)) = extra.iter().find(|(l, _)| *l == t.label) {
                    t.args.extend(vals.iter().cloned());
                }
            });
        }

        for b in f.blocks.iter_mut() {
            b.insts.retain(|inst| match &inst.kind {
                InstKind::Slot { .. } => !inst
                    .results
                    .first()
                    .is_some_and(|r| self.slots.contains_key(r)),
                InstKind::Load {
                    p: Operand::Value(n),
                    ..
                }
                | InstKind::Store {
                    p: Operand::Value(n),
                    ..
                } => !self.slots.contains_key(n),
                _ => true,
            });
            for inst in &mut b.insts {
                map_operands(&mut inst.kind, &mut |o| *o = self.resolve(o));
            }
            map_terminator(&mut b.term, &mut |o| *o = self.resolve(o));
        }
    }

    /// Follow renamings to the end.
    fn resolve(&self, o: &Operand) -> Operand {
        let mut o = o.clone();
        // A renaming chain is acyclic — each step names something defined
        // earlier — but a bound costs nothing and a hang costs a lot.
        for _ in 0..1024 {
            let Operand::Value(n) = &o else { return o };
            match self.subst.get(n) {
                Some(next) if *next != o => o = next.clone(),
                _ => return o,
            }
        }
        o
    }
}

/// Apply a function to every branch target of a terminator.
fn for_each_target(t: &mut Terminator, f: &mut impl FnMut(&mut Target)) {
    match t {
        Terminator::Br3 { neg, zero, pos, .. } => {
            f(neg);
            f(zero);
            f(pos);
        }
        Terminator::Br(d) => f(d),
        Terminator::Ret(_) | Terminator::Trap(_) | Terminator::Unreachable => {}
    }
}
