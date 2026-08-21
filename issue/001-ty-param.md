# 001 — `Ty::Param` does not exist, and two limits are the same wall

| | |
|---|---|
| **Status** | Closed, but for the scorecard row below, which is a spec edit and not a compiler one. |
| **Blocks** | Nothing. |
| **Closed** | Ch. 4 §2.2 (a generic body checked once), for the shapes §"What the read catches" lists; Ch. 4 §2.5 (`Sized` / `?Sized`) |
| **Contradicts** | Ch. 4 Appendix B — the scorecard claims the C++ template failure mode is removed by construction. It is removed at every place a type argument is supplied (G9.143), and now for a named method in the body. Not by construction. |
| **Tests** | `a_generic_body_is_read_once_against_its_bounds`, `a_parameter_is_called_against_its_fn_bound`, `an_associated_function_is_reached_through_a_bound`, `an_associated_type_binding_says_what_a_projection_is`, `a_bound_on_an_associated_type_holds_the_impl_to_it`, `a_projection_has_the_methods_its_declaring_trait_bound_it_with`, `known_limit_reading_a_generic_body_is_fail_open`, `a_type_is_held_to_its_own_bounds_where_it_is_named`, `an_impls_own_bounds_are_checked_against_the_receiver`, `a_type_parameter_is_sized_unless_it_says_otherwise` (all `compiler/tests/frontend.rs`) |

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

*(Fixed, in G9.144. Kept for the same reason as the first.)*

A `Sized` bound is a predicate attached to the parameter and discharged once,
where the parameter is bound. The implementation instead checked the size at
each **use** — `check_sized` — over parameters, locals, fields, and reads
through a reference, so

```rust
fn twice<S: Shape>(s: &S) -> t27 { s.area() * 2 }
let d: &dyn Shape = &c;
twice(d)                    // Rust rejects this; here it answered 20
```

was accepted, which is *more permissive* than Rust and not unsound — `S = dyn
Shape` never needs its own size, only the reference's.

**It was sound only while the list of use sites was exhaustive**, and it was
not: return position, array element, tuple member and enum payload were all
outside it. That is the argument for making it a predicate rather than a list,
and that is what it is now: `check_implicit_sized` refuses an unsized argument
at every place an argument is supplied — a call, an instantiation of a type,
and a method on an impl whose parameters the receiver settles — unless the
definition wrote `?Sized`. The use-site checks were kept, so what they still
catch they catch earlier. The program above must now write `fn twice<S: Shape +
?Sized>`, and `examples/trust/demo.tr` does.

## Why they were one wall

Both need the same capability: represent *some type known only to implement
`Shape`*, and resolve a question about it from the bound alone. `Sized` was the
second half, and it was the smaller one — `check_bound_named` already answered
`"Sized" => !ty.is_unsized()`, so a bound someone wrote by hand worked before
any of this. What was missing was the bound nobody writes.

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

- a method or an associated function named through a parameter that no bound of
  that parameter declares,
- a parameter called as a function with no `Fn` bound to make it one,
- a method, an associated function or a call with the wrong number of
  arguments,
- an argument whose type is wrong on both sides' *ground* types,
- a method called on a parameter with no `self`, and an associated function
  called on a parameter that takes one.

A question the reader cannot *answer* — what a projection's bounds are, what a
trait with arguments declares — sets `unsure`, and an unsure read reports
nothing at all. A comparison it cannot *decide* is not that: an argument whose
type cannot be told apart from what the bound asks for is passed over, and the
body is read on.

"Ground" means built from scalars, references, arrays and tuples, with no
parameter and no nominal name in it. A nominal name is excluded on purpose:
`Vec<T>` is `Vec.T` under a read and `Vec.t27` under an instantiation, and
those are the same type at exactly one of them.

## What the read still walks past

Measured over the corpus: 174 of 175 bodies read cleanly, none unsure, and one
rejected — `Range<T>`, the true positive below, which is not reported.

So nothing in the corpus walks past any more, and the last question the read
could not answer — **what a projection is bound by** — is answered (G9.142).

Ch. 4 §1.7 allows an associated type bounds — `type Iter: Iterator;` — and
`parse.rs` used to read one and throw it away, so both ends were open: an impl
choosing a type that failed the bound was accepted, and a body calling a method
on `T::Iter` had nothing to resolve it against. The bounds are kept now.
`check_assoc_bounds` holds every impl's choice to them, and `Check::new` files
them under the projection's key, so `T::Iter` has exactly the methods
`Iterator` gives it. The first thing this caught was the prelude:
`IntoIterator` declared `type IntoIter;` with no bound, which is not what
§1.7's own example writes.

One level only. `T::Item::Inner` is a projection of a projection, and the key
it would need is never built, so the read stays open there — the same
fail-open as before, over a much smaller thing.

Eight groups have been closed since the read landed.

- **An `Fn`-bounded parameter called as a function** (15). `param_call` reads
  the signature out of the bound: `impl Fn(A) -> R` and `F: Fn(A) -> R` are one
  thing by then (Ch. 4 §4.3), filed under a `Fn@key` bound, and the signature
  they were written with is what the call is checked against — whether the
  callee is a name or an expression, so `(self.f)(x)` counts.
- **An associated type projected through a parameter** (25). `T::Item` is a
  `Ty::Param("T::Item")`: opaque, and the same type at every instantiation,
  which is all the body needs of it (G9.141). This one also raised the number
  of bodies the read *attempts* from 154 to 172 — a signature it could not
  resolve used to stop the read before it began.
- **An associated function reached through a bound** (9). `param_assoc` finds
  it the way `param_method` finds a method, and reads `Self` in its signature
  as the parameter.
- **A bound carrying an associated-type binding** (15). `I: Iterator<Item =
  t27>` says which type `I::Item` is, and `check_assoc_bindings` holds every
  instantiation to it, so the read believes it: the binding goes into the
  environment under the key `I::Item`, and the projection resolves to `t27`
  instead of staying opaque. A binding is not a trait *argument* and does not
  divide the methods, so the guard that turns arguments away no longer turns
  bindings away with them.
- **An associated function with type parameters of its own** (9). `fn
  from_iter<J: Iterator>(it: J) -> Self` is settled by its arguments, and a
  read has none to settle it with, so `J` stands for itself. That makes every
  argument written in terms of it undecidable, and an undecidable argument is
  now passed over rather than treated as the read breaking down — what the
  call returns is still `Self`, which is still the parameter.
- **A callee whose own parameter is settled by inference** (5). `Map::next`
  learns its `B` from the closure `F` was handed, and under a read `F` is a
  parameter with no closure behind it. So `B` stands for itself as well, and
  the bounds the call site would have checked it against are skipped rather
  than failed — a parameter implements nothing yet, and the call sites that
  emit code ask the same question with a real type in hand. The instantiation
  this queues is never lowered: the read runs after `pending` has drained.
- **An associated type the self type does not name** (7). `Map<I, F>`'s `Item`
  is what `F`'s closure returns, so `assoc_of_instantiation` settles it from
  the closure's recorded signature — and under a read `F` is a parameter with
  no closure behind it. The impl's own parameter then stands for itself: `type
  Item = B` answers `B`, which is the name the body's values already have, so
  the two agree where an invented `Map.I.F::Item` would not. The answer is
  deliberately **not cached**, since `Types::assoc` outlives the read. Four
  bodies were unsure on this and three more could not have their signatures
  written at all.
- **A method called on a projection.** `Check::new` files a trait's declared
  associated-type bounds under `T::Item`, so `param_method` resolves through
  them exactly as it does for `T` itself. This is the only one of the eight
  that also *rejects*: a trait that declared no bound has said its associated
  type has no methods, which is an answer.

`never_called`-shaped bugs now hide only in the comparisons a read body passed
over, and in shapes the corpus does not contain.

There is also a hole that is not the read's: a program may declare an item the
prelude also declares (Ch. 6 §3.3), and `mod.rs`'s `merged` drops the prelude's
*item* but keeps prelude items that **name** it. Prelude `HashMap<K: Key>` ends
up bounded by a program's `Key`; a program that names anything `Take` has
`Iterator::take`'s return type point at nothing. The read treats a bound whose
trait comes from a lower file id than the body as unanswerable rather than
resolving against the wrong trait, and that is a workaround at the point of
use, not a fix. G9.138.

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
   for methods (`Fn::param_method`), calls (`Fn::param_call`), associated
   functions (`Fn::param_assoc`, including ones with parameters of their own)
   and associated types (a projection is a type, or the type a binding pinned
   it to), and for what a projection is bound by — `Check::new` files a trait's
   declared associated-type bounds under `T::Item`, one level deep. **Not done
   for a bound with arguments**, nor for a projection of a projection, nor for
   a parameter a callee's inference would have settled. Those are what still
   make a read unsure.
3. ~~A decision, per downstream component, between handling a `Ty::Param` and
   proving it cannot arrive.~~ Done, and the answer was neither: layout and
   codegen answer one word into output that is thrown away.
4. ~~`Sized` as an ordinary bound, implicit on every parameter, removable with
   `?Sized`, with `check_sized` kept as the enforcement for the `?Sized`
   case.~~ Done, and exactly that way: `check_implicit_sized` at every place an
   argument is supplied, `?Sized` parsed as a bound name no trait can have, and
   every `check_sized` left where it was.
5. Ch. 4 Appendix B's scorecard row re-earned, or amended to say what is
   actually removed. **Untouched** — and the `Range<T>` finding says the honest
   amendment is the shorter path.
