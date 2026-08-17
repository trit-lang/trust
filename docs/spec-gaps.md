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

- **Ch. 3 §1.2's note on `Copy` was self-inconsistent, and is now fixed.** It
  said the structural copy rule would be restated as a trait "automatically
  derived… exactly as Rust's `Copy` is", but Rust's `Copy` is *opted into*: a
  struct of integers is not `Copy` until someone writes the derive. The two
  clauses cannot both hold. The deciding argument is the implementation: the
  structural rule is already normative and already built, so the automatic
  reading costs zero lines and the opt-in reading would repeal a rule that is
  in force. Ch. 3 §1.2's note has been reworded to say "not Rust's `Copy`" and
  to point at the opt-out it lacked; Ch. 4 §5.1 supplies that opt-out as
  `impl !Copy for T`, the only negative impl in the language.

  **Left open:** Rust's opt-in exists because `Copy` is an API promise — a type
  that becomes copyable by accident stops being copyable the day a field
  acquires a destructor, silently turning every caller's copy into a move. That
  cannot bite a one-file draft, and it will bite once modules exist. The
  intended answer, recorded here rather than specified: a *published* type's
  copyability becomes part of its signature and must be written there. That
  rule can be added later without invalidating any program that predates
  modules, which is why it is deferred rather than guessed at now. Ch. 3 §1.2's note wants half a line changed.

**G0.7 — Ch. 4's first block is implemented: traits, impls and methods.**
Trait declarations with required and provided methods, supertraits, inherent
and trait impls, the four receiver forms, associated functions, method
resolution with auto-deref and auto-ref, elision rule 3, and `impl Drop for T`
all work end to end.

The implementation is a desugaring, and that is the interesting part. An impl
block's method becomes an ordinary function named `Type.method` with `Self`
substituted away and the receiver an ordinary leading parameter; `p.area()`
becomes `Point.area(&p)`, with as many dereferences inserted as Ch. 3 §2.3
calls for. Argument checking, loans, moves, drop flags and the borrow checker
then apply unchanged, because there is nothing new for them to see — which is
the claim Ch. 4 §1.2 makes when it says an impl block is a naming construct.
`impl Drop for T` lands on the same `drop.T` key Ch. 3 §1.4's `fn drop(self: T)`
already used, so both spellings produce identical code, as §1.4 promised.

**G0.8 — Ch. 4's second block is implemented: generics and monomorphization.**
Type parameters on functions, structs and enums; bounds, inline and in a
`where` clause; inference from arguments and from the expected type; and
monomorphization with a termination limit — all work end to end.

The implementation turns on one decision: **a type parameter is not a kind of
type.** There is no `Ty::Param`. A parameter is a name that an *environment*
maps to a concrete type, so lowering a generic body is lowering the same AST
under a different environment, and no AST is ever rewritten. A generic struct
or enum becomes an ordinary nominal type under a mangled name the first time
it is applied, so the layout engine, the drop machinery, the borrow checker
and code generation never learn that generics exist. That is also why no
generic construct reaches TIR, which the test suite asserts directly.

The payoff worth naming: `enum Opt<T> { None, Some(T) }` written in the
language gets every layout promise Ch. 2 §6 and Ch. 3 §2.5 make about
`Option`, because nothing told the compiler it was special — one tryte for
`Opt<trit>`, one word for `Opt<&t27>`, a word and a tag for `Opt<t27>`.

Three limits, all named in the diagnostics:

- **No `::<>`.** Ch. 4 §2.3's explicit instantiation is not implemented, so a
  parameter that appears in neither an argument nor the expected type cannot
  be determined and the diagnostic says which parameter it was. Const generic
  arguments (§2.4) are parsed and rejected for the same reason.
- **Generic impls work; associated functions on them do not.** `impl<T> Opt<T>`
  and `impl<T> Trait for Boxed<T>` both work: the impl's type parameters become
  the method's, so a method of a generic type is an ordinary generic function
  keyed by the base type, instantiated with the arguments the receiver's own
  instantiation was made with — recovered from a table, since a mangled name
  cannot be read back. What does not work is `Opt::<t27>::new()`, an associated
  function with no receiver to infer from, which needs `::<>`.
- **A generic body is checked at instantiation, not once.** Ch. 4 §2.2 makes
  the *bound* half normative — "an instantiation that fails a bound is
  rejected at the call site, not inside the body" — and that half holds: bounds
  are checked where the call is written, with a diagnostic naming the call, the
  parameter and the trait. The other half, that a generic body compiles once
  against its bounds, would need a checker that runs on abstract types. Today a
  body is checked once per instantiation, so a generic function that is never
  called is never checked, and one that uses a method its bounds do not grant
  is caught at the call rather than at the definition. This is the C++ failure
  mode Appendix B claims is removed by construction, and it is removed at the
  call site only. Recorded, not glossed.

Two fixes fell out of building it, both pre-existing:

- **`while let Some(x) = cell.borrow_mut().pop()` held the borrow for the whole
  loop body**, and the body queued more work into the same cell. A panic, not a
  wrong answer, and the classic `RefCell` shape.
- **A `match` scrutinee did not auto-dereference.** Ch. 3 §2.3 gives the rule
  for `.`, and a scrutinee reads the same way; without it a `&self` method
  could not match on `self`, which is most of what a method on an enum does.

**G0.9 — `derive`, and the comparison operators on user types.**
`#[derive(Eq, Ord, Clone)]` works, `<` `<=` `>` `>=` `<=>` on a nominal type
call `Ord::cmp` and `==` `!=` call `Eq::eq`, and a type with neither gets a
diagnostic naming the trait and offering the derive.

**§5.3.3's codegen guarantee holds, and is asserted rather than claimed.** A
derived `cmp` over scalar fields compiles to one `cmp` per field, one `sel3`
per field after the first, and **no branch at all** — the test reads the
emitted assembly between `f.V.cmp.entry:` and its `ret` and fails if a `br3`
or a jump appears in it. The combination of two field results is "the first
that is nonzero", which is exactly `sel3 rd, c1, c1, c2, c1`.

That guarantee is why the derived functions are emitted as **TIR rather than
as source**: the language has no expression that spells a three-way select,
so a derived `cmp` written in Trust would have had to branch.

Limits: deriving for a generic type is not implemented (the derived impl needs
the bound §6 puts on every parameter); nor is deriving for an enum with a
payload (§6 orders it by discriminant and then by payload, and only the first
half is built). Both say so.

**A pre-existing bug this uncovered.** Comparing two enums produced ill-formed
TIR — `cmp` applied to a pointer — rather than a diagnostic, because every
enum is passed by address and the built-in comparison path never checked. It
is now either a call to `Ord::cmp` or an error that names the missing trait.

**G0.11 — `dyn Trait` is implemented (Ch. 4 §3).** Trait objects, their fat
pointer, their vtables, object safety, and dispatch through an indirect call
all work end to end — resting on G0.10's TIR extension, which exists for
exactly this.

`&dyn Trait` is (data pointer, vtable pointer): 6 trytes, alignment 3, the
same shape as Ch. 3 §5.2's slice reference and with the data pointer first for
the same reason. The vtable is an ordinary TIR global holding `addr @…` items
in §3.3's order — size, align, drop, then one address per object-safe method,
**the supertraits' first**. One list produces both the table and the dispatch
index, which is the only way the two can be guaranteed to agree.

Object safety is checked where `dyn Trait` is *written*, not where a value is
coerced to it, which is what §3.4's "may be used as `dyn Trait` only if" says.
The diagnostic names the reason: a parameter mentioning `Self`, a missing
receiver, a returned `Self`, or the method's own type parameters.

Two gaps this uncovered, both pre-existing:

- **`.len()` was never implemented**, although Ch. 3 §5.4 requires it. It was
  listed among the built-in methods and had no case, so it fell through to
  "not a method in this milestone". Slices and arrays both report their length
  now.
- **An unsized local was accepted.** `let x: dyn Shape = …` and `let x: [t27]`
  passed the frontend, because the size check ran on parameters and fields but
  not on `let`. Ch. 3 §5.1 has always said a dynamically sized type appears
  only behind a reference.

`Box<dyn Trait>` is not implemented and cannot be until there is an allocator,
which §3.1 already says. The representation above is the one it will use.

**G0.12 — closures are implemented (Ch. 4 §4).** `|x| x * k`, capture
inference, the anonymous type, `Fn`/`FnMut`, and `impl Fn(…)` in argument
position all work end to end.

A closure becomes two things: an anonymous struct holding one reference per
captured place, and an ordinary function whose body is the closure's with
every capture rewritten to a field of that struct. Neither is nameable from a
program, and everything downstream sees a struct and a call — which is why
capture is checked by Ch. 3 §4's borrow checker unchanged, including its
non-lexical rule. §4.4's own example is rejected exactly as written, and the
same program with the closure's last use moved earlier is accepted.

`impl Fn(A) -> R` in argument position is desugared to a named type parameter
with a bound that carries the signature, so a closure argument monomorphizes
like any other type argument (§2.7). The bound then supplies the closure's
parameter and result types, which is why `|x| x + k` needs no annotations;
where nothing in the context says, the result type is read off the body.

Two limits:

- **`FnOnce` is not implemented.** Every capture is by reference — shared, or
  exclusive when the body writes it. A closure that moves a capture out is
  §4.3's `FnOnce`, and needs a move analysis of the closure body this
  milestone does not do. `Fn` and `FnMut` are checked against each other, so
  passing an `FnMut` where `Fn` is wanted is rejected with the reason.
- **Capture is by variable, not by place.** §4.4 says a closure using `p.x`
  borrows `p.x` and leaves `p.y` free; this one borrows `p`. Conservative in
  the right direction, and it costs only programs that use disjoint fields of
  one struct in a closure and outside it.

`move` is not provided, and §4.5 explains why it is not missed: its purpose is
to let a closure outlive its captures' scope, and a closure that cannot be
returned never does.

**G0.13 — associated types, `for` loops, and `Option`/`Result` (Ch. 4 §§1.7,
5.7, 5.8).**

An associated type is declared `type Item;` in a trait and chosen
`type Item = t27;` in an impl, and `Self::Item` resolves through the choice.
Checking an impl against its trait therefore had to move from comparing types
*as written* to comparing them *after resolution*: the trait says
`Option<Self::Item>` and the impl says `Option<t27>`, and only resolution can
see that those agree. A generic impl is still compared as written, because its
parameters stand for nothing yet.

`for x in e { … }` is expanded in the **parser**, into exactly §5.7's
desugaring and nothing else, so no later pass learns it existed. The iterator
binding is named with a dot in it, which no Trust identifier may contain, so
it cannot shadow or be shadowed. `break` and `continue` therefore work in a
`for` because they work in the `loop` it becomes.

`Option` and `Result` are prepended to every file as source:

```
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
```

§5.8 says they are ordinary enums laid out by Ch. 2's rules with no special
case, and prepending their source is the most direct way to keep that honest —
`Option<&t27>` is one word because niche optimization applies to a type the
compiler was never told about, not because anyone arranged it.

Two limits:

- **No `IntoIterator`.** `for x in e` requires `e` itself to implement
  `Iterator`. The blanket impl and the array/slice iterators are the library's,
  and §5.7 already says the adaptors are.
- **No associated type in a generic impl.** What it chose would depend on the
  instantiation, which the current table (keyed by type name) cannot express.

**G0.14 — `::<>`, `impl !Copy`, associated constants, and an exact constant
evaluator.**

`f::<T>(…)` and `Option::<t27>::None` give type arguments in declaration
order, and any omitted are inferred (Ch. 4 §2.3). `impl !Copy for T` is the
one negative implementation the language has (§5.1), and it is the opt-out
Ch. 3 §1.2 said it lacked: a type with it moves, and so does anything
containing it. Associated constants are ordinary constants under a qualified
name (§1.7), which is all `Type::NAME` needs them to be.

**A pre-existing gap this uncovered: `const N: t27 = -9;` did not compile.**
The constant evaluator accepted integer literals and nothing else, so a
negative constant — a unary minus applied to a literal — was rejected outright,
as was any arithmetic. Ch. 0 §3.2 requires a constant to be "evaluated at
compile time, exactly, in balanced ternary — the same evaluation the assembler
performs". It now evaluates `+ - * / % << >>`, unary minus and casts, with
division being the AM's one division so that a folded constant is what the
machine would have computed: `8 / 3` is 3 and `8 % 3` is −1 in a constant
exactly as at run time.

**What is left of Ch. 4, and why.** Generic traits — `trait From<T>`, and the
blanket impl `impl<T, U: From<T>> Into<U> for T` that §5.6 presents `Into` as
coming from — are not implemented. Two things stand in the way, and neither is
small:

- A type may implement a generic trait many times, so a method key must carry
  the trait's arguments and resolution must pick by argument type. Today a
  method is keyed `Type.method` and there is one.
- A blanket impl is quantified over types that satisfy a bound, so deciding
  whether it applies is a search rather than a lookup. Every other impl in
  this compiler is found by name.

Doing the first half without the second would leave the hole exactly where the
chapter puts its example, so both are recorded rather than half-built. `Fn`,
`FnMut` and `FnOnce` are generic traits in §4.3's presentation, and they work
because they are not resolved as traits at all: a closure's call is a direct
call to the function its body became.

Two limits worth naming:

- **`Ord`, `Eq` and the operator traits are not wired to the operators.**
  A trait named `Ord` may be declared and implemented, but `<` on a user type
  still does not call it. Ch. 4 §5's lang items need generics first, since
  their blanket impls do.
- **No coherence check across impls of the same trait.** Overlap cannot arise
  without generics — two impls for the same type collide on the method name
  and are caught — so §1.8's overlap rule has nothing to check yet.

**G6.9 — narrowing a runtime value did not legalize, and now does.** A `t9`
is not a legal register width on `tritium`, so TIR §6 promotes it to a word;
but a *memory access type* is not promoted, because a `t9` in memory occupies
one tryte whatever the registers are. The legalizer's `coerce` could only
widen, so storing a promoted value back into a `t9` failed outright with
"cannot convert a value of type t27 to t9". Every runtime narrowing was
affected — `n as t9`, `a[i] += 10` on a `[t9; N]` — and the existing cast test
missed it because its operands were constants and were folded before
legalization ever saw them.

The fix is a third case in `coerce`: narrow with `trunc`. It is exact, because
promotion renormalizes into the narrow type's own symmetric range after every
operation, so the trits being dropped are already zero. The spec is unchanged;
this was an implementation gap, not a specification one.

**G0.10 — TIR 0.1 gains function addresses and indirect calls.** A spec
change, made deliberately and with the author's agreement, because dynamic
dispatch cannot be expressed without it.

The gap: a virtual table is a constant holding function addresses, and TIR's
global initializer was a list of tryte *values* — an address is not known
until the module is placed, so it could not be written. And `InstKind::Call`
carried a `String`, a name rather than a value, so dispatching through a table
could not be expressed either. Between them these made `dyn Trait` (Ch. 4 §3)
and closures (§4) unimplementable in TIR at all.

Two additions, both minimal:

- **§1.2, global initializers.** An initializer is now a list of items, each
  filling a known number of trytes: a literal fills one, and `addr @name`
  fills one word with the address of a module-scope symbol. That is a
  relocation — TIR says which symbol, the target decides the number.
- **§3.7, indirect calls.** `call %p(…)` was already "parsed but reserved" in
  draft 0.1, so the syntax was anticipated; it is now defined. The signature
  is not recoverable from a pointer, so the call site's own types *are* the
  signature, and calling through a pointer with a different one is UB.

§4's inventory therefore grows from four UB sources to five, and the fifth is
the only one that is not about data: calling through something that is not a
function's address.

The alternative considered and rejected was to emit vtables and indirect
calls in code generation only, leaving TIR unchanged. That would have put
them beyond the reach of every pass *and* of the reference interpreter —
and every end-to-end test in this repository works by running the same
program on the interpreter and through the whole pipeline and demanding
identical results. Dynamic dispatch is exactly where that check is worth
most, so the check was kept and the IR was extended.

The interpreter models a function's address as an allocation with no trytes,
so loading or storing through one is out of range, which is what §1.2 says.
`compiler/tests/pipeline.rs` builds a vtable, dispatches through it, and
requires both engines to agree.

**G0.15 — a review of the ten specification documents, and what execution
said about it.** Findings raised against `spec/`, each checked against the
implementation rather than argued about. Four were real errors, and two of
those had reached code.

**AM Appendix A.2 was wrong.** It gave `abs(x) = tmul(x, sign(x))`. `cmp`
yields one trit (AM §3.5) and widening is value-preserving, so an unsplatted
sign reaches a word as `0…0s`; `tmul` against it keeps the lowest trit and
zeroes the rest. Run on `trit-core`, the identity returns ±1 for every nonzero
input — never a magnitude. It needs a **broadcast**, which is what a binary
machine spells `x >> 31` when it writes branchless `abs`, so nothing was saved
by omitting it. `splat` is now an AM §3.4 primitive; it already existed in
`trit-core`, which is its own small piece of evidence that the spec was
missing something the implementation needed. `core/tests/am.rs` now tests the
identity both ways.

TRISC-27 §4.1 had inherited the error verbatim. It now gives `sub` + `sel3` —
the same two instructions, no branch, no new primitive — and says why `splat`
is not being added as an instruction until legalization needs a broadcast for
its own reasons.

**Heptavintimal digit values were never specified**, and the consequence was
undocumented: `D` is the zero digit, so `0hJ` is 6 but `0h0J` is −345 and
`0h00J` is −9822. Leading zeros are neutral in every other notation in this
project and in the binary world. Ch. 1 §3.1 now carries the full table and
says so in a warning; `0h2C9`, the original and only example, is −8050 and is
retained solely as that warning. G1.1 had recorded the decision; the spec had
not.

**Ch. 0 §6's grammar had fallen two chapters behind.** It admitted neither
`&mut` (Ch. 3 §2.1's other reference form) nor `Name<T>`, while Ch. 0 §7
claimed "the type grammar admits `Name<T>` so that adding them changes no
other rule". It now covers Ch. 3 and Ch. 4 — references, generics, traits,
impls, associated items, `dyn`, closures, `for`, the turbofish — because a
reader asking "can I write this?" should not have to read four chapters.

**Ch. 4 §6's own example was ill-formed** by Ch. 4 §1.6: `#[derive(Ord, Clone)]`
without `Eq`, where `Ord: Eq`. Deriving `Ord` now derives `Eq`, which is what
the implementation already did.

**Ch. 4 §3.3's drop sentinel was unsound.** A vtable's drop slot is 0 when the
type has no destructor, justified by "0 is not the address of anything" — but
ISA §1.3 starts execution at address 0, and a vtable holds *function*
addresses, which Ch. 3 §2.4's non-null rule says nothing about. ISA §2.2 now
**reserves the first word of memory**: it is the all-zeros word, which §3.4
makes a `nop`, so execution still begins at 0 and falls through. The
assembler emits it, so hand-written `.t27` files get it too. The ISA had
listed this as an open question and observed that reserving it later would
break nothing; this is that later, and the drop sentinel is the motivation
that decided it. As a side effect the language's non-null invariant becomes
hardware-checkable rather than only type-checked.

**Three gaps rather than errors**, all now written down: alignment above 27
trits was undefined (AM §2.3 now caps it at a word, with the reason —
expansion produces word-sized parts and a chain of word-aligned parts is
word-aligned); `trap`'s code field was described as three trits in one table
and as the 14-trit immediate a paragraph later; and `Into` was open to hand
implementation despite a blanket impl that §1.8 makes every such impl overlap.
`AddAssign`'s absence is now recorded in §7 with what it costs: `c[i] += x`
through a user `Index` whose `Output` does not copy.

**One recommendation reversed.** Ch. 3 §5.5 presented its corrected fused
bounds check as the thing to reach for. Counted out it is five instructions
and one branch against two comparisons and two branches, so it is *slower*,
and it also converts one class of out-of-range index into `F_OVERFLOW`. The
section now recommends the naive form and confines the fusion to a
compile-time-constant length, where `len − 1` folds.

**One trade-off that read as a free win.** Ch. 3 §2.5 argued the reference
niche from "memory occupies only the non-negative half" while ISA §2.2 puts
the device region at negative addresses. Both hold, and together they mean no
reference can ever point at memory-mapped hardware. The choice is still right
— the niches are worth more than `&`-able device memory in a draft with no
volatile model — but §2.5 now states the cost, since every other trade-off in
these documents is stated.

**And a process finding.** Naming §6 required a sweep of all citing documents
on any change to 00 or 01, but nothing equivalent between chapters, so five
corrections were living only at the correcting end. The rule now applies to
every document, with two mechanics: `Supersedes` / `Superseded by` rows in
each header table, and a correction written at *both* ends — the correcting
document saying what it corrects, the corrected one saying it was corrected
and pointing forward.

*The review's closing recommendation was to stop writing specifications and
build the VM. That work was already done — `tritium`, `trustc`, the
assembler and 297 tests — which is why every finding above could be settled by
running something rather than by reading again. The recommendation was right
about the method and wrong about the state: two of the four real errors were
found by executing the specification's own claims, which is exactly what it
predicted, and the machine that did it already existed.*

**G0.16 — three memory-safety bugs in drop glue, found by reviewing the
handoff document rather than by running anything.** All three contradicted
Ch. 3 Appendix B's "double free | a moved-out value is not dropped", and all
three were invisible because Ch. 3 §1.5 gives draft 0.1 no resources: with no
allocator, no file handle and no lock, dropping twice and not dropping at all
release the same nothing.

1. **A nested destructor ran twice.** `drop_at` called `drop.T` *and* then
   emitted the field drops, while `drop.T` already dropped its own fields —
   so any struct with a droppable field double-dropped it, no move required.
   `drop.T` is now the complete glue for T and the call site only calls it,
   which is also what Ch. 4 §3.3's vtable drop slot assumes: a caller holding
   only a pointer and that slot must be able to drop the whole value.
2. **Moving a non-copyable field out was not tracked.** `take(o.a); take(o.a);`
   compiled, and one value was dropped three times. Reading a place of
   non-copyable type moves it (Ch. 3 §1.2) and that is as true of a field as
   of a whole local; ownership is still tracked per local, so a move out of
   any part moves the whole — conservative where doing nothing was unsound.
   This also fixes Ch. 3 §1.4 item 3: a field the destructor moved out of is
   no longer dropped again.
3. **An enum's payload was never dropped.** A leak for any droppable value
   inside an `Option`. Dropping is now a dispatch on the discriminant, one
   comparison per droppable variant, with the niche-encoded untagged variant
   recognized by elimination — the same shape `match` already emits.

None of the three could have been caught by the differential invariant, which
takes the same TIR into both engines and therefore covers `TIR → machine`
only. All three were on the lowering side. `docs/status.md` §8.1 now says so.

**G6.10 — TIR §6's post-condition is now checked.** §6 says backends "may
assume legalized input and are not required to handle any other", which makes
an unlegalized module a licence for the backend to emit anything. Legalization
is incomplete — G6.6 blocks `mul`, and `div`, `rem` and the shifts are
unwritten — so that path is reachable rather than hypothetical.
`verify_legalized(module, target)` checks that every width a backend must
select a native operation for is in the target's legal set, and it runs at
both seams that depend on it plus in every end-to-end test.

`widen` and `trunc` are deliberately excluded: they are the bridge between a
legal register width and a memory access width, and TIR §6 does not promote
the latter because a `t9` in memory is one tryte whatever the registers are.
The check caught this on its first run, which is a fair test of the check.

**G4.4 — `Sized` and `?Sized` are not implemented, which is G0.14's limit
seen from another side.** Ch. 4 §2.5 gives every
type parameter an implicit `Sized` bound and `?Sized` to remove it. The
implementation has neither: a parameter behaves as `?Sized`, and the size
requirement is enforced at each *use* — a parameter of unsized type, a `let`
of one, a field of one, a read through a reference to one. `fn f<T: Shape>(x: &T)`
therefore accepts `T = dyn Shape`, which Rust rejects without `?Sized`.

*Decision:* sound but more permissive than the chapter, and the difference is
where the error appears — **in the body rather than at the call**, which is the
same failure mode G0.14 records for generic bodies generally. Both have one
root: there is no `Ty::Param`, so nothing can be checked against a bound
without first being made concrete. `docs/status.md` §11 states the two
together and says why fixing either alone reaches the same wall.

*And one thing to watch:* the soundness of the `?Sized` behaviour rests
entirely on the list of use sites requiring a size being exhaustive —
parameters, `let` bindings, fields, reads through a reference. That list is
the kind that grows quietly. A new construct that needs a size must go
through the same check rather than adding a fifth one.

**G0.17 — three more drop bugs, found by giving the test suite the resource
the language does not have.** Ch. 3 §1.5 is why the previous three (G0.16)
were invisible: with no allocator, no file handle and no lock, dropping twice
and not dropping release the same nothing. The fix is not to wait for an
allocator but to **let the tests own something** — a destructor that prints,
and an assertion on the exact sequence. `every_owner_drops_exactly_once`
enumerates every construct that can own a value and states what each must
print. Three of the twenty rows failed on the first run:

1. **Assignment over an owned value leaked it.** `a = P { … }` stored over
   whatever `a` held. Ch. 3 §1.1 gives a value one owner and §1.4 one drop;
   the value being replaced is going away and has that drop to spend. It is
   now dropped after the right-hand side is evaluated, so `a = f(a)` still
   reads `a` before it dies.
2. **Shadowing dropped the shadower twice and the shadowed never.**
   `let a = P{1}; let a = P{2};` printed `22`. Ownership entries stored a
   *name* and resolved it at scope exit, and both entries resolved to the
   newer binding. An entry now carries the storage it will drop, which is
   unique even when the name is not — so shadowing is fixed structurally
   rather than by a special case.
3. **`break` and `continue` did not drop what they left.** A local declared
   inside a loop body simply leaked when the loop was left early. Leaving a
   scope early is still leaving it (Ch. 3 §1.1). `drop_scope` is now split:
   `drop_through` emits the drops, and only a scope that is genuinely ending
   also retires the entries — because `break` leaves along one path while the
   loop's other paths still own the same values.

**The rule this establishes:** the language having no resources is a reason
the *implementation* cannot observe a drop bug, not a reason the *test suite*
cannot. Any new construct that can own a value gets a row in that table
before it gets a feature test. §10's route A — the allocator — is what turns
each of these into a real double-free or leak, and the whole point of doing
this now is that nothing is at stake yet.

**G6.11 — legalization is now a checked transform, and TIR §6's central
claim holds further than the documentation said.**

Nothing checked that legalization preserved meaning. `pipeline.rs` runs a
program on the interpreter and through the whole pipeline, but both sides take
the same TIR, so it covers `TIR → machine` and says nothing about the passes
in between. `compiler/tests/legalize_semantics.rs` closes that: interpret,
legalize, interpret again, demand the same result — faults included. The
interpreter is width-generic, so it can execute a module legalized for a
machine that does not exist.

**Expansion covers more than was claimed.** `docs/status.md` said "expansion
complete for `add`/`sub`/`cmp`". Run against a t9 target, it also handles
`neg`, `tmin`, `tmax`, `tmul`, `select3`, the trapping flavors, and — the one
that matters for anything a frontend emits — **wide loads and stores**, split
into parts at addressable boundaries. A real Trust program compiles, legalizes
for a nine-trit machine, and computes the same answers as on the reference
one. TIR §6 justifies putting legalization in the core design by saying it
"lets one frontend serve a t27 reference machine and a t9 SBTCVM-class target
from identical mid-level IR"; that claim is now executed rather than asserted,
for every operation except the five below.

**The frontier, each with the reason it gives:** `mul` and `shl` need a
widening multiply TIR does not define (**G6.6** — TRISC-27 §4.1 now has
`mulh`, so the machine is ahead of the IR); `div`, `rem` and `shr` are
unwritten; and a wide value cannot cross a function boundary (**G6.5**). The
test asserts each refusal, so moving the frontier fails a test and forces this
entry to move with it.

**Two things this uncovered along the way.**

- **A hand-built `TargetDesc` skips `check()`.** The CLI validates what it
  parses; a test constructing the struct does not. Two of the diagnoses in
  this session were wrong because the probe used `ptr_width = 27` with a
  nine-trit legal set, which §7 forbids — a machine whose widest register is
  nine trits cannot hold a 27-trit address. `t9_target()` now asserts
  `check()` is empty, and `targets/t9.target` is in tree.
- **Alignment and `addr_unit` disagree.** TIR §3.4 says a load or store
  requires "the AM natural alignment of the access width (AM §2.3)", and
  AM §2.3's table is in *trytes*, which AM §1 fixes at nine trits. But §7
  lets a target set `addr_unit` to something else — `targets/sbtcvm.target`
  uses six. Expansion strides parts in the target's addressable units while
  the interpreter checks alignment in the AM's trytes, so legalizing for
  `sbtcvm` produces stores the interpreter calls misaligned, which is UB by
  §4 item 2. See G7.6.

**G7.6 — alignment is defined in AM trytes, but a target may not have them.**
AM §2.3 gives natural alignment as a table in trytes; TIR §3.4 imports it for
every load and store; TIR §7 lets a target declare `addr_unit` freely, and
`targets/sbtcvm.target` declares six trits. For that target the two rules
speak different units, and legalization and the reference interpreter
disagree about whether an expanded store is aligned.

*Not decided.* Three readings are available: alignment is a target property
that §7 should let a target state, defaulting to AM §2.3 when `addr_unit` is
nine; or alignment is always in AM trytes and a target with a different
addressable unit must round up to them; or the AM's table generalizes as
"align to the target's word". The first is most likely right — everything else
in §7 is target-supplied data rather than an assumption baked into a pass, and
this is the one place a pass still assumes. It is recorded rather than decided
because it changes a normative rule in two documents, and because the target
it currently affects has no code generator.

**G6.12 — the shifts expand, by a constant amount.**

`shl` needed no algorithm of its own. AM §3.3 says `x << k` **is** `x · 3ᵏ`,
and `mul` expands now (G6.6), so a constant shift is a rewrite: build `3ᵏ` as
one trit set at position k and hand it to the multiply. The flavors carry over
unchanged, and an amount outside 0…N−1 is a fault at every execution, so the
block ends in `F_SHIFT` rather than computing something.

`shr` is more interesting: **it needs no carries at all.** With `k = q·L + r`
over parts of width L, dropping q parts is a reindex and the rest is

> `out[i] = shr(a[i+q], r) + wrap(a[i+q+1], r) · 3^(L−r)`

and the two terms are bounded — `|shr(a,r)| ≤ (3^(L−r)−1)/2` and the other
`≤ (3^L − 3^(L−r))/2` — so their sum still fits one part and nothing can
escape. `wrap(x, r)` is written `x − shr(x, r)·3ʳ`, using only operations a
legal width has.

**And truncation is round-to-nearest, exactly.** The discarded remainder is
`Σ_{i<q} a[i]·Bⁱ + wrap(a[q], r)·B^q`, whose magnitude is at most
`(3ᵏ − 1)/2` — strictly less than half of 3ᵏ. So no correction is needed and
no tie can arise. That is AM §3.3's "exact rounding for free" seen from the
multi-part side, with the bound tight rather than comfortable: it is the
symmetric range doing the work, and the same computation in two's complement
needs a rounding fixup whose omission is a classic bug.

**Not done: a shift by a computed amount.** It needs `3ᵏ` from `k`, which is
itself a shift, so it wants either a select chain over the N possible amounts
or a loop. Refused by name.

**What is left of expansion:** `div` and `rem`. They are the only operations
that still cannot cross the wide boundary, and they are the hard ones — a
multi-part division is an algorithm rather than a rewrite. The likely shape is
a helper function in legal-width TIR, which will first have to get past G6.5,
since a wide value cannot cross a function boundary.

**G6.13 — `div` and `rem` expand, as a call to a helper written in TIR.**
Expansion is over: every operation now crosses the wide boundary.

Division is the one operation expansion cannot *rewrite*. Every other one is a
fixed pattern over the parts; division is an **algorithm**. So legalization
emits it as an ordinary function and turns a wide `div` into a call — which is
what a C compiler does with `__divdi3`, and for the same reason.

**The helper is written as TIR source text**, not assembled from `Inst`
values. It is a page of long division either way, and one of the two can be
read. It is parsed, its signature joins the module's before anything is
legalized, and its body is legalized like any other — including its own wide
arithmetic, which now expands.

**Why the digits run −2…2.** Schoolbook long division takes, at each step, the
digit leaving the smallest remainder, and in balanced ternary the obvious
digit set is one trit. **It is not enough.** Carrying `|r| ≤ |b|/2` through a
step gives `|3r + t| ≤ 3|b|/2 + 1`, and when `|b|` is even that bound is
reachable and no single trit pulls it back under `|b|/2` — after which the
error compounds down the rest of the quotient. Widening the digit set fixes it
and costs nothing structural, because the quotient is accumulated as a *value*
(`q ← 3q + d`) rather than as a string of trits: a digit outside −1…1 simply
carries into what is already there.

This is not a new discovery. `trit_core::Bt::divrem` was written the naive way
first, and the same bug was found and fixed there; the helper is that routine
transcribed. It is worth recording that the trap was fallen into twice, four
months apart, in two languages.

**The tie.** `|r| = |b|/2` exactly — again only for even `|b|` — is what AM
§3.2 sends *away from zero*. The loop leaves the remainder there rather than
stepping past it, so a final block steps one further out when the leftover
points the way the quotient already does. The quotient's sign is
`tmul(sign(a), sign(b))`: one trit-wise multiply of two comparison results,
where a binary machine needs a branch or an xor of sign bits.

One helper serves `div` and `rem` — the same loop, differing only in the last
block, since the remainder's fixup subtracts the step the quotient's adds.

**Checked three ways.** The algorithm alone, at a legal width, against
`Bt::divrem` as the oracle (180 cases). The expansion, at t27 for a nine-trit
target, against the unlegalized module (336 cases). And division by zero,
which is still `F_DIVZERO` after the rewrite.

**What is left of expansion:** a shift by a *computed* amount, which needs
`3ᵏ` from `k` and is therefore a shift; and `mulh` at a wide width, which is
only ever a step inside the multiply expansion and whose result would be twice
as wide again. Both are refused by name.

**G8.1 — a block-local register allocator.** Every SSA value used to live in
a frame slot, so code generation spent a load on every operand and a store on
every result — correct, and embarrassing.

Values are now kept in registers under a rule narrow enough to state in two
clauses, each of which pays for itself:

- **Defined and used only within one block.** A value crossing a block
  boundary needs the blocks to agree on where it lives, and this pass has no
  mechanism for that.
- **No call between the definition and the last use.** Every register in the
  pool is caller-saved (TRISC-27 §6.1), so a call ends a live range whether
  the allocator likes it or not.

The pool is `t4`…`t7` always, plus `a0`…`a7` in a block that makes no call —
the argument registers are free exactly when nothing is going to overwrite
them. `t0`…`t3` stay scratch, so an emitter always has somewhere to
materialize a constant or a spilled operand.

Measured on the three examples: **31% fewer instructions and 51% fewer memory
accesses.** `demo.tr` went from 4 379 instructions and 2 767 loads and stores
to 3 004 and 1 365.

Two things made this safe to do quickly. The differential invariant means a
mis-allocated register shows up as a wrong answer in a test rather than as a
subtle miscompile, and `sel3` was confirmed against the VM to read its
selector and chosen arm before writing its destination (TRISC-27 §4.2), which
is what lets a select write into a register it also reads.

**What is left in the backend:** values live across a block boundary or a call
still spill, which the callee-saved `s0`…`s6` could hold at the cost of
saving them in the prologue; there is no peephole pass, so the moves a
`widen` becomes are still emitted; and there is no TIR canonicalizer, so
expansion's `add.wrap %x, const 0` reaches the machine as written.

**G8.2 — what a benchmark needed, and what it found.** Four things reported
missing by a program written against this compiler. Three are done; the
fourth is smaller than its old excuse suggested.

**A cycle counter, which was not merely unimplemented but unspecified.** A
search of all ten documents for *clock*, *timer*, *counter* and *elapsed*
found nothing, so there was no gap number to hang it on. TRISC-27 §2.3 now
defines `CYCLES` at −9: a word, load-only, reading instructions retired since
reset.

It counts **instructions, not time**, and that is a decision rather than a
shortcut. A cycle count would commit an implementation to a timing model, and
the reference implementation is an interpreter whose timings mean nothing; an
instruction count is what an implementation can report honestly and a program
can compare against itself. The load has not retired when its value is
produced, so the difference of two readings is exactly the code between them
with nothing to subtract for the measurement.

It is a device address rather than an instruction because a `rdcycle` would
spend one of the seventeen reserved opcodes on something §2.2 already knows
how to express, and would need its own rule for an implementation that cannot
count — where a device address already has one.

`examples/trisc/runtime.t27` gains `f.elapsed`, so a Trust program declares
`fn elapsed() -> t27;` and subtracts two readings. `tritium run` now reports
the count when a program halts, which is how you measure a change to the
compiler rather than to the program.

**`mulh` could not be reached from the language.** TRISC-27 §4.1 had the
instruction, TIR §3.1 gained the operation with G6.6, and Ch. 1 §4's method
list had never been told. It is now `a.mulh(b)`, specified where the rest of
the arithmetic methods are, with the property that matters stated:
`a.mulh(b)·3ᴺ + a.wrapping_mul(b)` is the exact product. This is what
fixed-point arithmetic is built on — without it a `t27` format is confined to
roughly half a word, because the trits it needs are exactly the ones the low
half drops.

**A `const` could not be an array's length.** Ch. 0 §3.2 says a length is a
constant expression and a `const` is one; the implementation evaluated only
literal arithmetic, so `const N: taddr = 8; let a: [t27; N]` was rejected.
Constants are now evaluated to a fixpoint *before* types are built — a type
may need a number and types come first — so one constant may also be written
in terms of another. Note the limit this does **not** lift: `struct Grid<const
N: taddr>` is Ch. 4 §2.4's const generic and is still unimplemented.

**`checked_*` is now only unimplemented.** Its recorded reason — that it
returns `Option<T>` and generics were Ch. 4 — expired when Ch. 4 was built,
and a user-written `fn f() -> Option<t27>` compiles today. What blocks it is
narrower: the built-in methods are lowered where the layout of an
instantiated generic enum is not yet available. Worth fixing, and small.

*The report that produced this list also caught two documentation lines that
had stopped being true* — `docs/status.md` still listing the register
allocator as missing, and the note above. Both are corrected. That is the
sweep rule of Naming §6 failing in the direction it always fails: a change
lands, and the document that described the old state is not the one being
edited.

**G8.3 — values live across a call, and a heuristic that had to be measured
to be believed.** G8.1's allocator ended every live range at a call, because
`t4`…`t7` and `a0`…`a7` are caller-saved (TRISC-27 §6.1). `s0`…`s6` survive a
call, so the obvious next step was to hand one to every range that crosses
one.

**It made the benchmark 6% slower** — 91 668 instructions against 86 686 —
and the reason is a cost model that reads the wrong way round if you do not
write it down:

| | paid |
|---|---|
| spilled to a frame slot | one store at the definition, one load **per use** |
| held in a callee-saved register | one save and one restore **per invocation** |

A recursive `fib` pays the save and restore on every call, to keep a value it
reads once. The register is only worth taking when the value is used **more
than once** after the call, which is the threshold the allocator now applies.

With it, the same benchmark comes back to where it started — 86 685, a net
gain of one instruction — because values crossing a call and read more than
once are *rare in that shape of code*. On a loop that calls once and then uses
several live values, it is **7.3% better**: 20 451 against 22 050.

So the increment is conditional rather than uniform, and both numbers are
recorded because only having the second would misrepresent it.

**This is the first change in the project measured rather than argued.** It
was possible because G8.2 added `CYCLES` a few commits earlier; before that
the only instrument was counting lines of assembly, which would have shown
the first attempt as an improvement — it emits *fewer* instructions
statically, and executes more.

**G8.4 — `checked_*` is implemented, and Ch. 1 §4 is complete.** The last
method of that section, and the one whose recorded reason had outlived its
truth twice: first "generics are Ch. 4, which is not written yet", then "the
layout of a generic enum is not available where built-in methods are
lowered". Neither survived a look. `Option` is in the prelude, so
`Types::instantiate` produces `Option<t27>` on request like any other
application, and `build_variant` already knew how to write a variant — it
only needed a form that takes values rather than expressions, which a
built-in method is what has.

**It is one `.flag` operation and one three-way branch.** The overflow trit
already says whether the exact result fitted, so there is no second
computation and no comparison against a bound — and because the trit is the
*direction* of the overflow, both nonzero arms of the branch lead to the same
place. A binary machine computes the result, then tests a flag register, then
branches two ways.

`a.checked_add(b)` therefore costs the arithmetic plus a branch and a tag
store, and returns an `Option<T>` that is an ordinary enum laid out by Ch. 2's
rules — including its niches, so `Option<trit>` from `checked_add` on a `trit`
would still be one tryte.

**G8.5 — an audit of the machine against its specification.** Every table
and every normative claim in TRISC-27 and the assembly language, checked
against `tritium` by running it rather than by reading both.

**What holds.** All ten opcodes of §3.3 decode and execute. All thirteen
functs of §4.1 execute, `mulh` included. The seven formats of §3.2 name their
zero fields and the decoder checks all thirteen of them, so §3.4's
"malformed" clause has referents rather than being a clause about nothing.
Every directive of the assembly language assembles, every reserved one is
rejected by name, and all nine pseudo-instructions expand. `targets/tritium.target`
matches §§1–6: `word` 27, `addr_unit` 9, `ptr_width` 27, `legal` the word
alone, `call_conv` `tritium0`.

**Thirteen fault conditions, each now a test.** The specification states them
across §§2, 4 and 6 and they were covered unevenly; each is exercised in
`every_fault_the_isa_promises_is_raised`, because a fault the machine does not
raise is a fault a program cannot rely on. Access at or above A; a word load
from a tryte device and the reverse; a load from `IO_OUT`; stores to `IO_IN`,
`MEM_SIZE` and `CYCLES`; a narrow load from `CYCLES`; a reserved device
address; a shift amount above 26 and below 0; an unaligned word access; an
unaligned jump target. All thirteen raise what §5 says they raise.

**One inconsistency, and it was mine.** §6.4 said "the first instruction
assembled is the first instruction executed", which stopped being true when
G0.15 reserved the first word of memory: the first *assembled* instruction is
now at address 3 and address 0 holds the `nop`. The sentence is still true of
instructions and false of memory, so it now says which. This is Naming §6's
sweep rule failing in the direction it always fails — the document being
edited was §2.2, and §6.4 was three sections away.

**G8.6 — instruction selection was leaving three fields empty, and a profile
said so.** `docs/status.md` named cross-block register allocation as the next
backend work. A dynamic profile of `examples/trust/HPL.tr` — every instruction
decoded before it executed, 10 862 143 of them — said otherwise:

| | share of all executed |
|---|---|
| `ld`/`st` through `sp` | 34.9% |
| `li` and nothing else (`alui.add` from `zero`) | 12.9% |
| `ld`/`st` through a computed address | 14.4% |
| unconditional jumps | 7.5% |
| `br3` | 6.1% |

Three of those are fields the encoding already had and code generation was
not using:

1. **The immediate field.** Every operation but `wrap` has an `i` form
   (TRISC-27 §4.1) reaching ±2 391 484, and a constant operand was going
   through `li` into a scratch register instead. **−10.3%.**
2. **The branch displacements.** `br3` carries three of ±1093 words, and all
   three were pointing at adjacent stubs holding one jump each — which is why
   the jump count and the branch count differed by six. **−7.6%**, and 88% of
   the unconditional jumps.
3. **The access displacement.** `ld` and `st` carry fourteen trits, left at
   zero, so every address was computed with an `add` first. **−3.2%**, and
   47% of the *lines of assembly*.

Together **−19.7%**, all inside `compiler/src/codegen.rs`, no specification
touched, and the four residual checks byte-identical at every step.

What is worth recording is not the number but the mistake: the next task had
been chosen by reasoning about the code rather than by measuring it, and the
reasoning picked the hardest of the four items. The half-built instrument was
already there — G8.2 added `CYCLES` — but nothing had ever decoded the
instruction stream. A hundred lines of throwaway profiler reordered the whole
backlog, and is now `tritium profile` and `vm/src/profile.rs` — an instrument
that is not there does not leave you uninformed, it leaves you confident.

Two things the profile also settled. **Concentration**: 200 words of code are
64% of execution, so the target is a few hundred instructions, not a program.
And **the stack traffic is 2.4× the real data traffic**, which is the case for
cross-block allocation — still the largest single pool, and now the next item
rather than the first.

**G8.7 — the optimizer that did not exist.** `lang/lower.rs` opens by saying
that every local gets a `slot` — a read is a `load`, a write is a `store` —
because TIR is SSA and a mutable local is not, and that "the cost is real and
is paid back by the optimizer that does not exist yet". TIR §6 names where
that optimizer goes: legalization sits "between **target-independent
optimization** and instruction selection". The stage on the left of that
sentence had never been built.

`compiler/src/tir/canon.rs` is it, with one transformation to start:
`promote_slots` turns a `slot` whose every use is a `load` or `store` through
it, all within one block, into the value it holds.

**One block** is the restriction that keeps this from being SSA construction.
A slot written in one block and read in another needs a value along each
edge, which is block parameters and correct placement of them; within a
block, the value at each load is unambiguously the last thing stored above
it. The restriction is narrower than the shape it catches, because the
shape the frontend emits most is a **parameter** — which arrives as an SSA
value, is stored into a slot, and is read back:

```
%pa = slot tryte[3]     st.word a0, 3(sp)      ld.word t0, 3(sp)
store t27 %a, %pa   →   ld.word t1, 3(sp)  →   ld.word t1, 6(sp)
%v = load t27 %pa       st.word t1, 42(sp)     mulh    a7, t0, t1
%w = load t27 %pa       ld.word a7, 42(sp)
```

`@fmul` in `examples/trust/HPL.tr` went from 18 machine instructions to 14.

Promotion is refused when the address can be observed: passed to a call,
given to an `offset`, stored as a value, accessed from another block,
accessed at two different types, or read before it is written — the last
because reading uninitialized `slot` storage is UB and yields poison (TIR §4
item 4), and a pass is not the place to decide what poison is.

**What the test suite caught.** A first version renamed only inside the
promoted block, and a deleted load's *result* is often named in the blocks
below it. Three frontend tests failed with `%v.2 is not defined in this
function` — the verifier, doing exactly its job. The renaming is function-wide
and sound there: a block naming the result is dominated by the block that
defined it, and the value stored dominates everything the result did.

The pass is now in the path of every whole-program test: `frontend.rs` and
`pipeline.rs` canonicalize before legalizing, so the differential invariant
runs the module *as written* on the interpreter against the *canonicalized*
module on the machine. A canonicalization that changed an answer would fail
about a hundred tests. `compiler/tests/canon.rs` adds six that ask directly
what it promotes and what it refuses.

Measured on `examples/trust/HPL.tr`: 8 719 102 → 8 496 620 dynamic
instructions (−2.6%), output unchanged. Small statically — 66 lines — and
larger dynamically, because what it promotes is in the leaf functions the
inner loop calls.

**G8.8 — a parameter had no reason to leave the register it arrived in.**
Parameters arrive in `a0`…`a7` (TRISC-27 §6.1) and the prologue was storing
every one of them to a frame slot, from which every use loaded it back. In
the entry block of a function that makes **no call**, nothing can clobber
them: they are caller-saved, and it is the arguments of a call that overwrite
them. So there they stay.

And with nothing else in the frame, there is no frame: the measuring pass
already runs (G8.6), so it also reports whether anything touched `sp`, and a
function where nothing did opens none.

`@fmul` in `examples/trust/HPL.tr`, over the four changes of G8.6–G8.8:

| | instructions | memory accesses |
|---|---|---|
| before | 22 | 8 |
| after | 7 | 0 |

**A bug the tests caught**, worth recording because of its shape: the first
frame-elision test asked whether an emitted line contained `(sp)`. It missed
`addi.trap a7, sp, 15` — taking the *address* of a slot, which mentions `sp`
without the parentheses. Six frontend tests failed with an access above the
stack top, because the frame the address pointed into had never been opened.
The check is now "mentions `sp` at all, other than the frame adjust itself",
which is conservative in the direction that costs two instructions rather
than the direction that corrupts the caller's frame.

Measured: 8 496 620 → 8 033 644 dynamic instructions (−5.5% over two steps),
output unchanged. **Against the profile that started G8.6: −26.0%.**

This is not yet cross-block register allocation, which remains the largest
single item: stack traffic is still the biggest pool in the profile. It is
the part of that pool that needed no liveness analysis across edges.

**G8.9 — cross-block register allocation, and the interval that began one
instruction too late.** G8.1's allocator was block-local: a value could live
in a register only where one block both defined and used it, and everything
crossing an edge went to memory. G8.6's profile put a number on that —
**frame traffic 44% of every instruction executed, against 15% for the
program's own data**, and it stayed the largest single pool through four
changes.

`allocate` is now a linear scan over live intervals, decided once for the
whole function.

**What the frontend makes easy.** `lang/lower.rs` gives every local a slot,
so *no value crosses a block edge in a register* and the frontend emits **no
block parameters at all** — 412 blocks in `examples/trust/HPL.tr`, zero
parameters, before and after legalization. Agreeing on a register for a value
with a different definition on each incoming edge is the parallel-copy
problem, and this pass does not have to solve it: block parameters keep the
transfer area `move_args` already uses. Hand-written TIR has them and keeps
working.

**What needed care.** Three things, and one of them was got wrong:

1. *The interval is a hull.* A value defined inside a loop and read at the
   top of the next iteration is live at a point earlier in the linear order
   than its definition. The interval reaches back over the whole loop, or
   another value takes the register there.
2. *`a0`…`a7` are written while a call sets up its arguments*, not only when
   it executes. They are allocated only in a function that makes no call.
3. *What "crossing a call" means.* A value whose last use is an **argument**
   to the call is read before the call executes, and a caller-saved register
   holds it perfectly well; so does the call's own result. Only a value still
   live after the call returns needs `s0`…`s6`.

**The bug.** A block's first instruction had the same position as the block's
entry, so a value live *into* a block whose first instruction is a call had
an interval beginning *at* that call — and "strictly inside" was then false.
A function parameter is the common case: `@print_field(%v, %width)` calls
`@decimal_width` first and reads `%width` after, so `%width` went into `t6`
and the call destroyed it. HPL printed its table with no padding.

A block's entry now has a position of its own, before its first instruction.

**What found it was not the test suite.** 342 tests passed. HPL's output
changed, and only because it is a program that prints something formatted
enough for a wrong answer to be visible. The regression test written
afterwards fails on the old code — checked by reverting the fix, because a
test that cannot fail is not one.

Measured on `examples/trust/HPL.tr`: 8 033 644 → 5 538 068 dynamic
instructions (**−31.1%**), frame traffic 43.97% → 16.67%, output unchanged.

**Against the profile that started G8.6: −49.0%.**

**G8.10 — the comparison, and the two instructions between it and the
branch.** The frontend has no `bool`-producing comparison. `i < n` is

```
%c = cmp t27 %i, %n
%b = select3 %c, t1 const t1 1, const t1 0, const t1 0
br2 %b, ^body, ^done
```

and `br2` is a `br3` on the **sign of `%b`** — which is the sign of whichever
constant `%c` chose. So the branch can read `%c` directly, with each arm sent
where the constant it would have produced pointed.

On the machine the difference is seven instructions against two. `sel3` is
the one instruction with four register sources (TRISC-27 §3.2, format R4) and
has no immediate form, so its three constants each cost an `li`; and
legalization then adds a `cmp` against zero to project the widened `bool`
back down to a condition trit. 148 of the 185 `cmpi` in HPL's assembly were
that projection. All of it disappears, because `%c` is *already* a `t1` — the
result of a `cmp` — and needs no conversion at all.

`branch_through_select` and `remove_dead` in `compiler/src/tir/canon.rs`. The
second is what makes the first worth anything: rewriting the branch only
orphans the `select3`, and something has to notice.

**What `remove_dead` will not remove.** An instruction that can raise, whether
or not anything reads it: `store`, `call`, a trapping flavor, `div` and `rem`
(a zero divisor), `shl` and `shr` (an amount outside 0…26 faults `F_SHIFT`
whatever the flavor). `load` stays too — this pass has no reason to reason
about memory. A test asserts that a dead `div` by zero still faults.

Measured on `examples/trust/HPL.tr`: 5 538 068 → 5 076 223 dynamic
instructions (**−8.3%**), output unchanged.

**Against the profile that started G8.6: −53.3%.** Under half.

**G8.11 — which interval gives up its register.** G8.9's linear scan, faced
with an empty pool, spilled whichever interval had just arrived. That is the
easy answer and the wrong one: an interval that runs longer holds its
register over more instructions and saves fewer of them per instruction held.
The scan now takes the register from whichever *active* interval ends
furthest away, if that one ends further than the arriving interval — the
classical rule, and one this backend had simply not implemented.

A caller-saved register cannot be given to a value that lives across a call,
so only like replaces like.

Measured twice. On a synthetic function with twenty-six values live at once
and one short-lived value read eight times, frame accesses fall from 48 to
34. On `examples/trust/HPL.tr`: 5 076 223 → 4 803 365 dynamic instructions
(**−5.4%**), frame traffic 16.67% → 13.54%, output unchanged.

**Against the profile that started G8.6: −55.8%.**

**G8.12 — block parameters get registers, and an edge becomes a parallel
copy.** G8.9's allocator left every block parameter in memory, because a
parameter has a different definition on each incoming edge and agreeing on a
register for it is the parallel-copy problem. That was free at the time —
the frontend emits no block parameters — and it is the thing standing between
this backend and a real mem2reg, which cannot pay for itself while the block
parameters it introduces cost four memory accesses per edge.

Two changes.

**The interval reaches back to the predecessors.** A parameter is written by
the edge, in the predecessor, so its live interval starts at the earliest
predecessor's terminator rather than at its own block's entry. Without that,
the register could be handed to something live in the predecessor.

**`move_args` is a parallel copy.** Every argument is read as it stood before
any parameter was written. `parallel_copy` emits any move whose destination
no remaining move still reads; when none is left, what remains is a cycle, so
one register is parked in `t0` and whoever wanted it reads that instead —
which turns the cycle into a chain the drain empties completely, including
the move that reads the scratch, whose own destination nobody reads any more.
So the scratch is free again before another cycle is broken, and one scratch
suffices however many cycles an edge has.

Parameters the scan leaves in memory keep the transfer area, and their
arguments are read out *before* the register copy runs, since writing a
transfer slot cannot disturb a register.

`@fib` written with block parameters — the back edge exchanges two of them —
now compiles to registers throughout, with no frame at all.

**This changes nothing measurable yet.** HPL compiles to the identical
instruction stream, because no Trust program has a block parameter to
allocate. It is the prerequisite, and it is recorded as one.

**G8.13 — mem2reg, and the optimizer `lower.rs` was written against.**
`lang/lower.rs` opens by saying every local gets a `slot` because TIR is SSA
and a mutable local is not, and that the cost "is paid back by the optimizer
that does not exist yet". G8.7 built the single-block half of that optimizer;
this is the rest.

`compiler/src/tir/mem2reg.rs` answers, at a block's *entry*, what was stored
into a slot along the path that got here. Where the predecessors disagree the
answer is a new **block parameter** — which is what block parameters are for,
and which G8.12 made cost registers rather than memory.

**The algorithm** is Braun et al.'s on-demand SSA construction, which never
computes a dominance frontier. Three memoized questions: what a block stores,
what holds at its end, what holds at its entry. The entry question inserts a
parameter when the predecessors disagree *or when it is asked again while
still being answered* — which is a loop, and the parameter recorded before
the recursion is what a back edge finds instead of running forever. A
parameter whose arguments all turn out to be one operand is trivial, is
removed, and may make another trivial; that fixpoint is what stops a loop
collecting a parameter for every variable it does not touch.

**What it refuses.** The escape test of G8.7 minus the single-block
condition, and one thing more: a slot is promoted only when every load has a
store reaching it on **every path** (`definitely_assigned`, a forward
fixpoint). Reading uninitialized `slot` storage is UB and yields poison (TIR
§4 item 4). UB permits any answer — zero would do, since that is what the
frame holds — but a pass that quietly chose one would be deciding what poison
is, and that decision belongs in the specification.

**A test that had to be rewritten, for a good reason.** `@stored_as_a_value`
checked that storing a slot's address into another slot stops promotion. It
now returns its argument directly, and correctly: promoting the *second* slot
removes the store that let the first one out, after which the first is no
longer escaping. The test hands the second slot to a call, so the address
really does leave.

Measured on `examples/trust/HPL.tr`: 4 803 365 → 3 623 789 dynamic
instructions (**−24.6%**), assembly 32 944 → 27 521 lines, frame traffic
13.54% → **3.64%**, data traffic 25.83% → 13.93%, output unchanged.

**Against the profile that started G8.6: −66.6%.**

The profile it leaves is a different program. Branches and comparisons are
now 36% of everything executed — `br3` 18.2%, `alui.cmp` 12.1%, `alu.cmp`
6.1% — which is loop conditions and the two bounds checks every array index
still pays. The index multiply is 7.9%. Both are analysis, not encoding.

**G0.14a — a trait may take type parameters.** Half of Ch. 4's one
substantial hole, and the half that stands alone.

A trait's *associated* type is chosen by whoever implements it, once. A
trait's *type parameters* are chosen by the user, and a type may implement
such a trait many times over — that is the whole distinction §1.7 draws, and
`trait From<T>` is why the language has both.

**What stood in the way was the method's name.** An impl block becomes
ordinary functions named `Type.method`, which is one name per method per
type. Three `From` impls for `t27` all wanted to be `t27.from`. The arguments
are now part of the name — `t27.From.t9.from`, `t27.From.bool.from`,
`t27.From.Celsius.from` — using the same mangling instantiated generics use,
and everything else follows:

- a bound is a trait *with its arguments* (`ast::Bound`), so `U: From<T>` and
  `U: From<t27>` are different requirements and `Impls::pairs` records the
  mangled form;
- an impl's signature is compared against the declaration with the trait's
  parameters already substituted, since `fn from(x: T)` is `fn from(x: t9)`
  once `T` is chosen;
- which of several `from`s a call means is decided by the arguments given,
  and a call that fits none or more than one says what the candidates were.

A type parameter also resolves as a path head now, so `U::from(x)` inside a
generic body means the concrete `U`'s.

**A pre-existing bug this turned up.** `Option<Option<t27>>` had never
parsed: the closing `>>` is a token the lexer has every reason to read as the
shift operator, and only the parser knows it is two brackets. Splitting it —
taking one `>` and leaving the other — is what lets any generic argument
contain another, and it was needed here because `U: From<T>>` ends the same
way.

**What is still missing** is the blanket impl `impl<T, U: From<T>> Into<U> for
T`, which is why `x.into()` does not exist yet and one writes `U::from(x)`.
Since §5.6 closes `Into` to hand implementation, the only blanket impl will be
the language's own — so no general overlap search is needed, and §1.8's
coherence rule stays a comparison of names.

**G0.14b — the blanket impl, and Ch. 4 is closed.** The other half of
G0.14a. `impl<T, U: From<T>> Into<U> for T` is not an implementation for a
type; it is a rule about all of them, and that is the whole of what makes it
different: every other impl in the file is found by *name* — "does `Bar` have
`area`?" is a lookup — and this one is found by *checking a condition*.

**Nothing else had to change**, and the reason is architectural. A generic
body here is lowered by reading the same source under an environment, so the
rule's methods are ordinary generic functions with the impl's parameters, and
applying the rule is binding `T` and `U` and instantiating. `c.into()` binds
`T` from the receiver's type and `U` from the type the context wants, checks
the rule's own bound — `U: From<T>` — and calls `Into.into.Celsius.t27`.

Three things follow, and all three are conditions the language already
states:

- **A rule is the last thing tried**, so a type's own method of the same name
  is what a call means.
- **A trait a rule covers may not be implemented by hand.** The rule holds for
  every type, so any hand-written impl overlaps it, and §1.8 makes
  overlapping impls an error. Closing the trait says so where it is written
  rather than leaving a collision to be discovered — which is what §5.6
  already decided for `Into`, generalized to any trait with a blanket impl.
- **A bound is satisfied by a rule too**, so `fn show<A, B: Into<A>>(x: B) -> A`
  is writable. Checking it recurses into the rule's own bounds, with a depth
  limit standing in for the termination argument a real coherence checker
  would give.

**No overlap search is needed.** Because a covered trait is closed, two impls
can collide only by having the same name, which was already a duplicate
definition. §1.8's coherence rule stays a comparison of names, exactly as the
argument for scoping this way predicted.

**What is still not there**: a bound on a trait's own parameter, a method
with type parameters of its own inside a blanket impl, and a rule whose self
type is anything but a bare parameter. Each is rejected with a diagnostic
that says so.

**Ch. 4 has no substantial hole left.** `docs/status.md` §6 said generic
traits were the one, and it now lists neither half.

**G9.1 — Ch. 5, and the three things it had to decide.** The library chapter
exists (`spec/language/05-library.md`, 700 lines). Its job was to discharge
what nine other sections had deferred to it, and three of those needed a
decision rather than a write-up.

**Text is fixed width, one character per word — and AM §5 said the
opposite.** AM §5 promised "a tryte-based UTF-8 carrier format as the interop
default; a native ternary text encoding is a reserved appendix". The measured
comparison says otherwise:

| Code points | UTF-8 in trytes | one word each | |
|---|---|---|---|
| ASCII | 1 tryte | 3 | 3× |
| Greek, Cyrillic | 2 | 3 | 1.5× |
| **CJK** | 3 | 3 | **1×** |
| astral planes | 4 | 3 | **0.75×** |

Fixed width costs 3× only for the range UTF-8 was designed to optimize, and
nothing at all for the range this project's author writes in. What it buys is
that `s[i]` is a character and `s.len()` is a count of them, both O(1) — the
two things Rust cannot offer and the reason is UTF-8. AM §5 is corrected, and
the *denser* native encoding is what is now reserved: a variable-width scheme
using a tryte's **sign trit** as the continuation marker, which is
self-synchronizing for the same reason UTF-8 is and more ternary than UTF-8 is
binary (Ch. 5 Appendix A).

A `char` is a **word** and not two trytes, which would have been enough and
denser: the machine has exactly two access widths, and 18 trits is neither.

**`Box` is a language item, and there is one allocator.** An allocator turns a
size into an address, which is precisely the operation TIR §5 does not have
and Ch. 3 §6 reserves. So draft 0.1 says the smallest true thing: `Box`,
`Cell` and `RefCell` are language items; the allocator is two functions the
*target* supplies, like `putchar`; and `trait Allocator` is reserved to the
`unsafe` chapter. Pretending an allocator could be written in this language
would have been the alternative.

**`?` is two rules and no trait.** Rust's `Try`/`FromResidual` exists to make
`?` extensible to user types. The extension has almost no users, and the trait
is one most readers cannot recite — which is a bad property for the definition
of a control-flow operator in a language whose argument is that a reader can
tell what code does. Reserved rather than rejected.

Two smaller ones. **`expect` does not exist**: its whole value is the message,
and AM §4 gives a fault a code and nothing else, so a target that could print
would be printing from inside a failure on a port the failing program may have
been using. **`RefCell`'s borrow state is one word whose sign is the
three-valued part** — 0 unborrowed, *n* shared, −1 exclusive — which is the
neatness Ch. 3 §6 was reaching for when it said the state "is one trit, though
a shared borrow count needs more than a trit". It is not in a trit; it is in a
sign, and `cmp` against zero answers it in one instruction.

**Nothing here is implemented.** Ch. 5 is specification ahead of code, which
is the order that has repeatedly reduced the number of decisions made blind.

**G9.2 — `_end`, and the allocator that needed it.** Ch. 5 §2.2 places the
heap between the end of the loaded image and the stack, and names the end of
the image `_end`. The assembler published no such symbol, and a program had no
other way to learn where its own image stops.

*Closed.* The assembler defines `_end` — the first address past everything the
file emits, rounded up to a word — after pass one, which is when the answer is
known. It is a forward reference for every statement, which §3.3 already
allowed, so `li rd, _end` takes its two-word form and nothing else changes.
Assembly §3.4 specifies it.

`examples/trisc/runtime.t27` gains `alloc` and `free`, which Ch. 5 §2.1 makes
the target's business because turning a size into an address is the operation
TIR §5 does not have. First fit over a free list, by **exact** size — a block
is reused only for a request the size of the one that freed it, which is
predictable and never leaves a fragment nothing can name.

Two things fell out rather than needing decisions. **`align` is ignored**,
because AM §2.3 caps alignment at a word and every block is word-aligned, so
every alignment a type can ask for is already met. And rounding a size up to a
whole number of words hits the same trap the UTF-8 encoder did: `rem` is
symmetric, so `n - rem(n,3)` is the *nearest* multiple of three and may be
below `n`.

The runtime used `MEM_SIZE`, which is defined by the *compiler's* output and
not by the runtime — so the allocator worked only in a program that had been
compiled. It defines `MEM_TOP` for the same device address now, since a symbol
defined twice is an error and the two files are concatenated.

**G9.3 — the citation checker read a third of the citations.** `scripts/citations.sh`
checked `core/src compiler/src vm/src` and nothing else, so the specification's
citations of *itself* — far more numerous than the code's — were never
checked. Extending it to `spec/`, `docs/` and the README raised the count from
102 to 140 and immediately caught three stale ones in this document: two
naming a subsection of TIR's undefined-behavior inventory, which is a numbered
list, and one naming a subsection of its legalization contract, which is a
bulleted one. Those exact citations had already been fixed **in the source**
by the checker's first run; the document describing that fix was not swept.
Naming §6, failing in the direction it always fails.

The checker cannot tell a citation from the *quotation* of a broken one, so
neither this entry nor `docs/status.md` can name the two it caught. That is a
small price and it is the shape of the tool: it reads text, not intent.

It also confirms what the checker cannot do. Writing Ch. 5 produced five
citations that name real sections saying something else — `Ch. 2 §7`
(Unions) for the niche rule, `Ch. 0 §2.3` (Comparison) for a body-less
declaration, `Ch. 2 §5` (Enums) for the bounds check, `Ch. 3 §1.1` (One
owner) for the aliasing rule, and Ch. 0's loop section for a reservation of
macros that lives in that chapter's *closing* section instead. All five passed
the checker and were caught by reading. The §-level-anchor version of the
check is still worth building.

A sixth error was found the same way and is worth recording because of how it
was made: a case-sensitive search for `macro` in Ch. 0 reported nothing, so
this entry first claimed Trust reserved macros nowhere. The chapter says
"**Macros.** Reserved" — capital M. A grep is a claim about a document only as
strong as its pattern, and the conclusion drawn from it here was written into
the specification before anyone read the section.

**G9.4 — what Ch. 5 decided that a later revision can still take back, and
what it cannot.** Six of Ch. 5's decisions were made without asking, and the
question worth answering about each is not "is it right" but "is it
reversible". Four are, one is not, and one needed a reservation to stay that
way.

**Purely additive later**: `rev` and `DoubleEndedIterator` (a new trait
existing iterators simply do not implement); `expect` (the API is an addition,
and the hard part is the message mechanism, not the signature); sorting; `Rc`.

**Specified rather than left open, deliberately**: `Vec` grows by *doubling*
from four. Most languages leave the factor to an implementation. Relaxing
"doubling" to "some constant factor greater than one" later costs nothing —
the amortized bound holds for any of them, and only a program reading
`capacity()` could tell. Tightening in the other direction is what would
break something, so the direction chosen is the one that can be undone.

**Not deferred but refused**: there is no `\x` escape. The argument is that
this chapter's text contains no bytes, so `\x` names something that is not
there — it is a wrong thing rather than a missing one, and adding it later
would be overturning the reason rather than filling a hole.

**Fixed in advance because it cannot be added afterwards**: a map's iteration
order is unspecified. There is no map, and the requirement is written into
§7 anyway, because once programs exist that depend on an order the order is
part of the interface whether it was meant to be or not. Rust learned this and
had to randomize.

**Needed a reservation to stay reversible**: formatting. Every spelling of it
needs macros or variadic arguments; Ch. 0 §7 reserves macros, but nothing
reserved the *position* a macro invocation would occupy. Ch. 0 §1.5 now does:
an identifier immediately followed by `!` is a syntax error and nothing else
may claim it. That was the one decision where doing nothing today would have
cost something later.

**G9.5 — `char`, the first of Ch. 5 to be built.** A Unicode scalar value in
one word (Ch. 5 §1.2): a scalar like the integers and not one of them. It
compares, it matches, it sits in aggregates and arrays, and `Option<char>` is
one word because a word holds 7 625 597 484 987 values and 1 112 064 of them
are characters — the largest niche in the language by a wide margin.

Three decisions the chapter forced, and all three are refusals:

- **`char as t27` and nothing else.** Not `as t9`: narrowing a scalar value is
  a conversion that can be wrong, and Ch. 1 P2 does not let one be silent.
- **No `t27 as char`.** Most words are not scalar values. `char::try_from` is
  the checked form and is **not built yet**, so the diagnostic names Ch. 5
  §1.2 rather than a function that does not exist.
- **No `\x` escape**, with the reason in the diagnostic: it names a byte, and
  text here is characters.

**The lexer had to learn a lifetime from a character.** `'a'` and `'a` share
their first two characters and differ in the third — the same rule Rust uses,
and for the same reason. `'ab'` is diagnosed as more than one character rather
than mis-lexed as a lifetime.

**A pre-existing hole in the grammar summary**: Ch. 0 §6 used `literal` as a
terminal and never defined it. It does now, with the character form and its
escapes.

**`\u{…}` takes hexadecimal digits**, in a language whose own numeric
literals are decimal, `0t` and `0h`. Ch. 5 §1.4 first said "heptavintimal or
hexadecimal", which is ambiguous — the same digits read two ways — and is
corrected. The escape does not name a number this language owns; it names a
code point in an external standard that writes them one way, and an escape
that had to be transcribed before it could be checked against the table it
came from would be unreadable against the thing it names.

What is not built: `str`, string literals, `char::try_from`, `to_digit`,
`from_digit`, and the UTF-8 conversion. A string literal is diagnosed as
needing static storage this compiler does not emit, which is the honest
reason.

**G9.6 — string literals, and the division UTF-8 wanted and did not get.**
`str` is `[char]` and `&str` is the fat pointer every slice has: an address
and a length **in characters**. Fixed width is what makes that mean something,
and `s.len()` and `s[i]` are both O(1) — neither is available in a language
whose strings are UTF-8, and the reason is UTF-8.

Storage is a TIR global, one word per character, and identical literals share
one. That is not an optimization a program can observe: a `&'static str` has
no identity beyond what it points at and nothing in this language compares
addresses.

**What the first `print` got wrong.** Encoding a code point to UTF-8 is
slicing it into fields of six bits, and slicing wants division that rounds
*down*. Ch. 1 §4's `/` rounds to **nearest**, which is what arithmetic wants
and what a field extraction must not have: 19990 / 4096 is 5 to the nearest
and 4 to the floor, and only one of those is 世. The first version printed
`Hello, ...x.界!` and the mangled character was the one whose fields the
rounding crossed.

This is worth recording because it is the cost of the interop format showing
up exactly where Ch. 5 §1.1 said it would — at the boundary and nowhere else.
Native storage never divides. `examples/trust/hello.tr` now carries
`floor_div` and says why in a comment, and it is the first Trust program to
print text in three scripts.

**`char` may be `impl`ed on** — not yet used, but the list of built-in types
an impl block may name had `trit`, `bool`, `t9`, `t27` and `taddr` in it and
would have silently refused `impl char`.

Still not built: `char::try_from`, `to_digit`, `from_digit`, `str.to_utf8` as
a library function rather than an example's `print`, and `String`.

**G9.7 — the library is Trust, and a program pays for none of it.** Ch. 5 §1
is built, and all of it except one function is ordinary Trust in the prelude:
`char::to_digit`, `char::utf8_len`, `char::to_utf8`, `str::utf8_len`,
`str::to_utf8`, and the `floor_div`/`floor_mod` that G9.6 found the encoder
needs. A reader could have written every one, which is the claim Ch. 5 §3
makes about the iterator adaptors and which this section should make too.

**The exception is `char::try_from`**, and it is the only thing in the chapter
that cannot be written in the language: producing a `char` from a word is
exactly what no `as` does. It is a compiler builtin — four comparisons and
four branches, which is what Ch. 5 §1.2's definition has — and everything else
that needed it (`from_digit` in particular) is now writable.

**Making the prelude a place a library can live needed two things.**

*Reachability.* The prelude is prepended to every program, so without pruning
a program that never mentions text would emit the UTF-8 encoder. `keep_reachable`
drops every function nothing can call. The roots are `main`, or — in a file
with none, which is what a test and a library look like — every function the
*program* defined, since nothing else says which of them matter; prelude
functions are never roots, which is the point. And a function whose *address*
appears in a global is always a root, because a vtable slot is an address and
a method reached only through `dyn Trait` is reached only that way.

The pass lives in `lang::compile` rather than in `lower`, because it is the
place that knows what the prelude is. A first version put it in `lower`, where
it could only bail out when there was no `main` — and that silently kept the
whole prelude for every test that compiles a file without one, which three
tests caught by counting `cmp`.

*Methods on unsized types.* `impl str` gives methods to `[char]`, whose
receiver is a fat pointer. The method-call path stripped references only while
the pointee was sized, so `&str` had no name to look a method up under, and
then tried to *borrow* a receiver that was already the reference. Both are
fixed: an unsized pointee names the method, and the fat pointer is passed as
it stands.

`examples/trust/hello.tr` is now `print` over `s.to_utf8(&mut buf)`, and the
only thing in it that is not Trust is `putchar`.

**G9.8 — `?`, and the drop bug it found.** Ch. 5 §4.1: two rules and no
trait. On `Option`, `Some(v)` continues and `None` returns `None`; on
`Result`, `Ok(v)` continues and `Err(e)` returns `Err(F::from(e))`, with the
conversion skipped where the two error types are the same. The two do not mix,
and the diagnostic says what to write instead.

It is lowered by **desugaring to `match` and `return`**, which is what §4.1
says it is. That is not laziness: leaving a function early has to drop
everything the frame owns, and `return` already does — or rather, it was
supposed to.

**The bug.** `?`'s first test was a drop ledger, and it printed `1-2` where
`1-21` was right: the value declared *before* the `?` was dropped on the early
path and not on the other one. It is not `?`'s bug. The hand-written `match`
with a `return` in an arm does the same, and so does an `if` with a `return`
in it — a shape a reader meets on their first day.

`return` called `drop_all`, which is `drop_scope(0)`, which **retires** what it
drops. Retiring is right at the end of a function and wrong in the middle of
one: a `return` leaves by one path while the paths beside it still own the
same values, and retiring them there means the value the *other* path owns is
never dropped at all.

`drop_through` exists for exactly this and its comment names the three
statements that need it — "`break`, `continue` and `return` leave along one
path while the scope's other paths still own the same values". `break` and
`continue` were given it. `return` was not. The comment was correct and the
code was not, which is the failure mode a comment cannot catch.

Found the same way the three batches before it were: G0.16 and G0.17 both came
out of giving the test suite a resource whose destructor prints. That
instrument has now found four separate drop bugs and has never found anything
else.

**G9.9 — the iterator adaptors, and the five things in the way.** Ch. 5 §3
claims the adaptors are "a struct and an `impl` a reader could have written".
Writing them found five reasons that was not yet true, and each is now built.

**Associated types in a generic impl.** `impl<I> Iterator for Map<I> { type
Item = …; }` was refused outright — "what it chooses would depend on the
instantiation". It does, so the choice is kept *as written* with the impl's
parameter names and resolved when an instantiation exists. `Types::assoc` was
already a cell "because an impl on an instantiated generic chooses at
instantiation"; this is what that sentence was for.

**`Self::Item` in a declared signature.** A generic impl's methods are
compared against the trait's *as written*, because its parameters stand for
nothing yet — so `Option<Self::Item>` and `Option<t27>` looked different. The
impl's own choice is substituted textually first.

**Associated type bindings.** `I: Iterator<Item = t27>` (Ch. 4 §1.7) did not
parse: a bound's `<…>` accepted types, and `Item = t27` is not one. Both forms
are read now and told apart by the `=`, and the binding is checked — an
argument says *which* implementation is meant, a binding says what it must
have chosen.

**Calling a closure that is not a name.** An adaptor holds its closure in a
*field*, and `(self.f)(x)` had nowhere to go: a call was a path followed by
`(`, and nothing else. Any expression may be called now, and the diagnostic
for calling something that is not a closure says so.

**Inferring a closure inside a literal.** A closure has no type until it is
lowered, so `Map { inner: c, f: |x| … }` could not tell what `F` was, and
`total(Map { … })` could not tell what `I` was either. `instantiate_fn`
already lowered a closure *argument* early and bound it to a name; literals
and literals-holding-closures do the same now. What still needs writing is the
closure's own parameter type — `|x: t27|` — because `Map<I, F>` puts no `Fn`
bound on `F`, and a named `F: Fn(A) -> B` bound is not parsed. That is the one
piece left, and Ch. 4 §4.3 is where it belongs.

**And a pre-existing bug the adaptors uncovered.** `Filter::next` is a `loop`
whose every path `return`s, and the compiler emitted the loop's exit block
anyway — unreachable, and reading a slot nothing had defined on a path that
reaches it. A loop nothing breaks out of has type `!`, and the block after it
is emitted no more than the block after a `return` is.

That bug was reachable from Ch. 0 alone and had never been seen, because a
reduced version of it *passed*: the function was unused, so `keep_reachable`
(G9.7) dropped it before the verifier ran. Dead-code elimination hid a bug
from the checker that would have caught it.

**Fixed by reversing the order**: `lang::compile` verifies and *then* prunes.
A function nothing calls is still a function this compiler emitted, and an
ill-formed one is a bug whether or not any program reaches it. Confirmed the
way the eviction rule was — by disabling the `loop` fix and checking that the
unused function is now reported, where before it was silently dropped.

The general shape is worth keeping in mind wherever a pass removes code: a
checker that runs after a remover only checks what survived, and what a
remover removes is exactly the code nobody looked at.

`Iterator` moved into the prelude, where Ch. 4 §5.7 always said it was the
language's own. Three tests and `examples/trust/demo.tr` declared their own
and now do not.

**G9.10 — `!` was a type the compiler had and the specification did not.**
`Ty::Never` has existed since the beginning: `return`, `break` and `continue`
produce it, G9.9 gave it to a `loop` nothing breaks out of, and it prints as
`!` in diagnostics. No chapter defined it, the grammar had no production for
it, and `fn f() -> !` was a parse error. A user could be *shown* a type they
could not write and could not look up.

Ch. 1 §2 defines it now — the type with no values, from which both facts
follow: nothing can be a `!`, so no place has that type and it has no width,
size or alignment; and a value of a type with no values can be a value of any
type, vacuously, so an expression of type `!` stands wherever a type is
wanted.

**And it was not enough on its own.** The only way to *reach* `!` was
`loop {}`, which hangs rather than stops, so a library could not write "this
cannot go on" — which is what `Ch. 5 §4.3`'s `unwrap` needs. Ch. 1 §6 gains
`trap()`, of type `!`, and the name is the machine's: AM §4 calls it a fault,
the ISA has a `trap` instruction and TIR a `trap` terminator, and a second
word here would have been the only place the stack disagreed with itself.

With both, Ch. 5 §4.3 is written in the language:

```
impl<T> Option<T> {
    fn unwrap(self) -> T {
        match self { Option::Some(v) => v, Option::None => trap() }
    }
}
```

`unwrap`, `unwrap_or`, `is_some`/`is_none` and the `Result` four are now
prelude Trust. `expect` still does not exist, for the reason §4.3 gives.

**The citation checker caught the author again**, and this time on the same
turn: three comments named a subsection of Ch. 1's type table, which has
none. That is the third batch of citations it has caught and the second
written by someone who had just read the section they were citing.

**G9.11 — the consuming methods, a method resolution that never worked, and
what a prelude is.** Ch. 5 §3.3 is built. `count`, `last`, `nth`, `find`,
`position`, `all`, `any` and `for_each` are **provided bodies** on `Iterator`,
so an implementation gets them by writing `next` alone. `sum`, `product`,
`min`, `max` and `fold` are free functions taking the bound on the *iterator*
— `fn sum<I: Iterator<Item = t27>>(it: I) -> t27` — because a bound on an
associated type is not written yet, and this says the same requirement where
the language can say it.

**A method with type parameters of its own could never be called.** Method
resolution looked the key up in `sigs`, and a generic function is not there:
it is in `generic_fns`, to be instantiated at the call site. So *any* method
taking `impl Fn(…)` was unreachable — not a limit anyone had recorded, just
one nothing had tried. Both receiver forms needed it, and the second was
wrong twice before it was right: a temporary receiver has to be bound at
**its own** type and borrowed afterwards, because binding it at the
reference's type says the slot holds an address when it holds the value, and
the machine says `F_ALIGN` two instructions later.

**The prelude occupied the only namespace there is.** Adding `sum`, `min` and
`max` to it broke three tests, all of which defined their own. There are no
modules (Ch. 0 §1.3), so the fix is the rule a prelude has everywhere: **a
program's own item shadows the prelude's of the same name**. The prelude is
now parsed separately from the program rather than pasted in front of it,
which also removes the line-number arithmetic the paste needed — a program's
error lines are its own, with nothing to subtract.

**G9.12 — `Box`, and two things that had never come up because nothing owned
a pointer before.** Ch. 5 §2.3 is built for a sized `T`: `Box::new`,
`Box::try_new`, `*b`, `b.field`, moves, drops, and `Option<Box<T>>` in one
word.

**Why it is the compiler's and not the library's.** `alloc` returns a
*pointer*, and Trust has no way to name one — so the declaration is emitted in
TIR rather than written in Trust, and everything that reads it is inside the
compiler. That is the whole of `Box`'s special status; owning, moving,
dropping and dereferencing are what Ch. 3 already does to any value.

Two smaller consequences fell out. Testing an address against null goes
through a slot — `store ptr` then `load t27` — because TIR's `cmp` takes
integers and there is no int↔ptr cast (TIR §5); that is the same move the
niche machinery makes for `Option<&T>`. And the name mangler replaced spaces,
commas, brackets and parentheses but not **angle brackets**, so
`Option<Box<t27>>` reached the assembler with a `<` in its name — a hole that
had waited for the first type whose `Display` writes one.

**Two pre-existing bugs it turned up.**

`s = f(s)` did not work: assigning to a moved-out local did not give it back,
so the second `s = f(s)` said `s` was moved. Ownership is tracked per local,
so a *whole* local that is written to owns again — a field is not enough,
because that re-initializes part of it.

`*b` moved `b`. Dereferencing reads the pointer, not the value it owns, and
`*r` never moved `r` because a reference is `Copy`. `Box` is the first thing
that is a pointer and is not.

**A recursive type**, `enum Tree { Node(Box<Tree>, …) }`, is G9.13.

**G9.13 — Ch. 2 §8's example runs, and the three bugs between it and here.**
The binary tree that chapter wrote against `Box` compiles and runs. Getting
there needed one design change and turned up two bugs, and neither bug was
about `Box`.

**Drop glue moved out of line.** It was generated *inline*, field by field, at
each place a value died — and inlining recursion does not terminate, so a
recursive type stopped at the depth limit. Every nominal type that needs
dropping and has no destructor of its own now gets a synthesized `drop.T`.

The synthesis adds no mechanism. `drop.T` takes `self` by value and has an
**empty body**, and the fields are dropped when its frame ends — which is what
`fn drop(self) {}` already meant (Ch. 4 §5.2). The glue is a destructor that
does nothing, and that was already the definition.

It is synthesized **at the first drop**, not by a pass over the file, and the
first version got that wrong: a pass over the file fixed
`enum Tree { Node(Box<Tree>, …) }` and left
`enum List<T> { Cons(T, Box<List<T>>) }` at the depth limit, because an
instantiation of a generic type does not exist until something asks for it —
`List<t27>` is named while a body is being lowered, after any pass over the
file has run. The lazy form uses the queue closure bodies already use.

**A move in one `match` arm was a move in the next.** Arms are alternatives,
not a sequence, and `if`/`else` had joined ownership from the start; `match`
had not, so the first arm's move made every arm after it complain. Three arm
loops needed the same snapshot-restore-join `if_expr` already had.

**A niche-encoded enum dropped its untagged variant unconditionally.** The
untagged variant is recognized *by elimination* — everything else was tested
and did not match — and that needs something to eliminate. Only *droppable*
variants were tested, so an enum whose one droppable variant is the untagged
one had no tests at all, and dropped it whatever the discriminant said.
`enum Tree { Leaf, Node(Box<Tree>, …) }` is exactly that shape: `Leaf` lives
in the `Box`'s niche and has nothing to drop, so dropping a `Leaf` freed
whatever its storage happened to hold. Every variant with a discriminant is
tested now, droppable or not; one with nothing to drop jumps to the join, and
that jump is the elimination.

The third is the interesting one, because it is a bug the niche optimization
*created*: without niche encoding every variant has a discriminant, and the
elimination case does not exist. Ch. 2 §6 buys a word and costs this.

**G9.14 — `Vec`, and the third reason something is a language item.** Ch. 5
§2.6 in part: `Vec::new`, `push`, `len`, indexing and dropping. Three words —
the allocation, the length, the capacity.

`Box` is the compiler's because `alloc` returns a pointer and Trust cannot
name one. `Vec` inherits that and adds a second reason: **the room beyond the
length is memory that is not yet a `T`**, and this language has no way to say
so. A `Vec` written in Trust would have to store `Option<T>` in every slot to
have a spelling for "not yet", which costs a word per element for any `T`
without a niche. So the choice was forced rather than made.

**Growth doubles from four**, and the factor is in the specification rather
than left to the implementation because a program that pushes *n* elements is
entitled to know it did O(*n*) work in total, and the amortized argument is a
property of the factor (G9.4 records why that direction is the reversible
one). There is no `realloc`: growth allocates, copies and frees, which is what
§7 reserves a third target function for.

Two things the implementation had to be told twice. The pointer word is read
and written **as a pointer** whatever the layout calls it, because TIR has no
int-to-pointer cast and an address that is going to be offset must have been a
`ptr` all along. And a `Vec` receiver is a **place**: reading it would move it,
and `v.push(x)` in a loop would then move the same `v` on every iteration —
the borrow checker said so, which is the second time this session that
"a method on an owning receiver" needed the place and not the value.

Not built: `pop`, `insert`, `remove`, `clear`, `with_capacity`, `reserve`, and
the coercion of `&Vec<T>` to `&[T]` — without which a `Vec` cannot be handed
to anything that takes a slice. `String` and `collect` wait on that coercion
and on `FromIterator`.

**G9.15 — the rest of `Vec`, `String`, and the drop bug the ledger found
underneath them.** `pop`, `clear`, `reserve`, `capacity`, `is_empty`, the
coercion of `&Vec<T>` to `&[T]`, and `String`.

`pop` returns `Option<T>` and takes the length down **before** it reads the
element, so what comes back is no longer inside the `Vec` and cannot be
dropped twice. `clear` drops the elements and keeps the allocation, which is
the whole difference between clearing a `Vec` and dropping one. `reserve` does
not double — a program that says how much it wants has said something `push`'s
guess cannot improve on — so it gets `len + n` exactly, and asking for room
that is already there is not an error but nothing.

`insert` and `remove` stayed unbuilt for a reason worth writing down rather
than a lack of time: both shift a run of elements, and a shift *up* cannot use
the forward copy the language already has. That is `copy_within`, which §7
reserves, and building a private one for `Vec` would be deciding the reserved
question sideways.

`String` **is** `Vec<char>`, resolved as that name and not as a wrapper, so it
inherits `new`, `push`, `len` and dropping, and `&String` becomes `&str`
through the coercion. The only rule it needed of its own was that
`String::new` may skip the annotation `Vec::new` requires — the element type
is in the name.

*The bug underneath.* The drop ledger found its **fifth**, and once again
nothing else did. `enum_arm` pushed a scope for an arm's pattern bindings and
popped it **without dropping them**, so a binding stayed in the owned set at
its arm's depth and was swept up by the next scope that happened to end at the
same depth — in the reduced case, an unrelated block three statements later.
It had been invisible because every test that bound a payload bound one that
needed no destructor.

The fix is one line in principle and two conditions in practice. The drops
must be emitted **before** the arm's jump to the join, because a drop emitted
after a terminator is in no block at all — so the arm's scope ends inside
`arm_body`, between storing the arm's value and leaving. And the bindings own
only when the scrutinee was matched **by value**: matching a value moves it,
so the bindings receive what it held, while matching through a reference moves
nothing and the bindings are copies of storage the referent still owns. The
first version of the fix had no such condition and freed the binary tree of
G9.13 three times over — `total(t: &Tree)` walks a borrowed tree, and every
`Node(l, x, r)` arm was handing `l` and `r` a `Box` the tree still held.

*Closed, and the closing found a second thing.* The first attempt marked a
borrowed binding with `mark_moved`, which says the place is **uninitialized**
— so a program that only wanted to read `p.id` through a `&Holder` was told
`p` had been moved out of. `Owns` had been carrying two questions at once:
"must this be dropped" and "may this be read", which have the same answer for
every place but this one.

They are separate now. A borrowed binding gets a slot, a name and **no entry
in the owned set**, so nothing drops it, and it is marked so that moving out
of it is refused with a diagnostic that is true: *"`p` names part of a value
this `match` borrowed, so it cannot be moved out of; borrow it instead."*
Ch. 3 §1.3 states the rule.

Binding **by reference** — Rust's match ergonomics, which would let the
refused program be written by naming what it is — stays **reserved**. It was
built far enough to see what it costs, and then reverted; the decision was the
author's and this is what it was made on.

*The implementation is not the obstacle.* Binding by reference is ten lines:
declare the binding at `&T` and store the field's address instead of a copy
of it. What is here now — a `borrowed` flag on the local and one refusal — is
not something that would have to be worked around, it is something that would
be deleted.

*What it costs is `total(&*l)`.* With `l` bound at `&Box<Tree>`, `&*l` is a
`&Box<Tree>` and the function wants a `&Tree`. Rust closes that with **deref
coercion**; Trust has none, and the program in question is the binary tree
Ch. 2 §8 prints. So the real threshold is not match ergonomics at all — it is
the language's *third* implicit conversion, after `&T` to `&dyn Trait` and
`&Vec<T>` to `&[T]`. Both of those convert a representation and neither
follows a user-defined trait. A deref coercion is a different kind of thing
and deserves to be decided as one, rather than arriving as a consequence of a
change to `match`.

*Is it still reversible later?* Yes, and the cost is source rather than
design:

- Nothing promises otherwise. Ch. 3 §1.3 states the rule and then reserves
  this in the next paragraph, which is what that paragraph is for.
- **No spelling works under both.** `Opt::Some(v) => v` compiles today and
  needs `*v` after; `*v` today is "cannot dereference a `t27`". So switching
  is an edit, not a discipline that can be adopted early.
- **The shape it touches is narrow**: matching on a *reference* and using a
  scalar payload *as a value*. Nearly every `match` in the library is on an
  owned value — `fn unwrap(self)` takes `self` by value, and `it.next()`
  returns one — so the library is almost entirely unaffected.
- **Today the whole tree has three such sites**, all in tests. The cost grows
  with how much reference-matching code accumulates before the switch, and
  with nothing else.

**G9.16 — `if` in statement position is still an expression to the parser.**
`if c { … } (a) * 2` parses as a *call* of the `if`'s value, and reports that
`()` is not callable. Rust resolves this by giving block-like expressions in
statement position a statement reading; Ch. 0 §6's grammar says nothing about
the distinction.

*Decision:* Rust's rule. A block-shaped expression in statement position is
parsed **alone** — it ends at its closing brace, and an operator after it
begins the next statement. Ch. 0 §5.2 states it.

The choice was between that and requiring parentheses to disambiguate, and it
was not close: `block_like` already let these expressions stand as statements
without a `;`, with the comment "as in Rust", so the language had committed to
half the rule already and the half it was missing was the half that decides
where the statement *ends*. Tail position is untouched, so
`fn pick(c: bool) -> t27 { if c { 1 } else { 2 } }` still means what it looks
like, and the cost is that a method call on a `match` in statement position
needs parentheses — paid where it is visible.

**G9.17 — `print` moved into the library, and three things had to move with
it.** Ch. 5 §1.5 wrote `print` as library code from the day it was written;
`examples/trust/hello.tr` defined it anyway, in fifteen lines, because the
pieces it needs were not all there. They are now, and `hello.tr` is a `main`
with one statement in it.

`putchar` is declared in the prelude, which makes it a **required target
function**, the third after `alloc` and `free`. That qualifies AM §5's "I/O is
per target", and the qualification is narrow enough to state exactly: the
*character ports* are part of what a target must offer and nothing else about
I/O is. A target with no console supplies a `putchar` that discards, the way a
target with no heap supplies an `alloc` that fails. A program may still
declare `putchar` itself — its item shadows the prelude's, and both resolve to
the target's one symbol.

Three things moved with it.

**`char::to_utf8` now has the signature §1.5 always gave it**:
`(self) -> ([t9; 4], taddr)`, no buffer and no `Option`. The implementation
had diverged into `(self, out: &mut [t9]) -> Option<taddr>`, which made every
caller handle a case that cannot happen — a `char` is always encodable and
four units is always enough for one. The specification was right and the
implementation was wrong, which is the direction this log usually does not go.

**`let` binds a tuple.** §1.5's `print` is written `let (units, n) =
c.to_utf8();` and `let` took a single identifier. It is sugar, expanded in the
parser into a hidden binding for the whole tuple and one `let` per element, so
nothing below the grammar learns a new shape and the ownership rules need no
case of their own: moving out of one element leaves the others usable, which
Ch. 3 §1.3 already says. Ch. 0 §5.2 states it. `mut` is per element, and it is
the only pattern a `let` takes — anything richer is a `match`.

**`prelude_functions` was missing the provided methods.** A trait's provided
body is synthesized per implementing type (Ch. 4 §1.5), so `impl Iterator for
Chars` produces `Chars.count`, `Chars.nth` and five more that appear in no
impl block. That set decides which names `keep_reachable` may treat as
prunable, and a name it misses is treated as a **root** — so the prelude's own
iterator, and everything it called, was emitted into every program that never
mentioned text. Two tests that assert a `match` on a `trit` compiles to one
`br3` and *no* comparison caught it, which is a use for an exact-output test
nobody designed it for.

*What it was worth.* `examples/trust/HPL.tr` printed through a `print_str`
over `&[t9]`, and every line of its output was a hand-encoded array of ASCII
decimal with the text in a comment above it. All 53 became string literals:
**277 lines deleted, 63 added**, and the comment can no longer disagree with
what is printed.

It also got *cheaper*, which was not the expectation. Printing through the
general encoder cost **+8.7%** of the whole program's instructions, so `print`
takes the one-unit case directly — a character below 128 is one code unit and
*is* that unit, which is the test `utf8_len` makes first anyway, and taking
the branch there skips building a four-element buffer and returning it. With
that, the total is **3 623 789 → 3 615 930, −0.2%** against the hand-rolled
loop it replaced.

The measurement is unchanged where it matters, and that was checked rather
than assumed: the four solve intervals are bit-identical (542 928, 594 856,
844 517, 916 216 instructions) and every Time and Gflops line is byte-for-byte
what it was. What moved is the *absolute* cycle counters printed either side
of each solve, because there is more printing before them — which is the
reading ISA §2.3 warrants and the difference is what HPL uses.

*`print_char`, and what it cost.* The library could print a string and not a
character, so a program with a character it had *computed* — as against one it
had written down — had nothing to call but `putchar`. HPL's `print_rule(61,
80)` is what that reads like at the call site. With `print_char` it is
`print_rule('=', 80)`, and every constant `putchar` in the file is a literal.

That cost **+1.0%** (3 615 930 → 3 661 618), and the cause is worth naming
because it is not the design: **there is no inliner and no constant folding**.
`print_char(' ')` is a call, a cast and a compare where `putchar(32)` is a
call, and `spaces()` runs it a few thousand times. With an inliner the compare
folds — `' '` is a constant and `v < 128` is decidable — and the difference
goes to zero. §8 lists both passes as unbuilt; this is the first measurement
that says what one of them is worth.

The same reasoning kept `print` from being written as a loop over
`print_char`: it is that function's body repeated, because calling it per
character costs 1.3% of HPL. It is the one repetition in the prelude and it is
marked as one.

`putchar` survives in HPL in four places, all inside the digit loops, and that
is a boundary rather than an oversight: formatting a number is what §7 does
not define, so those loops *are* the formatter, and the bottom of a formatter
is where a value becomes a character. Going through `char` there would mean
`char::try_from(48 + d)` and an `unwrap` per digit — a check that cannot fail,
paid once per character printed.

*Still open.* Ch. 5 §1.5 would rather write `for c in s`, and that needs
`impl IntoIterator for &str` — an impl whose **self type is a reference**,
which the parser rejects with "expected a name, found `&`". `chars()` spells
the call out in the meantime. Related and also open: the `for` desugaring
calls `.next()` on the loop's expression directly, where Ch. 4 §5.7 says it
calls `IntoIterator::into_iter` first. The two are the same fix — the trait
and the blanket impl are writable today, and the impl on `&str` is not.

**G9.18 — `F: Fn(A) -> B` as a written bound, and what it was hiding.**
Ch. 5 §3's adaptors were built and could not be used. Checking why turned up
worse than the recorded "closures need annotations":

- `it.map(f)` did not resolve at all — `map` was not a method on `Iterator`,
  and nothing handed a program a `Map` except writing the struct literal out.
- `Map`'s `Item` was a **fixed `t27`**, because there was no way to name the
  closure's result. Silently wrong for every closure returning anything else,
  and nothing tested it.

Both are the same missing thing. `impl Fn(A) -> B` in argument position had
existed since Ch. 4 §2.2 as an anonymous type parameter with a bound; what was
missing was writing that bound where the parameter has a **name**. It is now
the same desugaring from both spellings, and nothing after the grammar can
tell them apart.

Four things had to follow.

**A `Fn` bound settles the parameters its signature names.** `impl<I, B, F:
Fn(I::Item) -> B> Iterator for Map<I, F>` has three parameters where `Map`
takes two, and an impl's parameters were matched to the self type's arguments
by *position*. They are matched by name now, and a parameter the self type
does not name is settled from a closure's recorded signature — which is
possible because a closure has exactly one. `Map`'s `Item` is `B`.

**The associated type needed the same.** `type Item = B` is resolved when the
instantiation exists, from a table that had the same positional zip, and it
reported `Map<Count, closure2>` "chooses no type for `Item`". The existing
test caught this, which is the second time an exact-output test has caught a
change nobody wrote it for.

**`Self` is substituted in bounds.** `fn map<B, F: Fn(Self::Item) -> B>`
writes `Self` where a method's *constraints* are, and substitution walked its
types only. A `Self` surviving to lowering is one written where there is no
impl, so it was reported as exactly that — truthfully and unhelpfully.

**A bound is checked under the call's environment.** `B` belongs to the call
that instantiated the method, not to the function being lowered, and the
check used the latter.

**G9.19 — instantiation in two stages, and §3.1's table becomes methods.**
A method with type parameters of its own inside a *generic* impl now works, so
`it.map(f).filter(p)` does, so Ch. 5 §3.1's adaptors are the provided methods
that section always described.

Such a method has two sets of parameters settled at different moments: the
impl's as soon as the receiver's type is known, the method's only from the
call's arguments. `instantiate_with` wants an environment naming every
parameter, so the impl's half is settled at method resolution and the method
is put back into the queue as an ordinary generic function of what is left —
a `Special`, living beside `generic_fns` in a cell for the same reason
`extra_fns` does.

Four things had to follow, and three were pre-existing:

**`same_ast_ty` had no case for an associated type.** `I::Item` did not equal
`I::Item`, so a generic impl's `next` never matched the signature `Iterator`
declares — which is the real reason **every adaptor's `Item` was a fixed
`t27`**. A generic impl could only choose an associated type it could *name*.
They follow their inner iterator now, and a `Map` whose closure returns a
`bool` no longer claims to yield `t27`.

**A method's own parameters may not reuse the impl's.** Both live in one
environment; a shadow makes `Self` mean two things at once, and the second
`map` of a chain looks for a receiver the first never produced. Rust refuses
this too. It is an error with a message rather than a wrong program.

**`fn_hint` resolved the wanted closure signature under the *caller's*
environment.** A specialized method's bound is written in the impl's
parameters, which live in what the specialization settled, so `Map<I, F>::Item`
came out as "`I` is not a type in scope".

**A receiver that is not a place was lowered twice** — once to be typed, once
to be passed. Harmless until it *contained* a closure: `c.map(f).count()` made
two closure types and then reported that the receiver was neither. It is
lowered once and bound, which is what the argument path already did for a
closure and for a literal holding one.

An argument that is a method call is lowered eagerly for the same reason:
`sum(it.map(f).filter(p))` cannot be told its argument's type without
resolving the chain, and resolving it is lowering it.

**G9.20 — `collect`, and `Vec` becoming a type the library can implement.**
Ch. 5 §3.3's `collect`, and `FromIterator` under it.

*The decision, taken with the author.* The specification wrote
`trait FromIterator<A>`, a trait with a type **parameter**. Implementing that
turned out to need a gap of its own: a *generic* impl of a *parameterized*
trait — `impl<T> Make<T> for Pair<T>` — did not register at all, because the
trait's arguments were resolved to concrete types when impls are collected and
`T` is not one. That hole is closed now; see G9.24. The decision below stands
on its own merits and was not reversed.

The element is an **associated type** now. A type is collected into from one
element type and no other: there is no "collect a `Vec<T>` from an iterator of
`&T`" here, because there is nothing to clone with, and `String` *is*
`Vec<char>` rather than a second thing collecting characters. So the element
is an output of the implementation, which is what an associated type is for.
Ch. 5 §3.3 is corrected. The general hole stays open and is worth its own
entry when something needs it.

**`Vec` is a nominal type now, as well as a language item.** §3.3 says
`Vec<T>` implements `FromIterator`, and an impl needs something to attach to:
`Vec<T>` registers its instantiation and answers to the mangled name, so
`impl<A> FromIterator for Vec<A>` is written in the library in Trust. `String`
registers the same instantiation, which is what makes `Vec`'s impls findable
for it — `String` is not a second type.

Two things were broken and one was missing.

**`copy_typed` had no case for a `Vec`.** One fell through to the scalar arm,
so *one word was copied where three were meant* — and a `Vec` returned from a
function arrived as the **address of the buffer it had been written into**.
Nothing had caught it because a `Vec` had always been built where it stood;
`from_iter` is the first function that returns one.

**An associated function of a generic type could not be called by path.**
`Pair::twin(5)` reported "`Pair.t27` is not an enum in scope": the head was
already the instantiation's mangled name, and nothing looked for its methods
under the base. This was never `Vec`-specific.

**A method that still has parameters of its own** reached `call_key`, which
wants a signature that exists. It goes through the generic path now, which is
where the call's arguments settle them.

*A note on the bisect.* `it.filter(|y| y % 2 == 1)` yielded nothing, and the
compiler was right: `%` is the **symmetric** remainder, so `3 % 2` is −1 and
`y % 2 == 1` is never true. Written `!= 0` it works. Ch. 1 §4's rounding, in
the fourth different place this session.

**G9.21 — the rest of `Vec`, and a reservation that was not there.**
`insert`, `remove`, `with_capacity` and `push_str`.

*A correction.* G9.14 said `insert` and `remove` stayed unbuilt because the
shift "is `copy_within`, which §7 reserves". §7 reserved no such thing — that
sentence pointed at a reservation that did not exist, and I wrote both it and
the §2.6 prose citing it. The reasoning was wrong twice over: `push` already
copies memory inside itself, so a shift inside `Vec` decides nothing about
what a *program* may call on a slice. §7 now reserves `copy_within` properly,
as the slice method it is, and says why that is a separate question: a program
can overlap two ranges of its own choosing.

`insert` accepts `i == len`, which is `push`. `remove` reads the element
**before** the shift, so the shift does not drop it and the caller owns it
exactly once — checked with the ledger, which is still the only thing that can
say so. `insert`'s shift runs **downwards**: a forward copy moving a block up
overwrites what it has yet to read, and which direction is safe is a property
of which way the block moves.

`push_str` is a loop around `push`, written in the library — see G9.22.

**G9.22 — an impl may name one instantiation.** `impl Vec<char>`, and
`impl Pair<t27>` with it. The parser rejected both: it required an impl's
parameter list and its self type's arguments to be empty together. Only one
direction of that is a real rule — parameters the self type does not name are
parameters nothing determines — and the other direction forbade an impl for a
*single* instantiation, which has no holes to fill.

Such an impl's methods are keyed by the name the instantiation answers to
everywhere else, and `Self` is that name rather than an application of it: the
arguments are already in it. With that, `String`'s one method of its own is
written in the library in Trust rather than in the compiler, which is what
Ch. 5 §2.6 always claimed.

It also needed a `Vec` instantiation to resolve *back* from its mangled name.
Every other one has a struct behind it; `Vec.char` has a language item, so
`Self` in `impl Vec<char>` had nothing to resolve to.

*And it found a leak.* `needs_heap` is set while a body is lowered, and a
prelude body that uses the heap sets it whether or not anything calls that
body — so `alloc` and `free` were declared by **every program** the moment the
library gained a method that pushes. Declarations are pruned now, by the same
rule as functions: what a program owes its target is what its *surviving* code
calls. The test that says a declaration lowers to a declaration caught it,
which is twice this session that a test written for something else has.

**G9.23 — an impl whose self type is a reference, and `for c in s`.**
Ch. 5 §1.5 wrote `for c in s` from the day it was written. It works now.

`impl Trait for &T` is written. The methods are found under the **referent**,
because a reference's methods have always been the referent's (Ch. 3 §2.3);
what changes is that `Self` is the reference, so a method may take `self` **by
value** for a type that could not otherwise be passed at all. That is exactly
what `str` needs: it is unsized, and `IntoIterator::into_iter(self)` could
never have taken it. `impl Trait for &mut T` is refused with a message.

**A blanket impl covers what its bounds allow, and no further.** The rule had
been that a trait with *any* blanket impl may not be implemented by hand at
all, which forbade `impl IntoIterator for &str` outright — `&str` is not an
iterator, so `impl<I: Iterator> IntoIterator for I` says nothing about it and
there was never an overlap. Ch. 4 §5.6 is corrected, and the `Into` sentence
that stated the old rule with it.

*The debt, and paying it.* Ch. 4 §5.7 says an iterator is an `IntoIterator`
**by blanket impl**, and the first attempt made it a provided method instead,
because a blanket impl appeared not to carry an associated type. It was one
line: `blanket_impl` never applied `subst_assoc_fn`, so `Self::Out` in a
declaration was never replaced by what the rule chose for it, and a blanket
impl that declared an associated type **never matched the signature it was
implementing**. The error said so and the next error — "`A` has no method
`wrap`" — was the one anybody would read.

With that, the blanket impl §5.7 specifies is written, and two more places
had to learn about rules:

- **A bound is satisfied by a blanket impl.** `fn f<T: IntoIterator>(x: T)`
  now accepts an iterator, because a rule says so rather than because a pair
  was written out. Only the *self* parameter's bounds are checked, since the
  rule's other parameters are settled by a call and this question is asked
  before there is one.
- **A reference's impls are looked for under its referent**, which is where
  `impl Trait for &T` puts them.

Nothing is owed here now.

**G8.14 — inlining, and a budget that wants to be small.** A call to a small,
non-recursive function becomes the callee's blocks, renamed, with its
parameters bound to the call's arguments and its `ret` turned into a branch to
whatever followed the call. **HPL: 3 661 618 → 3 247 122, −11.3%**, and the
four solve intervals fall about 12% each — this is the first change since the
register allocator to move the measured number rather than the total.

Two things it had to be told.

A vtable names its methods by **address** and nothing calls them, so a global's
initializer is a root for the pass that drops what is now uncalled. Three
trait-object tests said so.

A callee that never returns leaves nothing branching to the continuation, and
what followed the call is then code control cannot reach. Dropping it is not
an optimization: an unreachable block is one the verifier rejects.

*The budget is 24 instructions, and larger is worse.* Measured on HPL:

| budget | instructions | image |
|---|---|---|
| 24 | 3 247 122 | 24 741 lines |
| 40 | 3 311 650 | 24 749 |
| 64 | 3 315 827 | 35 735 |
| 96 | 3 383 444 | 36 769 |

A larger splice costs more in spills than it saves in calls, which is what a
machine with 27 registers and no cache should be expected to say. It is also
why Ch. 5 §1.5's `print` still repeats `print_char`'s body: `print_char` is
over the budget, and raising the budget until it fits is 3 315 827 against
3 247 122. The repetition is now a measurement rather than a guess.

**G8.15 — the interpreter is 32× faster, and nothing in it was clever.**
2.57 → **83.2 M instructions/second** on HPL; 1.26 s becomes 39 ms.

`word::shr3` was a **loop**: `k` iterations of subtract-the-low-trit and
divide by three. Every field the decoder extracts goes through it, and the
decoder runs once per instruction. Its own doc comment already stated the
closed form — "discarding low trits *is* round-to-nearest division by 3^k,
and since 3^k is odd no tie can arise" — so the loop was the definition
written out rather than an implementation of it. `floor((2v + p) / 2p)` is
that sentence as arithmetic: round-half-up, and no tie can arise because a
tie needs `2v = (2m+1)p`, whose right side is odd. `pow3` is a table now for
the same reason — `3i128.pow(n)` is also a loop. **2.57 → 21.7 M/s.**

Then a **decode cache**, direct-mapped, 2^16 entries, keyed by address.
**21.7 → 61.3 M/s.** It is the only change here with a soundness condition:
an image is ordinary memory and nothing stops a program from writing over its
own code, so every store forgets the decodings it may have invalidated. There
is a test that overwrites an instruction *after* executing it once — the loop
is what makes the first pass populate the cache — and it halts with 2 instead
of 1 if the invalidation is removed.

Then the arithmetic and memory paths, which were three more of the same
mistake:

- **`overflowing` divided on every `add`.** It computed `wrap_to(exact, 27)`
  — an `i128` remainder — and then asked `fits(exact, 27)` to decide whether
  anything had overflowed. A value that fits *is* its own residue, so the
  common case needs no division at all, and it was already computing the test
  that says so. `wrap_to` gained the same early return.
- **`Memory::word` looked its page up three times** and scaled each tryte by
  `3i128.pow(9i)` — another loop, on the path of every `ld.word`, which is an
  eighth of what HPL executes. One lookup and the `POW3` table. `set_word`
  the same.
- **`split` used the sign-correcting `div_euclid`/`rem_euclid`** on addresses
  every caller had already established were in memory, and memory starts at
  zero.

**61.3 → 83.2 M/s**, and **2.57 → 83.2 overall: 32×.**

*Why this matters here more than it would elsewhere.* There is no ternary
hardware and there is not going to be. Instructions retired and instructions
per second are two factors of one product, and every measurement in this log
until now moved only the first: −66.6% from the backend, −11.3% from
inlining. This is the first time the second has moved at all.

Every one of these was the same shape: a definition written out as a loop, or
a general form used where a cheap test had already ruled out the general
case. None of them needed a new idea, and all of them had been there since the
interpreter was written — which is what a profile is for, and why there was
not one until this session.

**G8.16 — constant folding, and −62 instructions.** An instruction whose
operands are all constants has a constant result, and the arithmetic is
`trit_core`'s — where the AM's rounding, wrapping and trit operations are
defined — so folding and executing cannot disagree without one of them being
wrong.

A fold that would **fault** is left alone: `div` by zero and a `.trap` that
overflows are things the *program* does, at the point it does them, and a
compiler that performed them early would report a fault for code that may
never run.

*The payoff, which was measured before the pass was written and again after.*
HPL's TIR has 440 binary operations and **31** of them have two constant
operands. The pass removes **62 instructions from 3 247 122** — 0.002%. That
is what the static count said it would be, and the reason to write it down is
that "constant folding" sounds like it should matter and here it does not:
the backend already folds constants into immediates where an immediate exists,
and the frontend does not emit arithmetic on two literals.

It stays because it is correct, costs nothing at run time, and closes a §8
item — not because it was worth 62 instructions.

**G9.24 — a generic impl may implement a parameterized trait.**
`impl<T> Make<T> for Pair<T>`, and `impl<T> From<T> for Wrapper<T>` with it.

The arguments a trait is given were resolved to concrete types when impls are
collected, and an impl's own parameter is not one — so such an impl was
dropped with an error about `T` not being a type, and every method on it was
missing afterwards. This is what made Ch. 5 §3.3's `FromIterator` take an
associated type instead of the parameter it was first written with (G9.20);
that decision stands on its own merits and was not reversed.

An impl whose trait arguments are **exactly its own parameters** needs no
qualifier to tell it from another, because there cannot *be* another: the
arguments are determined by the self type, so two such impls for one
(type, trait) would be the same impl. Which arguments it means for a given
instantiation is worked out then rather than recorded at the impl — the same
shape as a blanket rule's bounds, and in the same place, so a bound like
`P: Make<t27>` is satisfied by asking rather than by looking a pair up.

*Not done, and it is the next thing in the way.* Ch. 4 §5.5's `From` and
`Into` are **not in the prelude**, so a program that wants them declares them,
which is what every test here does. Putting them in the prelude collides with
exactly those programs: the shadowing rule is about *named* items, and an impl
has no name. Extending shadowing to cover impls is what that needs, and it is
a question about coherence rather than about conversion.

**G9.25 — `a..b`, and what an iterator costs per item.** Ch. 0 §5.5 and
Ch. 5 §3.4. The range is a struct in the library with an ordinary `Iterator`
and needs no bound — `<` and `+= 1` are Ch. 1's, resolved at instantiation —
so `for i in 0..n` works for `t27` and `taddr` alike from one definition.
Only the spelling is a language feature. `..=` is reserved: an inclusive
range cannot express an empty one and cannot reach the top of its type
without wrapping.

**A literal no longer pins a type parameter a neighbour determines.** `0..n`
with `n: taddr` was a `Range<t27>` whose end did not fit, because a struct
literal's inference asked the fields in order and an unsuffixed integer
defaults to `t27` (Ch. 1 §3). A default should be the last thing consulted,
not the first, so literals are asked in a second pass. `0..v.len()` needed one
thing more: `peek_ty` now knows that `len` and `capacity` are `taddr`
whatever the receiver is, which is the only case where a method's result is
fixed without resolving it.

*What it cost, and a fix that came out of measuring it.* `for i in 0..1000`
was **66 097** instructions against **7 010** for the `while` and an index it
replaces. Reading the emitted TIR, `Range::next` is 117 instructions for four
lines of source, and the reason is `build_variant_into`: it **zeroed the whole
enum, tryte by tryte, before writing the payload**, on every construction of
every variant. `Option<t27>` paid six stores each time. The comment said this
made padding "deterministic" — but Ch. 2 §1 says padding trytes have
unspecified contents and G7.4 reads that as every pattern being acceptable, so
it was buying something the specification does not ask for.

Removing it: the range loop is **54 085** (−18%), an adaptor chain is −14%,
and HPL is −150, because HPL constructs no enums in its hot loops. Two tests
failed and neither was about meaning: both had an SSA *number* baked into an
assertion about a *layout*, so deleting six instructions elsewhere broke a
test about `slot tryte[3]`. They assert the size now.

*Then the copies themselves.* `next` returns an `Option<T>`, which without a
niche is two words that go through memory — built into one temporary, copied
to a second, copied to the caller's. An enum is copied as **raw trytes**,
because its payload varies by variant, and that was a tryte at a time: twelve
instructions to move an `Option<t27>`, paid twice per item. But the ends are
word-aligned — AM §2.3 caps alignment at a word — so whole words move first
and only a remainder goes by tryte. Four instructions where twelve were.

The range loop is **38 068** now, from 66 097: **−42%** across the two
changes. An adaptor chain is −31%. HPL is −265 in total, which is what a
program that constructs no enums in its hot loops should see.

*Then the temporaries.* An aggregate has no value in TIR — it **is** its
storage — so a literal has to be built somewhere, and building it in a
temporary that is immediately copied is pure cost. `Range::next` built its
`Option` in one slot, copied it to the `if`'s result, and copied that to the
caller's.

An expression is now told where its value is going before it computes one.
The destination is set in two places — the function's result pointer, for a
body or a `return` — and consumed in one: a struct, tuple or variant literal
builds *there* instead of in a temporary. `if`, `match` and a block's tail are
**transparent** to it, because their value is one of their arms' values.

The rule that makes this safe is one line: **the destination is taken at the
top of every expression**, so only the constructs that put it back can pass it
on, and a subexpression can never take one meant for the expression containing
it. A block gives it to its tail and not to its statements, for the same
reason.

**`for i in 0..1000` is 21 043 instructions, from 66 097** — −68% across the
three changes, and 3.0× the `while` and an index rather than 9.4×. An adaptor
chain is **−54%**. HPL is unmoved, which is right: it constructs no aggregates
in its hot loops, and this was never about HPL.

*Then the call's result pointer, and the `let` that receives it.* A call
consumes a destination rather than passing one on — it *writes* its result —
and it takes one before its arguments are lowered, so an argument cannot take
it. And a `let` whose initializer **computed** an aggregate takes that
storage as the binding's own instead of copying out of it: the temporary was
made for this value and nothing else names it. A *place* is excluded, because
`let y = x` must not make `y` another name for `x`.

The adopted slot is **renamed** to the binding's, so that reading the TIR
still shows which binding storage belongs to. That is not tidiness: the enum
zeroing above, the double copy, and half of the drop bugs in this log were
found by reading TIR, and an optimization that made every `let` anonymous
would have cost more than it saved.

An adaptor chain is **791 → 731**, and HPL **3 246 793 → 3 233 721** — the
first thing since inlining to move HPL at all, because `let` of an aggregate
is something HPL does even though building one in a loop is not.

**G0.18 — `trust run`, and where the three seconds go.** `trustc compile`
writes assembly and stops, `tritium asm` makes an image and `tritium run`
executes one: three commands and two temporary files to answer "what does
this program print", which is the question asked most often. `trust run
file.tr` answers it with nothing on the disk, and `--stats` reports
instructions retired, which is the unit every measurement in this log is
quoted in.

It is a **separate crate**, and that is the point. `compiler/Cargo.toml`
already said why in a comment — "the compiler itself emits assembly text and
stops there" — and the machine knows nothing about Trust. Joining them is a
third thing's job, not either of theirs.

*What it made visible.* HPL takes **2.9 seconds to compile and 40 milliseconds
to run.** Every measurement in this log has been about the second number.
Nobody had looked at the first because the three-command shape hid it behind
two file writes.

**G9.47 — a construct no tree showed.** `for x in e { … }` was turned into
Ch. 4 §5.7's `loop` and `match` **in the parser**, so no AST ever held a
`for`. Three things followed. An editor could not point at one — `trust-lsp`
saw a binding named `it.3` and a `match` nobody wrote. A second parser could
not agree about one without also agreeing about the names the desugaring
invents and the order it invents them in, which is not a fact about the
language. And the desugaring ran before the `for` was known to be
well-formed, so its errors were reported against code the file does not
contain.

`Expr::For` is a node now, and §5.7's desugaring happens at lowering. The
tree a reader sees is the tree they wrote, `bootstrap`'s parser reads a `for`
as a `for`, and the two print the same one.

**G9.46 — a struct could not be taken apart.** Ch. 0 §4 lists
`Point { x, y }` among the patterns and says patterns appear in `match` arms
and in `let`. The implementation read one in neither: `match` on a struct
answered "cannot match on P", and `let` took a name.

That is not only a missing spelling. Ownership is tracked per *local* and
not per place, so a move out of one field marks the whole local moved —

```
let x = p.a;
let y = p.b;          // `p` was moved out of and cannot be used again
```

— which means a struct with two non-`Copy` fields could not be taken apart
at all, by any spelling. Found writing the resolver of Ch. 6 in Trust, where
every rewriting function wants to consume a node and hand its parts to the
functions below it.

A struct pattern in a `match` is implemented, and it is the shape that suits
the ownership model as it stands: one move takes the whole struct apart into
several locals, and nothing is left half-owned. A struct has one shape, so
such a `match` has one arm, one pattern and no guard — there is nothing for a
guard to fall through to.

`let P { a, b } = p;` follows, and so does `let (a, b) = pair;`. Both bind
the whole to a name no program can write, and are lowered through the same
pattern machinery rather than through field reads — `let_tuple` desugared
into `#t.0` and `#t.1`, which is the refused move above written twice, so
tuple destructuring had never worked for a part that owns anything. It does
now. `let (mut a, b) = …` still takes the old path: what a pattern binds is
what would be `mut`, and there is nowhere in a pattern to write that yet.

**Per-place ownership** followed, and Ch. 3 §1.3's sentence — "moving out of
a place leaves *that place* uninitialized, not the whole variable" — is now
what the compiler does. An `Owned` entry carries the direct fields moved out
of it; a move of `p.x` records `x` rather than marking `p`; the drop at the
end of the scope drops what is left, one field at a time, because the type's
own glue would drop all of them; and `p.x = …` puts the field back and makes
the value whole again.

Three things are still refused, and each for a reason worth stating:

- A field moved on one path of a branch and not the other. The drop flag
  decides for a whole local and not for a part of it, so this would need a
  flag per part. Refused rather than leaked — the other path still owns the
  field, and nothing would drop it.
- A field moved inside a loop, which is the same argument G9.9 makes about a
  whole local one projection along.
- Reading the *whole* of a value a field has gone from. It is not a value
  any more; its parts are.

A move through an index or through a reference still moves the whole
conservatively, because what is left of `v[i]` is not a set of names.

**G9.45 — the same double free, one projection along.** G9.27 refused
`take(*r)`: reading a non-`Copy` value through a reference moves it, and
there is nothing there to move *from*. The field and index cases were said
to be covered — "`E::Field` and `E::Index` beside this one have always
checked" — and they were not. They checked *ownership of a local*, and when
the place is reached through a reference the local they mark is the one
holding the reference. The owner is at the other end and drops the value
anyway.

```
fn take_one(h: &mut Holder) -> Note { h.v[h.at] }
```

compiled, and the ledger printed `drop 1` twice.

The prelude's own iterator was written that way:

```
fn next(&mut self) -> Option<T> {
    let x = self.v[self.at];          // moves out of a vector it borrows
    self.at = self.at + 1;
    Option::Some(x)
}
```

so **every `for x in v` over a `Vec` of anything with a destructor dropped
every element twice** — including every `String` in one. It is the worst bug
this repository has had, and the drop ledger found it seven for seven.

Two fixes, and the second follows from the first. A move out of a place
reached through a reference is refused, which is G9.27's rule with the
projections included. And `VecIter` is rewritten to `pop`, the one operation
that hands an element *over* — the vector no longer owns what it gave away,
so exactly one place does. Popping yields the last element first, so the
vector is reversed once on the way in; that is O(n) with no comparisons, and
the alternative is an iterator that cannot be written soundly at all.

**G9.44 — a module renamed the outside.** Ch. 6 §4 said the symbol an item
becomes is its path, with no exception, and Ch. 0 §3.1 says a function with
no body is defined outside the language. Put a `fn getchar() -> t9;` in a
module and the two rules together ask the linker for `input.getchar`, which
nothing has ever defined — the program compiled, assembled, and failed at
the last step with a name no source file contains.

Found by moving seven copies of `read_all` into one module, which is the
first time this repository had a declaration anywhere but a root.

A declaration now keeps the name it was written with, and §4 says so. It
does not name an item of this program; it names something the program is
linked against, which was written first and knows nothing about its modules.
Two modules may declare the same external function and they name the same
one — a declaration is a claim about the outside, and two identical claims
are one claim.

**G9.43 — two lexers, two strings.** The corpus held no bare `_` and no
escape inside a string, and both were wrong in the Trust lexer: `_` came
back as an identifier where Ch. 0 §1.5 makes it punctuation, and a string
was the raw slice between the quotes — so `"a\nb"` was five characters and
`"say \""` ended at the escaped quote.

Neither is a decision. Both are the differential invariant doing what it is
for, and both were invisible until a corpus reached them, which is the
argument for growing the corpus with the parser rather than after it.

The fix for the second is one function: `escape_at` answers with a value and
where it ends, and the character literal and the string both call it. Two
decoders that agreed today would be two decoders that could stop agreeing,
and there is nothing in the language that could express the difference.

**G9.42 — `pub` on a method of an `impl`.** Ch. 6 §2.2 says a `pub` on a
trait's method or an enum's variant is an error, and §5 says an `impl` block
takes no `pub` because it "is as visible as the more private of the type and
the trait". Neither sentence covers a method of an *inherent* `impl`, which
is where Rust does allow `pub` and where it means something: a `pub struct`
may have methods its module keeps to itself.

Settled by refusing it, which is what §5's rule implies read straight
through — an impl's methods are as visible as the impl — and §2.2 now says
so. The cost is that a `pub` type has no private helper method; the
invariant of Ch. 6 §2.3 is still safe, because that rests on the *fields*,
and a method that can only reach them through the same `pub` surface
everyone else has is not a way around anything. What is lost is smaller: a
helper that briefly breaks an invariant is as callable as the type.

The alternative — `pub` admitted on a method of an inherent `impl`, private
by default there — is a strictly larger language and can be added later
without changing any program that compiles today.

**G9.41 — "nothing yet" was the first arm's state.** `join_arm` folds each
`match` arm's ownership into what the arms before it left, and `None` meant
"no arm has contributed". G9.35's fix made a diverging arm contribute
nothing — but on the *first* arm there was nothing to fall back to, and the
code returned the diverging arm's own state instead. So

```
loop {
    match next() { E::Done => return out, E::More => {} }
    out.push(1);
}
```

was told `out` "is moved out of here, and the loop may reach this again". The
`return` is the only place it moves, and the loop cannot reach it again
because the function is over.

The fix is a type: `join_arm` returns `Option<Vec<Owned>>`, so "nothing yet"
and "this state" are different values and cannot be confused. That is the
third time this session that a diverging path needed excluding — an `if`
branch, a `match` arm, and now the *first* `match` arm — and each was found
by a parser written in the language, because a parser is written as `check,
leave, carry on`.

**G9.40 — a compiler written in Trust cannot open a file, and does not need
to.** The machine has a character port and no filesystem (ISA §2.2). Giving
it one would be putting an operating system inside a machine, so the line is
drawn elsewhere: **the driver walks the module tree and hands the program
over**.

    trust bundle main.tr | trust run bootstrap/whole.tr

That is where finding files belongs anyway. Ch. 6 §1.2 already says which
files are compiled is a fact about the *program* and not about a directory;
which files are *read* is a fact about a **build**, and a build is the thing
outside the language. What self-hosts is the compiler, not the driver, and
saying so is more honest than a `open()` that only the reference machine has.

A section is `mod <path> <chars>` and then exactly that many characters —
length-prefixed, and counted in **characters**, which is what a `str` is
indexed by (Ch. 5 §1.1). Nothing a program can write ends a section early, so
there is no escaping and nothing to get wrong about it.

*What it costs, measured.* The Trust lexer retires **11 816 799**
instructions over `HPL.tr` — 24 KB, about 140 ms of machine time against the
Rust lexer's 15 ms of wall clock. A compiler of 900 KB of source would take
seconds to lex and minutes to compile itself. That is the ordinary price of
bootstrapping on a reference interpreter, and it is written down here rather
than discovered later.

**G9.39 — three things a parser needs that the parser did not have.** Growing
`bootstrap/parse.tr` to statements and functions found each of them by
failing to compile itself.

*`let _ = f();`* Ch. 0 §5.2's grammar says a **pattern**, and `_` is the one
that binds nothing (§4). Draft 0.1 read only a name, so the way a value is
evaluated and discarded did not parse. `_` now binds a name no program can
write, which means the value drops at the end of the scope rather than at the
end of the statement as Rust's `let _` does — a difference worth knowing and
not worth a second mechanism.

*`&&T`.* It lexes as the logical-and by maximal munch (§1.5), and only the
type grammar knows it is two references. The token is split where it is read,
which is the device `close_angle` has always used for `>>` in `Vec<Vec<T>>` —
and the Trust parser needed both splits, since it has both problems.

*A type is read recursively.* The first version of `type_name` in Trust
stopped at the first name and left `<t27>` for its caller to trip over. A
type has types inside it, and reading one is reading them.

None of the three is deep. All three are what a compiler writes on its first
page, which is the argument for writing one.

**G9.37 — two sibling modules could not see each other, and the spec said
they could.** Ch. 6 §3.1 declined `crate::`, `super::` and `self::` and said
a module "reaches what is outside it by a `use` written at its top" — while
leaving the `use` itself **relative**, so the `use` could not name the
outside either. `parse.tr` could not say `lex`. A compiler written in this
language found it on its second file.

The chapter is amended rather than the implementation excused: **a path in a
`use` is resolved from the root**, and a path in an expression or a type is
resolved where it is written. That is the rule §3.1 already wanted — every
path that leaves a module is a `use`, and every `use` starts from the same
place — and it keeps the reason the prefixes were declined, since a `use` is
absolute and so does not change meaning when its file moves.

Resolution and **visibility** are now separate arguments: a `use` resolves
from the root and is still judged from where it was written, because a `use`
grants no access (§3.2). Two more things had to follow: a `use` of a *module*
binds a module name, which is what a path's first segment may be; and
`lex::Taken` in type position — which the parser reads as an associated type
— is an ordinary name once its head is a module (§4).

*The same divergence bug, in the other construct.* G9.35 taught the `if` that
a branch which returned contributes nothing to what is owned after it. A
`match` arm needed the same, and a parser is where it shows:

```
let op = match t.tok { Tok::Op(o) => o, _ => return Ok(left) };
...
left = Expr::Binary(op, Box::new(left), Box::new(right));
```

**G9.38 — `&mut T` was not a `&T` — RESOLVED.** A helper that only reads
takes `&Parser` and every caller holds `&mut Parser`, and there was no rule
that turned one into the other, so `bootstrap/parse.tr` said `&mut` and meant
`&`.

Ch. 3 **§2.3a** now says it: where a `&T` is expected, a `&mut T` is
accepted and means the same place. It is neither a dereference nor a
conversion — the same address, carrying less permission — which is why it
sits beside §2.3 rather than under it, and why it does not weaken "automatic
dereference applies to `.` and nowhere else". §2.2 cannot be broken by giving
away less: while the exclusive reference is live the place is reachable only
through it, and a shared reference derived from it is reached through it too.

The reverse stays an error, and says so: a `&T` where a `&mut T` is wanted is
asking for a permission nobody has.

**G9.35 — a branch that returned still moved.** Writing a lexer in Trust hit
this on its first page:

```
let w = slice(s, start, i);
if is_keyword(&w) { return Tok::Kw(w); }
Tok::Ident(w)
```

`w` is moved in the branch, and the branch **returns** — so the use after it
is unreachable from there. The `if` joined both branches' ownership
regardless, and the second use was told `w` "may have been moved out of on
some path". There is no such path.

`self.done` was already the flag for a block that diverged; it was read
*after* the jump to the join, by which time both branches have it. Read
before, it says which branch reaches the join, and only those are joined.
That shape — check, return, carry on — is half of what a compiler is written
in.

**G9.36 — `String` and `&str` could not be compared — RESOLVED, by a name.**
A `String` is a `Vec<char>` (Ch. 0 §3.6) and has `Eq`'s `eq`, which takes
another `Vec<char>`; a literal is a `&str`. There is no overloading, so the
`eq` on `Vec<char>` hid the one on `str` and text could not be compared with
text.

The fix is not a coercion and not overloading: `str`'s comparison is named
**`same`**, which `Vec<char>` does not have — so the fallback from a
`Vec<char>` to `str`'s methods finds it, and `s.same("hello")` compares text
with text however each side is held. `==` still compares two `String`s
through `Eq`, and `s == t` means what it says.

*Why a name and not a rule.* The alternatives were `==` coercing its operands
or method lookup resolving by argument type. The first makes `==` mean
something different for text than for everything else; the second is
overloading, which this language does not have anywhere and would have to
have everywhere. A name costs one sentence in the prelude and nothing else,
and the sentence is in it.

**G9.33 — four things a sweep found, and one of them was `==`.** Combining
what had just been built — a `String`, a `HashMap`, generics, `Eq` — turned
up four separate acceptances of the unacceptable.

*`==` moved its operands.* Reading a place of non-copyable type moves it
(Ch. 3 §1.2), and a comparison read both sides — so `x == y` consumed them,
and a `==` inside a loop said the value had already been moved. A comparison
*reads*; it takes the address, which is what `compare_nominal` wanted anyway.

*`#[derive(Eq)]` on its own emitted ill-formed TIR.* `eq` is written in terms
of `cmp` (§5.3.3), and only `#[derive(Ord)]` emitted one — so deriving `Eq`
alone produced a call to a function nobody defined. Deriving either now
derives the comparison.

*A derived comparison over a `Vec` field called a `cmp` that never existed.*
`nominal_name` gives a `Vec<t27>` a name, and having a name is not having a
comparison. Only a scalar, a struct and an enum can be compared field by
field, and a field of any other type is now refused with that sentence.

*`Vec<T>` did not unify.* `fn n<T>(xs: &Vec<T>)` could not be called with a
`&Vec<t27>` — the unifier's application arm did nothing, because an
instantiation's arguments are not recoverable from its mangled name. A `Vec`
is the exception: it keeps its element *in the type*, so it unifies
structurally, and it is the one application a program passes by reference all
day.

*And `%` bit for the fifth time.* A hash bucket was `hash % BUCKETS`, and `%`
is symmetric (Ch. 1 §4): `40 % 64` is **−24**. Taking the sign off the hash
first is not enough — the remainder itself can be negative — so the
correction is after, and a negative bucket met the bounds check as a trap.

**G9.34 — shadowing the prelude left its impls behind.** A program that
defines a `trait Key` shadows the prelude's, and the prelude's `impl Key for
t27` stayed: an `impl` has no item name, so the rule that drops a shadowed
item never saw it. What the program got was a complaint about a method of a
trait it had never read. An `impl` goes with what it is written on, and is
dropped when either its trait or its self type is shadowed.

**G9.30 — an operator on a reference compiled to a comparison of addresses.**
`a == b` on two `&t27` emitted `cmp` on two `ptr`s, which is not an
instruction TIR has; the verifier caught it and the frontend should have.
Ch. 3 §2.3 already says what to do: automatic dereference is for `.` and
nothing else, and "`r + 1` is an error, not `*r + 1`". So every operator on a
reference is now refused, and says to write `*a == *b`.

**G9.31 — `v[i].m()` on a `Vec` loaded twice.** `type_of_place` knew about an
array's element and a slice's, and not a `Vec`'s — while `place`, beside it,
has always known. So a method call on `v[i]` took the *non*-place path, which
binds the receiver to a name and reads it again, and the name it bound was an
SSA value rather than a slot. What came out was `load t27 %v`, a load from a
number.

Two fixes, because the second is the one that matters: `type_of_place` learnt
the case, and the non-place path now materializes a **scalar** receiver into
a slot before naming it. Only an aggregate's value *is* its storage (TIR §2);
a scalar's is a value, and binding a name to it says the name's slot is at
that number. Another disagreement of the same kind would now cost a copy
rather than a wrong program.

**G9.32 — the prelude defined `Map` twice, and nothing said so.** A program's
own duplicates are caught while resolving modules (Ch. 6); the prelude is
parsed and merged rather than resolved, so nothing checked it. The iterator
adaptor `Map<I, F>` (Ch. 5 §3.1) and a hash map both existed, and the field
lookup for one found the other's. A test now walks the prelude's items and
fails on a repeat.

*Two more things the map wanted.* A bound asking for `Eq` or `Ord` is
satisfied by a **primitive** without an impl: Ch. 4 §5.3 gives those traits
their meaning for a *nominal* type, and Ch. 1 §4 gives a primitive the
operators directly, so an impl for one would be writing out what the language
already does. And the builtin-method gate asked only whether *any* type had a
method of that name, so a program could not define `insert` on its own type;
it now asks what the receiver is, from the same table a completion reads.

**G9.29 — a pattern nests, and an arm may have a guard.** Both were refused
with "not lowered yet", and both needed one thing: an arm that has matched its
*head* and can still fail must have somewhere to go.

The general dispatch already had it. Each tested arm is emitted with a `body`
and a `next`, and `next` is where the discriminant comparison goes when it
does not match — so it is also where a failing nested pattern or a false
guard goes. What that costs is a few comparisons: falling out of an arm for
variant `V` re-tests the discriminant against every later arm's value, and
those cannot equal `V`, so control reaches the catch-all having done nothing
but compare. No ordering to get wrong, and no second code path.

The `br3` fast path for a trit-shaped enum (Ch. 2 §5.2) has no `next` — it
*is* the dispatch — so it is now taken only when every arm is **certain**:
no guard, and nothing nested that can fail.

*Exhaustiveness is conservative, and says so.* An arm that can still fail
does not cover its variant. Whether two nested arms together cover one is a
question about patterns that draft 0.1 does not answer, so it is not assumed;
the diagnostic says an arm with a guard or a nested pattern does not count,
and to add a `_` arm for when none of them matched.

*A pattern reaches through a `Box`, and what it takes is borrowed.* A
compiler's own AST is mostly `Box`, so `E::Add(E::Lit(a), E::Lit(b))` is the
shape that matters — two levels of a tree read in one arm. Reaching through
one is a load and then the same question about what it points at, which is
three lines.

What comes out is **borrowed, always**. The box still owns what is inside and
will drop it; a binding that owned it too would be the second owner, which is
G9.27's double free written as a pattern. So a pattern may look through a box
and not take from it, and a program that tries is refused by the machinery
that already refuses it for a `match` on a reference.

This is more than Rust gives — `box` patterns are unstable there — and it is
sound for a reason Rust does not have available: a binding here can be
*borrowed* rather than owning, which is what `match &e` has always produced.

HPL retires **1 985 745** instructions, unchanged.

**G9.28 — the inliner did not return on mutual recursion.** `is_even` and
`is_odd` are two four-instruction functions that call each other, and
compiling them hung. `check` passed; the frontend was fine. The `inline`
phase never finished.

The exclusion was "do not splice a call to the function being spliced into",
which stops direct recursion and nothing else. Splicing `is_even` into `main`
brings a call to `is_odd`; the scan continues forward into the body just
spliced, finds it, and splices it, which brings a call to `is_even`. Neither
is `main`. Draft 0.1's comment said a cycle of two "is stopped by `ROUNDS`
instead of by cleverness" — but `ROUNDS` bounds the outer loop, and all of
this happens inside **one** call to `inline_into`.

The fix is the general form of the rule already intended: **a function on a
call cycle is never spliced**, found by Tarjan's algorithm on the call graph,
recomputed each round because splicing changes who calls whom. Iterative
rather than recursive, since a compiler that compiles itself is one large
call graph and running out of stack while looking for its cycles would be a
poor way to find out.

It costs nothing: HPL retires **1 985 745** instructions before and after, to
the instruction. A cycle was never inlinable; it was only unbounded.

*Why it matters more than it looks.* This is the first thing found by asking
what a **self-hosted** compiler would need. A compiler is mutual recursion —
`expr` calls `binary` calls `expr` — so every version of this language that
compiles itself would have hung on itself.

**G9.27 — a move out of a reference was not refused, and the drop ledger
found it.** `fn take(h: Held)` takes ownership; `fn through(r: &Held) { take(*r) }`
gave it a value the reference only points at. It compiled, and the value was
dropped **twice** — a double free in a language whose whole claim is that it
has none.

The cause is one arm. `E::Field` and `E::Index` both check: reading a place of
non-copyable type *moves* it (Ch. 3 §1.2), so they call `check_access(Move)`
and mark the root moved. `E::Deref` beside them loaded and returned. It had
nothing to mark — the owner is whoever the reference points at — and did the
only other thing available, which was nothing.

It now refuses, and only for a **reference**. A `Box` is not one: reading
through it moves the box, which *is* the owner, so there is exactly one drop
and it happens there. That distinction is what the two tree tests found
within a minute of the first attempt, which refused both.

*What it reached.* A closure captures by reference (Ch. 4 §4.2), and a
capture is rewritten to `*self.field`, so `|| take(h)` was this same hole
wearing a hat: `twice(|| take(h))` dropped one value **three** times — once
per call and once at the end of the scope that still thought it owned it.
The same one-arm fix closes it, which is why `docs/status.md` §6's line about
`FnOnce` needing "a move analysis of the closure body" was describing a
missing *refusal* rather than a missing feature.

*What found it.* The drop ledger — a type whose destructor prints — which is
now six for six. Nothing else in this repository has found a drop bug.

**G9.26 — `print_int`, which is not formatting.** Ch. 5 §1.6. The library
could print a string and then a character and still not a number, and every
program wrote the same digit loop — HPL's, demo's, `traits.tr`'s, three copies
of one thing.

§7 reserves `Display`, `format!` and `println!`, and the reason it gives is
that **each needs variadic arguments or a macro**. One number takes one
argument and composes with nothing, so it is outside that reservation for the
same reason `print_char` was: what §7 is holding the door open for is putting
a value and the text around it in *one call*, and this is not that. §7 said as
much already — "a number with the same digit loop `examples/trust/HPL.tr`
writes today" — so writing it once is what the sentence was asking for.

The loop is not the one a binary machine would write: `%` is the **symmetric**
remainder, so `n % 10` runs −9 ..= 9 and a negative digit borrows from the
next place. Ch. 1 §4's rounding, once more, and it costs one `if`.

*And it leaked something.* `print_int` calls `char::try_from(…).unwrap()`, and
`unwrap` is generic, so the module gained `Option.unwrap.char` — a name
`prelude_functions` does not know, because the set holds names as *written*
and this one is mangled. Every program that never printed a number emitted it.
Mangling appends, so a prefix in the set is the answer. Two tests that assert
a `trit` match compiles to one `br3` and nothing else caught it — the third
time this session those two have caught something written for another reason.

*Not added:* `print_ternary`. Balanced ternary is the machine's own notation
and `0t` is its literal, but nothing has needed it that a program cannot write
in six lines, and the digit that has no character — `T` for −1 — is a choice
§1.6 would have to make.

**G8.17 — the compiler spent 96% of its time verifying, and I guessed twice
before measuring.** `trust run` made one number visible that three commands
and two temporary files had hidden: HPL took **1.6 seconds to compile and 40
milliseconds to run**, and every measurement in this log until then had been
about the second.

The first two guesses were wrong and both were about code I had just written.
The inliner restarted its scan from the top after every splice — genuinely
O(k²·size) — and cloned the callee table once per function. Both were real
and both were fixed and together they were worth **four milliseconds**.

One profile answered it:

| phase | before | after |
|---|---|---|
| canonicalize | 18 ms | 16 ms |
| inline | 4 ms | 2 ms |
| legalize | 5 ms | 4 ms |
| **verify** | **1371 ms** | **~230 ms** |
| codegen | 106 ms | 102 ms |

`verify_function` built, **for every block**, a `BTreeSet<String>` holding
every value defined by every block that dominates it — one string allocation
per value per dominator — and cloned the function's whole type map beside it.
That is O(blocks × values) allocations, which is fine for what a frontend
emits and is not fine after inlining triples it.

It answers one question: is this value in scope here. So it asks that instead
— which block defines the value, and does that block dominate this one — and
the per-block set holds only what is defined *here*. The type map and the
dominator set are borrowed rather than cloned. `dominators` also rebuilt its
predecessor lists on every pass of its fixpoint, by scanning every block.

**1.39 s → 0.455 s**, and the output is identical to the instruction:
3 233 721 either way.

*And the next profile did say what.* `dominators` was **280 ms of the 275 ms
verification** — the whole of it, and the instrumentation's own cost explains
the sign. It was the textbook set-intersection fixpoint, which is the right
thing to write first: each pass cloned a dominator set per block and
intersected it with another, so O(passes · blocks²) allocations. Fine for what
a frontend emits; not fine once inlining produces a function of **498 blocks**.

Cooper, Harvey and Kennedy's algorithm replaces it — a reverse-postorder
numbering and a fixpoint over *immediate* dominators, where the meet of two
blocks is a walk up their chains — and the sets are built from those chains
once, O(blocks · depth). Only one caller, so the interface did not have to
change at all.

**Verification: 275 ms → 10.4 ms.** The compile is **1.6 s → 0.167 s, ten
times**, and HPL is 0.24 ms per line where it was 2.67.

*Where it stops.* Code generation is now 106 of the 167 ms, and 46 of that is
the register allocator's liveness fixpoint — the same `BTreeSet<String>` shape
one level down. Fixing it means interning names as indices, which is not a
small change, and 167 ms for a 688-line program is not a problem worth it.
`demo.tr` is 0.04 ms per line, so a gap of six remains and is recorded rather
than chased.

**G8.18 — common subexpression elimination made this machine slower, and
that is the finding.** Route C in `docs/status.md` §10 was bounds-check
elimination, and the profile of an indexed loop said the checks are **four of
its sixteen instructions**:

```
^while:  %len1 = load (offset %v.slot 3)      k < v.len()
         %c1   = cmp %k1, %len1
^body:   %len2 = load (offset %v.slot 3)      the same load
         %k2   = load %k.slot                 the same k
         %c2   = cmp %k2, %len2               the same comparison
```

The second comparison is decided by the branch above it. It does not *look*
decided because every operand is a different SSA value, so the obvious first
move is to make them the same: a value-numbering pass that reuses a load or a
pure computation already made, inheriting across a **sole** predecessor, which
is sound because that is the only way in.

It was written, it is correct, it fires — and **every benchmark got worse**:
the range loop by 9.5%, an adaptor chain by 5%, HPL by 0.08%. The reason is
not subtle once measured: eliminating a recomputation **extends a live range**
across the loop's back edge, and a linear-scan allocator with 27 registers
spills something to pay for it. Recomputing an `offset` is one instruction;
spilling is two and a frame slot.

*What the finding changes.* Bounds-check elimination does not need the two
comparisons to *be* one value — it needs to know that they are *equal*. An
**equivalence oracle** costs no registers: walk the sole-predecessor chain
from the deciding branch, require no `store` or `call` on the way, and compare
the two comparisons' operands by their defining instructions rather than by
name. Nothing is rewritten, so nothing lives longer. That is the design; it is
not built.

This is also the third measurement in this log to contradict a change that
looked obviously good — after the inlining budget, which is worse when raised,
and constant folding, which was worth 62 instructions.

**G8.19 — how much a bounds check costs, and half of it removed.**
Compiling HPL with the index checks deleted outright says what they are worth:
**3 233 721 → 2 249 471, −30.4%.** That is the largest single number left in
this project, and most of it is not recoverable — HPL indexes `[t27; 35]` with
a loop bound that is a *runtime* value, so `i < 35` is not provable and the
check is the price of safety rather than a missing optimization.

The **lower** check is another matter. Every index pays `0 <= i` as well, and
an index is usually a counter that starts at zero and goes up. After `mem2reg`
that counter is a block parameter whose incoming values are `const 0` and
`add.trap %k, 1`:

```text
br ^loop(const t27 0)
^loop(%k: t27):
    %c = cmp t27 %k, const t27 0
    br3 %c, ^fault, ^ok, ^ok
```

The proof is a **greatest** fixpoint — assume every value non-negative and
refute — which is the only way a counter defined in terms of itself is
provable at all. `.trap` is what makes the arithmetic sound: a `.wrap`
addition can land below zero and is refuted, as is a subtraction, a load, and
anything a call returned.

The branch is rewritten only where the other two arms agree, since then
knowing the first is impossible makes it unconditional and the comparison goes
with it. That leaves blocks nothing can reach, and an unreachable block is one
the verifier rejects — so pruning them is part of the transformation, not a
tidy-up. HPL turned out to contain `if x < 0` on values that cannot be, which
is where the first unreachable blocks came from.

**HPL: 3 233 721 → 2 965 929, −8.3%**, and the reported Gflops rises with it.
An indexed loop over an array is −1.4%. Two tests hold the other side down: a
written negative index and one counted negative through a function that cannot
see where it came from, both of which still fault.

*Then the upper half, by the oracle G8.18's negative result asked for.*
`while k < v.len() { v[k] }` asks the same question twice and the second time
it is already answered — but it does not *look* answered, because every
operand is a different SSA value. Making them one value was measured worse, so
nothing is rewritten: this walks up the chain of **sole** predecessors looking
for a `br3` whose taken edge decided the same comparison, and compares the two
by their **defining instructions** rather than by their names. An oracle
extends no live range.

A `store` or a `call` anywhere on the path gives up, and that single condition
is what makes comparing two *loads* sound with no alias analysis at all.

**HPL: 2 965 929 → 2 762 032, −6.9%**, and an indexed loop over a `Vec` reaches
**exactly** the figure measured with the checks deleted outright — every check
in it is provable. Across both halves: **3 233 721 → 2 762 032, −14.6%**, which
is **48% of everything bounds checking costs**.

Three more tests hold the other side: a loop that counts past a constant
length, one bounded by a runtime value the callee cannot relate to the slice,
and one that outruns a `Vec`. All three still fault.

*Generalized to facts.* "The same comparison" is one case of "the facts force
it". What the walk collects now is every `x < y` a taken edge decided, and a
query is answered directly or through **one** transitive step — `i < j` and
`j <= n` gives `i < n`. One step is what the language produces, a loop bound
and a length; stopping there keeps this a decision rather than a search. 
*And the walk is over dominators.* It was over **sole predecessors**, which
breaks at a loop header — that has two, the entry and the back edge — so a
fact established before a loop never reached its body. What makes an edge's
fact sound in a block is that every path there goes down it, and that holds
when the taken target has one predecessor and dominates the block. HPL's
inner loops are inside outer ones, and the outer condition now reaches them:
**2 762 032 → 2 743 146**.

The condition "nothing wrote in between" moved with it, and had to become
finer in two directions at once. It gates only the comparison of two
**loads** — comparing by *name* needs no such condition, and that is worth
knowing, because a length the frontend loaded once is worth more than one it
reloads. And it is asked per *fact*: what matters is whether anything wrote
after that fact became true, not whether anything wrote in the whole function.
Asking the coarse question undid both earlier wins while gaining HPL's; asking
the fine one keeps all three.

**G8.20 — no, a release mode will not remove the bounds checks.** Asked once
and worth answering here, because a 30% number invites it.

Ch. 2 §3 is **normative**: an out-of-bounds access "faults or panics per the
safety chapter, **never proceeds**". A flag that removed the check would make
a program *proceed*, which is a change to what it means and not to how fast it
runs.

The line is in the same place Rust puts it, and Rust's own split says why:
`--release` turns off *overflow* checks and keeps *bounds* checks, because
overflow is merely wrong and out-of-bounds is memory-unsafe. Trust does not
need even that flag — the overflow behaviour is written into the operation as
`.wrap`, `.trap` or `.flag` (Ch. 1 §4) — so this language currently has **no
flag-dependent semantics at all**, and that is a property worth keeping. A
language that is safe only when the optimizer is off should not be called
Trust.

Three things legitimately remove a check, and a build flag is none of them:

1. **Proving it.** G8.19 recovered 48% of what they cost on HPL.
2. **Hoisting it** out of a loop, which needs the interval reasoning this
   compiler still has none of.
3. **An `unsafe` unchecked index**, which Ch. 3 §6 reserves the chapter for —
   and which must be visible *at the call site*, in the source, so that the
   line doing it says so.

The third is the only route to HPL's remaining 16%, and its price is that the
program admits what it is doing.

**G8.21 — `where n <= a.len()`, and the last of the bounds checks that can
go.** Ch. 4 §2.8. A `where` clause already carried type bounds; it carries
value predicates now, told apart by the `:` a bound has and an expression
does not.

The mechanism is smaller than it sounds and that is the point. A predicate
compiles to **one branch on entry** — pass, or trap — and the fact machinery
of G8.19 does the rest, because a branch is exactly what it reads. Nothing was
added to it. `i < n` from the loop and `n <= a.len()` from the clause give
`i < a.len()`, and the check inside the loop is gone.

It is checked in the **callee**, not at the call site, so a caller is not
obliged to *prove* anything. What the clause buys is one check per call
instead of one per use — which is the loop-invariant hoisting a compiler would
otherwise have to synthesize, written by the person who knows why it holds.

`while i < n { … }` in HPL's four hot functions became four `where` clauses.
**HPL: 2 743 146 → 2 552 691, −6.9%.** Against the start of this thread:
**3 233 721 → 2 552 691, −21.1%**, and the floor measured by deleting the
checks outright is 2 249 471 — so **69% of everything bounds checking costs is
now recovered**, and the rest is the price of the ones that are genuinely
undecidable.

*Two things it needed that were not obvious.* The facts had to learn `<=`,
which is the branch shape where the negative and zero edges agree. And
"nothing wrote in between" had to become **between** — a write the fact
reaches but which does not reach the use has not happened yet, and the code
that drops a `Vec` after a loop is exactly that. Asking the coarser question
made the whole thing do nothing at all.

*Reserved, and stated in §2.8:* discharging the predicate at the **call
site**, where a caller that can prove it pays nothing and one that cannot is
told at compile time. That needs a proof language. The runtime check is the
conservative reading of the same clause, so adding the proof later changes no
existing program's meaning.

**G8.22 — a loop that calls something keeps its bounds check, and the reason
is not the one it looks like.** `print`'s loop is `while i < s.len() { s[i] }`
— the shape G8.19 was built for — and its upper check survives. So does the
one in any loop containing a real call.

The obvious cause is the rule that **any call clobbers**, which makes
"nothing wrote between the fact and the use" false as soon as a loop calls
anything. And the obvious fix is an escape test: a slot whose address never
leaves the function cannot be written by a callee, which has no way to name
it, nor by a store through any *other* pointer — the only addresses into it
are the ones derived from it here. That is sound, it is what `promote_slots`
already reasons about, and it was written.

**It changed nothing measurable, so it was reverted.** A diagnostic says why
it was the wrong target:

```text
DBG idx.lo.20    facts=["idx.ok.23"]                above={body.8, entry, idx.lo.20, while.7}
DBG idx.lo.20.i1 facts=["body.8.i1", "idx.ok.23.i1"] above={body.8.i1, …}
```

In the standalone copy the loop header's fact — the one at `body.8` — is
**not collected at all**, and cleanliness never enters into it. In the inlined
copy it is collected and in scope, and the fold still does not happen, so
there the equivalence is what fails.

Two different causes wearing the same symptom, and neither is the one the
escape test addresses.

*The first is found and fixed.* `elide_dominated_checks` ran **before**
`branch_through_select`, and until that pass a loop's condition is a `select3`
and a two-way branch whose three targets are *exit, exit, body* — with the
`select3`, not the comparison, as the branch's operand. So the header's fact
was not merely on the wrong edge, it was never read at all. One line moved,
and the diagnostic goes from

```text
DBG idx.lo.20 facts=["idx.ok.23"]
```

to

```text
DBG idx.lo.20 facts=["body.8<", "idx.ok.23<"]
```

*The second is the rule that any call clobbers, and it is fixed.* A slot whose
address never leaves the function cannot be written by a **call** — the callee
has no way to name it — nor by a store through any *other* pointer, because
the only addresses into it are the ones derived from it here. So only a store
whose base is one of them clobbers, and a loop may call anything and still
know its own slice's length.

**HPL: 2 552 691 → 2 315 340, −9.3%.** Across the whole of G8.19 and G8.21:
**3 233 721 → 2 315 340, −28.4%**, against a floor of 2 249 471 measured by
deleting every check outright — **93% of what bounds checking cost is
recovered**, and what remains is 66 000 instructions of checks that are
genuinely undecidable.

*The three rounds this took are the entry's real content.* The escape analysis
was written, measured at zero, and reverted; then written again, measured at
zero again, and reverted again. Both times the number was right and the reason
was wrong: a `str.replace` with no assertion had silently matched nothing, so
`settled` was still being computed by the rule the new code existed to
replace. Two diagnostics in the same build disagreed —

```text
bases=Some({"a.slot.1"}) clobbered_by=[] settled=false
```

— and *that* is the shape of an edit that did not happen, not of an analysis
that is wrong. Every other edit in this session asserted on its match; this
one did not, and it cost three rounds and two correct reverts.

*Kept as a method note.* Three times in this session a change that looked
obviously right measured nothing or worse: CSE (G8.18), the inliner's
quadratic scan (G8.17), and this. Every one was found by measuring rather
than by reading, and the two that were reverted cost less than the one that
was not.

**G8.23 — an elided check leaves a dead load, and the allocator was spilling
it.** After G8.22 the profile of HPL had changed shape entirely — `br3` from
20.4% of everything executed to 5.4% — and a new thing was at the top:
**`st.word sp` at 6.11%, against `ld.word sp` at 1.38%.** Four words written
to the frame for every one read back is not what spilling looks like.

Reading the hot loop said why in three lines:

```text
f.factor.body.46:
    ld.word a1, 0(a7)
    ld.word t2, 3(a7)        ; the length
    st.word t2, 102(sp)      ; spilled, every iteration
f.factor.idx.lo.57:          ; the check that read it — gone
```

`102(sp)` is written once in the whole function and **never read**. The length
had one consumer and G8.22 removed it; the load stayed, so the allocator
dutifully spilled a value nothing would ever want.

The load stayed because `is_removable` says a `load` never is — a load can
fault, and removing a fault changes what a program does. But a load from a
slot whose address **never leaves the function** cannot fault: it is this
function's own memory, of a size it chose. The same `slot_bases` G8.22 needed
answers this too, and an unused such load is now removed.

**HPL: 2 315 340 → 2 059 024, −11.1%**, and frame traffic falls from 7.9% of
everything executed to **2.6%** — `st.word sp` from 141 410 to 13 317. Every
other measurement moved with it: an adaptor chain −4%, a range loop −4%, a
loop over a slice with a call in it −9%.

*What this says about the order of things.* Removing the checks was worth
−28%; removing what the checks had been keeping alive was worth another −11%
on top, and neither could have been found before the other. A profile after
each change, not a plan before them.

**G8.24 — a `while` puts its body before its test.** With the checks and the
loads gone, the profile's next entry was `jal (jump)` at **5.1%** — 104 944
unconditional jumps, none of which could fall through, because they were all
loop back edges.

A loop laid out test-then-body ends its body with a jump *backwards*, and a
backward jump is an instruction. Per iteration: the test, the branch, and the
jump — three transfers where two will do. Emitting the **body first** makes
its jump to the test a fall-through, and the branch back into the body is the
only transfer left.

It is four lines in the frontend and nothing else: no pass, no duplication, no
change to what a `while` means. The loop is still entered by jumping to the
test, so a condition false at the start still runs the body zero times, which
is the case the rotation could most easily have broken and the one the test
checks first.

**HPL: 2 059 024 → 1 985 745, −3.6%**, and `jal (jump)` from 104 944 to
**31 593**. Every other measurement moved too.

*Two things ruled out on the way, by measuring rather than assuming.* Block
layout already elides a jump to the next block — all 180 static jumps went
somewhere else, so there was nothing to gain there. And **strength reduction
is worth zero on this machine**: `alui.mul` is 14% of HPL, but replacing
`base + i·size` with an incrementing pointer trades a one-instruction multiply
for a one-instruction add. A register-starved machine with no multiply
penalty gets nothing from the classic transformation.

**G0.19 — an editor can be told what is wrong, and where.**
`trust check` compiles no further than the frontend and prints one diagnostic
per line; `trust-lsp` sends the same over stdio, so an editor squiggles as you
type, from **exactly the compiler that will build the program**. There is no
second implementation of the language to drift.

Both are small because the compiler is a library and `lang::compile` already
returns every error the frontend found — several, since it recovers at each
function.

*What used to stop there — and no longer does.* A `SyntaxError` carried a
**line** and nothing narrower, so a squiggle was a whole line. Every token,
every AST node and every error now carries a `Span`: the line, and the
half-open range of **characters** it covers. `LineMap` turns an offset back
into a 1-based column for a terminal and a 0-based UTF-16 offset for the
protocol, which are different numbers as soon as a file holds anything
outside the basic plane.

Offsets count characters and not bytes because the lexer reads a file as a
`Vec<char>` and an editor counts code units; bytes would mean converting at
both ends instead of at neither.

A span is the **whole extent** of what it covers, not the token that names
it: `a + b` reaches from `a` to `b`, because what reads a span is something
drawing a line under what is wrong, and what is wrong with `a + b` is
`a + b`. Two diagnostics moved with it — a body's type is its tail's, and a
`let`'s initializer is what has the wrong type — because a span made it
possible to say so.

`Span` carries a line it could recompute from its offset. That redundancy is
allowed by a test: for every token of a file, the line in the span is the
line `LineMap` finds. It is kept because most diagnostics are printed by
something holding a line and no source text to count newlines in.

*An outline and a jump, from the AST alone.* `lang::index` answers the two
questions an editor asks that a compiler does not — *what is in this file*
and *where was this defined* — and answers them without types or lowering, so
they work on a file that does not compile, which is the state a file is in
while it is being written. `trust-lsp` serves them as `documentSymbol` and
`definition`.

A name resolves by **scope and spelling**. That is enough for locals,
parameters, functions, types, constants and variants, and it is not enough
for two things, neither of which is guessed at:

  * `x.f` — which field that is depends on the type of `x`.
  * `x.m()` — likewise, *except* when the file defines exactly one method
    called `m`, when there is nothing to be wrong about.

Unresolved means absent. An editor that jumps to the wrong place is worse
than one that does not jump.

Making that true cost a change to the AST: a parameter and a field were
`(String, Ty)` — a name with **no place in the file** — so a jump to one
landed on its type. They are now a `Named`, which carries a span, and the
same span is what lets an outline select a field's name rather than the type
beside it.

*A hover shows the definition as it was written.* That is the whole rule, and
it is why hover needs no types: `fn area(s: Shape) -> t27`, `const ORIGIN:
t27`, `Shape::Rect(t27, t27)`, `s: Shape`, `let mut total`. A `let` with no
written type has none in its hover either — what it would be is inferred
during lowering, which the index runs before and without. Saying less is the
price of never saying something false.

A definition is also a place a cursor can be, so every one of them resolves
to itself: hovering `fn area` on the line that declares it is the most
obvious hover there is.

Making that true finished the same job `Named` started — a `let` binding's
name had no span either, so its `Stmt::Let` now carries one, and a jump to a
local lands on the name rather than on the `let` four characters before it.

*Find references and rename are exact, or refused.* The index knows which
definition each name means, so references are the places that **mean** this
one — a different `n` in another function is not among them, which a search
for the text `n` could not say. Rename is that set plus the definition, and
it refuses rather than half-does it in three cases:

  * a **method**, because it is found by spelling and the prelude — which the
    index never sees — has methods of its own, so the calls found here may
    not be the calls that mean this one;
  * a **name already written where this one is used**, which would shadow
    rather than rename. The region searched is the enclosing item, since that
    is where shadowing happens, plus the file's top-level names; a local
    called `i` in another function is not a collision and is not refused;
  * anything that is **not a name** (Ch. 0 §1.3), keywords included.

Getting the spans right cost one more AST change, of the same kind as the
last two: `Expr::Call` and `Expr::Method` covered their whole call, which is
right for a diagnostic and wrong for a rename — `helper()` is what a
diagnostic underlines and `helper` is what a rename changes. Both now carry
**two** spans, and a test holds each to its job.

*Completion is two features, and only one of them needs types.* Offering
what can be **named** here needs scope and nothing else: the index already
walks every scope, so recording how far each binding reaches is all it took.
A completion offers what is in scope, then the prelude's names, then the
keywords — and a name written twice is offered once, as the near one.

Offering what follows a **`.`** is the other feature, and it does need a
type: which fields and methods are there depends on what precedes the dot.
So after a dot the server offers **nothing** rather than the names in scope,
which are the one thing that certainly cannot follow one. A range is `a..b`,
so two dots are not one.

Writing the first scope test found a real bug. `for` is desugared into a
`match` (§5.7), and the arm it becomes carried the span of the `for` keyword
— so the loop's binding was in scope over an empty stretch of file, and `i`
was offered nowhere inside `for i in …`. The arm now reaches to the end of
the body, and the binding keeps the span of the name the file wrote rather
than the keyword's.

*A type at an offset, which is the one thing not written anywhere.*
`lang::analyze` lowers the file and keeps what lowering worked out: the type
of every expression, by where it was written (`lower::Noted`). A hover then
shows the definition **as it was written** and, when the file did not say it,
what it turned out to be:

```trust
let n
// taddr
```

The type is a comment rather than spliced into the line, because the rule a
hover follows is still *as it was written*, and `let n: taddr` is not what
the file says.

Two things bound it, and both are refusals rather than approximations:

  * **only the file's own functions.** The prelude is parsed as its own file,
    so its spans are offsets into *it* and would be read as places in this
    one. Recording only functions the file itself names is exactly the set
    whose bodies are in it.
  * **only where the answer is one answer.** A generic function is lowered
    once per instantiation, so `fn id<T>(x: T) -> T { x }` used at two types
    has an `x` that is both and neither. The entry is dropped: an editor
    shown one of several is shown a guess.

It costs one `Option` test per expression when nobody asked, so `trustc` and
`trust` pay nothing. `analyze` on the largest example here (HPL.tr, a
thousand lines) takes **4.7 ms**, which is a hover's budget many times over.

*A file being typed into is a file with a syntax error in it.* Stopping at
the first one meant the other twenty functions had no outline, no hover and
no completion — the editor went blank exactly when it was being used. So
`parse_recovering` skips an item that does not parse and reads the rest.

Recovery is at **item** level and nowhere finer: inside an item the parser
knows too much about what it expected to guess well, and half an item is
worse than none. It counts braces on the way out, so a `fn` inside the body
of the item that failed is not mistaken for the next one. A *lexical* error
still stops everything, because there is nothing to skip to — an unterminated
string swallows the rest of the file by definition.

A file that does not parse is **not type-checked**. What lowering would say
about the part that parsed is mostly about the part that did not — every
call to the function being typed is "not a function in scope" — and a list
of consequences buries the cause. The types are still kept, because a type
is not a complaint.

    fn broken( -> t27 { 1 }        one diagnostic, on that line
    outline                        helper, main
    hover on `total`               let total // t27
    go to `helper`                 line 1
    completion                     total, helper, main, Option, …

*Member completion, without teaching the parser about half an expression.*
`p.` with nothing after it is not an expression, so the statement it sits in
does not parse and there is no type to be had. Rather than make the parser
tolerate that, the server **writes a name in after the dot** and analyses
that. Everything before the dot keeps its offset, and the receiver is the
only thing asked about — writing after it cannot change what it is.

Three sources answer, and each is the only place its answer is written down:

  * the file's own `struct` fields and `impl` methods;
  * the prelude's, for `Vec`, `str`, `char` and the rest;
  * the **compiler's own table**, for what is not written in any file at all.
    `Vec::push` has no Trust source: its storage is the compiler's (Ch. 5
    §2), so the only place it exists is the list `lower` matches against, and
    that list is now `(what it is written on, its name, how it reads)` and is
    read by both. There is one list, so there is nothing to drift.

An `impl` is written on one of two things, and which one matters:
`impl<T> Vec<T>` is written on every `Vec`, while `impl Vec<char>` is written
on that one — so a `Vec<t27>` is not offered `push_str`. What tells them
apart is whether the arguments are the impl's own parameters.

One thing had to change in `lower` for this: `p` in `p.x` goes down the
*place* path and never reaches `expr`, so its type was recorded nowhere. A
place has a type like anything else, and an editor asking what `p` is has to
be told the same answer either way.

    p.        area, x, y
    v.        from_iter, len, is_empty, push, pop, clear, reserve, …
    p.x.      tmin, tmax, tmul, tneg, mulh, is_pos, …

*What is still not there.* A method reached through a **trait bound** rather
than a concrete type: `fn f<T: Area>(t: T) { t.` knows `T` and not what
implements it. And an editor's other half — formatting — which Ch. 5 §7
reserves and this repository does not define.

*The other half of an editor is a second grammar.* Zed's highlighting is
**tree-sitter only**, so `editors/tree-sitter-trust` exists, and it is a
second parser that can drift from the real one. `scripts/grammar.sh` is what
stops it: every example must parse with no ERROR or MISSING node, and a
corpus of nine cases covers each place the two grammars actually disagreed
while it was being written.

Six of those disagreements were with **Ch. 0 §6's grammar summary**, which is
informative and out of date in ways §6's own note anticipates:

  * `alt` has no negative literal, and `-1t => …` is how the sign arm of a
    `<=>` match is written (§4, Ch. 1 §4).
  * `self_param` has no `self: T`, which Ch. 4 §5.2's destructor form uses.
  * `where` is `( ident bounds ),*` only; Ch. 4 §2.8's predicates are also
    written there, and the two are told apart by what follows the name.

The first two are the summary lagging the chapters that cite it — exactly the
failure §6's note describes for draft 0.1 — and are recorded here rather than
fixed, because §6 is informative and the chapters it summarizes are not
wrong.

*No dependency was added.* A language server usually reaches for `serde_json`
and `lsp-types`. This repository has none at all, and a convenience tool is a
poor reason to acquire its first: the JSON it needs is a reader, a writer, and
the surrogate-pair case an editor sends for anything outside the basic plane —
which matters here, since the protocol carries source text and this language's
source may contain any of it.

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

**G4.2 — Uninitialized global storage.** TIR §4 item 4 makes reading uninitialized
`slot` storage yield poison but says nothing about a `global` with no
initializer.

*Decision:* the same — `= zeroinit` zero-fills, an omitted initializer leaves
the storage uninitialized and reading it yields poison.

**G4.3 — Poison propagation.** TIR §4 item 4 adopts "the standard modern" poison
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

**G6.5 — A wide value could not cross a function boundary — RESOLVED.**
A `t54` parameter on a `t27` target has to arrive in two registers, or through
a hidden pointer. TIR has neither multiple return values nor an `sret` form,
so expansion had nowhere to put the parts and reported the signature instead
of inventing an ABI.

*Resolution, and it needed no TIR change at all.* Legalization **reshapes the
signature**, and reshapes every call site to match — which it can do because
it sees the whole module:

- A wide **parameter** becomes one parameter per part, least significant
  first. That is AM §2.2's order for memory and TRISC-27 §6.3's for argument
  registers, so the two agree without either being consulted.
- A wide **result** becomes a hidden leading pointer, `lz.sret`, and the
  function returns nothing. The caller allocates a slot, passes its address,
  and reads the parts back.

The rewritten signature is an ordinary TIR signature, and `ret` still carries
at most one value. The alternative — giving TIR multiple results, to match
§6.3's "results come back in `a0`, then `a1`" — was rejected as the larger
change: every consumer of a call result assumes there is one.

*The choice was the author's, and it has a cost worth stating.* TRISC-27
§6.3's provision for a wide result in two registers is now something this
compiler will never emit. It remains correct for hand-written assembly, but a
specification with a clause nothing implements is a clause that will drift.
§6.3 carries a note saying so.

The frontend can now compile `fn add(a: t27, b: t27) -> t27` for a nine-trit
machine, recursion included, and the differential test checks the answers.

**G6.6 — Expansion of `mul` needed a primitive TIR did not have — RESOLVED.**
Expanding a multiply means multiplying a part by a part, and that product is
twice a part wide. No same-width instruction can deliver the top half, so a
multi-part multiply had nowhere to put its partial products. TRISC-27 §4.1
already provided `mulh` — the machine was ahead of the IR, which is the wrong
way round for an IR that claims to be target-independent.

*Resolution:* TIR §3.1 gains `mulh tN %a, %b`, the high tN of the exact
2N-trit product, defined so that `mulh · 3ᴺ + mul.wrap` reconstructs it. It is
the balanced split of §3.3's `shr`, so it needed no definition beyond "the
other half", and it takes no flavor because the high half of a product of two
N-trit values always fits in N trits — a property of the symmetric range, not
an accident.

Promotion cannot carry `mulh` through unchanged: at a wider type it is the
high half of a *different* product. It is recomputed instead, as `mul` then
`shr` by the narrow width, which is the definition read literally and is exact
whenever the whole product fits.

The expansion is schoolbook, with one choice worth naming: the accumulator is
**2k parts wide rather than k**. The extra half costs a little work and buys
the overflow answer for nothing — the product fits exactly when every part
above the result's width is zero, and the direction of an overflow is the sign
of the most significant nonzero part, because that is what the sign of a
balanced positional number *is*.

192 cases across all three flavors, including MAX and MIN as operands, agree
with the unlegalized module.


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

- **A wide shift by a *computed* amount.** `shr t54 %a, %k` with `%k` a value
  rather than a constant is refused, in as many words. Everything else in
  this family is built: multi-part `div` and `rem` expand into `lz.div.tN`
  and `lz.rem.tN` and are tested, and a wide shift by a constant expands. No
  Trust program can reach the missing case, because Ch. 1's types stop at a
  word; it is a hole in TIR's coverage rather than in the language's.
  (`mul` is different: see G6.6, where the instruction set is the obstacle.)

  *This entry said "multi-part division and multi-part shifting are
  unwritten" until it was checked.* `div t54` legalizes to 908 lines with a
  real body, and `1000000 / 7` through the interpreter is 142857. A gap log
  that claims something untrue is worse than one that is merely incomplete,
  and this is the first entry here to have gone stale.
- **Optimization passes** — the canonicalizer that "recognizes and re-fuses"
  two-valued predicate patterns (TIR §3.3). Inlining is built (G8.14), and so
  is constant folding (G8.16). Legalization
  emits obvious redundancies — a mask after an operation that provably cannot
  overflow, a `select3` merging two carries at most one of which is nonzero —
  that a canonicalizer should clean up.
- **Nothing of Ch. 2 remains unimplemented.** Structs, tuples, enums with
  payloads and explicit discriminants, both `repr`s, field access, variant
  patterns and niche optimization all run end to end, and so does `checked_*`
  (G8.4), which was the last method of Ch. 1 §4 outstanding.
- **Indirect calls** (TIR §3.7) — rejected with a diagnostic that says they
  are reserved, which is as far as "parsed but reserved" can go before the
  function-pointer chapter exists.
- **Concurrency** — AM §2.4 reserves it explicitly; the interpreter is the
  single-threaded, sequentially consistent machine that section defines.
