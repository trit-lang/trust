# A third implementation in C++ — DDC diversity and the spec-gap work list

| | |
|---|---|
| **Status** | Proposal, 2026-10-29. **Not started, not scheduled.** |
| **Depends on** | `docs/ddc.md` §3.4 (the need) and §5 (the work list); `docs/spec-gaps.md` (the gaps) |
| **Depended on by** | Nothing. The DDC claim in `docs/ddc.md` §3.3 stands without this; this plan is what upgrades §3.4 from *partly, and honestly not* |

`docs/ddc.md` §3.4 names the one thing standing between this repository and a
real Diverse Double-Compiling claim:

> Making the claim stronger needs a `cP` this project did not write — a third
> implementation, by someone else, from `spec/`.

This plan proposes that implementation be written in **C++**, and plans the two
work programs it requires, in the order they must happen: first the
specification is closed until a stranger can implement from it, then the
implementation is built. Neither program may treat the source of `trustc` or
`bootstrap/` as input; the rules of that separation are §6.

---

## 1. The claim this buys

The fixpoint of `docs/ddc.md` §3.3 already holds: `stage1.tir` and
`stage2.tir` are both sha256
`7027e0978d95396af3d11846b89cd2fc422eb4b8c7cb285791f3d7afe8346e2e`, 9 485 971
bytes, at fixpoint #33 (spec-gaps G9.208–G9.233). What that evidence supports
today is the weak claim — *the Trust compiler's binary is what its Trust
source says, given `trustc`* — because `trustc` and `bootstrap/` were written
here, by the same hands, from the same specification. A backdoor placed in
both would survive exactly as Thompson's survives a rebuild (ddc.md §1).

A third implementation `cP3` in C++, written by someone who did not write the
other two, upgrades the claim to: **given `cP3`**, the binary is what the
source says. The argument is Wheeler's unchanged: the result is corroboration,
not proof, and it is relative to the parent (ddc.md §1). What changes is that
the parent is no longer ours. Two limits survive and must be stated wherever
the claim is stated:

- The specification itself is single-authored. A backdoor *in the language
  design* — a rule that reads clean and weakens what it compiles — is outside
  what any number of implementations of the same spec can detect. DDC silences
  the trusting-trust attack on implementations; it does not review the design.
- The machine below the comparison is still `tritium`, written here in Rust.
  Full diversity below the TIR text is a fourth implementation (a VM) and is
  out of scope for this plan; it is recorded, not hidden.

## 2. Why C++

Diversity is the only requirement (ddc.md §1: different implementation,
different author, ideally a different lineage). C++ is proposed as the vehicle
for three reasons, in decreasing order of importance:

1. **The toolchain root of trust moves off `rustc`.** ddc.md §3.5 is honest
   that today's root is `rustc`'s, not ours. A C++ `cP3` roots its side of the
   comparison in `g++`/`clang++` and `libstdc++` — a different lineage in the
   exact dimension §3.5 says the question gets pushed up. Any other language
   with an independently-maintained toolchain would serve as well; this is the
   argument for *some* third toolchain, with C++ as the nomination.
2. **The balanced-ternary core is small and total.** AM `spec/00` defines the
   whole arithmetic — `Bt`, the width-typed `tN`, the flavors, the faults, the
   literal notations — with no floating point and no platform types. It is the
   kind of library C++ expresses with zero dependencies, which is the property
   ddc.md §3.5 credits `trustc`'s zero-external-crates decision with buying.
3. **Determinism is achievable but must be chosen.** The C++ analogues of
   ddc.md §3.1's warning are `std::unordered_map`'s iteration order,
   pointer-keyed containers, and address-derived names. None is forced by the
   language; all are forbidden by the comparison contract (§3 below). The
   implementation rule is one sentence: *no iteration order may reach the
   output unless the specification states it.*

The language choice is an open author decision (D2), recorded rather than
silently fixed, because "the third implementation is C++" is exactly the kind
of sentence that hardens into an assumption if it is not written down with its
reason.

## 3. The comparison contract

What `cP3` must agree to is ddc.md §3.3's contract, unchanged: the artifact is
the **canonical TIR module text**, and equality is byte-for-byte **including
the names**. ddc.md §4 step 3 already argues why the names may not be
normalized away — two implementations that agree about emission order agree
about `%v.3` for free, and two that do not are two that emit different code.
The contract carries over to `cP3` word for word.

The acceptance evidence, in increasing order of strength:

| Level | Corpus | Bar |
|---|---|---|
| L1 | the 101 + 9 quarantined minimal reproducers | byte-identical TIR to `trustc`'s |
| L2 | `scripts/bootstrap.sh`'s agreement line on the prelude | every counter: names defined, resolved, rewritten, laid out, typed, checked |
| L3 | the 22 whole programs | instruction-for-instruction assembly, since that is the only way an assembly *can* be compared (ddc.md §3.2, §4 step 3) |
| L4 | **sA itself** — `bootstrap/program.tr` | `cP3 tir bootstrap/program.tr` == `stage1.tir` == `stage2.tir`, the 9 485 971 bytes at their hash |

What does **not** have to agree: the refusal boundaries beyond sA. The
bootstrap side refuses families the spec permits (a niche-encoded `Opt<trit>`,
`f::<T>` corner cases — spec-gaps G9.181's `fnno`/`forref`/`cannot`/`folded`
probe corpus), because it was built to compile itself, not the whole language.
`cP3` written from `spec/` will naturally accept more. That is not a
divergence that reads as a failed DDC, because the DDC comparison only ever
runs on inputs both parents compile — sA first of all. Where the *suite* holds
refusals as the comparison (the `programs/refused/` lane), the refusals are
the comparison, and §4 phases G2/D5 decide their status rather than leaving a
stranger to guess which "implementation limit" is normative.

This is the section a reader of ddc.md §5's closing paragraph needs: the gap
file is the work list, but the *contract* is this table, and both must exist
before phase P0 opens.

## 4. Program one — close the specification for a stranger

The test for "closed" is operational: **a stranger is someone who reads
`spec/` and nothing else** — not the log, not the sources, not this file. Work
is ordered by what blocks that reader hardest. Every phase obeys the sweep
rule of spec-gaps G0.15's process finding: one logical change per commit,
corrections written at both ends, and `docs/spec-gaps.md` entries amended in
place (marked resolved with a pointer at the spec text) rather than deleted —
the log is the audit trail of what was guessed.

### G0 — the TIR textual format has no grammar. *(The blocker.)* — **landed 2026-10-29**

*Outcome:* `spec/tir-0.1.md` §8 grew from seven lines to four sections —
§8.1 lexis, §8.2 grammar with the six decisions inline (G5.1–G5.6, marked
resolved in spec-gaps at the other end), §8.3 the canonical serialization
rules, §8.4 the target-description file format. The fold surfaced exactly
the class of find this program exists for: `= zeroinit` meant three
different things to the table, the machine and the interpreter — recorded
and pinned as spec-gaps G4.2 (second clause superseded) and G4.5 (a live
interpreter bug nobody's corpus could see). G1's list already grew by that
much before opening.

`spec/tir-0.1.md` §8 was seven lines. It said the textual form is the
canonical serialization and showed no grammar: no rule for a global's
address operand, a declaration without a body, an initializer, `br2`'s
operand order, omitted instruction types, or even the comment character.
All six decisions existed — made in `docs/spec-gaps.md` §5 and implemented
identically by both compilers — and none was where a stranger could read
it. **Equality in this project was defined on a format the specification
did not define.**

### G1 — fold the recorded decisions into `spec/`.

Every topic-organized entry in spec-gaps whose status is *Decision:* and whose
decision never reached the document it decides about:

- spec-gaps §1: G1.2 (`0t` maximal munch), G1.3 (`t`/`T` case) → Ch. 1 §3.1's
  literal section.
- spec-gaps §2: G2.1 (the `.flag` trit is the *direction* of overflow) → AM
  §3.1; G2.3 (the shift amount's type) → TIR §3.1.
- spec-gaps §4: G4.1 (alignment above the word), G4.2 (uninitialized
  globals), G4.3 (the poison model, stated) → AM §2.3 / TIR §4.
- spec-gaps §6: G6.1 (`t1` is not an arithmetic width), G6.2
  (renormalization without `trunc`), G6.3 (memory widths are not promoted),
  G6.4 (overflow detection's width requirement), G6.7 (`ptr` as an access
  type) → TIR §6 and §3.4.
- spec-gaps §7: G7.2 (`repr(lang)` field order), G7.3 (**which** niches and
  **which** values encode them — the rule that generates what Ch. 2 §6 only
  promises), G7.5 (`repr(lang)` fallback shape) → Ch. 2.
- G0.4's flagged line, still standing today: `spec/02-composites.md` says a
  reference excludes null, "at least 1 niche", where Ch. 3 §2.5 promises every
  non-positive value is invalid. One sentence, corrected at both ends.

### G2 — decide the undecided.

Each of these is a user decision, not an editorial one; the plan's job is to
put them on the agenda with their citations:

| Entry | Question | Why `cP3` cannot proceed on a guess |
|---|---|---|
| G7.6 | alignment in AM trytes vs a target's `addr_unit`, for targets that lack nine-trit units | changes a normative rule in AM §2.3 and TIR §7 both; `cP3`'s legalizer must match the answer |
| G0.2a | three machine conditions raise `F_TRAP` with no code of their own | a stranger *implements* the ambiguity into a VM-visible difference |
| G9.137 | the prelude's `Range<T>` compiles only for `t27` | **the prelude is handed to every compilation including sA's** — the two ways out (a numeric bound, or `Range<t27>`) change what `cP3` must accept |
| G9.189 | a field moved out of a value whose type has a destructor: the body never runs, and no chapter says so | Ch. 3 §1.3 and §1.4 read side by side are silent; sA itself does not move through such a type, but the corpus and any future library code does |
| G9.197 (a)(b)(c) | three small mirrors: a statement-`?`'s join-slot text; `match` over a struct destructuring; trait-glue queue timing | (c) is a determinism rule (G3); (b) is either a spec limit or future work |

### G3 — the determinism appendix.

The byte-equality contract of §3 runs on rules that today exist only as
entries in this log and as two agreeing implementations. A stranger needs them
as one document. Contents, each with its evidence entry:

- **Names are decided by counters**, per function and per name family, and the
  alphabet of an invented name is the dot-mangled one of G9.183: `wild.N`,
  `pat.N`, `tmp.slot.N`, `q.N`, `r.N`, `it.N`, a macro's `.macN` — names no
  Trust identifier can hold (G9.183, Ch. 0 §5.2).
- **A `let` of a computed aggregate adopts the temporary** and renames it, so
  the numbers run 1, 3, 4, 5 with 2 missing (ddc.md §4 step 3; G9.174).
- **A generic instantiation's name** is the written type (`id.trit`, never
  `t1`), module paths mangle as `@util.id.t27`, and an impl's parameter is the
  first of a method's key (ddc.md §4 step 3).
- **The instantiation queue is a stack**, deduplicated when a key is *asked
  for*, drained after the ordinary functions (ddc.md §4 step 3); the glue of a
  tiny trait impl is queued per G9.197(c) — one of the two orderings must be
  chosen and written.
- **Emission order of everything else**: provided bodies per implementing
  type, vtables before every function, the allocator's pool as an *order*
  rather than a set (G9.187).
- **No wall-clock, path, address, or hash iteration reaches the output**
  (ddc.md §3.1's list, stated normatively).

Recommended home: a new `spec/determinism-0.1.md`, cited from TIR §8 and from
ddc.md §3.3. Alternative: fold the name rules into TIR §8 and the emission
rules into a `docs/` annex. See D4.

### G4 — the sA audit.

Enumerate every construct `bootstrap/program.tr` and the prelude use, and
point each at a normative line of `spec/`. Whatever the audit cannot point at
is a G1 gap that was invisible until read this way, and it joins G1's list.
This is the verification that closes program one: after G0–G3, sA is the
single most important input the DDC comparison will ever run, and every tree
form in it must be specified, not merely implemented twice. The audit's output
is a checklist appended to this file when P0 opens.

**Gate P0**: program one is done when the stranger test passes on sA — a
reader of `spec/` alone can, in principle, emit the 9 485 971 bytes.

## 5. Program two — build `cP3`

Phased so that every phase has its own oracle and its own claim. The phases
assume program one is complete; P1 may explore, P3 may not start, before the
P0 gate.

```
P1  the balanced-ternary core, from spec/00 and nothing else
P2  TIR: structures, §8 parser, canonical printer, verifier (§§1–7)
P3  the Trust front end → M1: byte-equality on TIR, sA included
P4  the pipeline below TIR → M2: an image of sA with no trustc code
P5  the claim documents
```

**P1** is the `trit-core` equivalent: `Bt`, `Tint<N>`, the flavors, the
faults, the literal notations. The oracle is the specification's own claims —
AM §1.2's consequences, the division tie-break's symmetry identities, and
Appendix A's worked identities, each transcribed into a C++ test (`core/`
holds the same 21 assertions in Rust and stays unread, §6) — plus the
heptavintimal warning table of G0.15 (`0hJ` is 6, `0h0J` is −345: leading
zeros are not neutral).

**P2** is the IR half: the §8 grammar of G0 made executable twice — a parser
and a canonical printer, with the round-trip identity
`print(parse(x)) == x` checked over every TIR artifact that exists, **the
fixpoint's own 9 485 971 bytes included**, and the verifier of TIR §§1–7
(provenance of §5 included). A printer disagreement at this stage is cheap; at
P3 it costs a fixpoint.

**P3** is the front end against the §3 ladder: L1 the 101 + 9 battery
byte-for-byte; L2 the prelude counters; M1 = L4 — `cP3`'s `tir
bootstrap/program.tr` byte-identical to `stage1.tir`. M1 is the milestone the
whole plan exists for at minimum: **three independent sources agree about the
compilation of the self-compiler.** From M1 alone, the existing fixpoint
transfers unchanged — identical TIR bytes produce identical images under any
agreed backend — and ddc.md §3.4 can be honestly rewritten with a parent this
project did not write, for the front end.

**P4** extends diversity below the TIR text: canonicalization, inlining,
legalization, and codegen per ddc.md §3.2's pass list, held to L3's
instruction-for-instruction bar on the 22 programs. M2 is an image of sA
assembled with no `trustc` code at all, and `scripts/ddc.sh` gaining a parent
flag: `stage1` built by `cP3` end to end, `stage2` run under it, the hashes
printed either way — because a number that is only printed when it is right is
a number nobody checks (ddc.md §4 step 4).

**P5** is documentation: ddc.md §3.4 rewritten with the new claim **and its
surviving limits** (§1 above — the single-authored spec, the Rust VM),
README's DDC section updated, plan/README's status column moved. A
corroboration presented as a proof is worse than none.

## 6. The separation rule

The diversity argument is void if `cP3` is a translation. The rule, stated
once and enforced socially (D3 is the real mechanism):

- Inputs to the implementer: `spec/` as closed by program one; the corpus
  *inputs* and their expected *outputs* as produced by the existing
  implementations; this plan.
- Not inputs: anything in `core/`, `compiler/`, `vm/`, `bootstrap/`.
- Every divergence between `cP3` and an expected output is resolved **in the
  specification first**: the divergence is a reader's question about `spec/`,
  answered in `spec/` — which may mean `spec/` was wrong, or the expected
  output was — and *then* implemented. This is the loop spec-gaps has been
  running for 290 entries; `cP3` is designed to run it against a stranger's
  assumptions, and each answer lands where the next stranger will read it.

This is what ddc.md §5's closing sentence means operationally. Note that the
loop makes `cP3` valuable before it is finished: every spec question a
stranger asks is a gap the two reading implementations were too close to see.

## 7. Open author decisions

| # | Question | Recommendation |
|---|---|---|
| D1 | Scope at minimum: M1 (front end) vs M2 (full pipeline) as the acceptance line | M1 as the claim, M2 as follow-up — independent-source agreement about sA is the substance; the backend halves already agree twice over |
| D2 | C++ as the vehicle, and which standard/toolchain pinning | C++ yes; standard ≥ C++17 fixed at P1, `g++` and `clang++` both in CI, zero external libraries — mirroring `trustc`'s zero-crates argument (ddc.md §3.5) |
| D3 | Who writes it | Someone who has not read `core/`, `compiler/`, `vm/`, or `bootstrap/` — the "by someone else" of ddc.md §3.4 is the one requirement with no technical substitute |
| D4 | Determinism rules: new `spec/determinism-0.1.md`, vs TIR §8 + docs/ annex | the new document — the rules are normative for equality, they span the front end and the IR, and TIR §8 is the wrong grave for half of them |
| D5 | Bootstrap's extra refusal families (spec-permitted, bootstrap-refused) | document as draft-0.1 normative limits where lifting them is out of scope (D5 per family: a niche-encoded `Opt<trit>` stays refused *by the spec* or gets built — decide per family at G2) |

## 8. Risks

- **G9.184-class unknowns.** The log carries scars whose root cause was never
  proven (G9.184's `Option<()>` runtime bomb; G9.182's corpus question). The
  mitigation is G4's audit and the corpus ladder — but a stranger's
  implementation is itself the best scar-finder this project will ever build,
  and the plan should expect new entries in spec-gaps, not be surprised by
  them.
- **Diversity theatre.** The fastest way to finish P3 is to read
  `bootstrap/lower.tr` and translate it, which produces a third copy of the
  same hands and proves nothing. §6's rule is the mitigation; D3 is the
  enforcement. Worth stating plainly because the temptation is structural.
- **Spec churn breaking the existing fixpoint.** G1 folds are documentation
  and cannot move code; G2 decisions can. Any G2 answer that changes behavior
  moves `trustc` and `bootstrap/` first, keeps `scripts/bootstrap.sh`,
  `scripts/ddc.sh` and `scripts/reproducible.sh` green per commit, and only
  then joins `cP3`'s inputs. The differential invariant is the safety rail
  that found everything from G9.43 onward; it does not get relaxed to make
  room for the thing it makes possible.
- **The determinism appendix ossifying an accident.** Some emission orders are
  design; some are history. Writing them down promotes history to law. G3
  includes a review pass per rule — ddc.md §5's warning that canonical output
  is now correctness cuts both ways: a rule written down badly is a bug that
  two more implementations will faithfully reproduce.
