# Emission determinism — the rules byte-equality is defined on

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/tir-0.1.md` (*TIR*) §8, `docs/ddc.md` §3 |
| **Audience** | Implementations of the Trust compiler — never programs. Nothing here is an API promise; it is the definition of *identical output*, and exists so a second (or third) compiler can be checked against it |

`docs/ddc.md` §3.3 defines the comparison: two compilers are run over the
same Trust source and the canonical TIR text (TIR §8.3) is demanded
byte-identical, **names included**. TIR §8 says what the text may look
like; it does not and should not say which of the many legal texts one
compiler emits for one program. That is this document's whole job: pin
every choice in the emission path — which counter, which name, which
position — so that equality is a property of the *rules*, not of two
run-to-run coincidences.

Two scopes are stated for every rule:

- **what is pinned**: the text an implementation must produce.
- **what is free**: internal scheduling, data structures and queue
  discipline, as long as the pinned text results. ddc.md §3.1's env rule
  stands beside this and is restated in §7 below.

The checking order is the corpus ladder of
`plan/cpp-third-implementation.md` §3 (L1 reproducers, L2 the prelude
counters, L3 whole programs, L4 sA), and each rule below carries the
evidence level it is pinned at: *spec*-level (already normative where it
links a chapter), or *corpus*-pinned (held byte-for-byte by the suites on
every line they reach, with the diffs that taught it logged in
`docs/spec-gaps.md`).

---

## 1. Names: the alphabet, and one counter per function

**The alphabet** is TIR §8.1's: letters, digits, `_` and `.`, and nothing
else — `#` in particular is not a name character (spec-gaps G9.183, the
episode that made this table load-bearing). Every name a compiler invents
is spelled with a dot so no source-level Trust or TIR identifier can
collide with it (Ch. 0 §1.5).

**One counter per function, shared by every invented name — values and
block labels alike.** It starts at 1 for each function definition and
mints in the order the emission sites fire (*corpus*-pinned; e.g. in one
function `%tmp.slot.1`, `%d.2`, `^match.join.3`, `^arm.4` show values and
labels drawing consecutive numbers from the same sequence). Each generic
instantiation is its own function and re-seeds its own counter.

**Name families**, the observed letter each mint site takes
(*corpus*-pinned as a set; the letters are for readers — equality is on
the full name, which is the point ddc.md §4 step 3 makes):

| Minted by | Shape |
|---|---|
| a local's storage slot | `%<binding>.slot.N` |
| an aggregate temporary nothing named | `%tmp.slot.N` — and it is registered under no binding (spec-gaps G9.174's second face: nothing a program writes can look it up) |
| a plain expression value | `%v.N` |
| a call's result value (scalar) | `%r.N` |
| a discriminant load | `%d.N` |
| a pointer-producing `offset` | `%p.N` |
| a comparison result | `%c.N` |
| a result bound under a written name | the binding's own letter — `%a.5` for a result the source calls `a` |
| a `for` loop's invented iterator binding | `it.<k>` — the function's *k*-th `for` (Ch. 4 §5.7's desugaring; the dot protects it from shadowing, spec-gaps G9.175) |
| a `?` operator's scrutinee join | `q.<k>` — the function's *k*-th `?` (G9.183's `q.N`/`r.N` family) |
| a `let _` binding | `wild.N` (G9.183) |
| the whole a pattern takes apart | `pat.N` (G9.183) |
| a macro expansion's own bindings | `.<name>.mac<k>` per Ch. 7 §4's hygiene rename — names no program can write (G9.173) |
| block labels | `entry`, then the dot-mangled families the lowering mints on the same counter (`arm.N`, `arm.next.N`, `match.join.N`, …) |

## 2. Slots: hoisting, adoption, and the hole in the numbers

**Slot instructions print at the top of the entry block, in mint order.**
A local's slot is invented where the emission evaluates it, so the
textual order of `slot` instructions need not match the numbers' first
occurrence — the numbers follow evaluation, the `slot` lines follow
hoisting (*corpus*-pinned).

**A `let` of a *computed* aggregate adopts the temporary** (the answer is
built destination-first — spec-gaps G9.25's destination rule): the
temporary mints its number, the binding mints its own, and the
temporary's number simply leaves **a hole in the sequence** — the
documented example runs 1, **3**, 4, 5 with 2 missing (ddc.md §4 step 3;
G9.174's rename rule; a call answering an aggregate into a binding takes
the same path). A `let` of a *place* is excluded: `let y = x` copies and
adopts nothing.

## 3. Generic instantiations

Per Ch. 4 §2's monomorphization, with the emission rule of ddc.md §4
step 3:

- **Name** — the function's name, a dot, then the **written** type
  arguments in declaration order (`@id.trit`, never `t1`; spec-gaps
  G9.192's dot-alphabet for wrappers: `raw.<T>`, `box.<T>`); across a
  module boundary the path mangles into it (`@util.id.t27`); an impl's
  own parameter is the first of a method's key (`@Pair.first.t27`); a
  closure in a key spells `main.closure<k>` under the function that
  defines it (`@apply.main.closure1`, `@Map.next.Range.t27.main.closure1`
  — G9.179).
- **Order** — instantiations are emitted **after the ordinary functions
  and impls**, in the reverse of the order the calls asked for them: the
  queue is a **stack**, deduplicated when a key is *asked for* (not when
  it is emitted), and drained at that point (*corpus*-pinned; the probe
  that asks `@id.t27` then `@id.trit` prints `@id.trit` first). A key an
  instantiation's own body asks for lands on top of what was already
  waiting.
- Each instantiation lowers the family's body with its parameters
  substituted by the key, in order — no AST is rewritten (spec-gaps
  G0.8's environment rule, Ch. 4 §2.3's instantiation-on-ask).

## 4. Trait glue, closures, vtables

- **A trait method with a body is emitted once per implementing type**,
  named `@<Type>.<method>` (`@Re.twice`), an ordinary function synthesized
  from the provided body — spec-level is Ch. 4 §1.5's desugaring; the
  **position** is *corpus*-pinned: beside the instantiations, after the
  ordinary functions (spec-gaps G9.197(c)'s queue-timing question resolves
  at the printed text: internal queue discipline is free, the position is
  not).
- **A closure is two things** (Ch. 4 §4): an anonymous struct of captures
  and an ordinary function named under the closure's family
  (`@main.closure<k>.call`) — G9.176's family registration.
- **A vtable is a global per (type, trait) pair**, named
  `@vt.<Type>.<Trait>`, holding `addr @…` items in Ch. 4 §3.3's order —
  size, align, drop sentinel, then the object-safe methods, supertraits
  first (probe-pinned: `@vt.Sq.Area : tryte[12] = [3,0,0, 3,0,0, 0,0,0,
  addr @Sq.area]`). Vtable names make them globals, so TIR §8.3's item
  order places them before every function — a table is a use (ddc.md
  §3.2), and an address in a global is a root, never pruned (spec-gaps
  G9.7).

## 5. The pipeline below TIR (backend determinism)

For L3's whole-program-assembly ladder, the same discipline holds one
stage down: the allocator's free pool is an **order**, not a set — a
release preserves the pool's order (`retain`-shape, never swap-remove),
because the next choice reads the pool as ordered (spec-gaps G9.187);
frame layout, immediate forms and the two-pass branch naming mint names
from the same machinery. *Corpus*-pinned on the twenty-two whole
programs; this section is bound from M2 of the cP3 plan, not M1.

## 6. The environment rule (restated)

No wall-clock, path, memory address, parallel completion order, or hash
iteration order may reach the output (ddc.md §3.1's list). For an
implementation written in C++: `std::unordered_map`'s iteration order is
a hash order — it may be consulted, never printed. `scripts/reproducible.sh`
holds the reference implementation to 32 compilations in separate
processes for exactly this reason, and a third implementation is held to
the same on its own corpus before L1.

---

## Appendix (informative) — how these rules are pinned

Nothing above is recalled; each rule cites where it was fought for. The
probes below cover the load-bearing ones and are kept small enough to
re-run by hand:

| Probe | Pins |
|---|---|
| `fn id<T>(x: T) -> T { x }` with two calls | the stack drain (last asked, first printed), one counter re-seeded per instantiation, written-type names |
| a provided trait body on a struct | glue name `@Re.twice`, position after the ordinary functions |
| a multi-statement `main` | slot hoisting, adoption holes, the shared values-and-labels counter |
| `?`, `for`, a closure, a `dyn` call | the `q.k` / `it.k` / `closure.k` / `@vt.` families |

`docs/spec-gaps.md` is the provenance: G9.183 (the alphabet), G9.174
(adoption), G9.192 (wrapper names), G9.187 (the pool's order), G9.197
(the glue's position), and the march of G9.208–G9.233 (every divergence
the byte comparison has ever caught, closed).
