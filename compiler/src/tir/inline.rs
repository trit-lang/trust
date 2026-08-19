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
        // Recomputed each round: splicing changes who calls whom, and a
        // function that was on no cycle before may be on one now.
        let cyclic = on_a_cycle(&out);
        let small: HashMap<String, Function> = out
            .funcs
            .iter()
            .filter(|f| worth_inlining(f) && !cyclic.contains(&f.sig.name))
            .map(|f| (f.sig.name.clone(), f.clone()))
            .collect();
        if small.is_empty() {
            return out;
        }
        let mut changed = false;
        for f in &mut out.funcs {
            // The tag has to be unique across rounds as well as within one,
            // or a second splice renames a value to a name the first made.
            //
            // The table is *borrowed*, and the exclusion is a name rather
            // than a removal: cloning it per function copied every small
            // body once for every function in the module, which is where
            // most of the inliner's time went — on HPL, three quarters of a
            // second of a compile that took under two.
            changed |= inline_into(f, &small, &mut tag);
        }
        if !changed {
            return out;
        }
    }
    out
}

/// Whether a function is small enough to splice. Being on a call cycle is
/// the other half of the test, and is `on_a_cycle`'s answer.
fn worth_inlining(f: &Function) -> bool {
    let insts: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
    insts <= BUDGET
}

/// The functions that lie on a call cycle. None of them may be spliced.
///
/// A function that calls itself would be spliced into itself forever, and so
/// would two that call each other. Excluding the function being inlined
/// *into* stops the first and not the second: splicing `is_even` into `main`
/// brings a call to `is_odd`, and the scan walks forward into the body it
/// just spliced, and splices `is_odd`, which brings a call to `is_even`.
/// Neither is `main`, so nothing stopped it — and `ROUNDS` never got the
/// chance, because this all happens inside one call to `inline_into`. Draft
/// 0.1 said a cycle of two "is stopped by `ROUNDS` instead of by cleverness",
/// and the compiler hung on `is_even`/`is_odd` (G9.28).
///
/// Tarjan's algorithm, and iterative rather than recursive: a compiler that
/// compiles itself is one large call graph, and running out of stack while
/// working out where the cycles are would be a poor way to find out.
fn on_a_cycle(m: &Module) -> HashSet<String> {
    let names: Vec<&str> = m.funcs.iter().map(|f| f.sig.name.as_str()).collect();
    let id: HashMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let edges: Vec<Vec<usize>> = m
        .funcs
        .iter()
        .map(|f| {
            let mut out: Vec<usize> = f
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .filter_map(|i| match &i.kind {
                    InstKind::Call {
                        callee: Callee::Direct(c),
                        ..
                    } => id.get(c.as_str()).copied(),
                    _ => None,
                })
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();

    let n = names.len();
    let (mut index, mut low) = (vec![usize::MAX; n], vec![0usize; n]);
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut work: Vec<(usize, usize)> = Vec::new();
    let mut next = 0usize;
    let mut cyclic: HashSet<String> = HashSet::new();

    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        index[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        work.push((start, 0));
        while let Some(&(v, child)) = work.last() {
            if child < edges[v].len() {
                work.last_mut().expect("just read").1 += 1;
                let w = edges[v][child];
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                // One component. It is a cycle if it holds more than one
                // function, or one that calls itself.
                let mut scc = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    scc.push(w);
                    if w == v {
                        break;
                    }
                }
                if scc.len() > 1 || edges[v].contains(&v) {
                    cyclic.extend(scc.into_iter().map(|w| names[w].to_string()));
                }
            }
        }
    }
    cyclic
}

/// Splice every eligible call in one function. Returns whether anything moved.
fn inline_into(f: &mut Function, small: &HashMap<String, Function>, tag: &mut usize) -> bool {
    // Not into itself, however small: a recursive function inlined into
    // itself is the same function one call deeper.
    let me = f.sig.name.clone();
    let mut changed = false;
    // The scan resumes where the splice happened rather than starting over.
    //
    // Starting over was simpler and quadratic: each splice makes the function
    // bigger and the next search rereads all of it, so a function with *k*
    // inlinable calls costs O(k²·size). On HPL that was 1.24 of the 1.56
    // seconds the whole compiler took. Blocks before the splice point have
    // already been searched and splicing does not add calls to them, so
    // moving forward loses nothing — and the blocks just spliced in are
    // searched next, which is what gives a call inside an inlined body its
    // chance in the same round.
    let mut from = 0;
    while let Some((bi, ii, name)) = find_call(f, small, &me, from) {
        *tag += 1;
        splice(f, bi, ii, &small[&name], *tag);
        changed = true;
        from = bi + 1;
    }
    changed
}

/// The first call at or after block `from`, in block order, whose callee is
/// worth splicing.
fn find_call(
    f: &Function,
    small: &HashMap<String, Function>,
    me: &str,
    from: usize,
) -> Option<(usize, usize, String)> {
    for (bi, b) in f.blocks.iter().enumerate().skip(from) {
        for (ii, inst) in b.insts.iter().enumerate() {
            if let InstKind::Call {
                callee: Callee::Direct(c),
                ..
            } = &inst.kind
                && c != me
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
    let mut live: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = roots.iter().map(|r| (*r).to_string()).collect();
    // A vtable names its methods by *address*, and nothing calls them
    // directly — dispatch through an object is what the table is for
    // (Ch. 4 §3.1). So a live global keeps the functions it names alive —
    // and a global is live only where surviving code writes its address,
    // which is why this alternates between the two rather than rooting at
    // every global (G9.95).
    while let Some(name) = queue.pop() {
        if !live.insert(name.clone()) {
            continue;
        }
        if let Some(f) = m.funcs.iter().find(|f| f.sig.name == name) {
            for b in &f.blocks {
                for inst in &b.insts {
                    if let InstKind::Call {
                        callee: Callee::Direct(c),
                        ..
                    } = &inst.kind
                    {
                        queue.push(c.clone());
                    }
                    super::canon::visit_operands(&inst.kind, &mut |o| {
                        if let super::ir::Operand::Global(g) = o {
                            queue.push(g.clone());
                        }
                    });
                }
                super::canon::visit_terminator(&b.term, &mut |o| {
                    if let super::ir::Operand::Global(g) = o {
                        queue.push(g.clone());
                    }
                });
            }
        }
        if let Some(g) = m.globals.iter().find(|g| g.name == name) {
            for item in g.init.iter().flatten() {
                if let super::ir::InitItem::Addr(name) = item {
                    queue.push(name.clone());
                }
            }
        }
    }
    m.funcs.retain(|f| live.contains(&f.sig.name));
    m.globals.retain(|g| live.contains(&g.name));
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

    #[test]
    fn two_functions_that_call_each_other_are_left_alone() {
        // Neither calls itself, so the old test — "is this call to me?" —
        // saw nothing wrong. Splicing `even` into `main` brings a call to
        // `odd`, and the scan walks forward into what it just spliced.
        // The compiler hung (G9.28).
        let text = "tir 0.1 target \"tritium\"\n\n\
             fn @even(%x: t27) -> t27 {\n\
             ^entry:\n\
             %v = call @odd(%x) -> t27\n\
             ret %v\n\
             }\n\
             fn @odd(%x: t27) -> t27 {\n\
             ^entry:\n\
             %v = call @even(%x) -> t27\n\
             ret %v\n\
             }\n\
             fn @main() -> t27 {\n\
             ^entry:\n\
             %v = call @even(const t27 5) -> t27\n\
             ret %v\n\
             }\n";
        let cycle = on_a_cycle(&parse_module(text).expect("parses"));
        assert!(cycle.contains("even") && cycle.contains("odd"), "{cycle:?}");
        assert!(
            !cycle.contains("main"),
            "main calls into the cycle, not round it"
        );
        let m = round(text);
        assert!(m.function("even").is_some() && m.function("odd").is_some());
    }

    #[test]
    fn a_long_cycle_is_a_cycle_too() {
        // Three deep, so that nothing about this rests on a pair.
        let text = "tir 0.1 target \"tritium\"\n\n\
             fn @a() -> t27 {\n^entry:\n%v = call @b() -> t27\nret %v\n}\n\
             fn @b() -> t27 {\n^entry:\n%v = call @c() -> t27\nret %v\n}\n\
             fn @c() -> t27 {\n^entry:\n%v = call @a() -> t27\nret %v\n}\n\
             fn @main() -> t27 {\n^entry:\n%v = call @a() -> t27\nret %v\n}\n";
        let cycle = on_a_cycle(&parse_module(text).expect("parses"));
        assert_eq!(cycle.len(), 3, "{cycle:?}");
    }

    #[test]
    fn a_call_that_is_not_a_cycle_is_still_spliced() {
        // The point of the exclusion is that it excludes cycles and nothing
        // else: a plain chain still collapses.
        let m = round(
            "tir 0.1 target \"tritium\"\n\n\
             fn @inner(%x: t27) -> t27 {\n^entry:\nret %x\n}\n\
             fn @outer(%x: t27) -> t27 {\n             ^entry:\n%v = call @inner(%x) -> t27\nret %v\n}\n\
             fn @main() -> t27 {\n             ^entry:\n%v = call @outer(const t27 5) -> t27\nret %v\n}\n",
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
}
