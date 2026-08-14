# TIR Specification — Ternary Intermediate Representation

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*) |
| **Stability** | **TIR is not a stable interface.** It is the internal contract between this repository's frontend and backends and may change without deprecation in any release. The only stable artifacts of the project are the language spec and (once frozen) the ISA spec. |

TIR is the typed, SSA-based intermediate representation sitting between the
Trust frontend and all code generators. Its semantics are defined entirely
in terms of the AM: a TIR program denotes a sequence of AM operations, and
any transformation (optimization, legalization, codegen) is correct iff it
preserves observable AM behavior for all defined executions.

Design position, stated once: TIR is **not** LLVM IR with the numbers
changed. It is radix-native (arbitrary trit widths, three-way branch as a
first-class terminator, round-to-nearest division as the only division) and
deliberately small. Where a design choice had a well-understood modern
answer independent of radix (SSA, opaque pointers, explicit UB markers),
TIR copies the state of the art rather than re-deriving it.

---

## 1. Module structure

A TIR module contains, in any order:

- **global definitions** — `global @name : tryte[N] = <initializer>` —
  named, tryte-addressed storage with optional constant initializer (§1.2);
- **function declarations** — signature only, body external;
- **function definitions** — signature plus a body of basic blocks.

```
fn @clamp_sign(%x: t27) -> t1 {
^entry:
    %s = cmp %x, const t27 0
    ret %s
}
```

Identifiers: `@name` for module-scope symbols, `%name` for SSA values,
`^name` for basic-block labels. Constants are written inline as
`const <type> <literal>`; literals accept decimal and `0t` forms.

### 1.1 Functions and blocks

A function body is a list of **basic blocks**. The first block is the entry
block; it may not be a branch target. Each block is a (possibly empty)
sequence of instructions followed by exactly one **terminator**. Standard
SSA rules apply: every `%value` is defined exactly once and every use is
dominated by its definition; cross-block value merging uses **block
parameters** rather than phi instructions:

```
^loop(%i: t27, %acc: t27):
    ...
    br3 %t, ^neg(%a), ^zero(%b), ^pos(%c)
```

(Block parameters are the phi-equivalent used by MLIR/Cranelift; they are
chosen over classic phis because they make `br3` clean — a three-way branch
carrying three argument lists is readable, whereas phis over three-way
predecessors are not.)

---

### 1.2 Global initializers

A global's initializer is a bracketed list of **items**, each of which fills
a known number of trytes, and the total must be exactly the `tryte[N]` the
global declares:

| Item | Fills | Meaning |
|---|---|---|
| a literal | 1 tryte | that value |
| `addr @name` | one word | the address of that module-scope symbol |
| `zeroinit` (whole initializer) | N trytes | all zero |

```
global @message : tryte[3] = [72, 105, 10]
global @vtable  : tryte[9] = [addr @Circle.area, addr @Circle.name, 0, 0, 0]
```

`addr @name` names a function or another global defined or declared in this
module. Its value is not known until the module is placed in memory, so it
is a **relocation**: TIR states which symbol, and the target decides the
number.

An address obtained this way carries the provenance (§5) of the thing it
names. `addr` of a function yields a pointer that may only be called (§3.7);
loading or storing through one is UB.

> **Note (informative).** This is the smallest extension that lets TIR
> express a virtual table, and a virtual table is the smallest thing a
> language needs in order to have dynamic dispatch at all. Without it a
> frontend must build its tables behind TIR's back, which puts them beyond
> the reach of every pass and of the reference interpreter.

---

## 2. Type system

TIR types, in full:

| Type | Meaning |
|---|---|
| `tN` (N ≥ 1) | balanced ternary integer of exactly N trits, symmetric range per AM §1.2 |
| `ptr` | an address; **opaque** — no pointee type |
| `tryte[N]` | N trytes of raw storage (globals and stack slots only; not an SSA value type) |

That is the entire list. Notes:

**`tN` is arbitrary-width.** The frontend emits mostly t1/t9/t27 (language
types), but legalization (§6) freely introduces other widths and constant
folding must be correct at every N. Widths above a module-declared maximum
(default 243) are ill-formed, purely to bound compile-time arithmetic.

**Pointers are opaque.** A `ptr` carries no pointee type; loads and stores
state their accessed type explicitly. This copies LLVM's endpoint (opaque
pointers) rather than its 15-year detour (typed pointers plus a bitcast
instruction that existed only to appease the type system). There is
consequently **no bitcast in TIR at all** — and unlike the binary world,
there is no legitimate demand for one: `tN` has no sign-reinterpretation
use case, and float↔int punning has no floats to pun.

**No aggregates in SSA values.** Structs and enums are lowered by the
frontend to scalar values (SROA-style) or to stack storage + explicit
offsets. TIR never sees a struct type. This keeps every instruction's
operand model trivial and pushes layout decisions to exactly one place
(the frontend's layout engine, implementing Language Ch. 2).

---

## 3. Instruction set

Operands are SSA values or inline constants; every instruction states its
result type. `<fl>` marks the overflow-flavor suffix: `.wrap` or `.trap`
(AM §3.1); a third form `.flag` yields two results (wrapped value, overflow
trit) for lowering `checked_*` / `overflowing_*`.

### 3.1 Arithmetic

```
%r = add<fl> tN %a, %b          ; also sub, mul
%r = mulh tN %a, %b             ; the *high* tN of the exact 2N-trit product
%r = neg tN %a                  ; total — no flavor
%r = div tN %a, %b              ; round to nearest, ties away from zero (AM §3.2)
%r = rem tN %a, %b              ; |r| <= |b|/2; traps F_DIVZERO on b = 0
%r = shl<fl> tN %a, %k          ; a * 3^k
%r = shr tN %a, %k              ; a / 3^k, exact round-to-nearest, total in value
                                ; k out of 0..N-1 traps F_SHIFT (both shifts)
```

There is exactly one division. Frontends for languages wanting truncating
division must synthesize it; Trust does not want it.

**`mulh` is the widening multiply, in the only shape that does not need a
wider type.** Let *P* be the exact product of `%a` and `%b`, which needs 2N
trits. Then

> `mul.wrap tN %a, %b` is `wrap(P, N)` and `mulh tN %a, %b` is `shr(P, N)`,

and the two together reconstruct *P* exactly: `P = mulh · 3ᴺ + mul.wrap`. That
is the balanced split of §3.3's `shr`, so `mulh` needs no definition of its own
beyond "the other half".

It takes no flavor and cannot overflow: the high half of a product of two
N-trit values always fits in N trits, which is a property of the symmetric
range and not an accident (AM §1.2).

> **Why it exists (informative).** §6's expansion cannot multiply without it.
> A part times a part is twice a part wide, and no same-width instruction can
> deliver the top half, so a multi-part multiply has nowhere to put its partial
> products. Every binary machine provides the same primitive for the same
> reason. TRISC-27 §4.1 already had it — the machine was ahead of the IR — and
> `docs/spec-gaps.md` G6.6 is the record of the gap this closes.

### 3.2 Trit-wise

```
%r = tneg tN %a                 ; alias of neg — one canonical form after parsing
%r = tmin tN %a, %b
%r = tmax tN %a, %b
%r = tmul tN %a, %b
```

### 3.3 Comparison and selection

```
%t = cmp tN %a, %b -> t1        ; three-way, the only comparison
%r = select3 %t, tN %vn, %vz, %vp   ; %t: t1 — picks by trit value
```

Two-valued predicates and two-way select are *patterns over* `cmp`/`select3`
(e.g. "a < b" is `cmp` + test for −1; two-way select is `select3` with two
equal arms). The canonicalizer recognizes and re-fuses these patterns;
backends for hardware with two-way flags may re-expand them. Keeping one
comparison and one select makes the instcombine rule table one-third the
size it would otherwise be.

### 3.4 Memory

```
%p = slot tryte[N]              ; stack allocation, function lifetime, yields ptr
%v = load tN %p
     store tN %v, %p
%q = offset %p, %d              ; ptr + d trytes (d: taddr-width tN); the entire
                                ; address-arithmetic instruction — no GEP
```

Alignment: `load`/`store` of `tN` require the AM natural alignment of the
access width (AM §2.3); misalignment is UB at TIR level (frontends emitting
only language-legal accesses cannot produce it). `offset` performs no
bounds reasoning; provenance rules are §5.

### 3.5 Conversion

```
%r = widen tM %a -> tN          ; M < N; value-preserving (sign-extension is
                                ;   meaningless here — there is one extension)
%r = trunc tN %a -> tM          ; M < N; wraps into tM's symmetric range
```

Exactly two conversions. The binary trio sext/zext/trunc collapses to two
because zero-extension has no meaning without unsigned types — a whole
instcombine bug farm (mixing up sext and zext) is gone by construction.

### 3.6 Control flow (terminators)

```
br3 %t, ^neg(...), ^zero(...), ^pos(...)   ; %t: t1 — the primitive branch
br ^dest(...)                              ; unconditional
ret %v                                     ; or `ret` for () functions
trap F_CODE                                ; deliberate fault (AM §4)
unreachable                                ; UB marker: control provably cannot arrive
```

`br3` is the *only* conditional terminator. A two-way branch is a `br3`
with two identical destinations; the printer displays that case as
`br2 %t, ^then, ^else` sugar, and backends pattern-match it back to
two-way hardware branches where those are cheaper. Multi-way dispatch
(`match` over enums, jump tables) is lowered by the frontend to `br3`
trees over the discriminant — a balanced ternary comparison tree, which
over a symmetric discriminant range is the radix-optimal search tree
(informative: this is where `match` performance on 3ᵏ-sized enums gets
its edge).

### 3.7 Calls

```
%r = call @f(%a, %b) -> tN          ; direct: the callee is a symbol
%r = call %p(%a, %b) -> tN          ; indirect: the callee is a ptr
```

An indirect call transfers to the function whose address `%p` holds. The
address of a function is obtained only from `addr @f` in a global
initializer (§1.2); TIR has no instruction that takes one, because a
language that needs one in an expression needs a function-pointer type
first, and that is the language's business rather than TIR's.

`%p` must be a `ptr` that is the address of a function definition or
declaration in this module. Calling through any other pointer is UB — the
fifth entry in §4's inventory, and the only one that is not about data.

The signature is not recoverable from the pointer, so the call site's
argument types and result type *are* the signature the callee is called
with. Calling a function through a pointer with a signature other than its
own is UB, not a fault: no target can check it, and every one of them would
have to for it to be a fault.

Calling convention is a target-description property (§7); TIR itself is
convention-agnostic.

---

## 4. Undefined behavior inventory

TIR has exactly five UB sources, listed exhaustively so every pass author
can enumerate them:

1. executing `unreachable`;
2. misaligned `load`/`store` (§3.4);
3. `load`/`store`/`offset` escaping an allocation's provenance (§5);
4. reading uninitialized `slot` storage (yields *poison*, and branching on
   poison is UB — the poison model is the standard modern one, adopted
   whole rather than re-invented);
5. an indirect call through a pointer that is not the address of a
   function, or through one whose function has a different signature from
   the call site's (§3.7).

Everything else that can go wrong is a defined **fault** (AM §4): division
by zero, trapping overflow, out-of-range shifts. Optimizations may not
remove, reorder past observable events, or duplicate faults except as
permitted by the as-if rule over AM semantics.

---

## 5. Provenance

Pointers carry provenance: a `ptr` derived (via `offset`) from allocation A
may access only A's trytes plus its one-past-the-end address (offset-only;
not accessible). Access outside that range, or through a pointer whose
provenance was laundered through integers, is UB (source 3 above).
`ptr`↔`tN` conversion instructions deliberately do not exist in draft 0.1 —
until the language's `unsafe` chapter needs them, TIR simply cannot express
integer↔pointer casts, which keeps the provenance model trivially sound.

---

## 6. Legalization

The pass pipeline contains a mandatory **legalization** stage between
target-independent optimization and instruction selection. Its contract:

- Input: any well-formed TIR (arbitrary `tN`).
- Output: TIR whose every value type and every operation width appears in
  the target description's **legal set**.
- Method (normative as to result, informative as to technique): widths
  below the smallest legal width are *promoted* (`widen`, operate,
  `trunc`); widths above the largest legal width are *expanded* into
  multi-part arithmetic over legal widths with explicit carry chaining
  (the flavor system's `.flag` form exists chiefly for this — carry out of
  a part is the overflow trit of that part's `add.flag`).

Backends may assume legalized input and are not required to handle any
other. This is the mechanism that lets one frontend serve a t27 reference
machine and a t9 SBTCVM-class target from identical mid-level IR — the
requirement that forced legalization into the core design rather than
being bolted on later.

---

## 7. Target descriptions

A target description is a declarative record consumed by legalization and
codegen. Draft 0.1 fields:

```
target "tritium" {
    addr_unit   = 9          ; trits per addressable unit (tryte)
    ptr_width   = 27         ; trits in an address
    legal       = [1, 9, 27] ; widths with native operation support
    word        = 27         ; preferred/register width
    call_conv   = "tritium0"   ; symbolic; defined in the target's own doc
}
```

Constraints: `legal` must contain `word` and at least one width ≥
`ptr_width`; `addr_unit` need not be 9 and `word` need **not** be a power
of three (a 12-trit target is expressible — the SBTCVM Gen 3 lesson,
learned before writing line one of a backend). Every field is
target-supplied data, never an assumption baked into a pass.

---

## 8. Textual format and versioning

The textual form shown throughout is the canonical serialization; a module
begins `tir 0.1 target "name"`. There is no binary encoding in draft 0.1
(premature; the textual form is the test-suite and differential-testing
medium). The version stamp is a compatibility *check*, not a promise:
mismatched versions are rejected, full stop, per the stability notice.

---

## Appendix (informative) — end-to-end example

Language source (syntax provisional):

```
fn steps_toward(target: t27, pos: t27) -> t27 {
    match pos <=> target {
        -1t =>  1,
         0t =>  0,
         1t => -1,
    }
}
```

TIR after frontend:

```
tir 0.1 target "tritium"

fn @steps_toward(%target: t27, %pos: t27) -> t27 {
^entry:
    %t = cmp t27 %pos, %target
    %r = select3 %t, t27 const t27 1, const t27 0, const t27 -1
    ret %r
}
```

The `match` became a `select3` (no branching — all arms are constants);
on the reference ISA this is a two-instruction function. The same source
compiled for a t9-word target legalizes `cmp t27` into three `cmp t9`
parts folded most-significant-first, and `select3` survives unchanged at
t9 width per part — a worked legalization trace belongs to the backend
documentation, not this spec.
