# Trust

The **Trust** programming language — a memory-safe systems language for
balanced ternary machines — with its compiler `trustc` and its intermediate
representation TIR.

This repository is at Draft 0.1. The specification in [`spec/`](spec/) is the
authority; the code implements it. Where the spec is silent, the decision made
is recorded in [`docs/spec-gaps.md`](docs/spec-gaps.md) rather than buried in
the source.

## What exists today

| Component | State |
|---|---|
| `core/` — crate `trit-core` | **implemented**: the trit, unbounded balanced ternary, width-typed `tN`, overflow flavors, faults, literal notation |
| `compiler/` — crate `trustc` | **implemented**: TIR data structures, textual format (parser + canonical printer), verifier, reference interpreter, target descriptions, CLI |
| TIR legalization (TIR §6) | **both directions implemented** and differentially tested — promotion, and expansion into multi-part arithmetic with carry chaining. `mul` expansion is blocked on a missing TIR primitive (G6.6); `div` and the shifts are unwritten |
| `compiler/` — layout engine (Lang Ch. 2) | **implemented**: sizes, alignments, offsets, both `repr`s, discriminants, niche optimization |
| `vm/` — crate `tritium` | **implemented**: the reference TRISC-27 machine — encoder/decoder, ALU, memory, the negative-address device region, CLI |
| assembler (`.t27`) | **implemented**: two-pass, exact balanced-ternary expressions, all directives and pseudo-instructions (`tritium asm`) |
| backend (TIR → TRISC-27) | **implemented**: `trustc compile` emits assembly. No register allocator yet — every value lives in a stack slot |
| `compiler/` — Trust frontend | **implemented through Ch. 2**: lexer, parser, type checker and lowering to TIR — scalars, arrays, tuples, structs, enums with payloads and niches, both `repr`s, all control flow |
| Ch. 3 — ownership and references | **built**: `&T`/`&mut T`, deref and auto-deref, slices, bounds-checked indexing, erased lifetimes, moves through branches and loops, destructors with drop flags, and a non-lexical borrow checker. A reference may be returned under elision rule 2; regions are not inferred, so rooting is checked syntactically and written lifetimes wait for Ch. 4 (G0.5) |
| Ch. 4 — generics and traits | **specified in full, built through §2**: traits, impls, methods, supertraits, default bodies, elision rule 3, `impl Drop`; generic functions, structs and enums, bounds, `where` clauses, inference and monomorphization. `Opt<T>` written in the language keeps every niche promise Ch. 2 and Ch. 3 make about `Option`. generic impls, so `Opt<T>` has methods; `derive(Eq, Ord, Clone)` with the comparison operators wired to them — and §5.3.3's promise that a derived comparison over scalars has no branch is asserted against the emitted assembly. `dyn Trait` with real vtables, object safety and indirect dispatch. closures with capture inference and `impl Fn(…)` arguments. associated types, `for` loops, and `Option`/`Result` as language types. `::<>`, `impl !Copy` and associated constants. Generic traits and their blanket impls are specified and rejected by name (G0.7–G0.9, G0.11–G0.14) |

## Building

```
cargo test          # 294 tests
cargo build
```

The AM's own claims are a test suite: `core/tests/am.rs` turns the
consequences it draws in §1.2, the symmetry identities behind its division
tie-break, and the worked identities of its Appendix A into 21 executable
assertions.

## Using `trustc`

```
trustc check <file.tir>                    parse and verify a TIR module
trustc fmt <file.tir>                      print a module in canonical form
trustc run <file.tir> [@fn] [args…]        interpret a TIR function
trustc legalize <file.tir> [file.target]   legalize for a target (TIR §6)
trustc target <file.target>                parse and check a target description

trustc build <file.tr>                     compile Trust source to TIR
trustc compile <file.tr|.tir> [@fn]        the whole way to TRISC-27 assembly

tritium asm <file.t27> [-o <image>]        assemble source into an image
tritium run <image> [--mem N]              run a TRISC-27 image
tritium dump <image>                       disassemble an image
```

The whole pipeline runs, from Trust source to the machine:

```
$ cat examples/trust/hello.tr
fn putchar(c: t9);          // declared here, defined outside the language

const MESSAGE: [t9; 14] =
    [72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33, 10];

fn main() -> t27 {
    let mut i: taddr = 0;
    while i < 14 {
        putchar(MESSAGE[i]);
        i += 1;
    }
    0
}

$ trustc compile examples/trust/hello.tr > hello.t27
$ cat examples/trisc/runtime.t27 >> hello.t27   # "linking" is concatenation
$ tritium asm hello.t27 -o hello.timg
$ tritium run hello.timg
Hello, world!
```

There is no string literal because there is no text encoding yet (AM §5 defers
it to the library chapter), and no `println!` because there is nothing for it
to print through — `putchar` is *declared* in Trust and *defined* in assembly,
since TIR §5 has no integer-to-pointer cast and so cannot name a device
address at all.

Correctness is held to the TIR spec's own criterion. Every test in
`compiler/tests/pipeline.rs` runs a function on the TIR interpreter *and*
through legalization, code generation, assembly and the machine, and demands
the same result — faults included.

The machine runs. `examples/trisc/echo.timg` is the worked program from the
ISA's own appendix, and both of its branches are three-way:

```
$ tritium dump examples/trisc/echo.timg
       0  ld.word sp, -6(zero)          ; sp ← A, whatever this machine's A is
       3  ld.tryte a0, -1(zero)         ; the input port
       6  cmp t0, a0, zero
       9  br3 t0, 1, 3, 3               ; code unit / waiting / closed
      12  addi.wrap t1, a0, 1
      15  br3 t1, 3, -4, 2
      18  st.tryte a0, -2(zero)         ; the output port
      21  jal zero, -6
      24  halt zero

$ echo '三進位 — trits' | tritium run examples/trisc/echo.timg
三進位 — trits
```

The example from the TIR spec's own appendix, executed:

```
$ trustc run examples/tir/steps_toward.tir @steps_toward 0 -5
1  (0t000000000000000000000000001, 0hDDDDDDDDE)
```

More in [`examples/tir/`](examples/tir/): a loop with block parameters and a
three-way branch (`factorial.tir`), globals and address arithmetic
(`sum_global.tir`), and hand-written multi-part arithmetic with carry chaining
(`wide_add.tir`).

## Legalization

`targets/t27only.target` describes a machine whose only arithmetic width is
the word. Legalizing a `t9` program for it shows the whole promotion story in
one function — the load keeps its access width, the value is widened into the
legal one, and `add.trap t9` becomes a wide wrapping add plus an explicit
check that the result still fits **t9**:

```
$ trustc legalize examples/tir/sum_global.tir targets/t27only.target
    %v = load t9 %p
    %lz.w1 = widen t9 %v -> t27
    %lz.r2 = add.wrap t27 %acc, %lz.w1
    %lz.h3 = shr t27 %lz.r2, const t27 9
    %lz.c4 = cmp t27 %lz.h3, const t27 0
    br3 %lz.c4, ^lz.fault5, ^lz.cont6, ^lz.fault5
^lz.fault5:
    trap F_OVERFLOW
```

Correctness is held to the criterion the TIR spec states: a transformation is
correct iff it preserves observable AM behavior. Every legalization test runs
the same function before and after, on the same inputs, and demands the same
answer — including the same faults.

## What the implementation is careful about

The radix is not a reskin, and the parts of the design that follow from it are
the parts under test:

- **One division.** Round to nearest, ties away from zero — so `7 / 2 == 4`,
  `8 / 3 == 3`, and `8 % 3 == -1`. `>>` is exact division by a power of three
  with no tie case, so rescaling needs no rounding fixup.
- **Negation is total.** `MIN == -MAX` at every width, so there is no
  `checked_neg` to lie with and `MIN / -1` is just `MAX`.
- **Comparison is three-way and primitive.** `cmp` yields a trit; `br3` is the
  only conditional terminator; two-way branching is the degenerate case.
- **Arbitrary widths are exact.** TIR permits `tN` up to 243 trits and
  requires constant folding to be correct at every N, so the arithmetic core
  is an arbitrary-precision balanced ternary implementation, not a wrapper
  around `i128`.
- **Faults are not UB.** A defined machine halt with an `F_*` code and the
  absence of defined behavior are different things, and the interpreter
  reports them differently.

## Types and layout

`compiler/src/layout.rs` implements Language Ch. 2 — the part of the frontend
that does not need the surface syntax, because the chapter is stated in terms
of types rather than of source text. It computes sizes, alignments, field
offsets, `repr(lang)` reordering, discriminants, and niche optimization, and
it is tested against the chapter's own appendix of worked layouts plus the
three niche guarantees §6 "elevates from implementation detail to spec":

- `Option<&T>` is pointer-sized;
- `Option<bool>` and `Option<trit>` occupy **one tryte** — a tryte holds
  19 683 patterns and a `trit` uses 3, leaving 19 680 niches;
- nesting within that budget is free, so `Option<Option<trit>>` is still one
  tryte.

## Repository layout

```
spec/        the specification — the authority
core/        crate trit-core: balanced ternary arithmetic
compiler/    crate trustc: TIR, legalization, and the Ch. 2 layout engine
targets/     target descriptions (TIR §7)
examples/    TIR modules that run
docs/        notes about the spec, including every derived decision
```
