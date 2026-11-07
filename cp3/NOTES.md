# cp3 — phase P1: balanced-ternary arithmetic core

Implemented from the specification alone (only files under `spec/` were read;
see the read audit in the final report).

## Deliverable

- `core.hpp` — `Bt` (the single trit, AM §1.1) and `Tint<N>` (the width-typed
  balanced-ternary signed integer, `N >= 1`, so arbitrary `tN` per TIR §2), with
  the spec's concrete widths aliased: `t9` (tryte), `t27` (word), `taddr`
  (word-width on the AM), and `t1` available as the arithmetic trit width.
- All AM arithmetic and its flavors (AM §3.1–3.5, §4):
  - `add / sub / mul`, each as `_wrap`, `_trap`, `_flag` (wrapped value plus
    overflow *trit*: +1 above MAX, −1 below MIN, 0 otherwise) and the language
    families `_checked` (`std::optional`) and `_sat` (clamp) — AM §3.1 says these
    last two are library-level compositions, see reader Q4.
  - `neg`, `abs` — total, no flavors (MIN = −MAX).
  - `div`, `rem` — one round-to-nearest division, ties away from zero;
    `div/rem(a, 0)` raise `F_DIVZERO`; `div(MIN, −1) = MAX` well-defined.
  - `shl_{wrap,trap,flag,checked,sat}`, `shr` — shifts by powers of three;
    `k` outside `0 … N−1` raises `F_SHIFT` (negative amounts fault too,
    TIR §3.1 G2.3).
  - `tneg` (same operation as `neg`), `tmin`, `tmax`, `tmul`, `splat`, `cmp`
    returning a `Bt`; two-way predicates are projections of `cmp`.
  - `mulh` — the high N trits of the exact 2N product (Lang Ch.1 §4, TIR §3.1,
    ISA §4.1), total: `mulh·3^N + mul_wrap` reconstructs the product exactly.
  - `widen` (value-preserving), `narrow` (wrapping), `checked_narrow`.
- Fault vocabulary of AM §4 (`F_OVERFLOW`, `F_DIVZERO`, `F_SHIFT`, `F_ALIGN`,
  `F_TRAP`; `F_ILLEGAL`/`F_ADDRESS` reserved and never raised).  A machine fault
  is realized as a `trt::MachineFault` exception carrying the code: this is the
  observable "defined halt with a fault code" the AM pins down, and what the
  tests assert.
- `literals.hpp` — parsing for every literal form of Lang Ch.1 §3/§3.1:
  decimal (signs, `_` separators), `0t` balanced ternary (`1`/`0`/`T`, lowercase
  `t` accepted and canonicalized on output), `0h` heptavintimal (`0-9` `A-Q`,
  balanced −13…+13 digits, `D` = zero digit), the three `trit` literals
  (`-1t`, `0t`, `1t`) with the maximal-munch rule (`2t` rejected), and
  `0x`/`0o` rejection with a diagnostic naming hexadecimal/octal.
  Also canonical trit-string and minimal-digit heptavintimal renderers, with
  the §3.1 leading-zero warning explicitly covered by tests.

## Layout / design decisions (not dictated by the spec)

- **Representation**: values are their unique balanced-ternary expansion
  (`std::array<int8_t, N>`, LST first), and exact intermediate results are
  unbounded trit vectors.  The AM is explicit that it "is not a description of
  any implementation": equivalence of observable behavior is the conformance
  criterion, so no claim is made here about *storage layout*, memory or
  alignment.  Nothing in this core accesses memory, therefore nothing raises
  `F_ALIGN`; it is in the vocabulary because AM §4 enumerates it.
- **Arithmetic engine**: exact trit-array add/sub/mul, then the flavor decides
  overflow (`wrap` = keep low N trits, i.e. the symmetric residue mod 3^N;
  `trap` = fault; `flag` = wrapped value + direction trit).  Division is
  computed from exact magnitudes with base-3 schoolbook division, promoted to
  round-to-nearest by the 2r ≥ b rule, with signs applied so that
  `div(−a,b) = div(a,−b) = −div(a,b)` holds by construction.
- **Trapping in a host library** is by C++ exception (see above) — there is no
  unwinding on the AM itself, and the exception is just how a test observes
  "the machine halted with code X".
- `__int128` (a GNU extension, no dependency) is used **only** in the host
  side oracles under `testharness.hpp` and in the `from_i64`/`to_i64`
  convenience bridges of `core.hpp`.  AM semantics are always computed on
  trits, and arbitrary N (e.g. t81) is exercised.

## C++ standard: C++20

GCC 15.2.0 fully supports C++20, and it is the oldest standard that buys every
convenience used here without a workaround:
- `std::span` — the engine operates on the unbounded temporary vectors *and*
  on `Tint<N>`'s inline array without copying;
- `requires (N >= 1)` — width constraints directly on `Tint<N>`;
- defaulted `operator==` — bit-free value equality on the trit array;
- `std::optional` — `checked_*`/`checked_narrow`'s empty-on-overflow result.
No external dependencies; headers + tests only, and `-std=c++20 -O2 -Wall
-Wextra -Werror` builds cleanly.  (`-Wpedantic` is omitted only because
`__int128` is a GNU extension; see above.)

## Build & run

```
/home/nixos/code/Young-TW/trust-lang/cp3/run.sh
```

`run.sh` invokes `~/.nix-profile/bin/g++` directly (no make/cmake/ninja) on
the test sources and runs the resulting `cp3/tests` binary.  Exit status 0
means every case passed; summary prints `cases / assertions / failures`.

## Reader questions (for adjudication by the spec's author)

**Q1 — Lowercase heptavintimal digits.**
Lang Ch.1 §3.1 defines `0h` digits as "`0-9` `A-Q`" and says nothing about
lowercase `a-q`, while the very same section explicitly grants lowercase
acceptance for the balanced-ternary `T` (G1.3: "`t` is accepted on input as a
synonym for `T`").  *Implemented*: lowercase `a-q` after `0h` is rejected
(`0hj`, `0hJq` are parse errors).  Rationale: the spec grants the lowercase
leniency where it intends it and stays silent elsewhere.

**Q2 — Leading `-` on `0h` (and generally on all three radices).**
Lang Ch.1 §3.1 states "A leading `-` on a `0t` literal is permitted but
redundant (`-0t1T` = `0tT1`)" and says nothing about `0h`; the decimal form's
table example (`-9841`) plainly takes a sign.  Meanwhile TIR §8.1 has "Numbers
begin with `-` or a digit … the literal notation is the language's (Ch.1 §3.1)
— decimal and the `0t`/`0h` forms, **sign included**."
*Implemented*: the parser accepts one optional leading `-` on all three
radices, so `-0hJ` = −6 and `-0h0J` = +345 (and `-0` = 0: no negative zero per
AM §1.2).  Rationale: the TIR wording reads like the general rule; this is also
the only self-consistent choice when literals are consumed by the assembler
and the IR as well as the language.  If the language intends `0h` to be
sign-free lexically (leaving `-0hJ` to the unary operator), the value produced
here is identical anyway.

**Q3 — Where `_` may appear in a digit run.**
Lang Ch.1 §3.1 says "Underscore separators are permitted in all radices
(`0t1T0_T01`, `3_812_798`)" and Assembly §2.2 says "`_` may separate digits in
any radix", yet the maximal-munch rule (G1.2) lists `_` itself as a
balanced-ternary character: "Immediately followed by a balanced-ternary
character (`0`, `1`, `T`/`t`, `_`) `0t` opens a balanced literal".  The two
sentences leave `0t_1`, `0t_`, `1_` not quite pinned down.
*Implemented*: after the prefix, `_` is ignored anywhere in the run and at
least one real digit is required, so `0t_1` = 1 and `0t_` is ill-formed
("separators but no digits").  Rationale: the munch rule would otherwise
maximally swallow `_` into the balanced literal and need *some* fate for it;
ignoring it (Rust's liberal separator treatment) is the only reading that
keeps both sentences true.

**Q4 — `saturating_*` and `checked_*` values are never pinned down.**
AM §3.1: "**checked / saturating** — defined in the language spec as
library-level compositions of the above plus comparison".  Lang Ch.1 §4/P4
says the Rust-named families exist ("checked_*, wrapping_*, saturating_*"
and the overflowing form) but no `spec/` document states the saturation value
or the checked failure shape; the library chapter does not define them (it
never mentions saturating/checked arithmetic).
*Implemented*: `checked_*` returns an empty `std::optional` on overflow, and
`saturating_*` clamps to `MAX` when the overflow trit is +1 and to `MIN` when
it is −1, computed as the AM prescribes — from the flag-producing primitive
plus comparison.  Rationale: this is exactly the "Rust discipline" Ch.1 claims
to inherit and the saturation direction is fully determined by the overflow
trit's sign.

## Scope notes (deliberately outside P1)

Memory, trytean order, alignment and the `F_ALIGN`/`F_ILLEGAL`/`F_ADDRESS`
conditions belong to AM §2 and to the ISA/VM layers, not to this arithmetic
core; their codes are declared (so fault handling is uniform) and the reserved
ones are never raised.  `taddr` is instantiated at the AM pointer width (27).
