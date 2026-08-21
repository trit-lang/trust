# 001 — `Ty::Param` does not exist, and two limits are the same wall

| | |
|---|---|
| **Status** | Half closed. A generic body is now read once against its bounds; `Sized` is still not a bound. |
| **Blocks** | Ch. 4 §2.5 (`Sized` / `?Sized`) |
| **Closed** | Ch. 4 §2.2 (a generic body checked once), for the shapes §"What the read catches" lists |
| **Contradicts** | Ch. 4 Appendix B — the scorecard claims the C++ template failure mode is removed by construction. It is removed at the call site, and now for a named method in the body. Not by construction. |
| **Tests** | `a_generic_body_is_read_once_against_its_bounds`, `known_limit_reading_a_generic_body_is_fail_open`, `known_limit_there_is_no_sized_bound` (all `compiler/tests/frontend.rs`) |

## The decision

`compiler/src/lang/lower.rs` defined `Ty` as concrete types only — `Trit`,
`Bool`, `T9`, `T27`, `TAddr`, `Char`, `Unit`, `Array`, `Tuple`, `Struct`,
`Enum`, `Ref`, `Boxed`, `RawOf`, `Slice`, `Dyn`, `Never`. There was no `Param`.

A type parameter was therefore not a type. It was a key into an environment:

```rust
// A type parameter in scope.
other if env.contains_key(other) => Ok(env[other].clone()),
```

`§7` states the consequence approvingly, and the approval is earned: no AST is
ever rewritten for generics, a generic body is lowered by reading the same
source under a different `HashMap<String, Ty>`, and the layout engine, the drop
machinery, the borrow checker and codegen never learn that generics exist.
Monomorphization was cheap because of this, and `bootstrap/lower.tr` could
reproduce it because of this.

**This was a deliberate choice, and it reproduces C++'s template model.** That
it does so was not recognised when it was made. The two limits below are the
two faces of that one decision.

## What it costs, on one side: a body is never checked

*(Fixed. Kept because the shape of the fix follows from the shape of the cost.)*

To check a generic body the compiler must resolve the types written in it. To
resolve `T` it needs an environment. An environment was produced by `unify`
matching declared parameter types against actual argument types **at a call
site**. No call, no environment, and the body was not visited at all.

```rust
trait Area { fn area(&self) -> t27; }
fn never_called<T: Area>(x: &T) -> t27 { x.no_such_method() }
fn main() -> t27 { 0 }
```

This used to compile. It is now rejected.

## What it costs, on the other: there is no `Sized` bound

*(Still open.)*

A `Sized` bound is a predicate attached to the parameter and discharged once,
where the parameter is bound. `Ty::Param` now exists, but nothing attaches
`Sized` to it and there is no `?Sized` to remove it: a parameter still behaves
as `?Sized`.

The implementation checks the size at each **use** — `check_sized` — over
parameters, locals, fields, and reads through a reference.

The observable difference is that this is *more permissive* than Rust:

```rust
fn twice<S: Shape>(s: &S) -> t27 { s.area() * 2 }
let d: &dyn Shape = &c;
twice(d)                    // Rust rejects this; here it answers 20
```

which is correct — `S = dyn Shape` never needs its own size, only the
reference's.

**It is sound only while the list of use sites is exhaustive**, and that list
is the kind that grows quietly. Any new construct that needs a size and does
not route through `check_sized` breaks the soundness argument without breaking
a test. Note that the read pass below does *not* weaken this: the read answers
one word for a parameter's size and throws the answer away, and every size
question that matters is asked again at instantiation.

## Why they were one wall

Both need the same capability: represent *some type known only to implement
`Shape`*, and resolve `s.area()` from the bound alone. The first half of that
now exists; `Sized` needs the second half of the same machinery to be pointed
at a bound the parameter carries implicitly rather than one it was written
with.

## What was wrong about "why it is not a small change"

The old §"Why it is not a small change" listed four components — layout, drops,
the borrow checker, codegen — and §"What would close this" item 3 guessed that
proving a `Ty::Param` cannot reach them "is likely right for layout, drops, the
borrow checker and codegen, since checking is what happens *before*
instantiation and those four run after".

**That guess is false, and it was measured false.** Trust has no check phase
separate from lowering: `lower::function` type-checks and emits TIR in one
walk. Reading a body therefore *is* running it through layout and codegen. Of
178 generic bodies in the corpus, 101 reached layout or `tir()` on the first
attempt. There is nowhere to stand that is before instantiation and after
type-checking, because those are the same pass.

So the decision went the other way for two of the four:

| Component | What it does with a `Ty::Param` |
|---|---|
| layout | Answers **one word** (`TAddr`). Wrong for any real type, and never used: the read's layouts go into a `Function` that is dropped. |
| codegen (`Ty::tir`) | Answers **one word** (`Int(27)`), for the same reason. |
| drop machinery | Never reached — a read emits drop calls into TIR that is discarded, and no `@drop.T` is queued for a parameter. |
| borrow checker | Runs on the discarded TIR. It cannot ask whether `T` is `Copy` from a bound, so what it concludes about a parameter is not reported. |

The safety of the one-word answer is not a proof about reachability. It is that
**the read only ever adds rejections**: a body that fails the read for any
reason other than a deliberate `verdict` is passed silently, and every
size-dependent check runs again, for real, at instantiation.

## What the read catches

`check_generic_bodies` (`lower.rs`) lowers each generic body once under an
environment binding every parameter to `Ty::Param(name)`, and reports only what
`Fn::reject` deliberately recorded:

- a method called on a parameter that no bound of that parameter declares,
- a method called with the wrong number of arguments,
- a method called on a parameter with no `self`.

Everything else the read stumbles on sets `unsure`, and an unsure read reports
nothing at all.

## What the read still walks past

Measured over the corpus after the change: 91 of 153 bodies read cleanly, 55
failed the read and were therefore discarded, 7 were unsure. The 55 group as:

| Count | Shape |
|---|---|
| 25 | an associated type projected through a parameter (`T::Item`) |
| 15 | an `Fn`-bounded parameter called as a function (`p(x)`, `f(x)`) |
| 9 | an associated *function* reached through a bound (`C::new()` in `collect`) |
| 5 | inference failing without a call site to unify against |
| 1 | `Range<T>` — see below |

Each is a body that is not being read, so `never_called`-shaped bugs still hide
in bodies of those shapes. Closing them is four separate pieces of work, and
none of them is `Sized`.

There is also a hole that is not the read's: a program may declare a trait the
prelude also declares (Ch. 6 §3.3), and `mod.rs`'s `merged` drops the prelude's
*item* but keeps prelude items whose **bounds** name it — so prelude
`HashMap<K: Key>` ends up bounded by the user's `Key`. The read treats a bound
whose trait comes from a lower file id than the body as unanswerable rather
than resolving against the wrong trait. Recorded in `docs/spec-gaps.md`.

## A finding the read produced and cannot report

`compiler/src/lang/mod.rs`'s `impl<T> Iterator for Range<T>` does `self.start
+= 1` with a `t27` literal. `Range<T>` therefore compiles only for `T = t27`,
which is exactly the C++ template failure mode Appendix B claims is removed —
sitting in the prelude, accepted only because nobody looked. The read looks,
and finds it.

It is not reported, because reporting it breaks the prelude. Closing it needs
either a numeric bound in the language (so `Range<T: Num>` can be written and
checked) or a prelude that says `Range<t27>`. That is a language decision, not
a compiler fix.

## What would close this

1. ~~`Ty::Param(String)` carrying its bounds, or an index into a table of
   them.~~ Done — bounds live in `Check::bounds`, keyed by parameter name, for
   the duration of one read.
2. ~~Method resolution from a bound rather than from a concrete type.~~ Done
   for methods (`Fn::param_method`). **Not done for associated items** — types
   or functions — which is 34 of the 55 unread bodies.
3. ~~A decision, per downstream component, between handling a `Ty::Param` and
   proving it cannot arrive.~~ Done, and the answer was neither: layout and
   codegen answer one word into output that is thrown away.
4. `Sized` as an ordinary bound, implicit on every parameter, removable with
   `?Sized`, with `check_sized` kept as the enforcement for the `?Sized` case.
   **Untouched.**
5. Ch. 4 Appendix B's scorecard row re-earned, or amended to say what is
   actually removed. **Untouched** — and the `Range<T>` finding says the honest
   amendment is the shorter path.
