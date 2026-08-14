# Document 01 — Naming and Terminology

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Stability** | Normative for all documents, code, and tooling in this repository |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*) for the terms it defines |

This document is the single source of truth for every name in the project:
the product family, the repository layout, binary and crate names, notation
conventions, and the technical glossary. Other documents cite names; only
this one defines them. A name not listed here is not yet official.

---

## 1. The family

| Name | What it is | Pronunciation / form |
|---|---|---|
| **Trust** | the programming language | English word *trust*; always capitalized when naming the language |
| **trustc** | the Trust compiler | "trust-see"; always lowercase, even at sentence start |
| **TIR** | the intermediate representation — *Trust IR*, equally *Ternary IR* (both expansions are official) | "tee-eye-are"; always uppercase |
| **TRISC** | the ISA family — *Ternary RISC* | "tee-risk"; always uppercase |
| **TRISC-27** | the 27-trit reference ISA, first member of the TRISC family | width suffix in decimal |
| **Tritium** | the reference virtual machine, implementing TRISC-27 | element style: capitalized as a name; the binary is `tritium` |

### 1.1 Etymology and intent (informative)

- **Trust** = **Tr**it + R**ust**: the radix and the safety lineage in one
  syllable. The name is also a statement of purpose — a memory-safe systems
  language *should* be called Trust — and inherits, at no charge, the most
  famous title in compiler literature (Thompson, *Reflections on Trusting
  Trust*, 1984), which this project intends to earn honestly by eventually
  self-hosting.
- **TIR** was named Ternary IR before the language was named; *Trust IR*
  arrived as a backronym and both are kept deliberately.
- **TRISC-27** follows the RISC-V convention of family name plus variant
  designator; future width variants (TRISC-9, TRISC-81) are reserved by
  this pattern and need no new naming decision.
- **Tritium**: hydrogen-3 — mass number three, three nucleons. Chosen over
  other tri- candidates specifically to avoid **Triton** (an existing,
  widely known GPU compiler) — see §5.

### 1.2 Searchability rule

*Trust* is a high-frequency English word; the language is not findable by
bare-word search and never will be. Therefore:

- The public handle is **`trust-lang`** — GitHub organization, domain,
  package scopes, social handles. This mirrors `rust-lang` exactly and for
  the same reason.
- First mention in any public document is "**the Trust programming
  language**"; thereafter plain *Trust* is fine.
- Never introduce alternate spellings (TRust, trust!, Trust-lang as the
  language name) to "fix" searchability; the handle is the fix.

---

## 2. Repository and artifact names

Monorepo: **`trust-lang/trust`**.

```
trust/
├── spec/
│   ├── 00-abstract-machine.md      the AM — shared normative base
│   ├── 01-naming.md                this document
│   ├── language/                   the Trust language spec, chaptered
│   ├── tir/                        TIR spec (unstable interface)
│   └── isa/                        TRISC-27 spec
├── compiler/                       crate: trustc
├── vm/                             crate: tritium
├── tests/                          cross-component differential tests
└── examples/
```

| Artifact | Name | Notes |
|---|---|---|
| compiler crate & binary | `trustc` | |
| VM crate & binary | `tritium` | |
| source file extension | **`.tr`** | `main.tr` |
| TIR textual file extension | **`.tir`** | |
| TRISC-27 assembly extension | **`.t27`** | reserved; assembler is future work |
| test manifest / build tool | *reserved* | the cargo-analogue is deliberately unnamed until it exists; do not squat a name |

Crate-internal module names, pass names, and similar are engineering
namespace and not governed here, with one rule: they must not collide with
§1 family names in ways that create ambiguity (`compiler/src/tir/` is fine;
a second thing called `tritium` is not).

---

## 3. Notation conventions

Fixed by AM and Language Ch. 1; collected here as the citation point:

| Notation | Meaning | Defined in |
|---|---|---|
| `T`, `0`, `1` | trit values −1, 0, +1 in written trit strings, MST first | AM §1.1 |
| `0t` prefix | balanced-ternary literal (`0t1T0` = 6) | Lang Ch. 1 §3 |
| `0h` prefix | heptavintimal (base-27) literal, digits `0-9A-Q`, 1 digit = 3 trits | Lang Ch. 1 §3 |
| `-1t`, `0t`, `1t` | `trit`-typed literals (suffix `t`) | Lang Ch. 1 §3 |
| `tN` | balanced ternary integer type/width of N trits | Lang Ch. 1, TIR §2 |
| `F_*` | fault codes | AM §4 |
| `@` / `%` / `^` | TIR module symbols / SSA values / block labels | TIR §1 |

The `p0n`-style notation used by some prior ternary projects (p = +1,
n = −1) is **not** used anywhere in this project; documents quoting
external material using it must translate to `1/0/T`.

---

## 4. Glossary

Normative meanings of terms as used across all documents:

- **trit** — the three-valued unit of information: −1, 0, +1. Never "ternary
  bit" in normative text.
- **tryte** — 9 trits; the smallest addressable unit. Plural *trytes*.
- **word** — 27 trits on the abstract machine; on a concrete target, that
  target's declared register width. When ambiguity matters, write *AM word*
  or *target word*.
- **balanced ternary** — the signed-digit base-3 system over {−1, 0, +1}.
  Never shortened to just "ternary" in normative text (unbalanced ternary
  exists and is not this project).
- **symmetric range** — the value range ±(3ⁿ−1)/2 of an n-trit value; the
  property MIN = −MAX.
- **heptavintimal** — base 27, the compact human notation (the hex analogue).
- **little-trytean** — multi-tryte storage order, least significant tryte at
  the lowest address (AM §2.2).
- **fault** — a defined machine halt with an `F_*` code (AM §4). Distinct
  from **UB**, which is the absence of any defined behavior. The two words
  are never interchangeable.
- **flavor** — the overflow-behavior variant of an arithmetic operation:
  wrapping, trapping, or flag-producing (AM §3.1, TIR §3).
- **legalization** — the mandatory TIR pass rewriting arbitrary widths into
  a target's legal set (TIR §6).
- **niche** — an invalid representation of a type, usable to store an enum
  discriminant (Lang Ch. 2 §6).
- **legal set / legal width** — the operation widths a target description
  declares native support for (TIR §7).
- **AM** — the abstract machine of `spec/00-abstract-machine.md`; the word
  *machine* unqualified in normative text means the AM.

---

## 5. Avoided names (informative but load-bearing)

Recorded so future contributors do not re-propose them:

| Name | Why avoided |
|---|---|
| **Triton** | established GPU compiler project; guaranteed collision in exactly this project's audience |
| **Setun** | the historical Soviet ternary computer; honored in prose, not claimed as a product name |
| **TVM** | Apache TVM collision |
| `i9`/`i27` type names | early drafts used them; rejected because `i` implies a signed/unsigned pair that does not exist — the `tN` scheme carries the radix in the name |
| **Ordering** (type) | deliberately does not exist in Trust; the comparison result type is `trit` (Lang Ch. 1 §5) |
| `repr(C)` | there is no C ABI to be compatible with; the defined-layout repr is `repr(linear)` (Lang Ch. 2 §1) |

---

## 6. Document numbering and status vocabulary

Spec documents carry a status of **Draft**, **Stable**, or **Reserved**
(section-level reservations use the word *reserved* inline). Version
numbers are per-document; TIR additionally stamps its textual format
(`tir 0.1 …`) and rejects mismatches outright per its stability notice.
The two shared-base documents (00, 01) version in lockstep with whichever
document forces a change to them, and every change to 00 or 01 requires a
sweep of all citing documents in the same commit — the monorepo exists so
that this is possible.

**The sweep rule applies to every document, not only 00 and 01.** A chapter
that corrects, supersedes or discharges something another one said must fix
the other one's text in the same commit. The rule was first written for the
shared base because that is where a stale citation obviously hurts; it turns
out to hurt everywhere. Draft 0.1 accumulated at least five corrections
living only at the correcting end — a bounds check, a niche count, a copy
rule, a discharged checklist, a settled open question — which together made
an invisible errata chain that a reader could only assemble by reading all ten
documents in order.

Two mechanics enforce it:

1. **Every document's header table carries `Supersedes` and `Superseded by`
   rows** naming the section on each side, or `—`. A reader who lands on a
   section can see whether it still stands without leaving it.
2. **A correction is written at both ends.** The correcting document says what
   it corrects and why; the corrected document says it was corrected and
   points forward. Neither alone is enough: the first leaves a reader of the
   old text wrong, and the second leaves them without the answer.
