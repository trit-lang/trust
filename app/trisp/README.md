# trisp — a Scheme for the balanced ternary machine

trisp is a complete Scheme interpreter written in Trust, for the machine
this repository emulates: balanced ternary, 27 trits to a word. It is a
practical R5RS core — not a toy — with a CEK evaluator, proper tail calls,
first-class continuations, quasiquote with splicing, vectors, strings, and
a mark-sweep garbage collector whose census is itself a primitive of the
language. Plus two radices no other Scheme has: balanced ternary `0t1T0`
and heptavintimal `0h0J`, because a Scheme on a ternary machine should read
and write the machine's own numbers.

## Run it

```
trust run app/trisp/main.tr
```

A REPL on the one stream this machine has: forms from standard input,
answers one per line, unspecific results `(define …)`, `(set! …)`, `(when …)`
silenced. An error prints `error: <message>` and the machine carries on — a
typo ends a form, not the session. End of input exits 0; `(exit n)` exits
with code `n`.

## The dialect

- **Numbers** are exact t27 integers (range ±3 812 798 742 493), checked:
  overflow is an error, never a halt. `/` insists the division be exact
  (`(/ 10 3)` is an error); `quotient`/`remainder` truncate, `modulo`
  floors. Decimal by default; radices `0t` (balanced ternary, digit `T`
  is −1), `0h` (heptavintimal, balanced −13…+13), and `#b #o #d #x`;
  `number->string` and `string->number` take a radix argument, and
  `number->trits` answers the raw trit list.
- **Core forms**: `quote quasiquote unquote unquote-splicing if define
  set! lambda begin let let* letrec` (named `let` too) `cond` (with `=>`)
  `and or when unless` — plus `eval`, `apply`, and `call/cc` (also
  spelled `call-with-current-continuation`). Single-valued throughout.
- **Data**: symbols (case-sensitive), immutable strings, characters,
  vectors, pairs, the empty list; `eq? eqv? equal?` behave as a report
  reader expects for this data.
- **Only `#f` is false.** `()` is true, `0` is true.
- **Deliberate lines**: no port objects (`display`/`write` mean stdout,
  `read` means stdin); no `defmacro`, no `do`, no `case` — named `let`
  and `cond` carry what those would; character names `#\space #\newline
  #\tab`; nesting depth fenced at 3⁹ — the reader's recursion is the only
  native-stack recursion the interpreter allows itself.
- **The library is Scheme** (`lib.tr`, loaded at boot): `map1 map for-each
  filter fold-left fold-right append reverse list-tail list-ref memq memv
  member assq assoc length iota take drop list* last list-sort caar …
  cddddr` compositions, `gcd lcm expt abs sgn even? odd? zero? positive?
  negative? min max`, `not`, `boolean?`.
- **Introspection**: `(gc)` collects; `(gc-stats)` answers
  `(pairs strings vectors continuations)` — live cells, after a full
  collection. The numbers are deterministic, and the test suite pins them.

One REPL nuance worth knowing: each top-level form is its own machine run
(Scheme's stacks are per-form), so a continuation captured in one form and
*invoked from a later form* resumes the captured form's own tail —
portable programs keep a continuation's captures and calls inside one
form, where the standard guarantees everything anyway.

## Architecture

Ten modules, each with a header that argues its one decision:

| module       | holds |
|--------------|-------|
| `err.tr`     | `SErr` — errors are values, built at the raise site |
| `sym.tr`     | the interner — one index per spelling, `eq?` is compare-ints |
| `val.tr`     | `Val` (13 Copy variants), the heap's arenas, printer, `equal?` |
| `read.tr`    | the reader — port and string sources share one cursor |
| `machine.tr` | `Machine` — heap + symbols + CEK registers; `Kont`, 15 frames |
| `gc.tr`      | mark-sweep; collections only at eval-loop boundaries |
| `eval.tr`    | the CEK trampoline — special forms, environments, quasiquote |
| `prim.tr`    | ~100 primitives; the arithmetic trio's rounding contracts |
| `lib.tr`     | the prelude, written in Scheme, loaded at every boot |
| `main.tr`    | the REPL, the test harness, and 22 transcript tests |

The load-bearing choices: a `Val` is Copy and every object is an index
into arenas the `Heap` owns, so the object graph never crosses the borrow
checker and GC roots are enumerable (registers + stack + one reader). The
continuation stack is a `Vec<Kont>` of Copy frames, so tail calls are
structural (a million turns runs in constant space) and `call/cc` is a
snapshot — capture clones the vector, resume clones it back. GC fires
only where every register is a valid root: the outermost eval loop, at a
pace of 3¹⁰ allocations, then three times live.

## Tests

```
trust test app/trisp/main.tr
```

34 tests: 22 end-to-end transcript pins (read-eval-print, errors, the GC
census, a 40 000-turn tail loop that crosses a mid-run collection) and 12
module unit tests in `sym.tr`/`val.tr`/`read.tr`. A transcript test runs
the whole program — boot, library load, the forms — and pins the output
verbatim, so the suite is an end-user contract, not a mock-up.

## Performance notes

Under the release `trust`, compiling trisp costs about ten seconds —
nearly all of it the compiler's codegen phase on a 5 000-line program;
`trust check` answers in a fifth of a second. The interpreter itself is
quick: boot + library load ≈ half a second, the eval loop ≈ 1.2 ms per
tail-call turn under the emulator, and a full collection is proportional
to the arena, paced at 3¹⁰ allocations.
