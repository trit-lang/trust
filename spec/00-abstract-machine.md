# Document 00 — The Balanced Ternary Abstract Machine

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Stability** | Normative for all documents in this repository |
| **Depended on by** | Language Specification, TIR Specification, ISA Specification |

This document defines the abstract machine (AM) that gives meaning to every
other specification in this project. The language specification defines source
programs in terms of AM behavior; the TIR specification defines IR semantics in
terms of AM behavior; the ISA specification defines one concrete realization of
the AM. No other document may redefine anything specified here.

The abstract machine is **not** a description of any implementation. Real
targets (the reference VM, SBTCVM, future hardware) may differ in word width,
address-space size, and instruction encoding; a conforming implementation must
merely produce observable behavior equivalent to the AM's for all defined
programs. Behavior explicitly listed as *undefined* or *reserved* carries no
such obligation.

---

## 1. Balanced ternary representation

### 1.1 The trit

The fundamental unit of information is the **trit**, taking one of three
values: **−1, 0, +1**.

In written notation this specification uses the symbols `T`, `0`, `1` for the
values −1, 0, +1 respectively. Trit strings are written most-significant trit
first: `1T0` denotes (+1)·9 + (−1)·3 + 0 = 6.

### 1.2 Numeric interpretation

A sequence of *n* trits *t*ₙ₋₁ … *t*₁ *t*₀ denotes the integer

> Σ *tᵢ* · 3ⁱ  for *i* = 0 … n−1

Every *n*-trit sequence denotes a unique integer in the **symmetric range**

> −(3ⁿ − 1)/2 … +(3ⁿ − 1)/2

and every integer in that range has exactly one *n*-trit representation. There
is no sign trit, no two's-complement analogue, and no distinct representation
of negative zero. Consequences that the rest of the stack relies on:

1. **All values are signed.** The AM has no unsigned integer types and no
   signed/unsigned distinction anywhere.
2. **The range is symmetric: MIN = −MAX.** Negation is a total function; it
   never overflows.
3. **Negation is trit-wise.** −x is obtained by replacing every trit tᵢ with
   −tᵢ. It is a local, carry-free, O(1)-depth operation.
4. **The sign of a value is the sign of its most significant nonzero trit.**
   Comparison does not require subtraction.

### 1.3 Storage units

| Unit | Width | Value range | Role |
|---|---|---|---|
| **trit** | 1 trit | −1 … +1 | smallest unit of information |
| **tryte** | 9 trits | −9 841 … +9 841 | smallest *addressable* unit |
| **word** | 27 trits | −3 812 798 742 493 … +3 812 798 742 493 | natural register width of the AM |

A word is exactly 3 trytes. These widths are properties of the **abstract
machine**; concrete targets may declare different native widths in their target
descriptions (see TIR spec, *Target Descriptions*), and code generation must
legalize AM-width operations onto whatever the target provides. The language's
type system, however, is defined against the AM widths above.

> **Design note (informative).** 9 and 27 are chosen as powers of three so
> that tryte↔word conversion, alignment, and radix shifts stay exact. The
> historical Setun used 18-trit words; SBTCVM Gen2 uses 9. Neither choice is
> normative here.

---

## 2. Memory

### 2.1 Address space

Memory is a flat, linear array of trytes. Each tryte has a unique address.
Addresses are word-sized values interpreted in the range 0 … A−1, where A is
the implementation-defined address-space size (at most 3²⁷ trytes for the AM).

There is no MMU, no permission model, and no segmentation in the AM. (Targets
may add them; programs relying on them are target-specific.)

### 2.2 Multi-tryte values in memory

A value wider than one tryte occupies consecutive trytes in
**little-trytean** order: the least significant tryte is stored at the lowest
address. A word at address *a* occupies trytes *a*, *a*+1, *a*+2 with the
least significant tryte at *a*.

### 2.3 Alignment

Natural alignment is defined per width:

| Value width | Natural alignment |
|---|---|
| 1 trit … 9 trits | 1 tryte |
| 10 … 27 trits | 3 trytes |

A load or store of a value at an address that is not a multiple of its natural
alignment has **undefined behavior** at the AM level. (The language's safe
subset statically prevents constructing such accesses; the reference VM traps
on them.)

Sub-tryte values (a single trit, a bool) occupy a full tryte when stored to
memory. Their in-memory encoding is the sign-extension-free 9-trit
representation of the value (i.e. the value itself, since −1, 0, +1 are all
representable). Packing multiple trits into one tryte is a library/codegen
concern, not an AM primitive.

### 2.4 Execution model

Draft 0.1 defines a **single-threaded, sequentially consistent** machine:
memory operations take effect in program order and each load observes the most
recent prior store to the same location.

Section reserved: a future revision will introduce a happens-before /
acquire-release concurrency model. Nothing in the current stack may assume
details of that future model beyond the existence of this reservation.

---

## 3. Arithmetic semantics

All arithmetic below is defined on *n*-trit values for arbitrary *n*; the
language and TIR instantiate it at specific widths.

### 3.1 Addition, subtraction, multiplication

`add`, `sub`, `mul` compute the exact mathematical result. If the result is
representable in *n* trits, that is the result. If not, the operation
**overflows**; what happens then is *not* defined by the AM but by the
operation flavor the higher layer selected:

- **wrapping** — the result is the unique *n*-trit value congruent to the
  exact result modulo 3ⁿ. (Because the range is symmetric, wrapping is
  negation-compatible: `wrap(−x) = −wrap(x)`.)
- **trapping** — the machine halts with an overflow fault.
- **checked / saturating** — defined in the language spec as library-level
  compositions of the above plus comparison; the AM provides wrapping and
  trapping primitives only, plus an overflow-flag-producing form for
  efficient checked lowering.

`neg` and `abs` are **total**: since MIN = −MAX, neither can overflow at any
width. They have no wrapping/trapping flavors because none are needed.

### 3.2 Division and remainder: round to nearest

This is the single largest semantic departure from binary-world C descent,
and it is deliberate.

`div(a, b)` with b ≠ 0 yields the integer **nearest** to the exact quotient
a/b. Equivalently: the unique q such that a = q·b + r with **|r| ≤ |b|/2**.
`rem(a, b)` yields that r. The identity `a == div(a,b)*b + rem(a,b)` always
holds.

**Ties** (possible only when b is even, with |r| = |b|/2 exactly) round
**away from zero**. This tie-break is chosen because it preserves the
symmetry identities the whole system is built on:

> div(−a, b) = div(a, −b) = −div(a, b)
> rem(−a, b) = −rem(a, b)

Truncating division ("round toward zero", the C/Rust default) and flooring
division do **not** exist as AM primitives. Implementations lowering to
binary hosts must synthesize round-to-nearest from the host's truncating
divide; the reference VM documents the canonical fixup sequence.

`div(a, 0)` and `rem(a, 0)` are trapping faults. There is no
representable-overflow case for division: since MIN = −MAX, `div(MIN, −1)`
is simply MAX and is well-defined — another entire class of binary-world
faults that does not exist here.

### 3.3 Radix shifts

Shifts are shifts by **powers of three**.

- `shl(a, k)` = a · 3ᵏ, with the flavor system of §3.1 applying on overflow
  (trits shifted out past the most significant position are lost under
  wrapping).
- `shr(a, k)` = div(a, 3ᵏ) — that is, **arithmetic right shift rounds to
  nearest**, consistent with §3.2. Discarding the low k trits of a balanced
  ternary number *is* round-to-nearest division by 3ᵏ, and because 3ᵏ is odd,
  ties are impossible: `shr` never invokes the tie-break rule. This
  truncate-is-round property is exact and carries no fixup cost.

Shift amounts k are values in 0 … n−1 for an n-trit operand; k outside that
range is a trapping fault (not masked, not undefined).

There is no "logical shift" — the concept presupposes a sign bit. Rotates are
reserved and not part of draft 0.1.

### 3.4 Trit-wise operations

The AM defines four primitive trit-wise operations, applied independently to
each trit position of same-width operands:

| Op | Definition per trit | Binary analogue |
|---|---|---|
| `tneg(a)` | −a | NOT |
| `tmin(a, b)` | min(a, b) | AND |
| `tmax(a, b)` | max(a, b) | OR |
| `tmul(a, b)` | a · b | XOR-like (nonzero iff both nonzero; sign composes) |

This set is closed, each operation is its own kind of monotone or involutive,
and each has O(1)-depth hardware realizations. Other ternary logic connectives
(consensus, implication, the full 3⁹ space of dyadic functions) are definable
from these plus constants and are library territory, not AM primitives.

`tneg` on a full word coincides with arithmetic `neg` (§1.2, consequence 3);
they are the same operation and the AM names it once at each layer only for
clarity of intent.

### 3.5 Comparison

`cmp(a, b)` yields a single **trit**: −1 if a < b, 0 if a = b, +1 if a > b.
Three-way comparison is the AM's *primitive* comparison; two-valued
predicates (=, <, ≤, …) are derived by testing the resulting trit. This
inverts the binary-world situation where three-way compare is synthesized
from flag bits, and it is the reason control flow in the TIR and ISA is
natively three-way (`br3`).

---

## 4. Faults

A **fault** is a defined, observable machine halt with a fault code. Draft
0.1 fault codes:

| Code | Cause |
|---|---|
| `F_OVERFLOW` | trapping-flavor arithmetic overflow (§3.1) |
| `F_DIVZERO` | division or remainder by zero (§3.2) |
| `F_SHIFT` | shift amount out of range (§3.3) |
| `F_ALIGN` | misaligned access, on targets that check (§2.3) |
| `F_TRAP` | explicit trap instruction |

Faults are not exceptions; the AM has no unwinding. Whether a language-level
mechanism maps onto faults is a language-spec concern.

---

## 5. What this document deliberately does not define

- **Text encoding.** Interchange encoding of text is specified in the
  language spec's library chapter (a tryte-based UTF-8 carrier format as the
  interop default; a native ternary text encoding is a reserved appendix).
- **Floating point.** No ternary floating-point format is defined in 0.1.
  Fixed-point conventions are a language/library concern built on the integer
  semantics above.
- **Concurrency.** Reserved (§2.4).
- **I/O.** The AM is pure; I/O is defined per target (the reference VM
  defines two memory-mapped character ports; see ISA spec).

---

## Appendix A (informative) — worked identities

A few identities that follow from the definitions above and that optimizer
rule tables may rely on:

1. `neg(neg(x)) = x`; `tneg` is an involution trit-wise and word-wise.
2. `abs(x) = tmul(x, sign(x))` where `sign(x) = cmp(x, 0)` — absolute value
   with no branch and no overflow case.
3. `shr(shl(x, k), k) = x` whenever the `shl` did not overflow.
4. `wrap(x + y) = neg(wrap(neg(x) + neg(y)))` — wrapping arithmetic commutes
   with negation, unlike two's complement where MIN breaks the symmetry.
5. `rem(x, 3) = t₀(x)` — the least significant trit *is* the remainder mod 3
   under round-to-nearest, taking values in {−1, 0, +1}.
