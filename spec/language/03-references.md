# Language Specification, Chapter 3 — Ownership, References and Slices

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), Language Ch. 0 (*Syntax*), Ch. 1 (*Types*), Ch. 2 (*Composites*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

This is the chapter that earns the name. Ch. 1 §1 states the discipline the
rest of the language follows — the Rust discipline, "inherited unchanged
because it is radix-independent" — and ownership, borrowing and lifetimes are
radix-independent to a fault. They are therefore inherited, deliberately and
almost entirely.

Two things are *not* inherited unchanged, and both are places the radix pays:

- **A reference has half the address space as niches** (§2.5), not the single
  null value a binary machine can offer, because an address is a signed value
  and memory occupies only the non-negative half.
- **The bounds check is one three-way branch** (§5.5) — and Ch. 2 §3's
  informative suggestion for how to fuse it is wrong, which this chapter
  corrects.

> **Design note (informative).** The temptation in a chapter like this is to
> improve on Rust. It was declined. Ownership and borrowing are the most
> carefully worked-out parts of the language this one descends from, their
> difficulties are well documented, and a novel variation would cost every
> reader their existing understanding to buy a change that has nothing to do
> with the radix. Where this chapter differs from Rust it is because something
> is *missing* — traits, closures, an allocator — not because a different
> answer was preferred.

---

## 1. Ownership

### 1.1 One owner

Every value has exactly one **owner**: the binding, field, or element that
holds it. When the owner goes out of scope the value is **dropped** (§1.4).

### 1.2 Moves and copies

Assigning, passing or returning a value either **copies** it or **moves** it.
A move leaves the source uninitialized, and using an uninitialized place is
rejected (§4).

Which one happens is a property of the type, and draft 0.1 fixes it
structurally rather than by a trait, because traits are Chapter 4:

> A type is **copyable** if it has no destructor (§1.4) and every type it
> contains is copyable. Every other type **moves**.

So all of Ch. 1's scalars are copyable, as are arrays, tuples, structs and
enums built only from copyable types. A type with a `drop` is not copyable,
and neither is anything containing one.

> **Note (informative).** Ch. 4 §5.1 restates this as a trait, `Copy`, which
> the compiler implements for every type meeting the rule above without being
> asked. It is deliberately *not* Rust's `Copy`, which must be opted into with
> a derive: an automatic one is what makes restating the rule change which
> programs compile in no way at all, and that is why the rule was written
> structurally here in the first place.

Draft 0.1 has no way to *opt out* of copying for a type that would otherwise
be copyable. That needs a trait to opt out of, and Ch. 4 §5.1 supplies it:
`impl !Copy for T`, the only negative implementation in the language.

### 1.3 Places and paths

A **place** is a location a value lives in: a local, a field of a place, an
element of a place, or the target of a dereference. `p`, `p.x`, `xs[i]` and
`*r` are places; a call's result is not.

Moving out of a place leaves *that place* uninitialized, not the whole
variable: after `let a = p.x;` where `p.x` moves, `p.y` remains usable and `p`
as a whole does not.

**A `match` binding is a place, and who owns it depends on the scrutinee.**
Matching a *value* moves it, so an arm's bindings receive what it held and are
dropped at the end of the arm — an arm is a scope like any other (§1.4).
Matching through a *reference* moves nothing: the bindings name storage the
referent still owns, so they may be **read and borrowed from but not moved
out of**, and nothing drops them.

```
match h {                       // h : &Holder
    Holder::Has(p) => p.id,     // fine — a read through the borrow
    …
}
match h {
    Holder::Has(p) => { let taken: Port = p; … }   // rejected: not yours
    …
}
```

The two questions "must this be dropped" and "may this be read" have the same
answer for every place but this one, which is why they are stated separately
here. A borrowed binding is initialized — reading it is not a use of something
uninitialized — and it is not an owner.

*Reserved:* binding **by reference**, so that the second example above could
be written by naming what it is rather than being refused. That is Rust's
match ergonomics, and it needs the binding's type to become `&T` — and with
it a **deref coercion**, since a binding at `&Box<T>` is then handed to
things that want `&T`. That is a third implicit conversion, and this chapter
declines to add one as a side effect of a change to `match`. Draft 0.1
refuses the move instead, which is sound and less convenient.

### 1.4 Destructors

A type acquires a destructor by defining one:

```
fn drop(self: Buffer) {
    // whatever releasing this value means
}
```

A function named `drop` whose single parameter is named `self` is the
destructor of that parameter's type. There may be at most one per type, and
its type must be a struct or enum declared in the same file.

When a value is dropped:

1. its destructor body runs, if it has one;
2. then every field or element it still owns is dropped, in **declaration
   order** for a struct and payload order for an enum variant;
3. a field the destructor moved out of is not dropped again.

Locals are dropped at the end of their scope in **reverse order of
declaration**. A value that has been moved out of is not dropped: that is the
point of tracking moves, and it is why a move is not merely a copy that
happens to be efficient.

`drop` is written this way because Chapter 4's `impl` blocks do not exist yet.
When they arrive it becomes `impl Drop for Buffer`, and no program written
against this section changes meaning.

> **Note on taking `self` by value (informative).** Rust's `Drop::drop` takes
> `&mut self`, because a by-value receiver would be dropped again on return
> and recurse forever. Here the recursion is closed differently: the body runs
> *before* the fields are dropped, and dropping the fields is not a drop of
> `self`. A destructor therefore cannot invoke itself, and a value cannot be
> dropped twice.

### 1.5 What draft 0.1 has to drop

Almost nothing. There is no allocator, no file handle and no lock, so a
destructor in 0.1 releases whatever a program's own conventions say it should
— a device port, an index into a table it maintains. The mechanism is
specified now so that the library chapter, which will have real resources, is
not also a language change.

---

## 2. References

### 2.1 The two kinds

```
&T          a shared reference
&mut T      an exclusive reference
```

A shared reference permits reading. An exclusive reference permits reading and
writing. Both are values: they can be passed, returned, and stored.

`&expr` and `&mut expr` borrow a place. `&mut` requires the place to be
declared `mut`.

### 2.2 The aliasing rule

At any point in a program, for any place, **either** one exclusive reference
to it exists, **or** any number of shared references do, and never both. While
any reference to a place is live, the place may not be moved out of, and while
an exclusive reference is live the place may not be read or written except
through that reference.

This is the whole discipline, and §4 is how it is checked.

### 2.3 Dereference and auto-dereference

`*r` names the place `r` refers to.

Field access and method calls dereference automatically: if `r: &Point`, then
`r.x` means `(*r).x`, through any number of references. This is Rust's rule
and is worth having for the same reason — the alternative is a program dense
with `(*(*r)).x`.

Automatic dereference applies to `.`, and nowhere else. `r + 1` is an error,
not `*r + 1`.

### 2.4 References are never null and never dangle

A reference always refers to an initialized value of its type that is live for
at least as long as the reference. Safe code cannot construct one that does
not, and §4 is why.

**An implementation must not place a referenceable object at address 0 or
below.** Code may occupy address 0 — the ISA begins execution there — but no
value a reference can name may. This is what makes §2.5's promise sound.

### 2.5 The niche budget of a reference

An address is a **signed** word, and memory occupies 0 … A−1 (AM §2.1).
Combined with §2.4, a reference's value is always **strictly positive**.
Therefore:

> Every value less than or equal to zero is an invalid representation of a
> reference, and is available as a niche (Ch. 2 §6).

That is (3²⁷+1)/2 niches — a little over 1.9 × 10¹² of them — where a binary
machine, whose addresses are unsigned, has exactly one. The consequences are
guarantees, not observations:

1. `Option<&T>` and `Option<&mut T>` are pointer-sized. (Already promised by
   Ch. 2 §6, and this is why.)
2. Nesting is free to a depth no program will reach:
   `Option<Option<Option<&T>>>` is still pointer-sized.
3. An enum with one reference-carrying variant and up to (3²⁷+1)/2 − 1
   fieldless variants is pointer-sized. A result type carrying a reference and
   an error code costs nothing over the reference.

Ch. 2 §6 states the reference case conservatively as "at least 1 niche",
which was written before this chapter. The stronger promise supersedes it.

**What it costs.** The niches are the non-positive addresses, and a device
region has to live somewhere; TRISC-27 §2.2 puts one at −1, −2 and −6. Those
two facts together mean **no reference can ever point at a memory-mapped
device**: the address a `&t9` would have to hold is a value the type system
has already spent as a niche. Device registers are reached by calling a
bodyless declaration instead (Ch. 0 §3.1), which is what that section's
"a memory-mapped device port, most immediately" refers to.

This is a trade, not a free win. It is taken deliberately: (3²⁷+1)/2 niches
against a form of access that a safe language would have to wrap in `unsafe`
anyway, and that 0.1 has no allocator or volatile model to give meaning to.
A revision that wants `&`-able device memory would have to move the device
region into the positive addresses and give up the guarantee above; it should
know that before it starts.

> **Design note (informative).** With that cost stated, this is still the
> clearest example in the language of the radix paying a dividend rather than
> merely costing a translation. Nothing was designed to obtain it: addresses
> are signed because *everything* is signed (Ch. 1, P1), and memory is
> non-negative because addresses index from zero. The niches fall out.

---

## 3. Lifetimes

### 3.1 What a lifetime is, and where it goes

A **lifetime** names a region of the program — a set of points at which a
reference may still be used. Lifetimes exist only for the borrow checker.
**They are erased before TIR**: no lifetime appears in the intermediate
representation, no lifetime affects layout, and two functions differing only
in lifetimes compile to the same code.

This is why lifetimes are specified here and generics are Chapter 4, although
both are written between `<` and `>`: a type parameter is *instantiated*,
which needs machinery this draft does not have, and a lifetime parameter is
*erased*, which needs none.

### 3.2 Syntax

A lifetime is written `'` followed by an identifier. Lifetime parameters are
declared in a parameter list before the value parameters:

```
fn longest<'a>(a: &'a [t27], b: &'a [t27]) -> &'a [t27] { … }

struct Window<'a> {
    data: &'a [t27],
    start: taddr,
}
```

Ch. 0 §3.5 reserves the same `<…>` list for Chapter 4's type parameters. The
two share it: `<'a, T>` will be well-formed once Ch. 4 exists, lifetimes
first.

`'static` is the lifetime of the whole program. A `const` (Ch. 0 §3.2) has it;
so does anything reached only through `const`s.

### 3.3 Elision

Most signatures need no lifetime written. Two rules, applied in order:

1. Every elided lifetime in a parameter position becomes its own fresh
   parameter.
2. If exactly one lifetime appears in parameter position, it is assigned to
   every elided lifetime in the return position.

If rule 2 does not apply and a return position elides a lifetime, the
signature is ill-formed and must be written out.

```
fn first(xs: &[t27]) -> &t27                // elided: one input, so it works
fn pick(a: &t27, b: &t27) -> &t27           // ill-formed: which one?
fn pick<'a>(a: &'a t27, b: &'a t27) -> &'a t27   // written out
```

Rust's third elision rule concerns `&self` in methods. Methods are Chapter 4;
the rule arrives with them.

### 3.4 Outlives

`'a: 'b` states that `'a` outlives `'b` — every point of `'b` is a point of
`'a`. It may be written in a parameter list:

```
fn narrow<'a: 'b, 'b>(x: &'a [t27]) -> &'b [t27] { x }
```

`T: 'a` — a *type* outliving a lifetime — is a bound, and bounds are part of
the trait system. It waits for Chapter 4. In its absence, a type parameter
cannot appear in this draft at all, so nothing is lost yet.

### 3.5 Variance

- `&'a T` is covariant in `'a` and in `T`.
- `&'a mut T` is covariant in `'a` and **invariant** in `T`.
- A struct is covariant in a lifetime parameter if every use of it is, and
  invariant otherwise.

Covariance is what lets a longer-lived reference be used where a
shorter-lived one is expected. The invariance of `&mut T` in `T` is the rule
that stops a shorter-lived reference being written through a longer-lived
exclusive one.

---

## 4. The borrow checker

### 4.1 What it checks

Three things, over every path through a function:

1. **Initialization.** A place is read only where it is certainly
   initialized, and moved out of only where it is certainly initialized.
2. **Aliasing.** §2.2's rule holds at every point.
3. **Lifetimes.** Every reference's region is contained in the region for
   which the place it refers to is live.

A program that passes has no dangling references, no aliased mutation, and no
use of a moved value. It may still fault — an out-of-bounds index, a trapping
overflow — because those are defined faults (AM §4) and not memory errors.

### 4.2 A borrow lives to its last use

A borrow's region ends at its last use, not at the end of its lexical scope:

```
let mut p = Point { x: 1, y: 2 };
let r = &p;
let a = r.x;        // the last use of `r`
p.x = 9;            // accepted: the borrow is over
```

This is what Rust calls non-lexical lifetimes, and it is specified rather than
merely permitted because the lexical alternative rejects the program above,
which is correct and which a reader expects to work.

A borrow used inside a loop is live across the whole loop, since a later
iteration may reach the use again.

### 4.3 What is rejected

```
let r;
{
    let v = Point { x: 1, y: 2 };
    r = &v;                     // rejected: `v` dies before `r` is used
}
let a = r.x;

let mut xs = [1, 2, 3];
let first = &xs[0];
xs[1] = 9;                      // rejected: `xs` is borrowed
let b = *first;

let a = Buffer { … };           // a type with a destructor
let b = a;                      // `a` is moved
let c = a;                      // rejected: `a` is uninitialized
```

### 4.4 What it does not check

Aliasing rules for concurrency, because AM §2.4 reserves the concurrency
model; `Send` and `Sync` arrive with it and with Chapter 4.

---

## 5. Slices

### 5.1 `[T]`

`[T]` is the type of a run of `T` whose length is not part of the type. It is
**dynamically sized**: it has no size known at compile time, so no local, no
field and no parameter may have this type directly. It appears only behind a
reference.

### 5.2 `&[T]` is two words

A shared slice reference is a **fat pointer**: a pointer to the first element,
then a length.

| Field | Offset | Type |
|---|---|---|
| pointer | 0 | address |
| length | 3 | `taddr` |

Size 6 trytes, alignment 3. `&mut [T]` has the same layout.

The length is a `taddr` because Ch. 1 §2 says length-reporting APIs use
`taddr` — signed, like everything else, and guaranteed non-negative by every
safe operation that produces one.

The pointer comes first so that a slice reference and an ordinary reference
agree on their first word, which is the arrangement a future `unsafe` chapter
will want and which costs nothing now.

### 5.3 Where a slice comes from

An array reference coerces to a slice reference:

```
let xs: [t27; 3] = …;
let s: &[t27] = &xs;        // length 3, from the type
```

This is the only way to obtain one in draft 0.1. Sub-slicing — `&xs[a..b]` —
needs range expressions, which Ch. 0 §4 reserves; it arrives with them.

### 5.4 Length and indexing

`s.len()` yields the length as a `taddr`. Indexing `s[i]` takes a `taddr` and
is bounds-checked.

### 5.5 The bounds check

An index is in bounds iff `0 ≤ i < len`. Ch. 2 §3 suggests fusing the two
comparisons into a single branch on `tmin(sign(i), i <=> len)`. **That fusion
is incorrect**: for an in-bounds index `i <=> len` is −1 and `sign(i)` is 0 or
+1, so the minimum is −1 — the same answer it gives for a negative index. The
suggestion is marked informative, and the normative requirement it accompanies
(that an out-of-bounds access faults and never proceeds) is unaffected.

**Two comparisons and two branches is the recommended form**, and it is what
the reference compiler emits.

A fusion that is at least *correct* is

> `tmin(sign(i), sign(len − 1 − i))`

which is −1 exactly when the index is out of bounds, so one `br3` decides.
It is not recommended, and the arithmetic says why: it costs `cmp i, 0`, a
subtract, a subtract, a `cmp` and a `tmin` — five instructions before the one
branch, against two comparisons and two branches for the naive form. It is
also valid only where `len − 1 − i` cannot overflow, which a trapping
subtraction guarantees at the cost of turning one class of out-of-range index
into `F_OVERFLOW` instead of a clean bounds fault.

The fusion earns its keep in one case: `len` a compile-time constant, where
`len − 1` folds and the count drops to three instructions and one branch. An
implementation may emit either form.

Negative indices are out of bounds. They are **not** end-relative, and no
future revision will make them so — Ch. 2 §3 claims this already, and it is
repeated here because a slice is where someone would try.

---

## 6. What this chapter deliberately does not define

- **`Box<T>`, allocation, and collections.** They belong to the library
  chapter. This chapter supplies what they will rest on: ownership, moves,
  destructors, and the layout facts of a pointer (word-sized, strictly
  positive, one owner). Ch. 2 §8's recursive-type example is written against
  `Box` and remains correct as a description of the pattern; it is not
  writable until the library chapter exists.
- **Raw pointers and `unsafe`.** TIR §5 already defers integer-to-pointer
  conversion to "the language's `unsafe` chapter"; raw pointers belong to the
  same document.
- **Interior mutability.** `RefCell` and its relatives are library types built
  on `unsafe`. Informatively: their borrow state is *three*-valued —
  unborrowed, shared, exclusive — which is one trit, though a shared borrow
  count needs more than a trit and so the representation is not as neat as it
  first looks.
- **Closures and trait objects.** Chapter 4, along with their fat-pointer
  representations (Ch. 2 §8).
- **`T: 'a` bounds, `where` clauses, and `for<'a>`.** Chapter 4.
- **`Send`, `Sync`, and any concurrency rule.** Reserved with AM §2.4.
- **`Pin` and self-referential types.** Not anticipated.
- **`ref` patterns.** Ch. 0 §4 reserves the keyword; binding modes wait for a
  revision with real evidence about which default hurts less.

---

## Appendix A (informative) — worked examples

```rust
struct Point { x: t27, y: t27 }

// A shared borrow: reads, and promises not to write.
fn magnitude_squared(p: &Point) -> t27 {
    p.x * p.x + p.y * p.y        // auto-dereferenced
}

// An exclusive borrow: the only reference to `p` while it lives.
fn translate(p: &mut Point, dx: t27, dy: t27) {
    p.x += dx;
    p.y += dy;
}

// One input lifetime, so the output's is elided.
fn head(xs: &[t27]) -> &t27 {
    &xs[0]
}

// Two inputs and one output: written out, because elision cannot choose.
fn longer<'a>(a: &'a [t27], b: &'a [t27]) -> &'a [t27] {
    if a.len() >= b.len() { a } else { b }
}

fn main() -> t27 {
    let mut p = Point { x: 3, y: 4 };
    let m = magnitude_squared(&p);      // shared borrow, ends here
    translate(&mut p, 1, 1);            // exclusive borrow, ends here

    let xs: [t27; 3] = [10, 20, 30];
    let s: &[t27] = &xs;
    m + *head(s) + p.x
}
```

## Appendix B (informative) — bug classes removed by construction

Continuing the scorecard, for this chapter:

| Binary-world bug class | Why it cannot occur here |
|---|---|
| use after free | a reference's region is contained in its referent's (§4.1) |
| double free | a moved-out value is not dropped (§1.4) |
| iterator invalidation | mutating a container needs an exclusive borrow (§2.2) |
| null pointer dereference | a reference is strictly positive (§2.4) |
| a null check costing a branch on the hot path | absence is a niche, not a test (§2.5) |
| `Option<&T>` costing a word more than `&T` | the same (§2.5, guarantee 1) |
| an end-relative index arriving by accident | negative indices are out of bounds and stay so (§5.5) |
