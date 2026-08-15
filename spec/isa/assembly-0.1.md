# ISA Specification — TRISC-27 Assembly Language

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), `spec/01-naming.md` (*Naming*), Language Ch. 1 §3 (literal notation) |
| **Depends on** | `spec/isa/trisc-27-0.1.md` (*ISA*), which supplies the instruction set (§6) and the encoding and object format (§8) |
| **Superseded by** | ISA §7 answers this document's §6.2 checklist item by item; where the two disagree, the ISA is normative |
| **Stability** | Draft. This document becomes a stable artifact when the ISA freezes, on the same terms as the ISA itself (TIR stability notice). |

This document defines the source language a TRISC-27 assembler accepts: its
lexical structure, its symbol and expression model, and its directives. It
does **not** define the instruction set — that is the ISA specification's job,
and until that document exists §6 and §8 stand as holes with a precise list of
what must fill them.

Everything outside those two sections is normative now, because none of it
depends on which instructions exist. That is the point of writing this first:
the parts of an assembler that get quietly wrong — literal notation, expression
arithmetic, range checking, alignment — are exactly the parts that are already
pinned down by the AM, and they are also exactly where a binary-world habit
would silently produce a wrong number.

Naming §2 records the tool itself as future work, and deliberately does not
name it. That rule is respected here: this document names a *language*, not a
binary. The extension is `.t27` (Naming §2).

> **Design note (informative).** An assembler is the last place in the stack
> where a value is written by a human and the first place it becomes a
> machine's bits — or here, trits. Every arithmetic rule below is the AM's,
> restated rather than re-derived, because an assembler that evaluated `7 / 2`
> as 3 while the machine it targets evaluates it as 4 would be a defect that no
> test of the compiler could catch.

---

## 1. Source format

### 1.1 Files and lines

A source file is a sequence of **lines** separated by U+000A. A trailing
U+000D immediately before the separator is ignored, so files with either line
ending assemble identically.

Source text is UTF-8. Outside comments and reserved string forms (§2.5), only
characters in the ASCII range are meaningful; a non-ASCII character elsewhere
is an error.

### 1.2 Comments

`;` begins a comment, which runs to the end of the line. There is no block
comment form.

The character is `;` and not `#` or `//` because every listing in every
document of this project already uses it, including the TIR specification's,
and the assembler is the least appropriate place in the stack to introduce a
second convention.

### 1.3 Statements

Each line holds at most one statement, optionally preceded by any number of
label definitions:

```
[label:]… [ directive | instruction ] [; comment]
```

Blank lines and comment-only lines are permitted anywhere. Horizontal
whitespace (space, U+0009) separates tokens and is otherwise insignificant:
indentation carries no meaning.

A statement is a **directive** if its first token begins with `.`, and an
**instruction** otherwise (§6).

---

## 2. Lexical elements

### 2.1 Identifiers

An identifier begins with an ASCII letter, `_`, or `.`, and continues with
letters, digits, `_`, and `.`. Identifiers are **case-sensitive**.

An identifier beginning with `.` in *label position* is a **local label**:
visible only within the file that defines it, and excluded from any exported
symbol table. There is no ambiguity with directives, because a label is always
followed by `:` and a directive never is.

### 2.2 Numeric literals

Exactly the three radices of Language Ch. 1 §3, with the same spellings, so
that a constant may be moved between a `.tr` source file, a TIR module and an
assembly file unchanged:

| Form | Example | Meaning |
|---|---|---|
| decimal | `6`, `-9841` | usual meaning |
| balanced ternary, prefix `0t` | `0t1T0` | digits `1`, `0`, `T` (= −1), most significant trit first; `0t1T0` = 6 |
| heptavintimal, prefix `0h` | `0h2C9` | digits `0`-`9` `A`-`Q`, one digit per 3 trits |

`_` may separate digits in any radix and carries no meaning: `0t1T0_T01`,
`3_812_798`.

`T` is the canonical spelling of the −1 trit (AM §1.1). An assembler must also
accept lowercase `t` in `0t` literals and in trit strings; a formatter
normalizes it to `T`.

There is **no hexadecimal and no octal**. Both are artifacts of the bit, and
an assembler must reject `0x` and `0o` prefixes with a diagnostic that says
so, rather than with a generic parse error — this is a mistake a newcomer will
make, and it deserves a real message.

### 2.3 Trit strings

A **trit string** is a quoted sequence of `T`, `0`, `1` and `_`, written most
significant trit first: `"1T0T_0011T"`. It denotes a *sequence of trits*, not
a number: its length is meaningful and leading zeros are not redundant.

Trit strings appear only where a directive asks for one (§5.1). A numeric
literal and a trit string are not interchangeable: `0t1T0` is the number 6,
whereas `"1T0"` is three specific trits that happen to encode 6.

### 2.4 Punctuation

`: , ( ) + - * / % < >` and the shift digraphs `<<` `>>`, plus `$` (§4.5).

`&`, `|`, `^`, `~`, `!`, `&&` and `||` are **reserved and must be rejected**.
Language Ch. 0 §2.5 has since settled this: `& | ^` stay reserved and the
trit-wise operations are the methods `tmin`, `tmax`, `tmul` and `tneg`. The
paragraph below is retained as the reasoning that fed that decision.

Language Ch. 1 §4 left open whether `& | ^` should be repurposed as
`tmin`/`tmax`/`tmul` operators; an assembler that answered that question on
its own would prejudge the language's syntax. The named forms of §4.3 are the
only spelling in draft 0.1.

### 2.5 Character and string literals

Text literals (`'a'`, `"hello"` as *text*) were reserved while no encoding
existed. Language Ch. 5 §1 fixes one, so their meaning is now settled: a
character literal is one Unicode scalar value in one word, and a string
literal is a sequence of them, which is what `.string` emits (Ch. 5 §1.5).
`.utf8` emits interchange code units instead, one per tryte.

Neither directive is implemented in draft 0.1, and the quoted form continues
to be used by §2.3 for trit strings, which are not text — a reader tells them
apart by which directive consumes them.

---

## 3. Symbols

### 3.1 Labels

`name:` defines a symbol whose value is the address of the current location
(§4.5) at the point of definition. A label may appear alone on a line, and any
number of labels may precede one statement; all of them take the same value.

Label values are addresses, and addresses are word-sized (AM §2.1). A symbol's
value is therefore an integer that must be representable in the target's
pointer width; an assembler that supports a target whose `ptr_width` is
narrower than the AM word (TIR §7) checks against that target's width.

### 3.2 Constants

```
.equ name, expression
```

defines `name` as the value of *expression*, evaluated at the point of
definition. The expression must be resolvable there: a constant may refer to a
label already defined, but not to one defined later.

### 3.3 Definition and resolution

A symbol may be defined once. A second definition is an error, including a
label that collides with an `.equ` name.

Instruction and directive operands may refer to labels defined **later** in
the file. Resolution therefore takes two passes: the first assigns an address
to every statement, the second evaluates expressions and emits. This requires
that the size of every statement be known in the first pass, which is a
constraint on the ISA (§6.2, item 6).

`.equ` expressions are *not* forward-resolvable (§3.2); only operand
expressions are. The asymmetry is deliberate: it keeps constant definitions
free of ordering surprises while leaving forward branches, the case that
actually matters, unrestricted.

### 3.4 Visibility

```
.global name
```

marks a defined symbol as externally visible. Local labels (§2.1) may not be
made global.

Symbols defined in *another* file, and linking generally, are **reserved**:
draft 0.1 assembles one translation unit at a time and has no relocation
model, because the object format it would relocate does not exist yet (§8).

---

## 4. Expressions

### 4.1 Where they appear

Wherever a directive or an instruction operand takes a value. An expression is
evaluated to a single integer.

### 4.2 Evaluation is exact

Expressions are evaluated in **unbounded balanced ternary**. No intermediate
result wraps, saturates, or is truncated to a machine width; the width check
happens once, at emission (§4.4).

This is not an efficiency concession — it is what makes `(a - b) * c` mean the
same thing in an assembler as it does on paper, and it is why the AM defines
its arithmetic "on *n*-trit values for arbitrary *n*" (AM §3).

### 4.3 Operators

| Form | Meaning | Notes |
|---|---|---|
| `-a`, `+a` | negation, identity | negation is total (AM §3.1) |
| `a * b` | multiplication | exact |
| `a / b` | division | **round to nearest, ties away from zero** (AM §3.2) |
| `a % b` | remainder | `\|r\| ≤ \|b\|/2`, matching `/` (AM §3.2) |
| `a + b`, `a - b` | addition, subtraction | exact |
| `a << k`, `a >> k` | multiply / divide by 3ᵏ | AM §3.3; `>>` is exact round-to-nearest |
| `tmin(a, b)`, `tmax(a, b)`, `tmul(a, b)`, `tneg(a)` | trit-wise operations | AM §3.4, applied positionwise; trits above a value's significant width are 0 |
| `cmp(a, b)` | three-way comparison | yields −1, 0 or +1 (AM §3.5) |
| `sign(a)` | `cmp(a, 0)` | |
| `wrap(a, n)` | the symmetric residue of `a` mod 3ⁿ | the only way to narrow a value on purpose (§4.4) |

Precedence, tightest first: primaries (literal, symbol, `$`, parenthesized
expression, function call); unary `-` and `+`; `*` `/` `%`; `+` `-`;
`<<` `>>`. Operators of equal precedence associate left to right.

Two rules deserve restating because every reader arriving from a binary-world
assembler will assume otherwise:

1. **`/` does not truncate toward zero.** `7 / 2` is 4, `8 / 3` is 3, `-8 / 3`
   is −3, and `8 % 3` is −1. An assembler that used a host language's integer
   division here would disagree with the machine it targets.
2. **`>>` is division, not a bit-slide.** `a >> k` is exactly `a / 3ᵏ` under
   round-to-nearest, and no tie can arise because 3ᵏ is odd (AM §3.3). There
   is no "logical" shift; the concept presupposes a sign bit.

Division or remainder by zero, and a negative shift count, are assembly-time
errors (§7).

### 4.4 Range checking at emission

A value that does not fit the width it is being emitted into is an **error**,
never a silent wrap. This matches Language Ch. 1 §3 ("a literal that does not
fit its inferred type is a compile-time error, never a silent wrap") and
applies equally to directive data and to instruction immediates.

`wrap(a, n)` is the escape hatch, and being a named function it is visible at
the point of use — which is the whole difference between a mask that was
intended and a value that was lost.

### 4.5 The location counter

`$` denotes the address of the **current statement** — for a data directive,
the address of the first tryte it emits. It is fixed for the whole statement:
`$` does not advance between the elements of a multi-value directive.

The current location advances by the size of each statement. `.org` (§5.3) is
the only way to set it directly.

---

## 5. Directives

Directive names are case-sensitive and lowercase.

### 5.1 Data

| Directive | Emits |
|---|---|
| `.tryte e, …` | one tryte per expression; each must lie in −9841 … +9841 |
| `.word e, …` | one word (3 trytes) per expression, **little-trytean**: least significant tryte at the lowest address (AM §2.2) |
| `.trits "…"` | the given trit string, packed 9 trits per tryte, least significant tryte first; the most significant tryte is zero-padded at the top if the string's length is not a multiple of 9 |
| `.zero n` | `n` zero trytes; `n ≥ 0` |
| `.fill n, e` | `n` copies of the tryte `e` |

The storage-unit names of AM §1.3 are used rather than the type names of
Language Ch. 1 (`t9`, `t27`): a data directive emits *storage*, not a typed
value, and the distinction is worth keeping visible in the one place where a
programmer lays out raw memory by hand.

`.trits` exists because a mask or a trit-pattern table is naturally written
trit-exactly, and writing it as a decimal number would be an act of
obfuscation. It is the assembler's counterpart to the `0t` literal.

### 5.2 Alignment

```
.align a
```

advances the location to the next multiple of `a` trytes, emitting zero trytes
as padding. `a` must be a **power of three** (Composites Ch. 2 §1); any other
value is an error. `.align 1` is a no-op.

Padding is zero-filled rather than unspecified: an assembler's output must be
reproducible, and "unspecified padding" (Composites Ch. 2 §1) is a statement
about what a *program* may observe, not a licence for a tool to emit
non-deterministic files.

### 5.3 Placement

```
.org a
```

sets the location counter to `a`, emitting zero trytes for any gap. `a` must
not be less than the current location: an assembler does not overwrite what it
has already emitted. Memory is flat and unsegmented (AM §2.1), so `.org` and
the implicit start address are the whole placement model.

`.section` is **reserved**. The AM has no permission model and no
segmentation; sections are a property of an object format, and there is not
one yet (§8).

### 5.4 Symbols

| Directive | Meaning |
|---|---|
| `.equ name, e` | define a constant (§3.2) |
| `.global name` | export a defined symbol (§3.4) |

### 5.5 Reserved directives

`.section`, `.extern`, `.string`, `.ascii`, `.macro`, `.endm`, `.if`,
`.else`, `.endif`, `.include`, `.byte`, `.half`, `.quad`.

The last four are listed so that they are rejected loudly rather than
mistaken for typos: they are binary-world names for units that do not exist
here. An assembler must diagnose `.byte` with a message naming `.tryte`.

Macros and conditional assembly are deferred, not rejected in principle. They
are the two features whose absence will be felt first, and they should be
designed once there is real assembly source to learn from.

---

## 6. Instruction statements — *interface to the ISA specification*

### 6.1 Shape

An instruction statement is a mnemonic followed by zero or more operands
separated by `,`:

```
mnemonic [operand [, operand]…]
```

Mnemonics are identifiers (§2.1), lowercase, matched case-sensitively.

An operand is one of: an expression (§4), or a **register reference**, or an
ISA-defined composite form such as a memory operand. Which forms each mnemonic
accepts is defined by the ISA specification, not here.

Fault codes are written as the `F_*` identifiers of AM §4 wherever an
instruction takes one; they are not numbers in source text.

### 6.2 What the ISA specification must supply

This document is complete only once the following exist. Listed explicitly so
that the ISA specification has a checklist rather than a vague dependency:

1. **The register set** and its syntax — how a register is spelled, and
   whether the spelling is distinguishable from an identifier by shape (a
   sigil) or only by context (a reserved name table). Either is workable; the
   choice must be stated, because §2.1's identifier rule is otherwise
   ambiguous at an operand position.
2. **The mnemonic table**: every instruction, its operand count, and each
   operand's kind.
3. **Immediate ranges** per instruction operand, so that §4.4's range check
   has a width to check against.
4. **Addressing modes** and their syntax, if the ISA has memory operands more
   structured than a plain address expression.
5. **The encoding** — the trit-level layout of each instruction.
6. **The size of every instruction**, and whether it is knowable before its
   operands are resolved. §3.3's two-pass model requires that it is; a
   variable-length encoding is permitted only if the ISA also defines a
   deterministic rule for choosing the size in the first pass.
7. **Instruction alignment**, if instructions must start at an address that is
   a multiple of something.
8. **Branch operand semantics**: whether a branch operand is an absolute
   address or a displacement, its range, and what an out-of-range target does
   — an error, or an automatic expansion the assembler is required to perform.
9. **The entry point convention**: which symbol, if any, execution begins at.
10. **Any assembler-synthesized instruction forms** (pseudo-instructions) the
    ISA wants to guarantee, and what each expands to.

### 6.3 Three-way branches (informative)

TIR §3.6 makes `br3` the only conditional terminator and displays the
degenerate two-destination case as `br2`. Whether TRISC-27's assembly mirrors
that — one three-way branch mnemonic, with a two-way form as sugar — is an ISA
decision. It is raised here only because the answer affects item 8 above:
a three-way branch has three targets and therefore three ranges to check.

---

## 7. Errors

An assembler must diagnose all of the following, and must not emit output for
a file containing any of them:

| Condition | Reference |
|---|---|
| a value that does not fit the width it is emitted into | §4.4 |
| division or remainder by zero in an expression | §4.3 |
| a negative shift count | §4.3 |
| `.align` with an operand that is not a power of three | §5.2 |
| `.org` moving the location counter backwards | §5.3 |
| a symbol defined more than once | §3.3 |
| a symbol that is never defined | §3.3 |
| a forward reference in an `.equ` expression | §3.2 |
| a `0x` or `0o` prefix, diagnosed as "no hexadecimal or octal" | §2.2 |
| a reserved operator (`&` `\|` `^` `~` `!` `&&` `\|\|`) | §2.4 |
| a reserved directive used as if it were implemented | §5.5 |
| an invalid character in a trit string, or an unterminated one | §2.3 |
| a non-ASCII character outside a comment | §1.1 |

Assembly-time errors are not faults (AM §4): a fault is a runtime halt of a
machine, and the two are never called by the same name.

---

## 8. Output — *reserved*

The object or image format an assembler produces is **reserved**, pending the
ISA specification's encoding (§6.2 item 5) and a decision about whether draft
0.1 has a linker at all. Until then an assembler has nothing it can correctly
write, which is the honest reason this document stops here rather than
inventing a container.

---

## 9. What this document deliberately does not define

- **The instruction set.** §6.
- **Text encoding.** Language Ch. 5 §1 fixes it, and Ch. 5 §1.5 says what
  `.string` emits: the native encoding, one word per character, so that a
  string the assembler builds and one the compiler builds are the same thing.
  `.utf8` emits interchange code units, one per tryte. Neither directive is
  implemented.
- **Macros and conditional assembly.** §5.5.
- **Linking and relocation.** §3.4, §8.
- **Debug information and listing output.** Neither has a consumer yet.
- **A name for the assembler binary.** Naming §2's rule against squatting a
  name for a tool that does not exist applies to this one too.

---

## Appendix A (informative) — a worked source file

Everything below is fully defined by this document; instruction lines are
omitted precisely because they are not.

```
; A trit-pattern table and the constants that index it.

.equ TABLE_LEN, 9
.equ WORD_TRYTES, 3

.align 3
table:
    .tryte  -4, -3, -2, -1, 0, 1, 2, 3, 4
table_end:

.equ TABLE_SIZE, table_end - table      ; 9 — labels are addresses, and
                                        ; expressions over them are exact

; A mask keeping the low nine trits of a word, written trit-exactly.
low_mask:
    .trits "111111111"                  ; one tryte: nine 1 trits

; The same value as a number, for comparison. Both spellings are legal;
; the trit string is the one that says what it means.
low_mask_again:
    .tryte 9841                         ; = 0t111111111

; Round-to-nearest division at assembly time, and the range check that
; catches the mistake it would otherwise hide.
.equ HALF, TABLE_LEN / 2                ; 5, not 4 — ties round away from zero
.equ SCALED, TABLE_LEN << 2             ; 9 * 3^2 = 81

scratch:
    .zero WORD_TRYTES

.global table
```

`.tryte 9842` on the line after `low_mask_again` would be an error, not a
wrap; `wrap(9842, 9)` would be the way to ask for the wrapped value on
purpose.

---

## Appendix B (informative) — bug classes removed by construction

Continuing the scorecard of Language Ch. 1's appendix, for the assembler
specifically:

| Binary-world assembler bug class | Why it cannot occur here |
|---|---|
| host truncating division disagreeing with target division | one division, specified as the AM's (§4.3) |
| a mask silently truncated to the emitted width | out-of-range emission is an error; narrowing is the named `wrap` (§4.4) |
| sign-extension vs zero-extension in a directive | there is one extension; a balanced value's high trits are zero (AM §3.5) |
| `.byte` emitting a different size than the reader expected | the unit names are the AM's, and the binary-world names are rejected by name (§5.5) |
| endianness confusion in multi-unit data | one order, stated once: little-trytean (§5.1) |
| hex typo producing a valid but wrong constant | there is no hexadecimal to typo (§2.2) |
