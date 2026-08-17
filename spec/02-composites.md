# Language Specification, Chapter 2 — Composite Types and Layout

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), Language Ch. 1 (*Types*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

This chapter defines the composite (aggregate and choice) types of Trust:
tuples, arrays, structs, and enums — their value semantics, their layout
rules, and the discriminant model. References, slices, and everything
involving borrowing are Chapter 3; traits and generics are Chapter 4. Where
this chapter writes `T`, `U` for element types, the rules are schematic and
apply once generics exist; draft 0.1 tooling may implement them
monomorphically.

---

## 1. Layout model and vocabulary

Every type has a **size** (in trytes) and an **alignment** (in trytes, a
power of 3, ≥ 1). A value of alignment *a* is stored only at addresses that
are multiples of *a* (AM §2.3). Size is always a multiple of alignment, so
arrays need no inter-element padding beyond what the element type already
contains.

Layout falls under one of two regimes, selected per type:

- **`repr(lang)`** — the default. The compiler may order, pad, and pack
  fields arbitrarily, may exploit niches (§6), and may change layout between
  compiler versions. Programs must not depend on it.
- **`repr(linear)`** — fields laid out in declaration order, each at the
  next address satisfying its alignment; trailing padding to the struct's
  alignment. Size and offsets are a documented, stable function of the
  declaration. This is the interop / memory-mapped-I/O / on-disk-format
  regime, the analogue of `repr(C)` — named `linear` because there is no C
  ABI to be compatible with.

Padding trytes have unspecified contents; reading them is possible only
through `unsafe` re-interpretation and yields unspecified (not undefined)
tryte values.

**Zero-sized types (ZSTs)** are permitted: size 0, alignment 1. `()` is the
canonical ZST; unit structs and empty tuples of ZSTs are ZSTs. ZST reads and
writes compile to nothing; an array of ZSTs is itself a ZST regardless of
length.

---

## 2. Tuples

`(T₁, …, Tₙ)` is an anonymous product. `()` is the unit type (Ch. 1 §2).
`(T,)` is distinct from `T`.

Tuples are always `repr(lang)`: their layout is unspecified and there is no
`repr(linear)` tuple. Code that needs defined layout names its fields —
i.e. uses a struct. Field access is positional (`x.0`, `x.1`).

Equality, ordering, and the three-way comparison lift componentwise and
lexicographically: `a <=> b` on tuples compares `a.0 <=> b.0` first and
short-circuits on the first nonzero trit. This makes `<=>` on tuples a fold
over trit results — the idiom `(a.0 <=> b.0).or_else(|| a.1 <=> b.1)` from
binary-world comparator chains is subsumed by the primitive.

---

## 3. Arrays

`[T; N]` is N contiguous elements of `T`. N is a compile-time constant of
type `taddr` and must be **non-negative**; a negative length is ill-formed
(this is the type-level face of the signed-`taddr` decision, Ch. 1 §2).

Size is `N · size_of::<T>()`, alignment is `align_of::<T>()`, elements at
offsets `i · size_of::<T>()` in index order — array layout is fully defined
even under `repr(lang)`, because iteration and indexing arithmetic depend
on it.

Indexing takes `taddr`. A bounds check rejects indices outside `0 … N−1`;
negative indices are simply out of bounds — they are **not** Python-style
end-relative aliases, and no future revision will make them so (claiming
this now so no library invents the convention). The bounds check is one
three-way comparison: `i <=> N` must be `-1t` *and* `sign(i)` must not be
`-1t`; codegen is encouraged to fuse these, and on targets with `br3` the
fused check is a single branch on `tmin(sign(i), i <=> N)`... (informative;
the normative requirement is only that out-of-bounds access faults or
panics per the safety chapter, never proceeds).

**Packed trit arrays are not `[trit; N]`.** Per Ch. 1 §7, `[trit; N]`
occupies N full trytes. The standard library provides `TritSlab<N>`
(⌈N/9⌉ trytes, 9 trits per tryte) as the packed form, built on `unsafe`;
it is a library type with getter/setter methods, not a primitive, and its
existence is noted here only to stop `[trit; N]` from being "fixed" to pack.

---

## 4. Structs

```
struct Point { x: t27, y: t27 }        // named fields
struct Trip(t9, t9, t9);               // tuple struct
struct Marker;                          // unit struct (ZST)
```

Structs are nominal products. Default `repr(lang)`; `repr(linear)` opts into
defined layout (§1). Field order in `repr(lang)` may be permuted by the
compiler to minimize padding — with trytes of 9 trits and only two scalar
alignments (1 and 3), padding arises exactly when a 3-aligned field follows
a size-not-multiple-of-3 prefix, and the compiler's reordering obligation
is correspondingly mild.

A struct's alignment is the maximum alignment of its fields (minimum 1);
its size is a multiple of that alignment.

---

## 5. Enums

```
enum Sign { Neg, Zero, Pos }
enum Shape { Dot, Line(t27), Rect { w: t27, h: t27 } }
```

Enums are nominal sums (tagged unions). A value is exactly one **variant**,
plus that variant's payload. `match` over an enum must be exhaustive.

### 5.1 Discriminants

Each variant has a **discriminant**: an integer constant identifying it.
Explicit discriminants may be assigned (`enum E { A = -1, B = 0, C = 1 }`)
and may be **negative** — the balanced range is symmetric and the language
does not pretend otherwise. Unassigned discriminants default to
0, 1, 2, … in declaration order. Two variants with the same discriminant
are ill-formed.

The discriminant's *storage type* under `repr(lang)` is unspecified (the
compiler picks the narrowest workable encoding, including niches, §6).
`repr(linear)` enums store the discriminant as a leading `t9` (or `t27` if
any explicit discriminant requires it) followed by the payload, union-style,
at the payload's natural alignment.

### 5.2 Three-variant enums and `br3` (informative)

A three-variant fieldless enum with discriminants −1, 0, +1 is
representation-identical to `trit`, and `match` over it lowers to a single
`br3`. `Sign` above, written with explicit `-1/0/1` discriminants, *is*
`trit` with names. The compiler applies this whenever discriminants fit
{−1, 0, +1} after translation; the guidance to library authors is that
naturally three-valued states (less/equal/greater, shrink/hold/grow,
left/center/right) should be declared as such enums rather than as `bool`
pairs or magic integers — the hardware three-way branch is waiting for them.

This is also why Ch. 1 removed Rust's `Ordering`: the comparison result is
already the optimal representation. User enums re-adding names for specific
domains are the intended pattern.

### 5.3 Fieldless enums and casts

A fieldless enum may be cast to an integer type with `as`, yielding its
discriminant. There is no cast in the reverse direction (fallible; library
`try_from` territory).

---

## 6. Niche optimization

`repr(lang)` permits — and the reference compiler implements — storing an
enum's discriminant inside **invalid representations** ("niches") of its
payload, so that the discriminant costs no extra space.

Ternary scalars are unusually niche-rich, and this section is where the
radix pays layout dividends:

- A `bool` occupies a tryte using 2 of 19 683 patterns → 19 681 niches.
- A `trit` occupies a tryte using 3 patterns → 19 680 niches.
- References (Ch. 3) exclude the null address → at least 1 niche.

The question asked of a payload is whether **one scalar in it** has enough
invalid values to tell the variants apart — not whether the payload as a
whole has that many. The two differ: a twelve-tryte payload has 3^108
patterns, a number no implementation carries, and asking about the whole
payload means asking a question whose answer cannot be represented. Asking
about the scalar keeps the arithmetic small, and the scalar is where the
discriminant would go in any case.

Consequences the standard library relies on and user code may rely on
(these are **guaranteed** for `repr(lang)`, elevating them from
implementation detail to spec):

1. `Option<&T>` is pointer-sized.
2. `Option<bool>`, `Option<trit>`, and any enum with ≤ 19 680 fieldless
   variants wrapped around a `bool`/`trit` payload occupy **one tryte**.
3. Nesting up to the niche budget adds no size:
   `Option<Option<trit>>` is still one tryte.

Guarantee 2's practical face: the ubiquitous "value or absent" tri-state
that binary systems hack as `-1 | 0 | 1` sentinel integers, and the
"unknown / false / true" of three-valued logic, are both one-tryte types
here with full type safety — `Option<bool>` *is* Kleene logic's value
space, and the library defines its `tmin/tmax/tneg` lifting under the name
`kleene` (informative; library chapter).

---

## 7. Unions

`union` (untagged, all fields sharing storage, reads `unsafe`) is
**reserved**: the keyword is claimed, the semantics are deferred beyond
draft 0.1. Rationale: unions exist chiefly for FFI and bit-reinterpretation,
and neither pressure exists yet in a stack with no foreign ABI. Type
punning through unions will *not* be the sanctioned mechanism when the need
arrives; a `transmute`-style checked-width intrinsic will be.

---

## 8. Recursive and dynamically sized types

Recursive types must be size-finite: a struct/enum may refer to itself only
through indirection (Ch. 3 pointer types). `struct Node { next: Node }` is
ill-formed; `struct Node { next: Option<Box<Node>> }` is the pattern (and
by §6.1 the `Option` is free).

Dynamically sized types (slices `[T]`, trait objects) are defined in
Chapters 3–4 with their fat-pointer representations; this chapter only
reserves that `size_of` is a compile-time constant exactly for the types
this chapter defines.

---

## Appendix (informative) — worked layout examples

Assuming the AM (tryte-addressable, alignments 1 and 3):

| Type | Size | Align | Notes |
|---|---|---|---|
| `(t9, t9)` | 2 | 1 | no padding possible |
| `struct { a: t9, b: t27 }` `repr(linear)` | 6 | 3 | a at 0, 2 trytes padding, b at 3 |
| same, `repr(lang)` | 6 | 3 | reordering (b at 0, a at 3) still pads to 6: size must be a multiple of align — an example of padding that reordering cannot remove |
| `struct { a: t9, b: t9, c: t27 }` `repr(linear)` | 6 | 3 | a at 0, b at 1, 1 tryte padding, c at 3 — declaration order already optimal |
| `struct { a: t27, b: t9 }` `repr(linear)` | 6 | 3 | b at 3, 2 trytes trailing padding |
| `[trit; 9]` | 9 | 1 | unpacked, one trit per tryte |
| `TritSlab<9>` (library) | 1 | 1 | packed, 9 trits in one tryte |
| `Option<trit>` | 1 | 1 | niche |
| `enum { A=-1, B, C=1 }` | 1 | 1 | trit-shaped; `match` → `br3` |
