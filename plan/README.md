# plan/ — proposed work, not yet scheduled

This directory holds **proposals**. Nothing here is in progress. Other work
(most recently `bootstrap/`) continues in parallel; nothing in `plan/` may be
treated as a statement about the current tree, and every file/line anchor in
these documents is approximate as of 2026-09-11 — verify against the tree
before relying on one.

The house rules of `docs/` apply: decisions cite the section they touch, what
is missing is called missing, and a claim about performance says which cost
model it is stated in.

## Documents

| File | Contents | Status |
|---|---|---|
| [`vdot-device.md`](vdot-device.md) | The accepted design: a dot-product **device** (negative addresses, no new instructions), a bitplane host kernel behind it, and the six-phase implementation plan. | **Accepted direction; not started** |
| [`alternatives-considered.md`](alternatives-considered.md) | The analysis record: why BitNet on a ternary ISA, what performance numbers can honestly mean without hardware, and the four routes that were rejected (fixed-window instruction, variable-length instruction, int8-kernel offload, VM auto-vectorization). Read this before re-litigating any of them. | Record |

## The decision in one paragraph

To make ternary-neural-network inference (BitNet-class: ternary weights,
integer activations) actually runnable — and to make the *argument* that a
ternary ISA is the right host for it — extend **TRISC-27's device region**
with a dot-product device, not its instruction set. The scalar ternary
semantics is the specification; the VM implements it with a bitplane
NEON/AVX2 kernel, differentially tested against the scalar reference. Every
intermediate observable value stays trit-exact. The known costs are
enumerated in `vdot-device.md` §5 (measurement honesty) and its list of open
author decisions.

## Standing warning

`vdot-device.md` names integration points in `vm/`, `compiler/`, and
`compiler/src/lang/`. If any of those files has changed shape since
2026-09-11, the plan's phases stand but its line numbers do not.
