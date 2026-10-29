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

The shift amount `%k` is an operand of the **same `tN`** as `%a` (G2.3),
so the instruction stays uniformly typed; a negative amount is
representable and faults like any other out-of-range amount.

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
only language-legal accesses cannot produce it). For an access wider than
the word — one legalization (§6) has not yet split — natural alignment is
the smallest power of three at least the access's size in trytes, which
reproduces AM §2.3's table at and below the word (the table is stated per
*width*: a `t18` occupies two trytes and still aligns to three) and extends
past it without a special case. AM §2.3's cap is the machine-layer
statement of the same rule. (G4.1) `offset` performs no bounds reasoning;
provenance rules are §5.

**`load` and `store` also accept `ptr` as the access type** (G6.7). That
is not the integer↔pointer conversion §5 declines to define: nothing about
the address becomes observable, and provenance (§5) travels with the
value. An implementation may keep a stored pointer's provenance beside the
trytes rather than in them, so that loading it back yields the same
allocation; writing integers over the storage destroys the pointer. On the
machine a pointer is a word and moves as one — which hands a frontend one
consequence worth stating here: an aggregate containing a pointer is
copied **field by field**, never tryte by tryte, because a tryte copy
carries no provenance. An enum's payload, whose fields vary by variant,
is the known limitation: its storage is copied as trytes, so a reference
inside an enum payload loses *checked* provenance — a limitation of the
checking, not of the machine.

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
4. reading uninitialized `slot` storage — yields *poison*. Poison
   propagates through every value-producing instruction; branching on
   poison is UB; loading or storing *through* a poison address is UB;
   `select3` on a poison selector yields poison rather than UB, since it
   is not a branch (the standard modern model, stated once rather than
   adopted by reference — G4.3);
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

Four decisions the contract leaves open, stated once each:

- **`t1` is a condition type, not an arithmetic width** (G6.1). The
  invariant the output column establishes is that every *arithmetic*
  operand and result has a width in the legal set, while `t1` survives as
  a comparison result, a `.flag` overflow trit, or a branch/select
  selector. Crossing between the two is explicit: `widen` on the way up,
  `cmp x, 0` on the way down — by *sign*, not by residue, since a `trunc`
  would be wrong for any value outside −1…1.
- **Renormalization cannot use `trunc`** (G6.2). After a promoted
  wrapping operation the result is wrapped into the *narrow* width's
  range at the legal width, with `tmul` against a mask of *w* ones:
  `tmul(x, 1) = x` and `tmul(x, 0) = 0`, so the mask keeps the low *w*
  trits and clears the rest — exactly the symmetric residue mod 3ʷ
  (AM §3.1), trit-wise, carry-free, expressible without leaving the legal
  set a `trunc` would reintroduce.
- **A memory access keeps its own width** (G6.3): `load`/`store` move the
  trytes they move whether or not the arithmetic unit has that width's
  adder. Only an access wider than the widest legal width is an error,
  and that is expansion's case, not promotion's.
- **Overflow detection has a width requirement** (G6.4): promoting
  `mul.trap` from *w* to *L* can only detect overflow if the exact
  product fits — *L* ≥ 2*w*. `.wrap` has no such requirement, because the
  low *w* trits of the exact product survive any wider wrapping. A case
  the pass cannot synthesize is reported rather than emitted as an
  operation that silently fails to trap.

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

Constraints: `addr_unit` is at least 1, and need not be 9; `legal` is
non-empty, contains `word` and at least one width ≥ `ptr_width`, and
names no width twice; `word` need **not** be a power of three (a 12-trit
target is expressible — the SBTCVM Gen 3 lesson, learned before writing
line one of a backend). Every field is target-supplied data, never an
assumption baked into a pass.

---

## 8. Textual format and versioning

The textual form is the canonical serialization; there is no binary
encoding in draft 0.1 (premature; the textual form is the test-suite and
differential-testing medium). The version stamp is a compatibility *check*,
not a promise: mismatched versions are rejected, full stop, per the
stability notice.

The format is specified twice, because equality on it is load-bearing — the
test suite, the differential invariant and the double-compiling comparison
(`docs/ddc.md`) all reduce to byte equality of this text. §8.1 and §8.2
give what a reader **accepts**; §8.3 gives what a writer **emits**, which
is stricter. A §8.4 grammar covers the target-description file of §7,
which is a second format in the same family.

### 8.1 Lexis

- **Whitespace** (space, tab, carriage return, newline) is insignificant;
  newlines are counted only for diagnostics.
- **Comments** begin with `;` and run to end of line. (G5.6)
- **Names** are `@name` (a module-scope symbol), `%name` (an SSA value)
  and `^name` (a block label). A name is one or more characters drawn from
  ASCII letters, digits, `_` and `.`, and nothing else — `#` in particular
  is not a name character. The alphabet names a smaller set than the
  source language's identifiers, deliberately: the dot is how a compiler
  invents names (`%tmp.slot.4`) that no source program can spell, so every
  name a printer may invent survives a round trip through this grammar.
  (Why the table is stated at all, and why it grows by the dot and nothing
  else, is `docs/spec-gaps.md` G9.183.)
- **Words** begin with a letter or `_` and continue with name characters.
  They are the keywords of §8.2, the instruction mnemonics — a flavor is
  part of the mnemonic, so `add.flag` is one word — the types `tN` and
  `ptr`, and the fault codes.
- **Numbers** begin with `-` or a digit and continue with name characters;
  the literal notation is the language's (Ch. 1 §3.1) — decimal and the
  `0t`/`0h` forms, sign included. A bare `-` is ill-formed.
- **Strings** are `"…"`, with no escapes and no newline; they appear
  exactly twice, as the module's target name and as §8.4's `call_conv`.
- **Punctuation** is `( ) [ ] { } , : =` and the two-character arrow `->`.

### 8.2 Grammar

```
module       = "tir" version "target" string { item }
version      = number                        ; draft 0.1 accepts exactly `0.1`
item         = global | declaration | definition

global       = "global" @name ":" "tryte" "[" uint "]" [ "=" initializer ]
initializer  = "zeroinit" | "[" [ init-item { "," init-item } ] "]"
init-item    = literal | "addr" @name

declaration  = signature
definition   = signature "{" block { block } "}"
signature    = "fn" @name params [ "->" type ]
params       = "(" [ %name ":" type { "," %name ":" type } ] ")"

block        = ^name [ params ] ":" { inst } terminator
inst         = [ results ] operation
results      = %name { "," %name } "="

operation    = binary | unary | "cmp" [type] operand "," operand
             | "select3" operand "," [type] operand "," operand "," operand
             | slot | load | store | offset | convert | call
binary       = flavored-op [type] operand "," operand
             | plain-op    [type] operand "," operand
flavored-op  = ("add" | "sub" | "mul" | "shl") "." ("wrap" | "trap" | "flag")
plain-op     = "mulh" | "div" | "rem" | "shr" | "tmin" | "tmax" | "tmul"
unary        = ("neg" | "tneg") [type] operand
slot         = "slot" "tryte" "[" uint "]"
load         = "load" type operand
store        = "store" type operand "," operand
offset       = "offset" operand "," operand
convert      = ("widen" | "trunc") type operand "->" type
call         = "call" (@name | %name) "(" [ operand { "," operand } ] ")"
               [ "->" type ]

terminator   = "br3" operand "," target "," target "," target   ; arms −1, 0, +1
             | "br2" operand "," target "," target              ; arm +1, else
             | "br" target
             | "ret" [ operand ]
             | "trap" word
             | "unreachable"
target       = ^name [ "(" [ operand { "," operand } ] ")" ]

operand      = %name | @name | "const" type literal
type         = "t" uint | "ptr"
uint         = a non-negative decimal integer
```

What the productions do not say, stated once each:

- **A type may be omitted wherever `[type]` appears** — in the two binary
  forms, in `unary`, in `cmp` and in `select3`, and nowhere else (`load`,
  `store` and the conversions always state theirs). The type is then taken
  from the first *constant* operand; if no constant operand carries one,
  the instruction is rejected. (G5.5)
- **`br2` is sugar.** `br2 %t, ^then, ^else` is a `br3` whose −1 and 0
  arms are `^else` and whose +1 arm is `^then` — the names reflect
  `bool`'s encoding (false = 0, true = 1). Parsing produces the `br3`;
  §8.3's printer writes `br2` whenever the −1 and 0 targets coincide.
  (G5.4, §3.6)
- **`tneg` is an alias of `neg`** (§3.2); one canonical form survives
  parsing, and §8.3's printer never writes `tneg`.
- **An operand of `@name`** denotes the base address of that global as a
  `ptr` value — G5.1's minimum addition, an operand form rather than an
  integer↔pointer conversion, which §5 continues not to have.
- **A signature with no `{ … }` body is a declaration** (§1): signature
  only, body external. (G5.2)
- **An omitted initializer reads as `= zeroinit`.** A global is therefore
  zero-filled unless an item list says otherwise; §1.2's table has each
  item's fill width (`addr @name` fills one word, a relocation) and the
  rule that the items' total is exactly the declared `tryte[N]`, filled
  upward from the global's base address. (G5.3; G4.2's first clause. The
  alias is why the "omitted" spelling of G4.2's second clause does not
  exist as a distinct state.)
- **A `const` of integer type** carries a value that fits the width; one
  that does not is rejected where the module is read, which is the only
  error §8.2's grammar checks beyond its own shape.
- **Result counts are the operation's business** (§3): the arithmetic,
  comparison, conversion, memory and call forms yield one result, a
  `.flag` form exactly two, `store` none, the terminators none. What is
  *well-formed* beyond the grammar — SSA definition and use, dominance,
  the entry block's ban on incoming branches, operand types and widths —
  is verified per §1.1 and §3 and is not §8's concern.

### 8.3 The canonical serialization

Two well-formed modules are compared by this text. A printer agrees with
the reference printer byte for byte exactly when it follows every rule
below:

1. **Header**: `tir 0.1 target "<name>"` on the first line.
2. **Item order**: all globals, then all declarations, then all
   definitions — each group in its input order — with exactly one blank
   line before every item, the first one included.
3. **Layout**: an instruction or terminator per line, indented four
   spaces; a block header at column 0 as `^label:` or
   `^label(%p: tN, …):`; the signature and `{` on one line; the closing
   `}` at column 0; the module ends with a newline after the last item.
4. **Explicit types**: every position the grammar marks optional is
   printed explicitly — `%r = select3 %t, t27 …` with the type, never
   without.
5. **Decimal literals**, always (never `0t` or `0h` spellings of the same
   value).
6. **The sugar shows**: a `br3` with coinciding −1 and 0 targets prints
   as `br2`; `tneg` never appears; a global with no item list prints
   `global @name : tryte[N] = zeroinit` on one line, initializer
   included.
7. **Spacing** exactly as §8.2's productions show: `global @g : tryte[9]`,
   `fn @f(%a: t27) -> t1`, `call @f(%a) -> t27`, `offset %p, %d` — one
   space after each comma, none before a parameter list.

Rules 2, 4, 5 and 6 are why the canonical form exists at all: a name from
a counter that depends on visit order, a type left to inference, a literal
in two notations or a branch in two spellings is a difference that means
nothing and still fails the comparison — exactly what `docs/ddc.md` §5's
canonical-output warning is about.

### 8.4 Target-description files

§7's record has its own small format, line-oriented with `;` comments:

```
target-file = "target" string "{" { "}" | field }
field       = word "=" value
value       = uint | "[" [ uint { "," uint } ] "]" | string
```

The five fields — `addr_unit`, `ptr_width`, `legal`, `word`, `call_conv` —
are each required and may appear in any order; quoting is as §7's example
shows (`call_conv` quoted, nothing else); unknown fields are ignored, so a
description written against a later draft still parses under an earlier
reader. §7's constraints check the values: `addr_unit` at least 1, `legal`
non-empty, containing `word` and at least one width ≥ `ptr_width`, and
naming no width twice.

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
