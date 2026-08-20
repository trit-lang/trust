# 001 — `Ty::Param` does not exist, and two limits are the same wall

| | |
|---|---|
| **Status** | Open |
| **Blocks** | Ch. 4 §2.2 (a generic body checked once), Ch. 4 §2.5 (`Sized` / `?Sized`) |
| **Contradicts** | Ch. 4 Appendix B — the scorecard claims the C++ template failure mode is removed by construction. It is removed at the call site only. |
| **Tests** | `known_limit_a_generic_body_is_checked_at_instantiation`, `known_limit_there_is_no_sized_bound` (both `compiler/tests/frontend.rs`) |

## The decision

`compiler/src/lang/lower.rs:43` defines `Ty` as concrete types only — `Trit`,
`Bool`, `T9`, `T27`, `TAddr`, `Char`, `Unit`, `Array`, `Tuple`, `Struct`,
`Enum`, `Ref`, `Boxed`, `RawOf`, `Slice`, `Dyn`, `Never`. There is no `Param`.

A type parameter is therefore not a type. It is a key into an environment,
resolved at `lower.rs:3973`:

```rust
// A type parameter in scope.
other if env.contains_key(other) => Ok(env[other].clone()),
```

`§7` states the consequence approvingly, and it is worth stating that the
approval is earned: no AST is ever rewritten for generics, a generic body is
lowered by reading the same source under a different `HashMap<String, Ty>`,
and the layout engine, the drop machinery, the borrow checker and codegen
never learn that generics exist. Monomorphization was cheap because of this,
and `bootstrap/lower.tr` could reproduce it because of this.

**This was a deliberate choice, and it reproduces C++'s template model.** That
it does so was not recognised when it was made. The two limits below are not
separate shortcomings; they are the two faces of that one decision.

## What it costs, on one side: a body is never checked

To check a generic body the compiler must resolve the types written in it. To
resolve `T` it needs an environment. An environment is produced by `unify`
(`lower.rs:3096`) matching declared parameter types against actual argument
types **at a call site**. No call, no environment, and the body is not visited
at all — not checked loosely, not visited.

```rust
trait Area { fn area(&self) -> t27; }
fn never_called<T: Area>(x: &T) -> t27 { x.no_such_method() }
fn main() -> t27 { 0 }
```

This compiles. `no_such_method` does not exist and nothing looks.

The bound half of Ch. 4 §2.2 does hold: a failed bound is reported at the call
site, naming the call, the parameter and the trait. It is only the body that
goes unread.

## What it costs, on the other: there is no `Sized` bound

A `Sized` bound is a predicate attached to the parameter and discharged once,
where the parameter is bound. There is no parameter-as-type to attach it to,
so there is neither an implicit `Sized` nor a `?Sized` to remove it: a
parameter behaves as `?Sized`.

The implementation does the only thing available to it and checks the size at
each **use** — `check_sized`, `lower.rs:1998` — over parameters, locals,
fields, and reads through a reference.

The observable difference is that this is *more permissive* than Rust:

```rust
fn twice<S: Shape>(s: &S) -> t27 { s.area() * 2 }
let d: &dyn Shape = &c;
twice(d)                    // Rust rejects this; here it answers 20
```

which is correct — `S = dyn Shape` never needs its own size, only the
reference's.

**It is sound only while the list of use sites is exhaustive**, and that list
is the kind that grows quietly: parameters, `let` bindings, fields and reads
through a reference today; return positions, array elements, tuple members,
closure captures and enum payloads tomorrow. Any new construct that needs a
size and does not route through `check_sized` breaks the soundness argument
without breaking a test.

## Why they are one wall

Both need the same capability: represent *some type known only to implement
`Shape`*, and resolve `s.area()` from the bound alone.

Method resolution today goes: concrete `Ty` → `Struct("C")` → `C.area`. Under
a `Ty::Param` it would have to go: the parameter's bounds → `Shape` →
`Shape::area`'s signature → check against that signature, without knowing
which impl will be selected. That is a different resolution path, and it is
the one both limits are waiting on.

## Why it is not a small change

`§7`'s claim that the absence is load-bearing is literal. Four components are
written on the assumption that any `Ty` reaching them is concrete:

| Component | The question it could not answer |
|---|---|
| layout | `size_of(T)`, and every offset past a field of type `T` |
| drop machinery | whether `T` has a destructor, and which `@drop.T` to call |
| borrow checker | whether `T` is `Copy`, and therefore whether reading it moves |
| codegen | the width. TIR spells `t1`, `t9`, `t27`, `ptr`, and has no spelling for `T` |

Each needs either a new case or a proof that a `Ty::Param` cannot arrive. A
test asserts that no generic construct reaches TIR, and that test is the
tripwire that keeps this from being done sloppily.

## Ordering constraint

**Do not attempt either limit in isolation.** Both routes add `Ty::Param` and
both then meet the same four components; doing them separately pays that cost
twice, or leaves the first half of it in place while the second is designed
against it.

## What would close this

1. `Ty::Param(String)` carrying its bounds, or an index into a table of them.
2. Method and associated-item resolution from a bound rather than from a
   concrete type.
3. A decision, per downstream component, between handling a `Ty::Param` and
   proving it cannot arrive — the second is likely right for layout, drops,
   the borrow checker and codegen, since checking is what happens *before*
   instantiation and those four run after.
4. `Sized` as an ordinary bound, implicit on every parameter, removable with
   `?Sized`, with `check_sized` kept as the enforcement for the `?Sized` case.
5. Ch. 4 Appendix B's scorecard row re-earned, or amended to say what is
   actually removed.

Steps 1–2 are the wall. 3 is where the cost is. 4 falls out. 5 is the point.
