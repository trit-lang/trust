# The `vdot` device — implementation plan

| | |
|---|---|
| **Status** | Accepted direction, 2026-09-11. **Not started.** |
| **Depends on** | Nothing in flight; anchors verified against the tree of that date |
| **Depended on by** | Nothing — no other document may cite this as authority until P0 lands |

The design rule of this plan is the ISA's own (ISA §2.3, *Why a device and
not an instruction*): a facility the device region can express does not get
to spend an opcode. A dot-product engine is exactly such a facility — four
memory-mapped words, zero new instruction formats, zero new opcodes, the
existing §2.2 fault rules untouched.

The purpose is twofold, and the order matters:

1. **The argument.** BitNet-class networks (ternary weights {−1,0,+1},
   integer activations, linear layers reduced to add/sub/skip) are the
   workload balanced ternary is *for*. A dot-product device is the smallest
   architectural artifact that says so — a thing a ternary chip could
   implement — and it comes with the repo's usual scorecard.
2. **The speed.** The reference VM executes ~83 M instructions/s scalar
   (`docs/status.md`), which puts BitNet-scale models four orders of
   magnitude out of reach (`alternatives-considered.md` §1). The device lets
   the VM run the same observable semantics through a bitplane host kernel
   at ~1 host instruction per MAC — 30–40×, and a 135M-parameter model at
   real tokens/second on one core.

---

## 1. Device interface

New negative addresses, continuing the existing block (−1 `IO_IN`,
−2 `IO_OUT`, −3 reserved, −6 `MEM_SIZE`, −9 `CYCLES`):

| Address | Symbol | Width | Access | Meaning |
|---|---|---|---|---|
| −10 | `VW_PTR` | word | store | weight base address — packed, 9 trits per tryte |
| −11 | `VA_PTR` | word | store | activation base address — one tryte per element |
| −12 | `V_LEN` | word | store | K, the MAC count; K = 0 yields 0 |
| −13 | `VDOT` | word | **load** | computes and returns `wrap₂₇(Σᵢ<K wᵢ·aᵢ)` |
| −14 | `V_LANES` | word | load | lane MACs retired by vector devices since reset |

Semantics, stated once and normative everywhere:

```
result = wrap_to( Σ_{i<K} trit(mem_tryte(W + i/9), i mod 9) · mem_tryte(A + i), 27 )
```

- **A load triggers the compute.** There is no command register and no
  device state beyond the three parameter words and the lane counter. Each
  row-dot costs exactly 3 stores + 1 load = **4 retired instructions,
  independent of K** — the per-MAC cost visible to the ISA vanishes, which
  is why the device shape beats every in-instruction-set encoding
  (`alternatives-considered.md` §3).
- **Exact-then-wrap**, not sequential wrapping. The sum is computed exactly
  and wrapped into the word range once. For K < 3.87×10⁸ the two are
  provably identical (|aᵢ| ≤ 9841, |wᵢ| ≤ 1, MAX_WORD ≈ 3.81×10¹²), so no
  program this machine can hold observes the difference; the spec still pins
  the choice, the way G6.6's `mulh` note (ISA §4.1) says such choices must
  be pinned.
- **Faults are the existing ones.** Wrong width at a device address: §2.2's
  rule. Any touched address outside 0…A−1: `F_ADDRESS`, at the load of
  `VDOT`. Negative K: **open decision A1** (recommendation: `F_ADDRESS`).

`V_LANES` is a **work counter, not time** — the phrase is §2.3's, reused
deliberately. It exists so that something in the system can answer "how much
work went through the device" now that `CYCLES` cannot (§5 below).

## 2. VM implementation

New file `vm/src/vdot.rs`, in three layers, plus wiring.

**1. The scalar reference is the executable specification.** Plain loops over
`Memory::tryte`, `word.rs` helpers, one `wrap_to(sum, 27)` at the end. It is
always present, always selectable (`--kernel scalar`), and is the oracle for
every kernel differential test. Nobody optimizes it.

**2. A bulk-read path in `mem.rs`.** `read_span(addr, buf)` validates the
range once, then walks pages and copies page runs; unallocated pages read as
zero, matching `tryte`'s behavior today. This removes per-tryte `BTreeMap`
overhead from the hot path without touching sparse semantics.

**3. Bitplane kernels** (`std::arch` intrinsics only; no dependencies;
runtime feature detection):

- **NEON first** (the development host is aarch64/darwin), AVX2 second,
  same structure.
- Weights transcode tryte → (pos, neg) 9-bit planes via a 19 683-entry
  lookup table (78 KB, built once at startup). The transcode is the VM's
  job at the memory boundary; the AM never sees anything but trytes.
- Activations load as i16 lanes — an exact fit for a tryte (±9 841) — and
  **widen to i32 accumulators every chunk**: 16 × 9841 exceeds i16 range,
  so accumulating in i16 lanes is wrong, and this is the kernel's one real
  correctness constraint.
- The structure is `acc = Σ(aᵢ·pos_maskᵢ) − Σ(aᵢ·neg_maskᵢ)`, one
  `wrap_to` per row. Estimated ~1 host instruction/MAC against the scalar
  VM's 6–10.

Wiring: `device_load` / `device_store` in `vm.rs` (≈ lines 484/502) gain the
match arms. `V_LANES` is a `u64` beside `steps`; `profile.rs`'s `classify`
learns the new addresses.

## 3. Compiler

**Not a new instruction — a well-known external function.** TIR already has
declarations ("signature only, body external", `tir/ir.rs`), `putchar` is
already such a declaration, and *legalize/divide.rs* already expands an
operation into an ordinary function call. All three precedents point the
same way:

```tir
decl fn @__tir_dotp(w: ptr, a: ptr, k: t27) -> t27
```

- **`tir/interp.rs`**: the call dispatcher recognizes the name and computes
  the §1 semantics through the interpreter's own allocation/provenance
  model. The semantic core lives in **trit-core** so VM and interpreter
  share one definition — single source of truth.
- **`tir/target.rs`**: `TargetDesc` gains `features: Vec<String>` (parser,
  validation). `targets/tritium.target` gains `vdot`; `t27only.target`
  does not.
- **`tir/legalize/`**: a target without `vdot` rewrites calls to
  `@__tir_dotp` into calls to a per-module `@__lz_dotp_scalar` helper —
  `divide.rs`'s pattern, reused verbatim. A **verification note**: the
  helper is itself legalized TIR, so it participates in the ordinary
  pipeline; check the pinned-refusal tests in
  `compiler/tests/legalize_semantics.rs` — this is a new *category* of
  expansion (operation-level, not width-level), and status §8.1's frontier
  pinning will need one new entry, not a rewrite.
- **`tir/verify.rs`**: `verify_legalized` rejects a surviving `@__tir_dotp`
  call when the target lacks the feature.
- **`codegen.rs`**: the call lowers to the 3-store + 1-load device sequence.
  Pointers arrive as provenance-checked TIR `ptr`s; no provenance rules
  change.

## 4. Language

Zero syntax changes.

- **`TritVec`**, a library type whose storage is `&[t9]` — each t9 holding
  9 packed trits. AM §2.3 makes packing "a library/codegen decision", and a
  `[t9]` slice *is* the packed layout, so no Ch. 2 change is forced. A
  packed-`trit` surface type can come later if it earns it.
- `TritVec::dot(&self, acts: &[t9]) -> t27`: checks
  `self.trits_len() >= acts.len()` **once**, then calls the intrinsic.
  `lower.rs` recognizes the wrapper as a lang item and emits the
  `@__tir_dotp` call with the underlying storage pointer — the same way
  `derive(Ord)` emits TIR directly today. Raw pointers never surface in
  the language; memory safety is preserved by the one up-front check.
- **Weights into the image, phase-1 scope**: a host-side packer emits Trust
  source, `const W: [t9; N] = [0t…, …];` — balanced-ternary literals,
  human-inspectable, through the array-constant → TIR global path that
  already exists (`lower.rs`, "an array constant lives in a TIR global").
  Good to ~10⁷ parameters. The 2B-scale story (streaming over `IO_IN`, or
  extern globals, which TIR does not have) is **explicitly out of scope**
  and listed as future work.

## 5. Measurement honesty

- `CYCLES` semantics do not change: instructions retired, exactly one per
  device access.
- `V_LANES` reports the lane work, with §2.3's sentence adapted: *a count
  of lane operations, not of time.*
- Every performance claim is stated as instructions + lane-work. Wall-clock
  numbers are permitted only as informative notes labeled as properties of
  the host implementation — never as properties of the machine. Existing
  claims (83 MIPS, the G8.x percentages, HPL's format) stand untouched.
- `vdot_bench.tr` (P6) is the teaching artifact: the same data through a
  scalar loop and through the device, printing both retired instructions
  and `V_LANES`.

## 6. Test strategy

The discipline of `docs/status.md` §8, instantiated:

| Discipline | Test |
|---|---|
| Differential invariant (interp ↔ machine) | a Trust `dot` program, run both ways, results identical **including faults** (`pipeline.rs` pattern) |
| Legalization is a transform | same module legalized for `t27only` → scalar helper → identical behavior (`legalize_semantics.rs` pattern); the refusal frontier gains one entry |
| Kernel differential | bitplane kernel vs scalar reference: random weights × acts × K, ~10⁶ cases, plus fault-address agreement |
| Semantics unit tests | K=0; K=1; K ≢ 0 (mod 9); **a sum that crosses MAX mid-accumulation and returns in-range** (this case *is* the exact-then-wrap definition); all-±1; sparse zeros |

Finish line every phase: `cargo test`, `scripts/stats.sh`,
`scripts/citations.sh`.

## 7. Spec revisions (sweep rule — one commit per logical change)

- `spec/isa/trisc-27-0.1.md`: new §2.4 (address table, protocol, the §1
  formula, faults, `V_LANES`, and a *why-a-device* note cross-referencing
  §2.3's `rdcycle` precedent); Appendix A gains a hand-worked dot; Appendix B
  gains an **honest** row — the device removes "hand-written assembly inner
  loops inside safe Trust programs" and adds a partial-failure surface that
  §2.4 must name.
- `spec/tir-0.1.md`: §3.8 *well-known external functions*; §6 expansion
  rule for feature-gated operations; §7 `features` field.
- `spec/01-naming.md`: the device symbols.
- `docs/spec-gaps.md`: one entry each for device-not-opcode,
  exact-then-wrap, negative-K, and the extern-name route over a new
  `InstKind`.
- `docs/status.md` / `README.md`: component table, pipeline diagram, the
  new limitation (`V_LANES` ≠ `CYCLES`).
- `spec/00-abstract-machine.md`: **untouched.** Devices are an ISA-layer
  concept; AM §5 promises two ports, not at most two ports. Record the
  reasoning in spec-gaps instead.

## 8. Phases

```
P0  spec drafts (all of §7)
P1  trit-core reference implementation + semantics unit tests
P2  VM: scalar path, device wiring, V_LANES, mem bulk-read
P3  TIR: extern name, interp dispatch, features flag, legalizer expansion
P4  Trust: TritVec + lang-item recognition, end-to-end trust test
P5  NEON/AVX2 bitplane kernels + 10⁶-case differential (avoids P2–P4)
P6  vdot_bench.tr + ~1 M-parameter ternary MLP demo + docs sweep
```

P0–P3 are small diffs at located seams. P5 is the bulk of the engineering.
P6 is the acceptance demo.

**Non-goals**: no new opcodes or formats; no vector register file; no AM
change; no language syntax; no floating point; no 2B-parameter attempt;
device-internal multithreading deferred (AM conformance permits it —
an implementation may use any internals whose observable behavior matches —
but it lands as its own evaluation).

## 9. Open author decisions

| # | Question | Recommendation |
|---|---|---|
| A1 | fault code for K < 0 | `F_ADDRESS` (no existing code fits better; a value judgment, so left open) |
| A2 | addresses consecutive from −10 | yes — the spacing of −1, −2, −6, −9 looks intentional and was not guessed at |
| A3 | `@__tir_dotp` in the TIR spec body vs spec-gaps only | spec body §3.8 — the alternative leaves the IR unable to admit the concept exists |
| A4 | P6 demo shape | MLP (~10⁶ params); a transformer needs attention/RoPE/softmax and is P7 material |

## 10. Risks

- **i16→i32 widening in the kernel** (§2.3): the one place a fast path can
  be silently wrong; the 10⁶-case differential exists for it.
- **Intrinsic recognition in `lower.rs`** must follow the derive/closure
  precedent exactly rather than inventing a new mechanism — a new one-off
  path is how §11's "lowering-side, invisible to differentials" bugs get in.
- **Scope creep from auto-vectorization**: the rejected alternative
  (`alternatives-considered.md` §4) stays rejected unless this plan has
  landed and is measured.
