# Project status — a handoff

Written for whoever picks this up next, human or otherwise. It says what
exists, what it can do, what it cannot, and which of the non-obvious decisions
you have to know about before you change anything.

Everything below was checked by running it, not recalled. Where a number
appears, `cargo test` or `wc` produced it.

---

## 1. What this is

**Trust** is a memory-safe systems programming language for **balanced
ternary** machines, with a specification, a compiler (`trustc`), an
intermediate representation (TIR), an instruction set (TRISC-27), an
assembler, and a reference virtual machine (`tritium`). A `.tr` source file
compiles the whole way to a machine image and runs.

The design rule, stated in Ch. 1 §1 and honoured throughout: **let the radix
decide, and inherit everything else from Rust unchanged.** Ownership,
borrowing, lifetimes, traits and generics are radix-independent, so they are
taken whole. What the radix changes — a symmetric range with `MIN = −MAX`, one
division that rounds to nearest, a three-way comparison as the primitive, a
tryte with 19 683 patterns and therefore enormous niches — changes the answer,
and those are the places the specification argues rather than copies.

Each chapter ends with a scorecard, **"bug classes removed by construction"**,
which forces every chapter to show what it earned rather than what it carried.

---

## 2. Repository map

All counts below come from `scripts/stats.sh`. Run it rather than trusting
them; a number nobody can reproduce quietly withdraws this document's opening
claim.

```
spec/                              5 660 lines of specification, 11 documents
├── 00-abstract-machine.md   308   the AM: trits, trytes, words, arithmetic, faults
├── 01-naming.md             191   names, radix notation, document conventions
├── 01-types.md              277   Ch. 1 — scalars, operators, overflow flavors
├── 02-composites.md         235   Ch. 2 — tuples, arrays, structs, enums, layout
├── tir-0.1.md               376   the IR: SSA, block params, provenance, legalization
├── language/
│   ├── 00-syntax.md         615   Ch. 0 — lexis, items, expressions, patterns, grammar
│   ├── 03-references.md     520   Ch. 3 — ownership, references, lifetimes, slices
│   ├── 04-generics.md     1 048   Ch. 4 — traits, generics, dyn, closures, lang items
│   └── 05-library.md        784   Ch. 5 — text, the heap, iterators, ?, interior mutability
└── isa/
    ├── trisc-27-0.1.md      713   the machine: registers, encoding, instructions
    └── assembly-0.1.md      518   the assembly language

core/      2 082 lines  crate trit-core — Bt, Tint, flavors, faults, literals
compiler/ 26 037 lines  crate trustc   — frontend, TIR, layout, legalization, codegen
vm/        4 566 lines  crate tritium  — machine, assembler, image format, profiler
docs/
├── spec-gaps.md               84 entries: every place the spec was silent or wrong
└── status.md                  this file
scripts/
├── stats.sh                   produces every number in this document
└── citations.sh               checks that every `Ch. N §M` in the source exists
examples/{trust,tir,trisc}
```

Ch. 1, Ch. 2, the AM, Naming and TIR are the author's. Ch. 0, Ch. 3, Ch. 4,
the ISA and the assembly spec were written during implementation.

---

## 3. The pipeline, and how to run it

```
.tr ──parse──▶ AST ──lower──▶ TIR ──legalize──▶ TIR ──codegen──▶ .t27 ──asm──▶ .timg ──▶ tritium
                                  │
                                  └──▶ TIR interpreter (the oracle)
```

```sh
trustc build   <f.tr>                 # parse, check, lower, print TIR
trustc compile <f.tr> @main           # the whole way to assembly
trustc check   <f.tir>                # parse and verify a TIR module
trustc run     <f.tir> @fn [args…]    # interpret a TIR function
trustc legalize <f.tir> [f.target]    # legalize for a target
tritium asm    <f.t27> -o <f.timg>    # assemble
tritium run    <f.timg>               # execute
```

Running the demo end to end:

```sh
cargo build
./target/debug/trustc compile examples/trust/demo.tr @main > demo.t27
cat examples/trisc/runtime.t27 >> demo.t27          # putchar, and nothing else
./target/debug/tritium asm demo.t27 -o demo.timg
./target/debug/tritium run demo.timg
```

```
0t1 0t10 0t100 0t1000 0t10000 0t100000
0tTT1 -11 0t1101001 1000
C=48 R=42 90
3 6 9 12
0.4 0.11 1.2 1.9
42
[2]
[3][1]
```

That exact output is asserted by `the_demo_runs_the_whole_way`.

---

## 4. Component status

| Component | State |
|---|---|
| `trit-core` | complete: arbitrary-precision balanced ternary, width-typed `tN`, three overflow flavors, the AM's five fault codes, three-radix literals |
| TIR data structures, text format, verifier | complete, round-trips |
| TIR reference interpreter | complete, with provenance-tracking pointers and a function-address space |
| TIR canonicalization | four transformations: `promote_slots` (a `slot` never escaping one block becomes the value it holds — the shape `lower.rs` emits for every parameter, G8.7), `mem2reg::promote` (the same across blocks, inserting block parameters where the predecessors disagree, G8.13), `branch_through_select` (a branch on a select of constants reads the select's selector, G8.10) and `remove_dead`. TIR §6's target-independent optimization stage, newly occupied |
| TIR legalization | promotion complete; **expansion complete**: `add`, `sub`, `mul`, `div`, `rem`, `shl`/`shr` by a constant, `neg`, `cmp`, `tmin`/`tmax`/`tmul`, `select3`, wide loads and stores, and wide values across a function boundary — a real Trust program legalizes for a nine-trit machine and computes the same answers (G6.11). `mul` expands (G6.6 closed — TIR §3.1 gained `mulh`), and so do `shl` and `shr` by a **constant** amount (G6.12); `div` and `rem` become a call to a helper written in TIR (G6.13). A wide value crosses a function boundary as its parts, with a wide result through a hidden pointer (G6.5). What is left: a shift by a *computed* amount |
| Layout engine (Ch. 2) | complete: sizes, alignments, offsets, both `repr`s, discriminants, niche optimization |
| Trust frontend | Ch. 0–3 complete; Ch. 4 complete except generic traits |
| Backend (TIR → TRISC-27) | works, with a **linear-scan register allocator over live intervals**, decided once per function (G8.9, G8.11: −34.8%, and frame traffic down from 44% of everything executed to 13.5%) and instruction selection that uses the fields the encoding has — immediates, branch displacements, access displacements (G8.6−G8.8: a further −26.0% on HPL). A parameter stays in the register it arrived in where nothing can clobber it, and a function whose values all fit in registers opens no frame. Block parameters are allocated too, with each edge a parallel copy (G8.12); no peephole pass |
| Assembler | complete: two-pass, exact balanced-ternary expressions, every directive and pseudo-instruction |
| `tritium` VM | complete: encode/decode, ALU, sparse memory, negative-address device region, and `tritium profile` — which instruction ran, how often, and addressed from what (G8.6) |

**393 tests, zero clippy warnings, 68 commits.** `scripts/stats.sh`.

---

## 5. What the language can express today

Scalars (`trit`, `bool`, `t9`, `t27`, `taddr`), the three radices, all
operators with the three overflow flavors, `match` compiling to one `br3`,
arrays, tuples, structs, enums with payloads and niches, both `repr`s,
constants with compile-time exact arithmetic, all control flow.

References and `&mut`, auto-deref, slices as fat pointers, bounds-checked
indexing, `.len()`, moves, destructors with drop flags, a **non-lexical borrow
checker**, returned references under elision.

Traits with required and provided methods, supertraits, inherent and trait
impls, the four receivers, associated functions, associated types and
constants, generic functions/structs/enums/impls with monomorphization and
bounds, `dyn Trait` with real vtables and object safety, closures with capture
inference and `impl Fn(…)` parameters, `for` loops over a user `Iterator`,
`#[derive(Eq, Ord, Clone)]` with the comparison operators wired to them,
`impl !Copy`, the turbofish, `Option` and `Result`.

## 6. What it cannot, and why

| Missing | Why |
|---|---|
| `String`, and growable text | `str` and string literals are built (G9.6) and are `&'static`; a growable one needs the heap |
| ranges, so `for i in 0..10` | Ch. 0 §4 reserves range expressions |
| a type parameterized by a `const` | `const N` works as an array *length* (G8.2); `struct Grid<const N: taddr>` is Ch. 4 §2.4 and unimplemented |

| returning a closure | needs `impl Trait` in return position or `Box<dyn Fn>`; Ch. 4 §4.5 |
| `FnOnce` | every capture is by reference; needs a move analysis of the closure body |
| `IntoIterator`, so `for x in xs` over an array | array iterators are the library's; the blanket impl it needs now works (G0.14b) |
| modules, `use`, `pub`, multiple files | reserved in Ch. 0 §1.3 |
| `unsafe`, raw pointers | reserved; `?` is built (G9.8) |
| bounds checks a loop condition already implies | every array index still costs two comparisons and two branches; branches and comparisons are 36% of everything HPL executes (G8.13). Removing them needs range analysis, which nothing here does |

**Generic traits were the one substantial hole, and it is closed**
(G0.14a, G0.14b). A trait may take type parameters, one type may implement it
many times, and the arguments are part of each method's name —
`t27.From.t9.from` against `t27.From.bool.from`. A bound carries them too. And
a **blanket impl** — `impl<T, U: From<T>> Into<U> for T` — holds for every
type satisfying its bounds, so implementing `From` gives `Into` for free.

A blanket impl is the one impl found by checking a condition rather than by
looking up a name. It needed no new lowering machinery: a generic body is
lowered by reading the same source under an environment, so the rule's methods
are ordinary generic functions and applying the rule is binding its parameters.
A trait a rule covers is closed to hand implementation, which is what keeps
§1.8's coherence rule a comparison of names rather than an overlap search.

What remains unbuilt is small and each part says so: a bound on a trait's own
parameter, a method with type parameters of its own inside a blanket impl, and
a rule whose self type is anything but a bare parameter.

---

## 7. Architecture you must know before changing anything

**A type parameter is not a kind of type.** There is no `Ty::Param`. A generic
parameter is a name that an *environment* (`HashMap<String, Ty>`) maps to a
concrete type, so lowering a generic body is lowering the same AST under a
different environment and **no AST is ever rewritten** for generics. A generic
struct or enum becomes an ordinary nominal type under a mangled name the first
time it is applied. Consequence: the layout engine, the drop machinery, the
borrow checker and codegen never learn that generics exist, and **no generic
construct reaches TIR** — asserted by a test.

**Everything is desugared into things that already worked.**

| Surface | Becomes |
|---|---|
| `impl` block method | a function named `Type.method`, `Self` substituted, receiver a leading parameter |
| `p.area()` | `Point.area(&p)`, with the derefs Ch. 3 §2.3 calls for |
| a closure | an anonymous struct of references + an ordinary function; captures rewritten to `(*self.field)` |
| `impl Fn(A) -> R` parameter | a named type parameter whose bound carries the signature |
| `for x in e` | §5.7's `loop`/`match` expansion, **in the parser** |
| associated constant | an ordinary constant named `Type.NAME` |
| `derive(Ord)` | TIR emitted directly, because §5.3.3 promises branchless and no expression spells `select3` |

Because of this, borrow checking, moves, drop flags and argument checking
apply to all of it without knowing it exists.

**Name mangling uses `.`** — `Type.method`, `Pair.t27.t9`, `main.closure1`,
`it.3`, `#a1` for internal locals. The dot is deliberate: TIR identifiers and
assembler labels accept it, **Trust identifiers do not**, so a generated name
can never collide with or shadow anything a program wrote. `#` appears only in
names that never reach TIR.

**`Types` has interior mutability** (`RefCell` on `db`, `structs`, `enums`,
`assoc`, `closures`, `instantiations`) because a generic type is registered
the first time it is applied, which can happen anywhere a type is resolved.
Beware `while let Some(x) = cell.borrow_mut().pop()` — the borrow lives for the
whole loop body. That has bitten once already.

**`Option` and `Result` are prepended to every file as source** (`lang::PRELUDE`),
and error line numbers are adjusted back down. Ch. 4 §5.8 says they are
ordinary enums with no special case, and prepending source is the most direct
way to keep that honest.

**`&dyn Trait` is (data pointer, vtable pointer)**, 6 trytes, alignment 3 —
the same shape as `&[T]`. The vtable is a TIR global of `addr @…` items in
§3.3's order: size, align, drop, then one address per object-safe method,
**supertraits' first**. One list produces both the table and the dispatch
index, which is the only way they can be guaranteed to agree.

**TIR was extended for this** (G0.10, a deliberate spec change): §1.2 global
initializers take items, one of which is `addr @name` (a relocation); §3.7's
indirect call, already "parsed but reserved", is now defined. §4's UB
inventory went from four sources to five.

**ISA §2.2 reserves the first word of memory.** It is the all-zeros word,
which is a `nop`, so execution still begins at address 0 and falls through.
The *assembler* emits it, so hand-written `.t27` gets it too. This is what
makes Ch. 4 §3.3's drop sentinel of 0 unambiguous, and it makes the non-null
invariant hardware-checkable.

---

## 8. The disciplines that hold this together

**1. The differential invariant, and exactly what it covers.** Every
end-to-end test runs the same program *two* ways — on the TIR interpreter and
through the whole pipeline to the machine — and demands identical results,
faults included. `compiler/tests/pipeline.rs`. Do not add a feature that only
one side can execute: this is why TIR was extended for `dyn` (G0.10) rather
than emitting vtables behind TIR's back.

**Both sides take the same TIR as input.** So the invariant covers
`TIR → machine` and says *nothing* about `.tr → TIR`. A lowering bug makes
both oracles agree on the same wrong answer, and the differential test sees
nothing. Every bug in §11 is on the lowering side — drop glue, move tracking,
when a generic body is checked — and not one of them was ever going to be
caught here.

The front half has two other checks and needs both:

- **Legalization is checked as a transform.**
  `compiler/tests/legalize_semantics.rs` interprets a module, legalizes it,
  interprets it again and demands the same result. Nothing else checks that a
  pass preserved meaning, and legalization is the only mandatory one. It also
  pins the frontier: each thing expansion still refuses is asserted, so moving
  it fails a test.
- **`verify` runs at every seam**, so lowering cannot emit ill-formed TIR;
  and after legalization it runs as `verify_legalized`, which additionally
  enforces TIR §6's post-condition — every arithmetic width is one the target
  has a native operation for. §6 says a backend "may assume legalized input
  and is not required to handle any other", which without a check is a licence
  to emit anything, and legalization is *incomplete* (`div`, `rem` and a
  computed shift amount are unwritten at wide widths), so that path is
  reachable.
- **Output assertions and negative tests.** `run()` checks a program's exact
  output, and `error()` checks that a program that must be rejected *is*, with
  the reason. Discipline 3 below has teeth only through the second: of the 94
  frontend tests, most assert both directions. When you fix a lowering bug,
  the regression test is an output assertion or a rejection — never a
  differential one.

**2a. The drop ledger.** `every_owner_drops_exactly_once` enumerates every
construct that can own a value and asserts the exact sequence a run prints.
It exists because Ch. 3 §1.5 gives the language no resources, so a value
dropped twice or never releases the same nothing and no ordinary test can see
it — which is how six drop bugs got in, three found by review and three by
this table on its first run. **Add a row before you add a feature test.** If
you cannot say what the output should be, that is the bug.

**2. `docs/spec-gaps.md` is not optional.** Every place the specification is
silent gets an entry: what was ambiguous, what was decided, and why. 45
entries. When you decide something the spec did not, write it down there in
the same commit. When you find that the spec is *wrong*, say so there and fix
the spec.

**3. Refuse what cannot be checked.** Where a feature cannot be verified yet,
reject it with a diagnostic naming the chapter and section it waits for,
rather than accepting it unsoundly. This is why returned references were
rejected before the borrow checker existed.

**4. The sweep rule (Naming §6).** A document that corrects or supersedes
another must fix the other's text **in the same commit**, and every header
table carries `Supersedes` / `Superseded by`. This was extended from the
shared base to all documents after five corrections were found living only at
the correcting end.

**5. Prose style, and citations that are checked.** Specification text and
code comments both cite the section they implement (`Ch. 3 §2.2`, `AM §3.4`,
`TIR §6`). Comments say *why*, not *what*. Neither hedges. Where something is
missing, it says it is missing and what it is waiting for. Match this; it is
most of what makes the documents usable.

`scripts/citations.sh` checks every citation in the source **and in the
specification itself** against the document it names. It cannot tell whether a
citation is *apt* — only whether its section exists — and that low bar has
caught two batches. On its first run, five citations in the source named
subsections of TIR's undefined-behavior inventory and its legalization
contract; neither has subsections, because both are lists. They became
"item N" and a bare section reference.

Extending it to the specification and to these documents caught three more of
exactly the same kind, in `docs/spec-gaps.md` — the fix had been applied to the
code and not to the document describing it, which is Naming §6's sweep rule
failing in the direction it always fails.

The stronger version of this check is worth building. The drop-glue
double-free would have failed it: `drop_at` cited Ch. 3 §1.4, whose item 3
says "a field the destructor moved out of is not dropped again", while the
code dropped every field twice. A citation that names a real section it
contradicts is the next thing to catch.

---

## 9. Traps a newcomer will hit

- `for x in (Counter { … })` needs the parentheses — a struct literal is not
  allowed in a scrutinee position without them (Ch. 0 §2.8).
- **`0h` literals: leading zeros change the value.** `D` is the zero digit, so
  `0h0J` is −345 while `0hJ` is 6. Ch. 1 §3.1.
- `%` is round-to-nearest, so `18 % 10` is **−2**, not 8. Decimal digit
  extraction needs a correction; balanced-ternary extraction does not, which
  is the point `examples/trust/demo.tr` is making.
- There is no `mut` binding mode on a parameter (Ch. 0 §3.1). Bind a local.
- Enums are always passed by address, so an enum comparison is a call to
  `Ord::cmp` or an error, never a machine `cmp`.
- The `.tr` compiler needs `examples/trisc/runtime.t27` appended for
  `putchar`; that file is the entire runtime.
- **`while let Some(x) = cell.borrow_mut().pop()` holds the borrow for the
  whole loop body**, and several of the loops here queue more work into the
  same cell. This has caused one panic already. Bind and break instead. (Also
  in §7; it is repeated here because §9 is the section someone opens while
  debugging.)
- There is no `Sized` bound. Ch. 4 §2.5 says every type parameter has an
  implicit one and `?Sized` removes it; the implementation has neither, so a
  parameter behaves as `?Sized` and the size requirement is checked at each
  *use* instead. More permissive than Rust, equally sound, and the difference
  shows only in where the error appears.

---

## 10. Where to go next

Three coherent directions, in no forced order:

> **Ordering constraint.** Before route A, re-read §11 and hunt for anything
> of the shape it describes: a drop or move bug that no test can see because
> the language owns no resources. Three were found and fixed after this
> document's first draft; an allocator turns the next one into a real
> double-free, found by a crash, in a session that is thinking about
> allocation. The cheapest time to look is while nothing is at stake.

**A. Ch. 5 as implementation.** §1 is done (G9.5–G9.7): `char`, `str`, both
literal forms, `char::try_from`, and a text library written in Trust in the
prelude, which a program pays nothing for unless it calls it.
`examples/trust/hello.tr` prints `Hello, 世界! 🙂` and the only thing in it
that is not Trust is `putchar`. §4's `?` is built too (G9.8), and §3's adaptors —
`Map`, `Filter`, `Take`, `Skip`, `Enumerate`, `Zip` — are structs in the
prelude (G9.9). §4.3's `unwrap` is written in the
language too, on `!` and `trap()` (G9.10). §3's consuming methods are built too
(G9.11), and the heap's two halves below the language
are built: the assembler defines `_end` and the runtime supplies `alloc` and
`free` (G9.2). `Box` is built (G9.12), Ch. 2 §8's binary tree runs
(G9.13), and `Vec` is built: `new`, `push`, `pop`, `len`, `capacity`,
`is_empty`, `reserve`, `clear`, indexing, dropping, and the coercion of
`&Vec<T>` to `&[T]` (G9.14, G9.15). `String` **is** `Vec<char>` and needed no
rules of its own. Building the last of those uncovered the drop ledger's fifth
bug — an arm's pattern bindings were never dropped — and closing it needed the
condition that they own only when the scrutinee was matched by value.

§1.5's `print` and `println` are in the prelude now, so
`examples/trust/hello.tr` is a `main` with one statement in it and
`examples/trust/HPL.tr` is 277 lines shorter — every line of its output was a
hand-encoded ASCII array with the text in a comment above it, and all 53 are
string literals now, for −0.2% instructions rather than a cost; `putchar` is
declared there too, which makes it the third required target function after
`alloc` and `free` (G9.17). `char::to_utf8` has the signature the
specification always gave it, `let` binds a tuple, and `str::chars` iterates.

Both gaps that opened underneath it are closed: a `match` binding taken
through a reference may be read and borrowed but not moved out of (Ch. 3
§1.3), and a block-shaped expression in statement position ends its statement
(Ch. 0 §5.2).

`F: Fn(A) -> B` is a bound a program can write now (G9.18), and instantiation
happens in two stages where a method has type parameters of its own inside a
generic impl (G9.19) — so §3.1's adaptors are the provided methods that
section always described, and they chain:

```
sum(it.map(|x| x * x).filter(|v| v % 2 == 0))
```

Every adaptor's `Item` follows the iterator underneath it rather than being a
fixed `t27`, which it had been because `same_ast_ty` had no case for `I::Item`
and so a generic impl could only choose an associated type it could name.

`collect` is built (G9.20), which made `Vec` a nominal type the library can
implement traits for — `impl<A> FromIterator for Vec<A>` is written in Trust
in the prelude:

```
let v: Vec<t27> = it.map(|x| x * x).filter(|y| y % 2 != 0).collect();
let s: String = text.chars().map(f).collect();
```

`Vec` is finished: `insert`, `remove`, `with_capacity` and `push_str` are
built (G9.21), and the claim that `insert` waited on a `copy_within` §7
reserved was wrong — §7 reserved nothing of the kind, and now reserves it
properly as the slice method it is.

An impl may name one instantiation now (G9.22), so `String`'s `push_str` is
written in the library in Trust as `impl Vec<char>`, and declarations are
pruned like functions — `alloc` and `free` had begun appearing in every
program the moment the library gained a method that pushes.

What is left of Ch. 5 is `Cell`/`RefCell` (§5), `expect`, formatting, maps and
sets, sorting and `Rc` — all of §7, all reserved. The nearest thing merely
unbuilt is an impl whose self type is a **reference**, which would give
`for c in s`, the `IntoIterator` step in `for`'s desugaring, and binding by
reference in a `match` — three recorded gaps, one fix.

**B. Backend quality.** The instruction stream is a third of what it was
(G8.6–G8.13, −66.6% on HPL), and memory is nearly out of the picture: frame
traffic is 3.6%. What is left is analysis. Branches and comparisons are 36%
of everything executed — loop conditions, and the two bounds checks every
array index still pays — and the index multiply is 7.9%. Bounds-check
elimination and strength reduction both need a range analysis, which is the
next thing this compiler does not have. Expansion is complete except for a
shift by a computed amount.

**C. Generic traits.** §6 above says what it needs. It closes Ch. 4 and
unblocks `From`/`Into`.

Smaller, self-contained: `IntoIterator`; `FnOnce`; capture by place rather
than by variable (Ch. 4 §4.4 says place, the implementation does variable);
associated types in generic impls; `F_ILLEGAL` / `F_ADDRESS` fault codes,
which **G0.2a** reports the AM as lacking.

---

## 11. Known-wrong or under-tested

Three entries that stood here in the first draft of this document were
**memory-safety bugs**, not approximations, and one of them was described as
conservative when it was the opposite. They contradicted Ch. 3 Appendix B's
"double free | a moved-out value is not dropped" — the row the language is
named for — and were unobservable only because Ch. 3 §1.5 has no resources to
leak or free twice. They are fixed; they are kept here as the shape of what
this section can hide.

- ~~A nested destructor ran **twice**~~ — the field drops were emitted both
  inside `drop.T` and again at the call site. No move was needed; any struct
  with a droppable field did it. `drop.T` is now the complete glue and the
  call site only calls it, which is also what Ch. 4 §3.3's vtable drop slot
  assumes.
- ~~Moving a non-copyable **field** out was not tracked at all~~ —
  `take(o.a); take(o.a);` compiled and dropped one value three times. The
  first draft of this section called this "conservative"; it was unsound.
  Reading a place of non-copyable type now moves it, per Ch. 3 §1.2, with
  ownership still tracked per local rather than per place — *that* is the
  conservative part.
- ~~An enum's payload was not dropped~~ — a leak, for any value inside an
  `Option`. Dropping is now a dispatch on the discriminant, one comparison per
  droppable variant.

**The lesson worth carrying:** all three were on the `.tr → TIR` side, which
§8.1 explains the differential invariant does not cover, and all three were
invisible because the language has no resources yet. **Route A in §10 adds an
allocator.** Anything of this shape still hiding here becomes a real
double-free or leak on that day, in a session whose attention is on the
allocator. Prefer finding it now.

**Every entry below cites a test.** A claim about how the implementation
falls short is worth no more than any other claim without something that
runs, and the entries claiming *conservatism* need a **wrongly-rejected
program** specifically: "conservative" is an assertion about which direction
the error goes, and no passing test can show that. Where an entry describes
behaviour that is simply wrong, the test asserts the wrong behaviour, so that
fixing it fails here and forces this section to be updated. An entry with no
test is an unverified claim and must say so.

That rule exists because of the second bug above. It sat here for a draft
labelled "conservative, harmless" — a claim nobody had evidence for, that
pointed the wrong way, about a memory-safety hole.

| Limit | Test |
|---|---|
| a generic body is checked at instantiation, not once against its bounds | `known_limit_a_generic_body_is_checked_at_instantiation` |
| there is no `Sized` bound | `known_limit_there_is_no_sized_bound` |
| a returned borrow is rooted syntactically | `known_limit_a_returned_borrow_is_rooted_syntactically` |
| a closure captures by variable, not by place | `known_limit_a_closure_captures_by_variable_not_by_place` |
| ownership is per local, not per place | `per_local_ownership_rejects_two_programs_that_are_legal` |
| every owner drops exactly once | `every_owner_drops_exactly_once` (the ledger, §8.2a) |
| diagnostics print mangled names | `known_limit_diagnostics_print_mangled_names` |

**Two of these are one thing.** *A generic body is checked at instantiation*
and *there is no `Sized` bound* have the same root, and it is §7's first
decision:

- The bound half of Ch. 4 §2.2 holds — a failed bound is reported at the call
  site, naming the call, the parameter and the trait — but a generic function
  that is never called is never checked at all. This is the C++ failure mode
  Ch. 4's Appendix B claims is removed by construction, and it is removed at
  the call site only.
- Ch. 4 §2.5 gives every type parameter an implicit `Sized` bound and `?Sized`
  to remove it. There is neither: a parameter behaves as `?Sized`, and the
  size requirement is enforced at each *use*, so the error surfaces in the
  body rather than at the call — **which is the same failure mode again.**

Both need the same thing: the ability to represent "some type known only to
implement `Shape`", and to resolve `s.area()` from the bound alone. That is
`Ty::Param`, and **§7 explains that its absence is load-bearing**: it is what
keeps the layout engine, the drop machinery, the borrow checker and code
generation ignorant of generics, and a test asserts that no generic construct
reaches TIR. Adding it is not adding an enum variant; it is four downstream
components meeting a kind of type they were designed never to see. Do not
attempt either limit in isolation — you will reach the same wall from two
directions.

The `?Sized` behaviour is sound *provided the list of use sites that require
a size is exhaustive*, and that list is the kind that grows quietly as a
language does: parameters, `let` bindings, fields, reads through a reference
today; return positions, array elements, tuple members, closure captures and
enum payloads tomorrow. If you add a construct that needs a size, route it
through the same check rather than writing a fifth one.

The rest:

- **Ownership is per local, not per place.** Ch. 3 §1.3 says moving out of a
  place leaves *that place* uninitialized, not the whole variable. Moving out
  of `o.a` here moves `o`. Two legal programs are rejected as a result —
  moving out of disjoint fields, and moving a field out and putting one
  back — and those two assertions are what must flip if per-place ownership
  arrives.
- **Region inference does not exist.** A returned borrow must be rooted
  syntactically at a parameter. Every program accepted would be accepted by
  full region inference; some rejected ones would not be, and those are the
  ones needing written lifetimes.
- **Capture is by variable, not by place.** Ch. 4 §4.4 says a closure using
  `p.x` borrows `p.x` and leaves `p.y` free; this one borrows `p`.
- **Diagnostics print mangled names**: `Pair.t27.t9`, not `Pair<t27, t9>`.
