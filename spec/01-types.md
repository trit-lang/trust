# Language Specification, Chapter 1 — Type System

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (hereafter *AM*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

This chapter defines the primitive types of Trust, their value sets, layout,
conversions, and the semantics of the operations the language exposes over
them. All numeric semantics are those of the AM; this chapter adds only the
type-level rules (which widths exist, what converts to what, which overflow
flavor each surface operator selects). Composite types (tuples, arrays,
structs, enums), references, and the ownership model are Chapters 2–4.

---

## 1. Design principles

**P1 — No unsigned types.** Balanced ternary is inherently signed with a
symmetric range (AM §1.2). Trust has no unsigned integers, no `u`-prefixed
anything, and therefore no signed/unsigned comparison rules, no implicit
sign-conversion pitfalls, and no `MIN`-related partiality: `-x` and `x.abs()`
are total for every numeric type.

**P2 — No implicit numeric conversions.** As in Rust: every conversion
between distinct numeric types is written explicitly. Mixed-width arithmetic
is a compile-time error.

**P3 — Three-way is primitive, two-way is derived.** The primitive comparison
yields a `trit` (AM §3.5). Boolean predicates are projections of it.
`match` over a `trit` is exhaustive with three arms and compiles to the
TIR/ISA three-way branch.

**P4 — Overflow is never silent in the default profile.** Arithmetic
operators trap on overflow in checked builds and wrap in release builds,
with explicit `checked_*` / `wrapping_*` / `saturating_*` forms always
available — the Rust discipline, inherited unchanged because it is
radix-independent.

---

## 2. Primitive types

| Type | Values | Width (AM) | Stored size | Alignment |
|---|---|---|---|---|
| `trit` | −1, 0, +1 | 1 trit | 1 tryte | 1 tryte |
| `bool` | `false`, `true` | 1 trit | 1 tryte | 1 tryte |
| `t9` | −9 841 … +9 841 | 9 trits | 1 tryte | 1 tryte |
| `t27` | ±3 812 798 742 493 | 27 trits | 3 trytes | 3 trytes |
| `taddr` | target address range | word | 3 trytes | 3 trytes |
| `()` | () | — | 0 | 1 |

Notes, in order:

**`trit`** is a first-class scalar, not sugar over an integer. Its literals
are `-1t`, `0t`, `1t` (the `t` suffix disambiguates from integer literals).
It supports the trit-wise operations of AM §3.4 (`min`, `max`, `neg` via
unary `-`, `*` as `tmul`) and is the result type of the three-way comparison
operator. `trit` is *semantically* the 1-trit integer, and `as` conversions
to/from wider integers are value-preserving in the obvious way, but there is
no implicit coercion in either direction (P2).

**`bool`** is a distinct nominal type, not a subrange of `trit`. Rationale:
the majority of control flow remains genuinely two-valued, and making `if`
take a three-valued condition would either leave a silent third path or force
ceremony on every conditional. `if` requires `bool`. `match` accepts `trit`
(and everything else). There are no implicit `bool` ↔ `trit` conversions;
the projections in §5 are the sanctioned bridge. Storage encoding:
`false` = 0, `true` = 1 in a full tryte; the values −1 … remaining tryte
patterns are invalid `bool` representations (as in Rust, producing one is
library-`unsafe` UB, which the safe subset cannot do).

**`t9`, `t27`** are the balanced ternary integers, named by trit width. Draft
0.1 deliberately ships exactly two integer widths. A `t3` (±13) and a `t81`
(double-word) are reserved names, not part of 0.1. There is no `tsize`
bikeshed: `taddr` below covers the pointer-sized-integer role.

**`taddr`** is the address-width integer, the analogue of `usize` — except
that, unsigned types not existing (P1), it is *signed* like everything else,
and its width is the target's pointer width (word-width on the AM). Indexing
and length-reporting library APIs use `taddr`; negative values are
representable but every safe API that produces a length or index guarantees
a non-negative result, and bounds checks reject negatives like any other
out-of-range index. (This is the one place the absence of unsigned types
costs a compare; it buys the total absence of the unsigned-underflow bug
class, e.g. the perennial `len - 1` wraparound.)

---

## 3. Literals

Integer literals may be written in three radices:

| Form | Example | Meaning |
|---|---|---|
| decimal | `6`, `-9841` | usual meaning |
| balanced ternary, prefix `0t` | `0t1T0` | digits `1`, `0`, `T` (= −1), MST first; `0t1T0` = 6 |
| heptavintimal (base 27), prefix `0h` | `0hJ` = 6 | digits `0-9` `A-Q`; compact form, 1 digit = 1 heptavintimal group = 3 trits |

`0t` literals are trit-exact: each character is one trit of the
representation, which makes them the natural notation for masks and
trit-pattern constants. `0h` is the compact professional notation (3 trits
per digit, so a `t27` value is at most 9 digits), playing the role hex plays
in the binary world. There is no hexadecimal or octal: both are artifacts of
the bit.

### 3.1 The heptavintimal digits

A heptavintimal digit is a **balanced** group of three trits, so it denotes
−13 … +13 and not 0 … 26. That is what "playing the role hex plays" requires:
hex's defining property is that it is digit-exact over the underlying radix,
and for balanced ternary that means balanced digits. It is also the only
reading under which a `0h` literal can denote a negative number without a
sign, matching `0t`.

| Digit | `0` | `1` | `2` | `3` | `4` | `5` | `6` | `7` | `8` | `9` | `A` | `B` | `C` | `D` |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Value | −13 | −12 | −11 | −10 | −9 | −8 | −7 | −6 | −5 | −4 | −3 | −2 | −1 | **0** |

| Digit | `E` | `F` | `G` | `H` | `I` | `J` | `K` | `L` | `M` | `N` | `O` | `P` | `Q` |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Value | +1 | +2 | +3 | +4 | +5 | +6 | +7 | +8 | +9 | +10 | +11 | +12 | +13 |

> **A leading zero is not neutral.** `D` is the zero digit, not `0`. So `0hJ`
> is 6, `0h0J` is −13 × 27 + 6 = **−345**, and `0h00J` is −9822. Every other
> notation in this document and in the binary world permits padding a literal
> with leading zeros; this one does not, because its zero is written `D`.
> Pad with `D` or do not pad. A formatter must not zero-pad a `0h` literal,
> and a reader should treat a `0h` literal beginning with `0` as suspicious.

`0h2C9`, the example given in earlier drafts, is −8050. It was a poor first
example for exactly this reason and is retained only here, as the warning it
turned out to be.

A leading `-` on a `0t` literal is permitted but redundant (`-0t1T` = `0tT1`);
style tooling normalizes to the unsigned-magnitude-free canonical form.

Underscore separators are permitted in all radices (`0t1T0_T01`,
`3_812_798`). Literal type is inferred as in Rust, defaulting to `t27` when
unconstrained; a literal that does not fit its inferred type is a
compile-time error, never a silent wrap.

`trit` literals use the `t` suffix as noted above. `bool` literals are
`false` and `true`.

---

## 4. Operators and overflow flavors

Binary arithmetic operators `+ - * / %` and unary `-` are defined on `t9`,
`t27`, `taddr` (same-type operands only, P2), with AM semantics:

| Surface | AM operation | Overflow flavor (default profile) |
|---|---|---|
| `+` `-` `*` | add / sub / mul | trap (checked build) / wrap (release) |
| unary `-` | neg | **total — no flavor needed** |
| `/` | div, **round to nearest, ties away from zero** | total except `/ 0` → fault |
| `%` | rem, `|r| ≤ |b|/2` | total except `% 0` → fault |
| `<<` `>>` | shl / shr by powers of 3 | `<<` flavors as `*`; `>>` total (AM §3.3) |

Two consequences deserve their own sentences, because every reader arriving
from C or Rust will assume otherwise:

1. **`/` does not truncate toward zero.** `7 / 2 == 4` (nearest, tie away
   from zero), `8 / 3 == 3`, `-8 / 3 == -3`. The remainder is
   correspondingly small in magnitude and may differ in sign from the
   dividend: `8 % 3 == -1`.
2. **`>>` is division, not truncation, and it is exact rounding for free.**
   `x >> k` equals `x / 3ᵏ` under round-to-nearest with no tie case ever
   arising. Fixed-point rescaling therefore needs no rounding fixup code.

Every arithmetic method family from Rust is carried over with identical
naming: `checked_add`, `wrapping_add`, `saturating_add`, `overflowing_add`,
and likewise for `sub`, `mul`, `shl`. The families for `neg` and `abs` do
**not** exist — those operations are total (P1) and providing a
`checked_neg` would be an API lie.

**`a.mulh(b)` is the high half of the product**, and it is a method rather
than an operator because there is no operator anyone would recognize. Where
`a * b` gives the low `N` trits of the product, `a.mulh(b)` gives the high
`N`, so that

> `a.mulh(b) · 3ᴺ + a.wrapping_mul(b)` reconstructs the exact product.

It is total: the high half of a product of two `tN` values always fits in
`tN`, which is a property of the symmetric range and not an accident. It has
no flavors for the same reason.

This is the operation fixed-point arithmetic is built on. Multiplying two
values scaled by 3ᵏ gives a product scaled by 3²ᵏ, and recovering the
original scale means keeping trits the low half has already dropped —
`mulh` is where they are. Without it a `t27` fixed-point format is limited
to roughly half the word, and with it the full word is usable.

TRISC-27 §4.1 provides `mulh` as an instruction and TIR §3.1 as an operation;
this is the same one, named the same way, at the surface.

Trit-wise operations on integers are provided as methods, not operators, in
draft 0.1: `a.tmin(b)`, `a.tmax(b)`, `a.tmul(b)`, `a.tneg()` (the last
being identical to unary `-`; provided for symmetry when writing
trit-manipulation code). Whether `& | ^` should be repurposed as
`tmin/tmax/tmul` operators is an open syntax question deferred with the rest
of surface syntax; the semantics above are fixed regardless.

---

## 5. Comparison

The three-way comparison operator `<=>` is primitive:

```
a <=> b   : trit     // -1t, 0t, or 1t   (AM cmp)
```

The six two-way predicates `== != < <= > >=` yield `bool` and are defined as
projections of `<=>`. The compiler is required to treat them as such (a
chained `if a < b … else if a > b …` must cost one comparison), and
`match a <=> b` with arms `-1t / 0t / 1t` is the idiomatic exhaustive
three-way dispatch, compiling to `br3`.

Ordering traits mirror Rust's `Ord`/`PartialOrd` shape, except the core
method returns `trit` rather than an `Ordering` enum — the hardware type *is*
the ordering type. `Ordering` as a nominal enum does not exist.

---

## 6. Conversions

All numeric conversions are explicit via `as` (draft 0.1; `From`/`TryFrom`
style traits arrive with the trait system chapter):

| Conversion | Semantics |
|---|---|
| widening (`t9 as t27`, `trit as t9`, …) | value-preserving, total |
| narrowing (`t27 as t9`, …) | **wrapping** (AM §3.1 wrap into the narrow symmetric range); `checked` narrowing via library |
| `trit as bool`, `bool as trit` | **not provided** — see below |
| `bool as t9` / `t27` | `false` → 0, `true` → 1 |
| `taddr` ↔ `t27` | value-preserving on the AM (same width); on narrower targets, target-defined width rules apply per TIR target description |

Narrowing-as-wrapping follows Rust's `as` precedent; note that ternary
wrapping is negation-symmetric (AM Appendix A.4), so `(-x as t9) == -(x as t9)`
holds — a small identity, but one binary `as` cannot offer.

`trit` ↔ `bool` has no `as` path *by design*: the two reasonable mappings
(is-nonzero? is-positive?) are both plausible, so the language refuses to
pick silently. The bridge is the explicit projection methods:

```
t.is_pos()  t.is_zero()  t.is_neg()   : trit → bool
b.to_trit()                           : bool → trit   (false→0t, true→1t)
sign(x)  ≡  x <=> 0                   : integer → trit
```

---

## 7. Layout guarantees

For the composite-type chapters to build on, this chapter fixes the scalar
guarantees:

1. Sizes and alignments are exactly the table in §2 on the AM; targets with
   different native widths express layout through the TIR target description,
   and `size_of` / `align_of` are target-dependent constants.
2. `trit` and `bool` occupy a full tryte in memory; their unused capacity is
   *not* observable in safe code. Packed trit arrays (`[trit; N]` occupying
   ⌈N/9⌉ trytes) are a standard-library type built on `unsafe`, not a
   primitive layout rule.
3. Multi-tryte scalars are little-trytean (AM §2.2). Code that never leaves
   the safe subset cannot observe trytean order.

---

## 8. Reserved

Reserved type names, claimed by this chapter so no user identifier can take
them, with semantics deferred: `t3`, `t81` (additional widths); `f27`-family
(ternary floating point, if a future revision adopts a format); `fix9_18`
naming pattern (fixed point). Draft 0.1 programs using these names are
ill-formed.

---

## Appendix (informative) — bug classes removed by construction

A running list, maintained because it is the honest scorecard of the design:

| Binary-world bug class | Why it cannot occur here |
|---|---|
| signed/unsigned comparison confusion | no unsigned types (P1) |
| unsigned underflow (`len - 1` when len = 0) | no unsigned types; `taddr` is signed |
| `-MIN` overflow / UB, `checked_neg` failure | MIN = −MAX; negation total |
| `abs(MIN)` negative result | same |
| `MIN / -1` overflow fault | `div(MIN, −1) = MAX`, well-defined |
| truncation-vs-floor division sign bugs | one division: round-to-nearest, negation-symmetric |
| implicit integer promotion surprises (C) | no implicit conversions (P2) |
| bool-as-int arithmetic accidents | `bool` nominal, no arithmetic, explicit projections |
