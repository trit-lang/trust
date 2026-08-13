# Specification gaps and derived decisions

| | |
|---|---|
| **Status** | Living document, tracks `spec/` at Draft 0.1 |
| **Purpose** | Every place the implementation had to decide something `spec/` does not say |

The implementation follows `spec/` wherever `spec/` speaks. This file records
the places it does not, so that the decisions below are visible, reviewable,
and easy to reverse — each one is a question for the spec authors, not a
settled matter. Items marked **blocking** stop a component from being built at
all.

---

## 0. Missing documents

**G0.1 — `spec/00-abstract-machine.md` — RESOLVED.** The AM is present and
normative. It settled the fault code list, the alignment table, the division
tie-break and its symmetry identities, and the shift range rule; the entries
it closed are marked resolved below rather than deleted, so the history of
what was guessed stays auditable. Its worked identities (Appendix A) and the
consequences it draws in §1.2 are now a test suite of their own,
`core/tests/am.rs` — 21 tests, all passing against the arithmetic core as it
was written before the document arrived.

**G0.2 — the ISA — RESOLVED as to specification, open as to review.**
`spec/isa/` now holds both documents: `trisc-27-0.1.md` (the instruction set)
and `assembly-0.1.md` (the source language). Between them they answer all ten
items of Assembly §6.2, define the `tritium0` calling convention that TIR §7
defers to this family, and define the two memory-mapped character ports AM §5
promises. The assembler, a backend and the `tritium` VM are therefore all
unblocked.

The caveat is that TRISC-27 is a **design**, not a transcription: every one of
its choices — 27 registers named by their field value, one-word fixed-width
instructions, the one-trit flavor field, three 7-trit branch displacements, the
negative-address device region, the register roles of the ABI — was made while
writing it and wants review before anything depends on it. The TIR interpreter remains
the executable oracle in the meantime.

**G0.2a — three machine conditions have no fault code.** TRISC-27 §5 records
the finding: a malformed instruction, an access outside the address space, and
a reserved I/O access all currently raise `F_TRAP`, whose AM §4 definition is
"explicit trap instruction". A program therefore cannot distinguish its own
`trap` from a jump into garbage.

*Suggested:* AM §4 grows `F_ILLEGAL` (malformed instruction or reserved
encoding) and `F_ADDRESS` (access outside the address space). Adding fault
codes is the AM's decision, so the ISA document reports rather than invents.

**G0.3 — Trust surface syntax — RESOLVED, and implemented.** The frontend in
`compiler/src/lang/` lexes, parses, type-checks and lowers to TIR, and a
Trust program now runs on the machine. What it covers is what the
specification covers: scalars, arrays, tuples, structs, enums, functions
(including body-less declarations), constants, and every control-flow form.
References, generics and strings are rejected with diagnostics that name the
chapter they are waiting for.


`spec/language/00-syntax.md` defines the lexical structure, items,
expressions, patterns and statements, and settles the four questions that were
genuinely open: `& | ^` stay reserved rather than becoming `tmin`/`tmax`/
`tmul`, comments are `//`, a function without a body is a declaration, and
three-way dispatch is `match` and nothing else. Like the ISA, it is a
**design** and wants review.

What it cannot reach still waits on unwritten chapters: strings and characters
(the library chapter, via AM §5's deferred text encoding), references and
borrowing beyond the spelling `&T` (Ch. 3), generics and traits (Ch. 4),
modules and visibility, and `for` loops, which need iteration and therefore
traits. A `.tr` "hello world" is expressible today only as an array of `t9`
code units — which is what the chapter's own appendix shows.

**G0.4 — Chapter 3 is written, and two earlier statements want updating.**
`spec/language/03-references.md` defines ownership, moves, destructors,
references, lifetimes, the borrow checker and slices. It is a **design** like
the ISA and the syntax chapter, and it makes four choices worth review:
lifetimes follow Rust (they are erased, not instantiated, so nothing about
them waits on Chapter 4); the borrow checker is non-lexical; destructors are
`fn drop(self: T)` until `impl` blocks exist; and `Box` is left to the library
chapter.

Two consequences for documents already written:

- **Ch. 2 §6 understates the reference niche.** It says a reference excludes
  the null address, "at least 1 niche". Ch. 3 §2.5 promises that *every*
  non-positive value is invalid, which is (3²⁷+1)/2 niches, because an address
  is a signed word and memory is non-negative. The implementation already
  models it this way. Ch. 2 §6 wants one line changed.
- **Ch. 2 §3's fused bounds check is wrong.** `tmin(sign(i), i <=> N)` is −1
  for an in-bounds index as well as an out-of-bounds one, so it cannot
  distinguish them. Ch. 3 §5.5 records the correction and gives a fusion that
  works. The sentence is marked informative and the normative requirement is
  unaffected, so nothing was ever mis-compiled — the frontend emits two
  comparisons.

**G0.5 — Ch. 3 is implemented.** References, exclusive references, dereference
and auto-dereference, slices and their fat pointer, the array-to-slice
coercion, bounds-checked slice indexing, and lifetime syntax (parsed and
erased) all work end to end.

Ownership is implemented: the structural copy rule of §1.2, move tracking
through branches and loops, destructors with their drop order, and drop flags
where a branch leaves ownership undecided. A destructor's `self` is not
dropped as a whole, which is how §1.4's recursion is closed.

Borrowing is implemented as §4.2 describes it. A loan records the place it
covers, whether it is exclusive, and the statement it dies at; two loans
conflict when one place is a prefix of the other, with an index matching any
element. Loans are non-lexical — a borrow dies at its last use, so §4.2's own
example, where `p.x` is written after the last use of a shared borrow of `p`,
is accepted. Reading, writing, moving and re-borrowing are each checked
against the live loans.

A reference may be returned under elision rule 2 of §3.3: exactly one
reference among the parameters, and the result borrows from it. The loan on
the argument is then extended to the last use of whatever the call was bound
to, so the caller may not disturb the referent while the result lives. A
function that returns a reference and has no reference parameter is rejected,
because the reference could only point into a local; so is one with several,
because elision cannot choose between them. Rooting is checked syntactically:
a returned borrow must be rooted at a parameter.

Regions are not inferred, so what §4.1 states as a subtyping relation is
approximated by that syntactic rule. Every program it accepts is one a full
region inference would accept; some it rejects would be accepted, and those
are the ones needing written lifetimes, which arrive with Ch. 4.

Three further approximations, all conservative:

- Reading a non-copyable *field* moves the whole local, where §1.3 says only
  that place moves.
- An enum's payload is not dropped by variant, so a droppable value inside an
  enum payload is not dropped. The enum's own destructor still runs.
- A destructor that moves a field out of `self` is not detected, so that field
  is dropped anyway.

The chapter itself is unchanged: this is a staged implementation of a complete
design, and each stage refuses what it cannot yet verify.

**G0.6 — Chapter 4 is written, and it resolves one earlier ambiguity.**
`spec/language/04-generics.md` defines traits, impl blocks, methods, generic
type/lifetime/const parameters, bounds and `where` clauses, trait objects,
closures, the language's own traits, `for` loops, and `derive`. It is a
**design** like the ISA, the syntax chapter and Ch. 3, and it makes four
choices worth review, all four picked deliberately over the Rust-shaped
alternative where they differ:

- **There is no `PartialOrd` above `Ord`.** Rust's ordering hierarchy is
  shaped by floating point's `NaN`, and this language has no floating point.
  `Ord::cmp(&self, &Self) -> trit` is the base trait; genuinely partial orders
  get a separate, unrelated `PartialOrd::partial_cmp -> Option<trit>`, which
  is one tryte by Ch. 2's own appendix. No operator calls it.
- **Operator traits carry one method, and the overflow flavors of Ch. 1 §4 do
  not enter the trait system.** `+.wrap` applies to Ch. 1's integer types and
  nothing else, because `wrap` means "into this width" and a user type has no
  width. Built-in indexing likewise does not go through `Index`, so Ch. 3
  §5.5's bounds check stays a language guarantee rather than a library
  convention.
- **Generics monomorphize; `dyn Trait` is a fat pointer.** Data pointer then
  vtable pointer, 6 trytes, alignment 3 — the same shape as Ch. 3 §5.2's slice
  reference and for the same reason. Both words are strictly positive, so both
  carry Ch. 3 §2.5's niches; the compiler uses the data pointer's.
- **Closures are fully defined but cannot be returned.** `Fn`/`FnMut`/`FnOnce`,
  capture inference and anonymous types are all specified; returning a closure
  needs `impl Trait` in return position or `Box<dyn Fn…>`, and both wait for
  an allocator. A consequence worth noting: `move` is not provided and is not
  missed, because a closure that cannot escape never needs to force capture by
  value.

One consequence for a document already written:

- **Ch. 3 §1.2's note on `Copy` is self-inconsistent.** It says the structural
  copy rule will be restated as a trait "automatically derived… exactly as
  Rust's `Copy` is", but Rust's `Copy` is *opted into*: a struct of integers is
  not `Copy` until someone writes the derive. Only the automatic reading keeps
  the sentence immediately after it, which promises the restatement changes
  which programs compile "in no way at all". Ch. 4 §5.1 takes the automatic
  reading and says so; opting out is `impl !Copy for T`, the only negative impl
  in the language. Ch. 3 §1.2's note wants half a line changed.

Nothing in Ch. 4 is implemented yet. The frontend still rejects `trait`,
`impl`, `for`, `dyn` and `<…>` type parameters with the diagnostics Ch. 0 §1.3
requires, which now name a chapter that exists.

**G0.3a — the language chapters are not where Naming §2 says.** Naming §2's
layout puts the chaptered language specification under `spec/language/`, but
Ch. 1 and Ch. 2 live at `spec/01-types.md` and `spec/02-composites.md`. The
new chapter follows the layout document (as `spec/isa/` does); moving the
existing two is the author's call, not the implementation's.

---

## 1. Numeric notation

**G1.1 — Heptavintimal digit values.** Naming §3 and Types Ch. 1 §3 give the
alphabet (`0-9`, `A-Q`, 27 characters, "1 digit = 3 trits") and one example
(`0h2C9`) with no value; the AM does not mention heptavintimal at all. Two
mappings are possible: each character denotes an *unsigned* 0…26, or each
denotes a *balanced* 3-trit group, −13…+13.

*Decision:* balanced, `value = index − 13`. Heptavintimal is described as
playing "the role hex plays in the binary world", and hex's defining property
is that it is digit-exact over the underlying radix — which for balanced
ternary requires balanced digits. It is also the only reading under which a
`0h` literal can denote a negative number without a sign, matching `0t`.

*Consequence, and the reason this needs confirming:* `0hD` is zero and `0h0`
is −13. Leading `0`s are not neutral in heptavintimal.

**G1.2 — `0t` is ambiguous between two literal forms.** `0t` is both the
balanced-ternary prefix (`0t1T0`) and the `trit` literal for zero (`-1t`,
`0t`, `1t` — Types Ch. 1 §2). `0t1` could be either.

*Decision:* maximal munch. `0t` followed by one or more of `0 1 T _` is a
balanced ternary literal; a bare `0t` is the trit zero. So `0t1` is the
integer 1, and the trit zero must not be followed by trit-string characters.

**G1.3 — Trit-string case.** AM §1.1 fixes `T` for −1. Lowercase `t` is
accepted on input as a synonym and normalized to `T` on output.

---

## 2. Arithmetic

**G2.1 — The `.flag` overflow trit for `mul` and `shl`.** AM §3.1 establishes
that the machine provides "an overflow-flag-producing form for efficient
checked lowering" but does not say what the flag holds; TIR §3 calls it an
"overflow trit"; TIR §6 says "carry out of a part is the overflow trit of that
part's `add.flag`", which defines it for `add` (and by symmetry `sub`) but not
for `mul`/`shl`, whose exact result can exceed the width by more than one
trit.

*Decision:* the overflow trit is the **direction** of the overflow: `+1` if
the exact result exceeded MAX, `−1` if it fell below MIN, `0` otherwise. For
`add`/`sub` this is exactly the carry out of the top trit, so the §6 carry
chaining works as written, and the definition extends to every flavored
operation without a special case.

**G2.2 — Ties in division — RESOLVED.** AM §3.2 fixes round-to-nearest with
ties away from zero, and justifies the tie-break by the symmetry identities
`div(−a,b) = div(a,−b) = −div(a,b)` and `rem(−a,b) = −rem(a,b)`. Those
identities are tested directly (`core/tests/am.rs`).

**G2.3 — Shift amount type.** AM §3.3 settles the *behavior* — "shift amounts
k are values in 0 … n−1 for an n-trit operand; k outside that range is a
trapping fault (not masked, not undefined)" — but TIR §3.1 writes
`shl<fl> tN %a, %k` without giving `%k` a type.

*Decision:* `%k` is an operand of the same `tN` as `%a`, so the instruction
stays uniformly typed. Negative amounts are representable and fault like any
other out-of-range amount.

---

## 3. Faults

**G3.1 — The fault code list — RESOLVED.** AM §4 tabulates exactly five codes
for draft 0.1: `F_OVERFLOW`, `F_DIVZERO`, `F_SHIFT`, `F_ALIGN`, `F_TRAP`. The
implementation now carries that list and nothing else. (An earlier guess added
`F_BOUNDS` and `F_ASSERT`; both are gone. `F_ASSERT` was the wrong name for
what AM §4 calls `F_TRAP`.)

**G3.2 — Bounds checks have no code.** Composites Ch. 2 §3 requires that an
out-of-bounds access "faults or panics per the safety chapter", but AM §4 has
no bounds-specific code and the safety chapter does not exist.

*Decision:* nothing is implemented that needs one yet; when the frontend
arrives, a bounds violation will raise `F_TRAP` unless the AM grows a code for
it. Recorded so the choice is not made silently later.

---

## 4. Memory and layout

**G4.1 — Alignment beyond the word.** AM §2.3 tabulates natural alignment for
1…9 trits (1 tryte) and 10…27 trits (3 trytes), and stops there. Legalization
may introduce widths above 27.

*Decision:* natural alignment is the smallest power of three at least the
access's size in trytes (`⌈N/9⌉`). This reproduces both AM rows exactly —
note that a `t18` occupies two trytes but still aligns to three, because the
table is stated per *width*, not per size — keeps alignment a power of three
(Composites Ch. 2 §1), and extends past the word without a special case.

**G4.2 — Uninitialized global storage.** TIR §4.4 makes reading uninitialized
`slot` storage yield poison but says nothing about a `global` with no
initializer.

*Decision:* the same — `= zeroinit` zero-fills, an omitted initializer leaves
the storage uninitialized and reading it yields poison.

**G4.3 — Poison propagation.** TIR §4.4 adopts "the standard modern" poison
model by reference without restating it. Implemented as: poison propagates
through every value-producing instruction; branching on poison is UB; loading
or storing *through* a poison address is UB; `select3` on a poison selector
yields poison rather than UB, since it is not a branch.

---

## 5. TIR syntax

The textual format is "the canonical serialization" (TIR §8) but is only ever
shown by example. These forms are inventions filling holes in the examples.

**G5.1 — Naming a global's address.** TIR §1 defines `global @name : tryte[N]`
but no instruction or operand form ever mentions a global again, leaving the
storage unreachable.

*Decision:* `@name` is an operand of type `ptr`, denoting the global's base
address. This is the minimum addition that makes globals usable, and it needs
no new instruction.

**G5.2 — Function declarations.** TIR §1 lists "function declarations —
signature only, body external" without syntax. A trailing `;` is unavailable,
because `;` starts a comment in every listing in the spec.

*Decision:* a signature with no `{ … }` body is a declaration:
`fn @g(%a: t9) -> t9`.

**G5.3 — Global initializers.** `= <initializer>` is never given a grammar.

*Decision:* `= zeroinit`, or `= [v₀, v₁, …]` with exactly one value per tryte,
lowest address first (little-trytean, so index 0 is the least significant
tryte of a multi-tryte scalar). An absent `= …` means uninitialized (G4.2).

**G5.4 — `br2` operand order.** TIR §3.6 says the printer displays a `br3`
with two identical destinations as `br2 %t, ^then, ^else`, without saying
which arms `then` covers.

*Decision:* `then` is the `+1` arm, `else` is the `−1` *and* `0` arms —
matching `bool`, whose storage encoding is `false = 0`, `true = 1` (Types
Ch. 1 §2). Parsing `br2` produces the equivalent `br3`; the two forms are the
same instruction.

**G5.5 — Omitted operand types.** TIR §1's own example writes
`%s = cmp %x, const t27 0` with no type on the instruction, while the appendix
writes `cmp t27 %pos, %target` with one.

*Decision:* both parse. When the type is omitted it is taken from the first
constant operand, which is the only place it can come from without a typing
pass; if no operand supplies one, the instruction is rejected. The printer
always emits the explicit form.

**G5.6 — Comment character.** Never stated; `;` is used for comments
throughout the spec's listings and is implemented as such.

---

## 6. Legalization

TIR §6 states legalization's contract — arbitrary `tN` in, legal-set widths
out — and names the two techniques, but the details below are not written
down anywhere.

**G6.1 — `t1` is a condition type, not an arithmetic width.** `cmp` yields
`t1` by definition (TIR §3.3, AM §3.5) and `br3` consumes one, but TIR §7 does
not require a target's legal set to contain 1 — the reference target lists it,
a hypothetical one need not.

*Decision:* the invariant legalization establishes is that every *arithmetic*
operand and result has a width in the legal set, while `t1` survives as a
comparison result, a `.flag` overflow trit, or a branch/select selector.
Crossing between the two is explicit: `widen` on the way up, and `cmp x, 0` on
the way down — by *sign*, not by residue, since a `trunc` would be wrong for
any value outside −1…1.

**G6.2 — Renormalization cannot use `trunc`.** After a promoted wrapping
operation the result must be wrapped into the *narrow* width's range, but
`trunc tL -> tw` would reintroduce the illegal width the pass exists to
remove.

*Decision:* renormalize at the legal width with `tmul` against a mask of `w`
ones. `tmul(x, 1) = x` and `tmul(x, 0) = 0`, so the mask keeps the low `w`
trits and clears the rest, which is exactly the symmetric residue mod 3^w
(AM §3.1) — trit-wise, carry-free, and expressible without leaving the legal
set.

**G6.3 — Memory access widths are not promoted.** A `load t9` moves one
tryte; whether the arithmetic unit has a 9-trit adder is unrelated. TIR §6
speaks of "value types and operation widths" without separating the two.

*Decision:* `load`/`store` keep their access width, and the loaded value is
converted into the arithmetic width immediately after. Only an access wider
than the widest legal width is an error, and that needs expansion, not
promotion.

**G6.4 — Overflow detection has a width requirement.** Promoting `mul.trap`
from `tw` to `tL` can only detect overflow if the exact product fits, i.e.
`L ≥ 2w`; `.wrap` has no such requirement, because the low `w` trits of the
exact product survive any wider wrapping.

*Decision:* the pass reports the case it cannot synthesize rather than
emitting an operation that silently fails to trap.

**G6.5 — A wide value cannot cross a function boundary (blocking for
expansion).** A `t54` parameter on a `t27` target has to arrive in two
registers, or through a hidden pointer. TIR has neither multiple return
values nor an `sret` convention, and the calling convention is a
target-description property (TIR §7) whose reference value, `"tritium0"`, is
"defined in the target's own doc" — which is G0.2, the missing ISA spec.

*Decision:* expansion handles wide values *inside* a function body and
reports wide parameters and returns instead of inventing an ABI.

*Half resolved.* TRISC-27 §6.3 now defines the ABI: a value wider than a word
occupies consecutive argument registers, least significant word first, and
results come back in `a0` then `a1`. So the machine can pass a `t54`. TIR
cannot yet *express* it — `ret` carries one value and there is no `sret` form
— so the remaining work is a TIR change, not a missing convention.

**G6.6 — Expansion of `mul` needs a primitive TIR does not have.** Multiplying
two `k`-part values by the schoolbook method needs the *full* product of two
parts, which is `2L` trits wide — and `L` is already the widest legal width.
Binary ISAs solve this with a widening multiply (`mulhi`/`umulh`); TIR §3.1
has only `mul<fl>`, which returns a same-width result, and `.flag` reports
only the *direction* of the overflow, not the high half.

*Decision:* reported, not approximated.

*Half resolved.* TRISC-27 §4.1 now provides `mulh`, which yields the high 27
trits of the 54-trit product — the machine has the primitive. TIR still does
not: §3.1 has only same-width `mul`, and `.flag` reports the direction of an
overflow rather than its high half. Adding a widening multiply to TIR §3.1 is
the remaining change, and it is now a change with a known hardware
counterpart rather than a speculative one.

---

**G6.7 — TIR could not hold a pointer in memory.** §3.4's `load`/`store` take
an integer width, and §5 deliberately has no integer↔pointer conversion.
Between them, draft 0.1 as written cannot spill a reference, put one in a
struct, or represent a fat pointer — which makes Language Ch. 3 inexpressible.

*Decision:* `load` and `store` accept `ptr` as an access type. This is **not**
the conversion §5 declines to define: nothing about an address is exposed, and
provenance travels with the value. The interpreter keeps a stored pointer's
provenance beside the trytes rather than in them, so that loading it back
yields the same allocation; writing integers over that storage destroys the
pointer, as it should. On the machine a pointer is a word and `ld.word` moves
it.

One consequence the implementation had to follow: an aggregate must be copied
**field by field, not tryte by tryte**, or a pointer inside it arrives without
its provenance. An enum's payload varies by variant, so its storage is still
copied as trytes — a reference inside an enum payload therefore loses
provenance in the interpreter, which is a limitation of the checking, not of
the machine.

*Suggested:* TIR §3.4 gains a sentence permitting `ptr` as an access type, and
§5 a note distinguishing that from the integer↔pointer casts it declines.

---

## 7. Layout (Language Ch. 2)

**G7.1 — Unassigned discriminants continue from the previous variant —
RESOLVED by the chapter's own appendix.** §5.1 says "unassigned discriminants
default to 0, 1, 2, … in declaration order", which could mean the positional
index. The appendix settles it: it lists `enum { A=-1, B, C=1 }` as
trit-shaped with size 1, and that only holds if `B` is 0 — the positional
reading would make `B` = 1, colliding with `C` and making the enum ill-formed.
So an unassigned discriminant is the previous one plus one, as in Rust.

**G7.2 — `repr(lang)` field order.** §1 permits the compiler to "order, pad,
and pack fields arbitrarily" and §4 notes the obligation is mild.

*Decision:* fields are placed most-aligned first (a stable sort by descending
alignment). With only two scalar alignments this removes every removable
pad, and the appendix's own counter-example — `struct { a: t9, b: t27 }`
staying at size 6 — still holds, because size must be a multiple of
alignment.

**G7.3 — Which niche an enum uses, and which values encode it.** §6 guarantees
the *outcomes* (`Option<&T>` pointer-sized, `Option<bool>`/`Option<trit>` one
tryte, nesting free) without specifying the rule that produces them — and a
count of niches is not enough to generate code against, which needs to know
*which* patterns are invalid.

*Decision:* an enum is niche-encoded when exactly one variant carries a
payload, every other variant is fieldless, and the payload has at least as
many niches as there are fieldless variants. The encoding consumes that many
niches and the rest stay available to an enclosing type, which is what makes
guarantee 3 (nesting) fall out rather than being special-cased.

The niche-bearing scalars — `bool` and `trit` — have a contiguous valid
range, so the invalid values are taken from above that range first and then
from below it: `Option<trit>`'s `None` is the tryte 2. A reference's valid
range is treated as 1…MAX, which under-counts its niches but puts the *null*
address first among them, which is the one every `Option<&T>` uses. The
untagged variant is recognized by elimination rather than by a stored value,
so its arm is tested last whatever order it was written in.

**G7.4 — Padding contributes no niches.** §1 says padding trytes have
unspecified contents. Unspecified means every pattern is acceptable, so
padding cannot be used to hide a discriminant.

*Decision:* implemented that way. A struct's niches are exactly its fields'.

**G7.5 — `repr(lang)` enums that cannot be niche-encoded.** §5.1 specifies the
tag layout for `repr(linear)` only, and leaves `repr(lang)` unspecified.

*Decision:* fall back to the `repr(linear)` shape — leading tag, then the
payload union at its natural alignment — with the tag narrowed to `t9`
whenever every discriminant fits.

---

## 8. Not yet implemented (no gap — just unbuilt)

Specified well enough to build, simply not built yet:

- **Expansion of `div`, `rem` and the shifts** — multi-part division and
  multi-part shifting are unwritten. (`mul` is different: see G6.6, where the
  instruction set is the obstacle.) Reported rather than mis-compiled.
- **Optimization passes** — the canonicalizer that "recognizes and re-fuses"
  two-valued predicate patterns (TIR §3.3), constant folding (which `Bt`
  already supports at every width), and dead-code elimination. Legalization
  emits obvious redundancies — a mask after an operation that provably cannot
  overflow, a `select3` merging two carries at most one of which is nonzero —
  that a canonicalizer should clean up.
- **Nothing of Ch. 2 remains unimplemented.** Structs, tuples, enums with
  payloads and explicit discriminants, both `repr`s, field access, variant
  patterns and niche optimization all run end to end. `checked_*` is the one
  method of Ch. 1 §4 still missing, because it returns `Option<T>` and
  generics are Ch. 4.
- **Indirect calls** (TIR §3.7) — rejected with a diagnostic that says they
  are reserved, which is as far as "parsed but reserved" can go before the
  function-pointer chapter exists.
- **Concurrency** — AM §2.4 reserves it explicitly; the interpreter is the
  single-threaded, sequentially consistent machine that section defines.
