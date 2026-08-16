# Trust

The **Trust** programming language — a memory-safe systems language for
balanced ternary machines — with its compiler `trustc` and its intermediate
representation TIR.

This repository is at Draft 0.1. The specification in [`spec/`](spec/) is the
authority; the code implements it. Where the spec is silent, the decision made
is recorded in [`docs/spec-gaps.md`](docs/spec-gaps.md) rather than buried in
the source.

[`docs/status.md`](docs/status.md) is the handoff: what exists, what it can
and cannot do, the architecture decisions you need before changing anything,
and where to go next.

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

## A program that runs today

[`examples/trust/demo.tr`](examples/trust/demo.tr) exercises every feature the
compiler has, and its exact output is a test. Build and run it:

```
trustc compile examples/trust/demo.tr @main > demo.t27
cat examples/trisc/runtime.t27 >> demo.t27
tritium asm demo.t27 -o demo.timg
tritium run demo.timg
```

```
0t1 0t10 0t100 0t1000 0t10000 0t100000
0tTT1 -11 0t1101001 1000
C=48 R=42 90
3 6 9 12
0.4 0.11 1.2 1.9
42
[2]
[3][1]
```

The first two lines are the point of the language. Writing a number in
balanced ternary is `n % 3` and `n / 3` and nothing else, because
round-to-nearest division leaves the remainder in −1…1 every time; the
decimal printer in the same file needs a correction step inside its loop that
the ternary one has no use for.

## Building

```
cargo test          # 558 tests
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
tritium profile <image>                    run it and report what ran
```

The whole pipeline runs, from Trust source to the machine:

```
$ cat examples/trust/hello.tr
fn putchar(c: t9);          // declared here, defined outside the language

fn print(s: &str) {
    let mut i: taddr = 0;
    while i < s.len() {
        let c = s[i] as t27;
        ...                                     // UTF-8, at the boundary
        i += 1;
    }
}

fn main() -> t27 {
    print("Hello, 世界! 🙂\n");
    0
}

$ trustc compile examples/trust/hello.tr > hello.t27
$ cat examples/trisc/runtime.t27 >> hello.t27   # "linking" is concatenation
$ tritium asm hello.t27 -o hello.timg
$ tritium run hello.timg
Hello, 世界! 🙂
```

Text is **one character per word, fixed width** (Ch. 5 §1.1), so `s.len()`
counts characters and `s[i]` is one — both O(1), and neither is available in a
language whose strings are UTF-8. Against a UTF-8 carrier that costs 3× for
ASCII, 1.5× for Greek, exactly the same for CJK, and *less* above the BMP.
UTF-8 is what the boundary converts to, and `print` above is the boundary.

There is a `println` and it is a **function**, not a macro. Macros exist
(Ch. 7) and a variadic one can print a sequence — but `println!("{} {}", a, b)`
needs a *format string* taken apart at compile time, which Ch. 5 §7 reserves
and Ch. 7 §7 does not unreserve. `putchar` is *declared* in Trust and
*defined* in assembly, since TIR §5 has no integer-to-pointer cast and so
cannot name a device address at all.

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
vm/          crate tritium: the reference machine and its assembler
driver/      crate trust: compile a program and run it, in one command
lsp/         crate trust-lsp: diagnostics for an editor, over stdio
editors/     a tree-sitter grammar, and the Zed extension that loads it
bootstrap/   the compiler, in Trust — the lexer so far, held to the Rust one
targets/     target descriptions (TIR §7)
examples/    TIR modules and Trust programs that run
docs/        notes about the spec, including every derived decision
```

## Licence

**GNU Affero General Public License, version 3** — see [LICENSE](LICENSE).

The AGPL's distinguishing clause is §13: offering a modified version to users
over a network counts as conveying it, so those users are owed the source. A
compiler is a thing people run behind services, and that is the case this
licence is chosen for.
