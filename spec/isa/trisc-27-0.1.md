# ISA Specification — TRISC-27

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), `spec/01-naming.md` (*Naming*) |
| **Depended on by** | `spec/isa/assembly-0.1.md` (which §7 answers in full), TIR §7 (which cites this document for `call_conv = "tritium0"`), the `tritium` reference VM |
| **Stability** | Draft. The ISA is one of the two artifacts intended to become stable (TIR stability notice); nothing here is frozen yet. |

TRISC-27 is the 27-trit reference member of the TRISC family (Naming §1): a
concrete realization of the abstract machine, with a register file, an
instruction encoding, and a memory map. It is a load–store RISC with
fixed-width instructions, and it exists to be implemented — by the `tritium`
VM first, and by hardware later if anyone is so inclined.

Nothing in this document redefines AM semantics. Where an instruction says
"add", it means AM §3.1's add; where it says "div", it means AM §3.2's single
round-to-nearest division, ties away from zero. What this document adds is
*where the operands live* and *how the operation is spelled in trits*.

> **Design note (informative).** The design rule throughout was: let the radix
> decide, and take the resulting simplification rather than reaching for the
> binary-world shape out of habit. Three of the four places that rule bit
> hardest are worth flagging up front, because each removes something a binary
> RISC needs:
>
> - The overflow flavor is **one trit** (§4.1). Wrapping, trapping and
>   flag-producing are three states, and three states are what a trit holds.
> - The conditional branch has **three destinations in one instruction**
>   (§4.5), and its three 7-trit displacements fill a 27-trit word exactly.
> - `lui`/`addi` compose to cover a full word with **no correction term**
>   (§4.3). RISC-V's `+0x800` fixup exists because splitting a two's-complement
>   constant borrows; splitting a balanced ternary constant does not.

---

## 1. Machine state

### 1.1 Registers

Twenty-seven general-purpose registers, each one word (27 trits) wide.

A register operand is a **3-trit field**, so a register's name *is* its field
value, written in the notation of AM §1.1:

| Name | Field value | Heptavintimal digit |
|---|---|---|
| `r0` | 0 | `D` |
| `r1` … `r13` | +1 … +13 | `E` … `Q` |
| `rT1` … `rT13` | −1 … −13 | `C` … `0` |

Naming a register by anything other than its field value would introduce a
translation step that exists only to make the machine look binary. A useful
consequence: printed in heptavintimal, every register operand of an
instruction is a single digit, and that digit is the register's name.

**`r0` is the zero register.** It reads as 0 and discards writes. It is the
one register whose field value is 0, which is why an all-zeros instruction
word is a no-op (§3.4) — zeroed memory executes harmlessly.

The ABI names of §6 (`sp`, `a0`, `s3`, …) are aliases for these registers and
are equally reserved in assembly source.

### 1.2 The program counter

`pc` holds the address of the executing instruction. It is not a
general-purpose register, is not addressable as one, and is written only by
the control-transfer instructions of §4.5.

Instructions are one word and word-aligned, so `pc` is always a multiple of 3
trytes.

### 1.3 Reset

At reset:

- `pc` = 0 — execution begins at address 0;
- every general-purpose register is 0;
- memory outside the loaded image is 0.

There is no boot ROM, no reset vector table and no privileged mode. The
machine has one mode, and it is the one programs run in.

---

## 2. Memory

### 2.1 Address space

Memory is a flat array of trytes at addresses 0 … A−1, where **A is
implementation-defined** (AM §2.1). TRISC-27 constrains it only at the ends:

- A ≥ 3¹² trytes (531 441), so that a program may assume *some* memory;
- A ≤ 3²⁷ trytes, the AM's limit;
- A is a multiple of 3, so the last word is whole.

An implementation reports its A through `MEM_SIZE` (§2.2), and a program that
needs to know where memory ends reads it rather than assuming. A conforming
implementation may therefore grow its address space to the AM's limit without
any program, port address or ABI detail moving.

Multi-tryte values are little-trytean and word accesses are 3-tryte aligned,
both per AM §2.2–2.3; §4.4 restates the consequences for each instruction.

An access to a non-negative address at or above A faults (§5). Negative
addresses are not memory at any A, and are the subject of §2.2.

> **Note (informative).** The reference VM defaults to A = 3¹⁵ = 14 348 907
> trytes (4 782 969 words), which is a platform choice, not an architectural
> one, and is documented with the VM rather than here.

### 2.2 The device region: negative addresses

Addresses are word-sized values and words are signed, but memory occupies only
0 … A−1. **The negative addresses are therefore never memory, whatever A
is** — and that is where TRISC-27 puts the two memory-mapped character ports
AM §5 promises, together with the one register a program needs to discover its
own machine:

| Symbol | Address | Width | Access | Meaning |
|---|---|---|---|---|
| `IO_IN` | −1 | tryte | load | reads one UTF-8 code unit |
| `IO_OUT` | −2 | tryte | store | writes one UTF-8 code unit; the value must be 0 … 255 |
| — | −3 | — | — | reserved |
| `MEM_SIZE` | −6 | word | load | the address-space size A |
| `CYCLES` | −9 | word | load | instructions retired since reset (§2.3) |

Every other negative address is reserved and faults. An access of the wrong
width for a device address faults, as does a load from `IO_OUT` or a store to
`IO_IN` or `MEM_SIZE`.

This placement is what makes A implementation-defined workable. Putting the
ports at the *top* of memory would have tied their addresses to A and forced
either a fixed A or a program that computes port addresses at run time; below
zero, they are fixed for every implementation, and the memory that grows
upward can never reach them.

The addresses are small negative numbers on purpose: they fit in a 14-trit
immediate many times over, so every device access is one instruction with
`r0` as the base register — `ld.tryte a0, IO_IN(zero)`. A high fixed address
would have needed `lui` first.

### 2.3 `CYCLES`

`CYCLES` reads the number of instructions the machine has retired since reset,
as a word. The load itself has not retired when its value is produced, so two
consecutive reads differ by exactly the number of instructions between them —
which is what makes a difference of two readings the cost of the code between
them, with nothing to subtract for the measurement.

It is a **count of instructions, not of time.** This machine has no clock and
this document does not give it one: a cycle count would commit an
implementation to a timing model, and the reference implementation is an
interpreter whose timings mean nothing. An instruction count is the thing an
implementation can report honestly and a program can compare against itself.

It wraps at MAX like any other word, and a program that runs longer than
(3²⁷−1)/2 instructions gets a wrapped answer rather than a fault: there is no
sensible value to trap with, and the alternative — a wider counter — would
need a second device address for its top half.

> **Why a device and not an instruction (informative).** A `rdcycle`
> instruction would spend one of the seventeen reserved opcodes on something
> the device region already knows how to express, and would need a rule for
> what it does when an implementation cannot count. As a device address it
> follows §2.2's existing rules: an implementation that cannot count faults
> the load, which is the same answer it gives for every other address it does
> not implement.

`IO_IN` yields:

| Value | Meaning |
|---|---|
| 0 … 255 | the next code unit |
| −1 | no input is available right now |
| −2 | end of input; no further input will ever be available |

Three outcomes, one signed tryte, no status register. In a binary machine
"ready", "empty" and "closed" need either a second port or a sentinel that
collides with a real code unit; here the code units are non-negative and the
other two states are simply negative numbers. Values below −2 are reserved.

Text is carried as UTF-8 code units, one per tryte, per AM §5's interop
default. A native ternary text encoding remains reserved there, and this
document does not anticipate it.

**The first word of memory is reserved.** Address 0 holds one word that no
program may claim: not a function's entry, not a global, not an object. It is
the all-zeros word, which §3.4 makes a `nop`, so execution begins there (§1.3)
and falls through to address 3.

This costs one word and buys three things:

1. **A null pointer is a pointer to a nop, not to code.** Composites Ch. 2 §6
   and References Ch. 3 §2.4 guarantee that a reference is never null; that
   guarantee is still enforced by the type system, but an implementation may
   now also make address 0 unreadable and have the hardware say so.
2. **Zero is available as a sentinel for "no function here."** Generics Ch. 4
   §3.3's virtual tables use a drop slot of 0 to mean "this type has no
   destructor", and that is only unambiguous if no function can begin at 0.
   Without this reservation the sentinel is indistinguishable from a real
   destructor that the layout happened to place first.
3. **The entry point stops being an accident.** §6.4 says the first
   instruction assembled is the first executed; now the first *assembled* word
   is one the assembler emits, and a program's own first instruction is at a
   fixed, non-zero address.

> **Note (informative).** Draft 0.1 first left this open, observing that
> "reserving it later would break no program that does not already rely on
> executing from 0". Reserving it is that later. The motivation that decided
> it was Ch. 4 §3.3's drop sentinel, which is a use of zero that a language
> cannot make safe on its own.

---

## 3. Instruction format

### 3.1 Width and alignment

Every instruction is exactly **one word — 27 trits, 3 trytes — and is stored
at a word-aligned address**, least significant tryte first like every other
multi-tryte value (AM §2.2).

There is no compressed encoding and no variable-length form. This satisfies
the assembly language's requirement that an instruction's size be known in the
first pass (Assembly §3.3, §6.2 item 6) without any rule at all: every
statement is one word.

Trits within an instruction are numbered 0 (least significant) to 26, matching
AM §1.2's numbering of any other value. A field written `[a..b]` occupies
trits *a* through *b* inclusive, and its value is the balanced ternary number
those trits denote.

### 3.2 Formats

Seven formats. Every register field is 3 trits; every ordinary immediate is 14
trits. Each format's fields are listed low trit first, and every one totals 27.

**R** — register-register arithmetic:

| `[0..2]` | `[3..5]` | `[6..8]` | `[9..11]` | `[12..14]` | `[15]` | `[16..18]` | `[19..26]` |
|---|---|---|---|---|---|---|---|
| opcode | rd | rs1 | rs2 | funct | flavor | rc | zero |

**I** — register-immediate arithmetic, loads, `jalr`, `sys`:

| `[0..2]` | `[3..5]` | `[6..8]` | `[9..11]` | `[12]` | `[13..26]` |
|---|---|---|---|---|---|
| opcode | rd | rs1 | funct | flavor | imm |

**S** — stores, which have a second source instead of a destination:

| `[0..2]` | `[3..5]` | `[6..8]` | `[9..11]` | `[12]` | `[13..26]` |
|---|---|---|---|---|---|
| opcode | rs2 | rs1 | funct | zero | imm |

**B** — the three-way branch, whose three displacements fill the word exactly:

| `[0..2]` | `[3..5]` | `[6..12]` | `[13..19]` | `[20..26]` |
|---|---|---|---|---|
| opcode | rs1 | off₋ | off₀ | off₊ |

**J** — jump and link:

| `[0..2]` | `[3..5]` | `[6..26]` |
|---|---|---|
| opcode | rd | off |

**U** — load upper immediate:

| `[0..2]` | `[3..5]` | `[6..18]` | `[19..26]` |
|---|---|---|---|
| opcode | rd | imm | zero |

**R4** — three-way select, the one instruction with four register sources:

| `[0..2]` | `[3..5]` | `[6..8]` | `[9..11]` | `[12..14]` | `[15..17]` | `[18..26]` |
|---|---|---|---|---|---|---|
| opcode | rd | rt | rn | rz | rp | zero |

Immediate ranges follow directly from the field widths:

| Field | Trits | Range |
|---|---|---|
| ordinary immediate (I, S) | 14 | −2 391 484 … +2 391 484 |
| `lui` immediate (U) | 13 | −797 161 … +797 161 |
| branch displacement (B), each of three | 7 | −1093 … +1093 words |
| jump displacement (J) | 21 | ±5 230 176 601 words — the whole address space from anywhere |

### 3.3 Opcodes

The opcode is `[0..2]`, so there are 27 of them. Ten are assigned:

| Digit | Value | Mnemonic group | Format |
|---|---|---|---|
| `D` | 0 | `alu` — register-register arithmetic | R |
| `E` | +1 | `alui` — register-immediate arithmetic | I |
| `F` | +2 | `ld` — load | I |
| `G` | +3 | `st` — store | S |
| `H` | +4 | `br3` — three-way branch | B |
| `I` | +5 | `jal` — jump and link | J |
| `J` | +6 | `jalr` — jump and link, register | I |
| `K` | +7 | `lui` — load upper immediate | U |
| `L` | +8 | `sel3` — three-way select | R4 |
| `M` | +9 | `sys` — halt and trap | I |

The remaining 17 opcodes — `0` … `C` (−13 … −1) and `N` … `Q` (+10 … +13) —
are **reserved**.

### 3.4 Reserved encodings

An instruction word is **malformed** if it uses a reserved opcode, a reserved
funct, a flavor trit on an operation that takes none, or a nonzero value in a
field this document requires to be zero.

A malformed instruction faults. Draft 0.1 uses `F_TRAP`, because AM §4's code
list has nothing better — see §5.

The all-zeros word is `alu` with funct 0, flavor 0, and every register field
`r0`: `add.wrap r0, r0, r0`, which writes nothing. Zeroed memory is therefore
a field of no-ops rather than a field of faults, which is the behavior a
person debugging a runaway jump wants.

---

## 4. Instructions

### 4.1 Arithmetic

`alu` (R) computes `rd ← rs1 ⊕ rs2`; `alui` (I) computes `rd ← rs1 ⊕ imm`.
The operation is selected by `funct`:

| Digit | funct | Operation | Flavored | Notes |
|---|---|---|---|---|
| `D` | 0 | `add` | yes | AM §3.1 |
| `E` | +1 | `sub` | yes | |
| `F` | +2 | `mul` | yes | low 27 trits of the product |
| `G` | +3 | `mulh` | no | **high 27 trits** of the 54-trit product |
| `H` | +4 | `div` | no | round to nearest, ties away from zero (AM §3.2) |
| `I` | +5 | `rem` | no | matching remainder, \|r\| ≤ \|b\|/2 |
| `J` | +6 | `shl` | yes | ×3ᵏ; k out of 0…26 faults `F_SHIFT` |
| `K` | +7 | `shr` | no | ÷3ᵏ, exact (AM §3.3); same fault |
| `L` | +8 | `tmin` | no | AM §3.4 |
| `M` | +9 | `tmax` | no | |
| `N` | +10 | `tmul` | no | |
| `O` | +11 | `cmp` | no | §4.2 |
| `P` | +12 | `wrap` | no | `alui` only, §4.3 |

Other funct values are reserved.

The immediate form of an operation is spelled with an `i` suffix — `addi`,
`shli`, `tmini` — and takes the immediate where the register form takes `rs2`.
`wrap` has no register form and therefore no suffix (§4.3).

**The flavor trit** — `[15]` in R, `[12]` in I — selects the overflow behavior
of AM §3.1 for the four flavored operations:

| Trit | Flavor | Behavior on overflow |
|---|---|---|
| 0 | wrapping | the result is the symmetric residue mod 3²⁷ |
| +1 | trapping | fault `F_OVERFLOW` |
| −1 | flag | wrapping result in `rd`, **overflow trit in `rc`** |

Three behaviors, three trit values, one field. The flag form's second
destination is the `rc` field `[16..18]`, which exists in the R format only:
`alui` with the flag flavor is reserved, because there is nowhere to put the
trit. The overflow trit is +1 if the exact result exceeded MAX, −1 if it fell
below MIN, and 0 otherwise — the definition TIR §6's carry chaining consumes.

For unflavored operations the flavor trit must be 0.

`neg` is not an instruction: negation is `sub rd, r0, rs1`, and AM §3.4 notes
that `tneg` and `neg` are the same operation. `abs` is not one either — it is

```
sub  rt, r0, rs1          ; the negation
sel3 rd, rs1, rt, rs1, rs1 ; pick it when rs1 is negative
```

two instructions and no branch, using the three-way select this machine
already has (§4.2).

> **Erratum (informative).** Draft 0.1 wrote this as "`cmp` followed by
> `tmul` (AM Appendix A.2)". That identity was wrong, and AM Appendix A.2 now
> carries the correction: `cmp` yields one trit, and `tmul` against a widened
> one keeps only the lowest trit. The AM's repaired identity needs `splat`,
> which this machine does not have as an instruction; the `sel3` form above
> is the same two instructions and needs nothing new. A `splat` funct is
> worth adding when the legalization pass needs a broadcast for its own
> reasons, and not before.

**`mulh` exists to make multi-part multiplication possible.** Expanding a
multiply beyond the word needs the full 54-trit product, and no
same-width instruction can deliver it. Implementing TIR's legalization pass
found this the hard way: TIR §3.1 has no widening multiply, so `mul` at a
width above the target's widest legal width currently cannot be expanded at
all. The machine now provides the primitive; TIR needs a matching instruction
before a frontend can reach it (`docs/spec-gaps.md` G6.6).

### 4.2 Comparison and selection

`cmp rd, rs1, rs2` sets `rd` to −1, 0 or +1 as `rs1` is less than, equal to,
or greater than `rs2` (AM §3.5). It is the only comparison; the two-valued
predicates are tests of its result.

`sel3 rd, rt, rn, rz, rp` (R4) sets `rd` to `rn`, `rz` or `rp` according to
the **sign** of `rt`. One instruction, three arms, no branch — the direct
realization of TIR's `select3`, and the reason a three-way `match` over
constants compiles to straight-line code.

Both read `rt`/`rs` by sign, so a value that is not a trit is not an error:
`sel3` on any register selects by its sign.

### 4.3 Constants and narrowing

`lui rd, imm` (U) sets `rd ← imm × 3¹⁴`.

Together with a 14-trit `addi`, this reaches **every word value exactly**:
the largest reachable magnitude is (3¹³−1)/2 · 3¹⁴ + (3¹⁴−1)/2 = (3²⁷−1)/2,
which is MAX. The split of a constant *c* is `hi = c >> 14` and
`lo = wrap(c, 14)`, and it needs no correction term, because a balanced
ternary number splits into high and low trits with nothing borrowed across the
boundary. The equivalent binary sequence needs a fixup whose omission is a
classic assembler bug; here there is no fixup to omit.

`wrap rd, rs1, n` (`alui` funct `P`, the immediate carrying *n*) sets `rd` to
the symmetric residue of `rs1` modulo 3ⁿ, for `n` in 1 … 27. It is the
narrowing operation, and it is one instruction because legalization needs it
constantly: a value promoted to a wider legal width must be renormalized to
its logical width after every wrapping operation (TIR §6). `n` outside 1 … 27
is malformed.

### 4.4 Memory

| Instruction | Format | Effect |
|---|---|---|
| `ld.word rd, imm(rs1)` | I, funct `D` | `rd ← ` the word at `rs1 + imm` |
| `ld.tryte rd, imm(rs1)` | I, funct `E` | `rd ← ` the tryte at `rs1 + imm` |
| `st.word rs2, imm(rs1)` | S, funct `D` | the word at `rs1 + imm` ← `rs2` |
| `st.tryte rs2, imm(rs1)` | S, funct `E` | the tryte at `rs1 + imm` ← the low tryte of `rs2` |

The width suffixes are the AM's storage-unit names in full (AM §1.3), not
abbreviations: `.t` next to the flavor suffix `.trap` would be one character
of distance between "a tryte access" and "trapping on overflow", and that is
not a distance worth economizing.

Displacements are in **trytes**, and so is every displacement written in the
`imm(rs1)` form (§4.5). Word accesses require a 3-tryte-aligned address and
fault `F_ALIGN` otherwise (AM §2.3); tryte accesses have no alignment
requirement.

There is **one load per width**, not two. A binary ISA needs `lb` and `lbu`
because a narrow value can be extended two ways; balanced ternary has one
extension (AM §3.5), so `ld.tryte` is unambiguous and the whole
sign-extension bug class is absent. `st.tryte` stores the low tryte of `rs2`
and ignores the rest, which is the only sensible reading of a narrowing store.

A single trit in memory occupies a full tryte (AM §2.3) and is loaded with
`ld.tryte`; there is no trit-width access.

### 4.5 Control transfer

**`br3 rs1, off₋, off₀, off₊`** (B) reads the **sign** of `rs1` and sets
`pc ← pc + 3 × off` in trytes, choosing the displacement by that sign.

This is the primitive branch, and the reason TIR makes `br3` its only
conditional terminator (TIR §3.6). All three destinations are named in one
instruction: there is no fall-through arm, so a three-way dispatch is one
instruction and one control transfer, not two compares and two branches.

Each displacement is 7 trits, giving ±1093 words. A displacement that does not
reach its target is an **assembly-time error**; the assembler does not expand
it into a longer sequence, because doing so would make an instruction's size
depend on its operands and break the one-word rule (§3.1). Placing blocks
within reach is the compiler's job.

The two-way case is `br3` with two equal displacements, and assembly spells it
`br2 rs1, then, else` (§7, pseudo-instructions), mirroring TIR's printer.

**`jal rd, off`** (J) sets `rd ← pc + 3` (the address of the following
instruction) and `pc ← pc + 3 × off`. With `rd = r0` it is a plain jump.

**`jalr rd, imm(rs1)`** (I) sets `rd ← pc + 3` and `pc ← rs1 + imm`. The
displacement is in **trytes**, because it is written in the `imm(rs1)` form
and that form means trytes everywhere it appears (§4.4) — a unit that changed
with the mnemonic would be a trap laid for the reader. A resulting address
that is not word-aligned faults `F_ALIGN`.

The PC-relative displacements of `br3` and `jal` are in **words**, since they
count instructions and an instruction is a word. The two units never appear in
the same syntax.

`rd` is written after the target is computed, so `jalr ra, 0(ra)` is
well-defined.

### 4.6 System

`sys` (I) selects by funct:

| Digit | funct | Instruction | Effect |
|---|---|---|---|
| `D` | 0 | `halt rs1` | the machine stops; `rs1` is the exit status |
| `E` | +1 | `trap code` | raise the fault named by the `code` immediate |

`halt` is a normal termination and is **not** a fault: a fault is a defined
halt *with a fault code* (AM §4), and a program that finishes has none. The
exit status is delivered to whatever hosts the machine.

`trap` carries the fault code in its **immediate field** — 14 trits, of which
draft 0.1 uses five values and reserves the rest — with `rd` and `rs1` being
`r0`. (Draft 0.1 first described this field as three trits in the table above
and as the immediate here; the immediate is what it is, and no encoding ever
used the three-trit reading.)

| Immediate | Code |
|---|---|
| 0 | `F_TRAP` |
| +1 | `F_OVERFLOW` |
| +2 | `F_DIVZERO` |
| +3 | `F_SHIFT` |
| +4 | `F_ALIGN` |

Other values are reserved. The code is written as its `F_*` identifier in
assembly source, never as a number (Assembly §6.1).

---

## 5. Faults

Every fault this machine raises is one of AM §4's five codes, raised by the
conditions AM defines: `F_OVERFLOW` from a trapping-flavor overflow,
`F_DIVZERO` from division or remainder by zero, `F_SHIFT` from a shift amount
outside 0 … 26, `F_ALIGN` from a misaligned word access, and `F_TRAP` from the
`trap` instruction.

A fault halts the machine and its code is observable to the host (AM §4).
There is no handler, no vector table and no unwinding.

> **Finding for the AM (informative).** Three machine conditions have no code
> that fits: a malformed instruction (§3.4), an access outside the address
> space (§2.1), and a reserved device address (§2.2). All three currently raise
> `F_TRAP`, which is defined as "explicit trap instruction" and is therefore a
> poor fit — a program cannot distinguish its own `trap` from a jump into
> garbage. AM §4 should probably grow `F_ILLEGAL` (malformed instruction or
> reserved encoding) and `F_ADDRESS` (access outside the address space).
> Recorded rather than invented: adding fault codes is the AM's decision, and
> `docs/spec-gaps.md` tracks it.

---

## 6. The `tritium0` calling convention

TIR §7 names this convention and defers its definition here.

### 6.1 Register roles

| Register | Alias | Role | Saved by |
|---|---|---|---|
| `r0` | `zero` | always 0 | — |
| `r1` | `ra` | return address | caller |
| `r2` | `sp` | stack pointer | callee |
| `r3` | `fp` | frame pointer | callee |
| `r4` … `r11` | `a0` … `a7` | arguments and results | caller |
| `r12`, `r13`, `rT1` … `rT6` | `t0` … `t7` | temporaries | caller |
| `rT7` … `rT13` | `s0` … `s6` | saved | callee |

### 6.2 The stack

`sp` is word-aligned at every instruction boundary and grows **downward**. A
program sets it up itself; reset leaves it 0 (§1.3). The portable idiom is to
start it at the top of whatever memory this machine has:

```
    ld.word sp, MEM_SIZE(zero)      ; sp ← A, the top of memory
```

which is one instruction and works unchanged on an implementation with a
larger address space.

### 6.3 Arguments and results

Arguments are assigned to `a0` … `a7` in order. A value wider than a word
occupies **consecutive argument registers, least significant word first** —
the register-file counterpart of the little-trytean order AM §2.2 fixes for
memory. Arguments
that do not fit in the eight registers are passed on the stack, in order, at
ascending addresses from `sp`.

Results come back the same way: `a0`, then `a1` for a second word.

> **Note on TIR (informative).** The ABI half of `docs/spec-gaps.md` G6.5 is
> discharged here: a `t54` value has a defined way to cross a function
> boundary, in two argument registers. The TIR half was closed differently,
> and the difference is worth knowing about.
>
> TIR's `ret` carries a single value and has no `sret` form, so rather than
> grow one, legalization reshapes a wide signature: a wide parameter becomes
> one parameter per part — which is exactly the order this section
> specifies — and a wide *result* travels through a hidden pointer instead.
>
> So **the provision above for a result in `a0` and `a1` is one the reference
> compiler will never use.** It stands for hand-written assembly and for any
> other frontend, and it is recorded here as unused rather than quietly left
> to drift.

### 6.4 Entry and exit

Execution begins at address 0 (§1.3). In draft 0.1 there is no linker and no
startup object: the first instruction assembled is the first instruction
executed, and a program ends by executing `halt`.

---

## 7. Answers to the assembly language's interface

Assembly §6.2 lists ten things this document must supply. For review
convenience, here they are with their answers:

| # | Requirement | Answer |
|---|---|---|
| 1 | register set and its syntax | §1.1; a **reserved name table** — `r0`, `r1`…`r13`, `rT1`…`rT13` and the §6.1 aliases. No sigil. These names may not be used as labels or `.equ` symbols |
| 2 | mnemonic table | §4, by opcode and funct |
| 3 | immediate ranges | §3.2 |
| 4 | addressing modes | one: `imm(rs1)`, a register base plus a displacement (§4.4). No indexed or scaled mode |
| 5 | encoding | §3.2, §3.3 |
| 6 | instruction size and first-pass knowability | §3.1 — always one word, always knowable |
| 7 | instruction alignment | §3.1 — word-aligned |
| 8 | branch operands and out-of-range behavior | §4.5 — displacements in words; out of range is an assembly-time error, never an automatic expansion |
| 9 | entry point | §6.4 — address 0 |
| 10 | pseudo-instructions | below |

### 7.1 Pseudo-instructions

An assembler must provide these expansions:

| Written | Expands to |
|---|---|
| `nop` | `add.wrap r0, r0, r0` |
| `mv rd, rs` | `add.wrap rd, rs, r0` |
| `neg rd, rs` | `sub.wrap rd, r0, rs` |
| `li rd, imm` | `addi.wrap rd, r0, imm` if it fits in 14 trits; otherwise `lui rd, imm >> 14` then `addi.wrap rd, rd, wrap(imm, 14)` |
| `la rd, symbol` | as `li` with the symbol's address |
| `j off` | `jal r0, off` |
| `call symbol` | `jal ra, symbol` |
| `ret` | `jalr r0, 0(ra)` |
| `br2 rs, then, else` | `br3 rs, else, else, then` |

`li` is the one expansion whose size depends on its operand. Both forms are
one or two words and the choice is made from the immediate's value, which is
known in the first pass whenever the operand is a constant expression; `la`
of a forward-referenced label always assembles to the two-word form, so that
§3.1's rule holds.

---

## 8. Object format — *reserved*

Draft 0.1 has no object format and no linker. An assembler emits a **raw
image**: the trytes it assembled, in address order, to be loaded at address 0.
That is enough for the reference VM to run a single-file program, and it is
deliberately not enough to be mistaken for a container format anyone should
depend on.

Separate assembly, relocation and symbol export (Assembly §3.4) are reserved
with it.

---

## 9. What this document deliberately does not define

- **Floating point.** AM §5 defines none, and neither does this.
- **Concurrency, atomics, memory ordering.** AM §2.4 reserves the model; a
  single-threaded sequentially consistent machine needs nothing here.
- **Privileged state, protection, interrupts.** The AM has no MMU and no
  permission model (AM §2.1); adding modes to a reference ISA before there is
  a program that needs them would be inventing requirements.
- **Caches, pipelining, timing.** No instruction has an architected latency.
- **A second TRISC width.** TRISC-9 and TRISC-81 are reserved by Naming §1's
  pattern and are not anticipated here.

---

## Appendix A (informative) — worked encodings

`add.trap a0, a1, a2` — that is `add` (funct `D`, 0), trapping flavor (+1),
`rd = r4`, `rs1 = r5`, `rs2 = r6`, `rc = r0`:

| Field | `[0..2]` opcode | `[3..5]` rd | `[6..8]` rs1 | `[9..11]` rs2 | `[12..14]` funct | `[15]` flavor | `[16..18]` rc | `[19..26]` |
|---|---|---|---|---|---|---|---|---|
| Value | 0 | +4 | +5 | +6 | 0 | +1 | 0 | 0 |
| Heptavintimal | `D` | `H` | `I` | `J` | `D` | — | `D` | `DDD` |

`ret` — `jalr r0, 0(ra)`: opcode `J` (+6), `rd = r0`, `rs1 = r1`, funct 0,
flavor 0, immediate 0.

A three-way sign dispatch, in full:

```
    cmp   t0, a0, zero        ; t0 ← −1, 0 or +1
    br3   t0, negative, zero_case, positive
```

Two instructions for what a binary machine spells as a compare, a
branch-if-negative, a second compare and a branch-if-zero.

A whole program — echo input until it ends — using the device region and
nothing else:

```
.equ IO_IN,    -1
.equ IO_OUT,   -2
.equ MEM_SIZE, -6

start:
    ld.word  sp, MEM_SIZE(zero)     ; portable across any A
loop:
    ld.tryte a0, IO_IN(zero)        ; ≥ 0 code unit, −1 waiting, −2 closed
    cmp      t0, a0, zero
    br3      t0, check_eof, echo, echo
check_eof:
    addi.wrap t1, a0, 1             ; −1 → 0, so t1 is zero only when waiting
    br3      t1, done, loop, done   ; −2 → −1: closed, stop
echo:
    st.tryte a0, IO_OUT(zero)
    j        loop
done:
    halt     zero                    ; exit status 0
```

Note that neither branch is two-way: the first dispatches on the sign of the
port value, the second on whether the machine is waiting or closed. Both are
genuinely three-valued questions, and each costs one instruction.

Every device access is one instruction with `zero` as its base, because the
device addresses are small negative numbers (§2.2).

---

## Appendix B (informative) — bug classes removed by construction

Continuing the scorecard, for the ISA specifically:

| Binary-world ISA bug class | Why it cannot occur here |
|---|---|
| `lui`/`addi` constant split off by one | balanced splitting borrows nothing; no correction term exists (§4.3) |
| `lb` vs `lbu` — wrong extension on a narrow load | there is one extension, so there is one load per width (§4.4) |
| logical vs arithmetic right shift | no sign trit, so no second shift to choose wrong (AM §3.3) |
| flag-register clobbering between compare and branch | the comparison result is an ordinary register value (§4.2) |
| two-instruction three-way dispatch, with the second compare forgotten | `br3` names all three destinations in one instruction (§4.5) |
| overflow silently ignored because the check was a separate instruction | the flavor is a field of the arithmetic instruction itself (§4.1) |
