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

```
spec/                              5 932 lines of specification, 10 documents
├── 00-abstract-machine.md   304   the AM: trits, trytes, words, arithmetic, faults
├── 01-naming.md             191   names, radix notation, document conventions
├── 01-types.md              277   Ch. 1 — scalars, operators, overflow flavors
├── 02-composites.md         235   Ch. 2 — tuples, arrays, structs, enums, layout
├── tir-0.1.md               376   the IR: SSA, block params, provenance, legalization
├── language/
│   ├── 00-syntax.md         604   Ch. 0 — lexis, items, expressions, patterns, grammar
│   ├── 03-references.md     520   Ch. 3 — ownership, references, lifetimes, slices
│   └── 04-generics.md     1 048   Ch. 4 — traits, generics, dyn, closures, lang items
└── isa/
    ├── trisc-27-0.1.md      713   the machine: registers, encoding, instructions
    └── assembly-0.1.md      513   the assembly language

core/     2 082 lines   crate trit-core — Bt, Tint, flavors, faults, literals
compiler/ 19 777 lines  crate trustc   — frontend, TIR, layout, legalization, codegen
vm/       4 148 lines   crate tritium  — machine, assembler, image format
docs/
├── spec-gaps.md         930   every place the spec was silent, and what was decided
└── status.md                  this file
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
| TIR legalization | promotion complete; expansion complete for `add`/`sub`/`cmp`; `mul` blocked on G6.6, `div`/`rem`/shifts unwritten |
| Layout engine (Ch. 2) | complete: sizes, alignments, offsets, both `repr`s, discriminants, niche optimization |
| Trust frontend | Ch. 0–3 complete; Ch. 4 complete except generic traits |
| Backend (TIR → TRISC-27) | works; **no register allocator** — every value lives in a stack slot |
| Assembler | complete: two-pass, exact balanced-ternary expressions, every directive and pseudo-instruction |
| `tritium` VM | complete: encode/decode, ALU, sparse memory, negative-address device region |

**297 tests, zero clippy warnings.** By target: `trit-core` 14 + 22 + 14,
`tritium` 3 + 25 + 21, `trustc` 7 + 15 + 90 + 28 + 13 + 11 + 32, plus 2
doc-tests. 23 commits.

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
| strings and characters | AM §5 defers text encoding to the library chapter; write `[t9; N]` of code units |
| `Box`, any heap allocation | no allocator; Ch. 3 §6 |
| returning a closure | needs `impl Trait` in return position or `Box<dyn Fn>`; Ch. 4 §4.5 |
| `FnOnce` | every capture is by reference; needs a move analysis of the closure body |
| `IntoIterator`, so `for x in xs` over an array | the blanket impl and array iterators are the library's |
| generic traits, `From`/`Into` | see below — the one substantial hole in Ch. 4 |
| modules, `use`, `pub`, multiple files | reserved in Ch. 0 §1.3 |
| `unsafe`, raw pointers, `?` | reserved |
| a register allocator | generated code is correct and slow |

**Generic traits** are the one substantial hole. Two things stand in the way
and both must arrive together: a type may implement `trait From<T>` many
times, so a method key must carry the trait's arguments and resolution must
pick by argument type (today a method is keyed `Type.method` and there is
one); and a blanket impl like `impl<T, U: From<T>> Into<U> for T` is
quantified over types satisfying a bound, so deciding whether it applies is a
search rather than a lookup (every other impl here is found by name). Half of
it would leave the hole exactly where Ch. 4 §5.6 puts its example. Recorded in
G0.14.

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

**1. The differential invariant.** Every end-to-end test runs the same program
*two* ways — on the TIR interpreter and through the whole pipeline to the
machine — and demands identical results, faults included. `compiler/tests/pipeline.rs`.
This is the correctness criterion; do not add a feature that only one side can
execute. It is why TIR was extended for `dyn` rather than emitting vtables
behind TIR's back.

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

**5. Prose style.** Specification text and code comments both cite the section
they implement (`Ch. 3 §2.2`, `AM §3.4`, `TIR §6.2`). Comments say *why*, not
*what*. Neither hedges. Where something is missing, it says it is missing and
what it is waiting for. Match this; it is most of what makes the documents
usable.

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

---

## 10. Where to go next

Three coherent directions, in no forced order:

**A. The library chapter (Ch. 5) as specification.** Strings and text
encoding (AM §5's deferred question), `Box` and an allocator, `Iterator`
adaptors, `checked_*`, the `?` operator and the trait describing what it
propagates. Everything it rests on is now defined, and writing the spec ahead
of the implementation has repeatedly reduced the number of decisions made
blind.

**B. Backend quality.** A register allocator is the biggest single win —
every value currently lives in a stack slot, so the generated code is
correct and embarrassing. Then a peephole pass, a TIR canonicalizer, and
expansion for `div`/`rem`/shifts. `mul` expansion is blocked on **G6.6**: TIR
has no widening multiply, though TRISC-27 now provides `mulh`.

**C. Generic traits.** §6 above says what it needs. It closes Ch. 4 and
unblocks `From`/`Into`.

Smaller, self-contained: `IntoIterator`; `FnOnce`; capture by place rather
than by variable (Ch. 4 §4.4 says place, the implementation does variable);
associated types in generic impls; `F_ILLEGAL` / `F_ADDRESS` fault codes,
which **G0.2a** reports the AM as lacking.

---

## 11. Known-wrong or under-tested

- Reading a non-copyable *field* moves the whole local, where Ch. 3 §1.3 says
  only that place moves. Conservative.
- An enum's payload is not dropped by variant; the enum's own destructor runs.
- A destructor that moves a field out of `self` is not detected, so that field
  is dropped anyway.
- A generic body is checked at instantiation, not once against its bounds.
  The *bound* half of Ch. 4 §2.2 holds — a failed bound is reported at the
  call site — but a generic function never called is never checked. This is
  the C++ failure mode Ch. 4's Appendix B claims is removed by construction,
  and it is removed at the call site only.
- Region inference does not exist. A returned borrow must be rooted
  syntactically at a parameter. Every program accepted would be accepted by
  full region inference; some rejected ones would not be, and those are the
  ones needing written lifetimes.
- Diagnostics print mangled names: `Pair.t27.t9`, not `Pair<t27, t9>`.
