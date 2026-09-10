# Alternatives considered — the analysis record

| | |
|---|---|
| **Status** | Record, 2026-09-11. Normative for nothing; exists so the next session does not re-derive or re-litigate. |
| **Sibling** | [`vdot-device.md`](vdot-device.md) — the accepted design this analysis selected |

This file records the four analyses that produced the accepted design, with
their numbers, so that any future proposal to revisit them starts from
evidence instead of preference.

---

## 1. Can the current stack run BitNet? — measured, not guessed

BitNet-class networks ({−1,0,+1} weights, integer activations, matmul as
add/sub/skip) are the workload this ISA was, accidentally or otherwise,
designed around:

| BitNet needs | TRISC-27 already has |
|---|---|
| Weight range {−1,0,+1} | the trit itself; `.trits` packs 9/tryte natively |
| Ternary MAC = add/sub/skip | `cmp` yields a trit; `br3` is the primitive branch |
| Negate a weight | trit-wise, carry-free (AM §1.2) |
| Accumulator width | int8 × trit × K≈4096 ≈ 21 binary bits; a word holds ±3.8×10¹² |
| Requantization division | round-to-nearest is the *only* division; `>>` is exact by 3ᵏ with no tie case |

And `examples/trust/HPL.tr` already demonstrates fixed-point LU at scale
3¹¹ via `mulh`, so the arithmetic surroundings (softmax, norms) are
engineering, not research.

**The wall is speed, and it is memory-shaped.** The reference VM retires
83 M instructions/s; a hand-tuned scalar inner loop costs ~6–10 instructions
per MAC. A 2B-parameter model needs ~10¹⁰ instructions per token — minutes
per token, ~4 orders of magnitude behind bitnet.cpp on the same host. The
decode phase of LLM inference is memory-bound GEMV: every token streams all
weights once, so

```
tokens/sec ≤ memory bandwidth ÷ weight volume
```

- 2B params ≈ 222 M trytes packed → ~0.9 s/token even at one instruction
  per *word* of weights — the floor no software can go under on this VM.
- The comfortable envelope of the current VM is **10⁴–10⁷ parameters**.

So: conceptually the perfect target; practically out of reach for real
models without one of the interventions below.

## 2. What a "performance number" can honestly mean (no hardware exists)

This settled how every number in both documents must be stated.

| Anchor | Answers | Cannot answer |
|---|---|---|
| **Instruction-count cost model** (what the repo uses) | "given a single-issue, ~1-cycle-per-instruction ternary RISC, relative throughput?" | real clocks, memory system |
| **Emulation wall-clock** | "how fast is the *simulator* on this host?" | anything architectural |
| **FPGA / RTL soft core** | area, clock, per-instruction cost | — the only physical answer, at proportionate cost |

ISA §2.3 chose the middle deliberately ("an instruction count is the thing
an implementation can report honestly"). The implicit model — single-issue,
ALU ≈ 1 cycle, memory free — is fine *when stated*. The real hardware
questions (27-trit buses, tryte banking, the register file) are invisible
to it, and the only honest way to price them is silicon or an FPGA, both
out of scope here. Consequence for `vdot-device.md`: `CYCLES` stays a
retired-instruction count; device work is reported separately (`V_LANES`),
and seconds are never quoted as properties of the machine.

## 3. Shape of the extension — four candidates

| Candidate | Encoding | Semantics cost | Verdict |
|---|---|---|---|
| Fixed-window instruction (`dotp27`: one word of weights × 27 activation trytes) | R-format funct; free | fixed cost, simple faults | good, but capped (§5) |
| Variable-length instruction (`dotp rd, w, a, K`) | R4-ish; fits | **partial-execution semantics** on faults; breaks uniform-cost | rejected: the ISA's cost model is the feature |
| **Device** (4–5 negative addresses) | **zero new encodings** | inherits §2.2 faults wholesale; cost protocol explicit | **accepted** |
| VM auto-vectorization | zero spec surface | an optimizer in the reference — see §4 | rejected for now, recorded |

Two ISA facts made the encoding debate cheap: 17 of 27 opcodes are reserved,
and the 14-trit immediate (±2 391 484) holds any realistic K. The difficulty
was never encoding; it is that the ISA's own precedent (§2.3: a `rdcycle`
instruction "would spend one of the seventeen reserved opcodes on something
the device region already knows how to express") says facilities the device
region can express belong there. A load-triggered dot engine is the cleanest
such facility: 3 stores + 1 load per row, **4 retired instructions
independent of K** — below the ~0.15 inst/MAC floor any in-ISA instruction
has, because it also amortizes the loop.

### The language/TIR frictions (independent of shape)

These survive every candidate and are mostly outside the ISA:

1. **AM §2.3's impedance.** Sub-tryte values occupy a whole tryte; "packing
   multiple trits into one tryte is a library/codegen decision." A
   `[trit; N]` array is 9× bloated. The packed form today exists only as
   `.trits` data and TIR global initializers — hence `TritVec` over `[t9]`
   rather than a new layout.
2. **No int→pointer cast (TIR §5).** Deliberate — "keeps the provenance
   model trivially sound" — but it means an assembly-side weight blob
   cannot be handed to Trust as a pointer. Phase 1 uses generated
   `const [t9; N]` instead; extern globals remain future work.
3. **UB vs fault.** TIR §4's UB inventory is five items, and the repo's
   culture is "faults are not UB". The device's out-of-range reads are
   faults (`F_ADDRESS`), checkable differentially — never new UB.
4. **Legalization is width-shaped today.** Feature-gated operations are a
   new expansion *category*; `legalize/divide.rs`'s emit-a-helper-call
   pattern absorbs it without new machinery.

## 4. VM auto-vectorization — the serious alternative

Idea: no ISA change at all; the VM statically recognizes the compiled
dot-product idiom and executes it through a bitplane kernel, charging
`CYCLES` the exact scalar count. Same kernel, same 30–40× ceiling.

**Where it genuinely wins:**
- zero spec/governance surface; accelerates existing and hand-written images
  for free;
- `CYCLES` semantics preserved **exactly** — the VM knows how many scalar
  instructions it replaced. The device cannot say this.

**Where it loses, decisively for this repo:**

1. **Verification shape.** The device has one semantics with two
   implementations and a convergent differential test space. An idiom
   recognizer has an open set of program shapes, each needing its own
   equivalence argument: wrap-only accumulation (or the K < 3.87×10⁸ proof),
   first-faulting-address agreement, exact retirement counts. Negative
   tests ("this near-miss must NOT match") never complete. `docs/status.md`
   §11's lesson is that every serious bug here has been transformation-side.
2. **Hidden coupling.** The recognizer's hit rate depends on the backend's
   loop shapes — which this repo's own history (G8.19, G8.22, G8.24) shows
   changing every few weeks. Misses are *silent slowdowns*, and pinning them
   takes contract tests across a seam no document admits exists — precisely
   the coupling the citation/sweep culture exists to prevent.
3. **No architectural artifact.** It makes the emulator faster without
   making the argument; a second VM or an FPGA core inherits nothing, and
   the wall-clock/CYCLES divergence becomes uninspectable (no `V_LANES`).

Recorded facts affecting any revival: self-modifying code is legal and the
icache invalidates per store (`vm.rs:225`); a matcher must reuse that path.
Verdict: **rejected for now**; revisit only as a documented emulator-only
fast path after the device lands and is measured, with one pinned idiom and
both positive and negative pin tests.

## 5. Execution substrate — int8 kernels vs trit-level bitplane SIMD

Two ways to host the device kernel on a binary machine, and the distinction
is what makes the accepted design worth doing:

| | int8 kernel offload (bitnet.cpp-shaped) | **bitplane trit SIMD (accepted)** |
|---|---|---|
| What the *kernel* computes | binary int8 arithmetic (legal for BitNet only via the requantization argument) | ternary truth tables; **every intermediate trit-exact** |
| Activations | i8 lanes | **i16 lanes — a tryte fits exactly** |
| Weights | 2-bit dense unpack | (pos, neg) planes, 2 bits/trit, 128 trits per ymm |
| Legality argument needed | yes (requantize) | none — the same move tritium already makes (i16 per tryte, i128 per word), one level up |

Bitplane operation costs (the whole taxonomy):

| Operation | Cost on host |
|---|---|
| `tneg` / `tmul` / `tmin` / `tmax` / per-lane `cmp` | 0–5 boolean ops per *vector* — free-class |
| horizontal reduction | popcnt(pos) − popcnt(neg) — free-class (VPOPCNT/psadbw) |
| **tryte-lane add/sub** | i16/i32 lanes **+2–4 ops wrap fixup**, because 3⁹ ≠ 2ᵏ — this is the emulation tax, precisely located |
| per-lane multiply-add with carry | expensive → **never propagate**: carry-save accumulation, one `wrap_to` per row |
| tryte → bitplanes transcode | one 78 KB LUT lookup per 9 trits, VM-side at the memory boundary |

The NN observation that makes the table decisive: the hot op is elementwise
trit work plus reductions — both free-class; the expensive carry forms
barely occur. The BitNet inner product is exactly

```
acc = Σ(aᵢ·pos_maskᵢ) − Σ(aᵢ·neg_maskᵢ)
```

with i16→i32 widening each chunk (16 × 9841 overflows i16 — the kernel's
one hard constraint). ~1 host instruction per MAC, 30–40× over the scalar
VM, and the bitplane overhead re-reads as a clean number: **~3–5× is the
quantified cost of not having ternary silicon** — which is the argument the
project wants to make, in numbers.

Two hard-won subtleties kept:

- **Exact-then-wrap ≡ sequential-wrap** for K < 3.87×10⁸ (tryte bounds
  against MAX_WORD). The spec pins exact-then-wrap anyway; G6.6's `mulh`
  note is the standing reminder that unpinned bit-exactness choices are
  the ones that resurface.
- **Activation traffic dominates, not weight traffic**: a packed weight
  tryte serves 9 MACs, an activation tryte serves 1 — per-row act re-reads
  are the bandwidth term. Inside the device the host cache absorbs it
  (unlike naive per-instruction estimates of ~0.5 inst/MAC); the earlier
  25× fixed-window estimate was optimistic against this effect, and the
  honest device figure is 5–15× *if instruction-shaped* — another reason
  the device (4 instructions per row) beat the instruction shapes.

## 6. Conclusion

- **On the current VM**: ternary NN demos up to ~10⁷ params; real BitNet
  models are 4 orders of magnitude away and memory-bound, not ALU-bound.
- **The accepted intervention**: the dot-product device of
  [`vdot-device.md`](vdot-device.md) — zero instruction-set change, explicit
  cost protocol, spec-shaped deliverable.
- **Rejected**: fixed-window and variable-length instructions (good but
  capped, or semantics-breaking), int8-kernel offload (semantically
  borrowed), auto-vectorization (unverifiable shape-space, hidden coupling,
  no architectural artifact).
- **Out of scope**: FPGA (the only physical performance anchor), 2B-scale
  weight ingestion, device-internal threading (AM-lawful, needs its own
  evaluation).
