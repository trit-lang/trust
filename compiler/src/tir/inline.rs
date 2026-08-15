//! Inlining — splice a small callee into its caller (TIR §6).
//!
//! Why this pass exists is a measurement rather than a principle. Ch. 5
//! §1.5's `print` is `print_char`'s body written out instead of called,
//! because calling it per character cost 1.3% of HPL's instructions; and
//! `print_char(' ')` costs 1.0% against `putchar(32)` for the same reason,
//! a call where a constant would fold. Both are the library paying for the
//! absence of this pass in the shape of its own source.
//!
//! What it does is the textbook thing: a call to a small, non-recursive
//! function becomes the callee's blocks, renamed, with its parameters bound
//! to the call's arguments and its `ret` turned into a branch to whatever
//! followed the call.
//!
//! What it does *not* do is decide anything about the language. Every
//! program means what it meant; this only changes how many instructions
//! saying so takes.

use std::collections::{HashMap, HashSet};

use super::canon::{map_operands, map_terminator, successors};
use super::ir::{Block, Callee, Function, Inst, InstKind, Module, Operand, Target, Terminator};

/// How many instructions a callee may have and still be worth splicing.
///
/// Chosen by measurement rather than by argument: 24 takes `print_char` and
/// the one-line accessors and leaves everything else alone. A larger number
/// trades image size for call overhead, and this machine has no cache for
/// the trade to be interesting.
const BUDGET: usize = 24;

/// How many times to sweep, so that a call exposed by inlining is itself
/// considered. Two is enough for the library's shapes and bounds the growth.
const ROUNDS: usize = 2;

/// Inline what is worth inlining, everywhere in the module.
pub fn inline_module(m: &Module) -> Module {
    let mut out = m.clone();
    let mut tag = 0usize;
    for _ in 0..ROUNDS {
        let small: HashMap<String, Function> = out
            .funcs
            .iter()
            .filter(|f| worth_inlining(f))
            .map(|f| (f.sig.name.clone(), f.clone()))
            .collect();
        if small.is_empty() {
            return out;
        }
        let mut changed = false;
        for f in &mut out.funcs {
            // The tag has to be unique across rounds as well as within one,
            // or a second splice renames a value to a name the first made.
            // Not into itself, however small: a recursive function inlined
            // into itself is the same function one call deeper.
            let mut here = small.clone();
            here.remove(&f.sig.name);
            changed |= inline_into(f, &here, &mut tag);
        }
        if !changed {
            return out;
        }
    }
    out
}

/// Whether a function is small enough, and simple enough, to splice.
fn worth_inlining(f: &Function) -> bool {
    let insts: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
    if insts > BUDGET {
        return false;
    }
    // A function that calls itself would be spliced into itself forever. The
    // check is direct recursion only; a cycle through two functions is
    // stopped by `ROUNDS` instead of by cleverness.
    !f.blocks.iter().flat_map(|b| &b.insts).any(
        |i| matches!(&i.kind, InstKind::Call { callee: Callee::Direct(c), .. } if *c == f.sig.name),
    )
}

/// Splice every eligible call in one function. Returns whether anything moved.
fn inline_into(f: &mut Function, small: &HashMap<String, Function>, tag: &mut usize) -> bool {
    let mut changed = false;
    // One call per pass over the function: splicing rewrites the block list,
    // and finding the next call from the top again is simpler than keeping
    // an index valid across that.
    loop {
        let Some((bi, ii, name)) = find_call(f, small) else {
            return changed;
        };
        *tag += 1;
        splice(f, bi, ii, &small[&name], *tag);
        changed = true;
    }
}

/// The first call, in block order, whose callee is worth splicing.
fn find_call(f: &Function, small: &HashMap<String, Function>) -> Option<(usize, usize, String)> {
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, inst) in b.insts.iter().enumerate() {
            if let InstKind::Call {
                callee: Callee::Direct(c),
                ..
            } = &inst.kind
                && small.contains_key(c)
            {
                return Some((bi, ii, c.clone()));
            }
        }
    }
    None
}

/// Replace the call at `blocks[bi].insts[ii]` with the callee's body.
fn splice(f: &mut Function, bi: usize, ii: usize, callee: &Function, tag: usize) {
    let InstKind::Call { args, .. } = f.blocks[bi].insts[ii].kind.clone() else {
        unreachable!("find_call found a call")
    };
    let results = f.blocks[bi].insts[ii].results.clone();

    // The caller's block splits in two: what ran before the call, and what
    // runs after it — and what runs after takes the call's result as a block
    // parameter, which is how a `ret` from any of the callee's exits reaches
    // one place.
    let after_label = format!("inline.{tag}.after");
    let tail: Vec<Inst> = f.blocks[bi].insts.drain(ii + 1..).collect();
    f.blocks[bi].insts.pop(); // the call itself
    let term = std::mem::replace(&mut f.blocks[bi].term, Terminator::Unreachable);

    let ret_param = match (&results[..], &callee.sig.ret) {
        ([r], Some(t)) => vec![(r.clone(), *t)],
        _ => Vec::new(),
    };
    let after = Block {
        label: after_label.clone(),
        params: ret_param,
        insts: tail,
        term,
    };

    // Rename everything the callee defines, so that two splices of the same
    // function — or a splice into a function that already uses the name —
    // cannot collide. A dot is what every generated name already separates
    // with, and the tag is unique across the whole module.
    let rename = |s: &str| format!("{s}.i{tag}");
    let bound: HashMap<String, Operand> = callee
        .sig
        .params
        .iter()
        .map(|(n, _)| n.clone())
        .zip(args)
        .collect();

    let entry = callee.blocks[0].label.clone();
    let mut spliced: Vec<Block> = Vec::new();
    for (k, b) in callee.blocks.iter().enumerate() {
        let mut nb = b.clone();
        nb.label = rename(&b.label);
        // The entry block's parameters are the function's, and they are
        // bound to the arguments rather than passed.
        nb.params = if k == 0 {
            Vec::new()
        } else {
            b.params.iter().map(|(n, t)| (rename(n), *t)).collect()
        };
        let mut fix = |o: &mut Operand| {
            if let Operand::Value(v) = o {
                if let Some(a) = bound.get(v) {
                    *o = a.clone();
                } else {
                    *v = rename(v);
                }
            }
        };
        for inst in &mut nb.insts {
            for r in &mut inst.results {
                *r = rename(r);
            }
            map_operands(&mut inst.kind, &mut fix);
        }
        map_terminator(&mut nb.term, &mut fix);
        for label in successors_mut(&mut nb.term) {
            *label = rename(label);
        }
        // A `ret` is where the callee's control flow rejoins the caller's.
        if let Terminator::Ret(v) = nb.term.clone() {
            nb.term = Terminator::Br(Target {
                label: after_label.clone(),
                args: match (v, &callee.sig.ret) {
                    (Some(v), Some(_)) => vec![v],
                    _ => Vec::new(),
                },
            });
        }
        spliced.push(nb);
    }

    f.blocks[bi].term = Terminator::Br(Target {
        label: rename(&entry),
        args: Vec::new(),
    });

    // A `slot` is a stack allocation of *function* lifetime, so the callee's
    // belong in the caller's entry block — where every other one is, and
    // where the frame layout expects to find them.
    let mut slots: Vec<Inst> = Vec::new();
    for b in &mut spliced {
        b.insts.retain(|i| {
            if matches!(i.kind, InstKind::Slot { .. }) {
                slots.push(i.clone());
                false
            } else {
                true
            }
        });
    }
    for (k, s) in slots.into_iter().enumerate() {
        f.blocks[0].insts.insert(k, s);
    }

    // A callee that never returns — one whose every exit is a trap — leaves
    // nothing branching to the continuation, and what followed the call is
    // then code control cannot reach. Dropping it is not an optimization: an
    // unreachable block is one the verifier rejects.
    let returns = spliced
        .iter()
        .any(|b| successors(&b.term).contains(&after_label));

    let mut rest = f.blocks.split_off(bi + 1);
    f.blocks.append(&mut spliced);
    if returns {
        f.blocks.push(after);
    }
    f.blocks.append(&mut rest);
}

/// The labels a terminator names, mutably.
fn successors_mut(t: &mut Terminator) -> Vec<&mut String> {
    match t {
        Terminator::Br3 { neg, zero, pos, .. } => {
            vec![&mut neg.label, &mut zero.label, &mut pos.label]
        }
        Terminator::Br(d) => vec![&mut d.label],
        Terminator::Ret(_) | Terminator::Trap(_) | Terminator::Unreachable => Vec::new(),
    }
}

/// Drop functions nothing calls any more, which is most of what inlining
/// leaves behind (`main` and anything a global's initializer names stay).
pub fn drop_uncalled(m: &mut Module, roots: &[&str]) {
    let mut live: HashSet<String> = roots.iter().map(|r| (*r).to_string()).collect();
    // A vtable names its methods by *address*, and nothing calls them
    // directly — dispatch through an object is what the table is for
    // (Ch. 4 §3.1). A global's initializer is therefore a root.
    for g in &m.globals {
        for item in g.init.iter().flatten() {
            if let super::ir::InitItem::Addr(name) = item {
                live.insert(name.clone());
            }
        }
    }
    let mut queue: Vec<String> = live.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        let Some(f) = m.funcs.iter().find(|f| f.sig.name == name) else {
            continue;
        };
        for inst in f.blocks.iter().flat_map(|b| &b.insts) {
            if let InstKind::Call {
                callee: Callee::Direct(c),
                ..
            } = &inst.kind
                && live.insert(c.clone())
            {
                queue.push(c.clone());
            }
        }
    }
    m.funcs.retain(|f| live.contains(&f.sig.name));
    let called: HashSet<String> = m
        .funcs
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.insts)
        .filter_map(|i| match &i.kind {
            InstKind::Call {
                callee: Callee::Direct(c),
                ..
            } => Some(c.clone()),
            _ => None,
        })
        .collect();
    m.decls.retain(|d| called.contains(&d.name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::{parse_module, print_module, verify};

    fn round(src: &str) -> Module {
        let m = parse_module(src).expect("parses");
        let out = inline_module(&m);
        let errs = verify(&out);
        assert!(errs.is_empty(), "{errs:?}\n{}", print_module(&out));
        out
    }

    #[test]
    fn a_small_callee_is_spliced() {
        let m = round(
            "tir 0.1 target \"tritium\"\n\n\
             fn @twice(%x: t27) -> t27 {\n\
             ^entry:\n\
             %d = add.wrap t27 %x, %x\n\
             ret %d\n\
             }\n\
             fn @main() -> t27 {\n\
             ^entry:\n\
             %v = call @twice(const t27 5) -> t27\n\
             ret %v\n\
             }\n",
        );
        let main = m.function("main").expect("main");
        let calls = main
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i.kind, InstKind::Call { .. }))
            .count();
        assert_eq!(calls, 0, "{}", print_module(&m));
    }

    #[test]
    fn a_recursive_callee_is_left_alone() {
        // Splicing a function into itself is the same function one call
        // deeper, and there is no bottom to that.
        let m = round(
            "tir 0.1 target \"tritium\"\n\n\
             fn @down(%x: t27) -> t27 {\n\
             ^entry:\n\
             %v = call @down(%x) -> t27\n\
             ret %v\n\
             }\n\
             fn @main() -> t27 {\n\
             ^entry:\n\
             %v = call @down(const t27 5) -> t27\n\
             ret %v\n\
             }\n",
        );
        assert!(m.function("down").is_some());
    }
}
