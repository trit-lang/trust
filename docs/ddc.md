# Diverse Double-Compiling

What `bootstrap/` is *for*, stated once so that the work it needs is work
someone can plan.

Diverse Double-Compiling is David A. Wheeler's technique (2005, completed in
his 2009 dissertation), and it is the only practical answer anyone has to the
attack Ken Thompson described in *Reflections on Trusting Trust*: a compiler
binary that inserts a backdoor into what it compiles, and inserts the
inserter into any compiler it compiles, so that the source of both the
program and the compiler is clean and the backdoor survives anyway. Reading
the source cannot find it. Rebuilding the compiler from that source with
*itself* cannot find it, because that is exactly the step the attack rides.

---

## 1. What the technique is

Write `cA` for the compiler binary under suspicion, `sA` for the source it
claims to be, and `cP` for some **other** compiler for the same language —
the *parent*, which is trusted for this argument and need not be trusted for
anything else.

```
stage1 = cP(sA)          # sA compiled by the parent
stage2 = stage1(sA)      # sA compiled by that
```

`stage2` is a compiler built entirely from `sA` by something that is not
`cA`. If `stage2` and `cA` are **bit-for-bit identical**, then whatever `cA`
does, `sA` says it. A backdoor in `cA` that is not in `sA` would have to
survive a path it never touched, which it cannot.

The result is *corroboration*, not proof, and it is relative to `cP`: if the
parent is compromised in exactly the same way, the comparison passes and says
nothing. That is why the P stands for a compiler chosen to be **diverse** —
different implementation, different author, ideally a different lineage.

---

## 2. Why this repository is unusually well placed

The thing DDC needs and almost nobody has is a *second implementation of the
same language that agrees about everything*. This repository is building one
for a different reason — the differential invariant, which has found every
bug in `docs/spec-gaps.md` from G9.43 onward — and the two are the same
artifact:

| DDC wants | this repository has |
|---|---|
| `sA`, the compiler's source in its own language | `bootstrap/`, growing toward it |
| `cP`, a diverse compiler for that language | `trustc`, written in Rust |
| the two to agree about the language | `scripts/bootstrap.sh`, question by question |
| output that is a function of input | `scripts/reproducible.sh` |

The differential invariant is **not** DDC and does not replace it. It asks
whether two *sources* agree; DDC asks whether a *binary* is what its source
says. They are complementary, and this repository is one of the few places
where the second is reachable at all, because the first was built first.

---

## 3. What must be true before it can be run

These are requirements on the compiler, not on the ceremony, and each is a
piece of work.

### 3.1 The compiler is a function of its input — **holds**

Two runs must give the same bytes. `scripts/reproducible.sh` checks 32
compilations, each twice, in **separate processes** — which is the point,
because Rust seeds every `HashMap` differently per process, so iteration
order reaching the output shows up there as a difference. Nothing does
today.

What could break it later, and so is worth naming: a timestamp or a path in
the output, a name derived from a memory address, a parallel pass that emits
in completion order, and any iteration over a hash whose result is written
out rather than looked up.

### 3.2 `bootstrap/` compiles Trust — **in progress**

It reads the language (lexer, parser, Ch. 6's three passes), it answers
Ch. 2 and Ch. 4's checking, and it **lowers to TIR** — every pass compared
against `trustc` character for character, and the lowering compared
including the names, since equality is on the text.

What it lowers is the whole of the language it *reads*: generics —
functions, methods and types alike — aggregates, enums and their variants,
`self` by value, and the drops Ch. 3 §1.4 puts at the end of a scope
(§4 step 3).

**It reads the library now.** All 28 173 characters of the prelude, 57
items, parsed into the same tree by both implementations — associated
types, `Fn(A) -> R` bounds, `impl Fn` parameters, tuple and array types,
array literals, and a call whose callee is not a name. Getting there fixed
five things on the *`trustc`* side, not the Trust one (G9.62, G9.63): four
tree forms that printed as Rust debug output with spans in them, which no
second implementation could have reproduced, and an `impl … for &str` that
printed without its `&`.

And it **compiles part of it**: `bootstrap/programs/library/main.tr` uses
the handed-over `Option`, its variants and its generic methods, and its TIR
is the same characters as `trustc`'s. Every other pass agrees about the
whole library too — the names it defines, what each `use` reaches, the
layout of every type in it, and which of its functions type-check.

An `if` or a `match` whose value is an aggregate builds each arm into the
storage the caller gave now (G9.65) — where a value is going is known
before it is computed, so there is nothing to join. That is the shape half
the library is written in.

**Traits are done as far as static dispatch goes.** A bound needed nothing
at all: `fn sum<T: Area>(x: T) { x.area() }` knows the type by the time the
body is read, because monomorphization made it known, so `x.area()` is that
type's `area` by the same lookup any method call uses — the bound is the
checker's business. What needed building is a trait method *with a body*,
which is one function per implementing type named after the type
(`Re.twice`), and type arguments that are aggregates (`@sum.Sq`).

Closures are done, captures and all. What is left is `dyn Trait` and its
vtables (Ch. 4 §3), and Ch. 5's `Vec` and `String` — and those two are a
different kind of thing.

**`Vec` is the compiler's code, not the language's.** Its methods are
intrinsics: `trustc` expands `push` into hand-written TIR, and no chapter
says what that TIR is. A second implementation cannot derive it, only copy
it, which is exactly §3.4's warning. The decision is to **move them into the
library** — Trust source in the prelude, with `alloc` and `free` left as the
compiler's, because those are the target's and not the language's
(Ch. 5 §2.1). The prerequisite is a language one: Trust cannot name a raw
pointer, which is why `Box` is the compiler's today. So the order is decide
how a pointer is written, then move the library, then this comparison can
reach the whole of it (G9.73).

Until then, what a program may use of the library is what the Trust side
lowers, and what it may not is refused rather than lowered wrongly — which
is the same rule the rest of this file is about.

### 3.3 The comparison has something to compare — **not started**

`stage2` and `cA` must be *the same kind of thing*. The natural artifact here
is the **TIR module**, which has a canonical textual form (`trustc fmt`) and
is the compiler's real output; the assembly and the image are downstream of
it and are compared by the pipeline tests already. So the DDC comparison is:

```
stage1.tir = trustc tir bootstrap/main.tr          # cP(sA)
stage2.tir = stage1 build bootstrap/main.tr        # stage1(sA)
```

and `stage2.tir` must equal `stage1.tir` byte for byte. That equality is the
**fixpoint**: a self-hosting compiler compiled by itself reproduces itself.

### 3.4 The parent must be diverse — **partly, and honestly not**

`trustc` is Rust and `bootstrap/` is Trust: different languages, different
authors of the *runtime* beneath them. But both were written here, by the
same hands, from the same specification. A backdoor placed in both by
whoever wrote them would survive DDC exactly as Thompson's survives a
rebuild.

What this repository can honestly claim once §3.3 holds is the weaker and
still worthwhile statement: **the Trust compiler's binary is what its Trust
source says, given `trustc`**. Making the claim stronger needs a `cP` this
project did not write — a third implementation, by someone else, from
`spec/`. That the specification is the authority, and is complete enough to
implement from, is what would make such a thing possible; it is one of the
reasons `spec/` is written the way it is.

### 3.5 The environment must be pinned

`cP` must be built from a known compiler, on a known libc, with known flags.
Today `trustc` needs a Rust toolchain and nothing else — zero external
crates, which is a decision made for other reasons and pays here too. The
remaining variable is `rustc` itself, and the honest statement is that the
root of trust is `rustc`'s, not ours. DDC pushes the question up the chain;
it does not end it.

---

## 4. The plan

Each step is checkable on its own, and none of them is only for DDC.

1. **Keep §3.1 true.** `scripts/reproducible.sh` runs with the rest.
   *Done, and now guarded.*
2. **Finish the checker** (Ch. 4 in `bootstrap/`): the types of expressions,
   then whether they agree — the first place the Trust side must say *no*.
   *Begun: `trust types` and `trust agree` are both compared, the second by
   rule rather than by wording or position, since `bootstrap/`'s tree
   carries no spans.*
3. **Lower to TIR** in Trust, compared against `trustc tir` the way every
   other pass is compared: same input, same module, character for character.
   This is the step that makes `stage1` exist.

   What that comparison is, exactly, now that the rest is in place:

   ```
   $ trustc tir tiny.tr
   tir 0.1 target "tritium"

   fn @add(%a: t27, %b: t27) -> t27 {
   ^entry:
       %a.slot.1 = slot tryte[3]
       %b.slot.2 = slot tryte[3]
       store t27 %a, %a.slot.1
       store t27 %b, %b.slot.2
       %v.3 = load t27 %a.slot.1
       %v.4 = load t27 %b.slot.2
       %a.5 = add.trap t27 %v.3, %v.4
       ret %a.5
   }
   ```

   The second implementation has to reproduce **the names too** — `%v.3`,
   `%a.5` — because equality is on the text. That is a stricter demand than
   it looks and it is the right one: the counter is per function and
   deterministic, so two implementations that agree about the *order* of
   what they emit agree about the names for free, and two that do not are
   two that emit different code. A comparison that normalized the names away
   would be a comparison that could not see the difference.

   The order to build it in is the order this file's corpus grew: scalars
   and arithmetic first, then calls, then blocks and `br3`, then aggregates,
   then the drops Ch. 3 §1.4 puts at the end of a scope.

   *Begun.* `bootstrap/lower.tr` emits parameters, a `let` of a scalar,
   Ch. 1's arithmetic, all six comparisons and `<=>`, calls, `return`, a
   tail, `if`/`else` and `while` — and `scripts/bootstrap.sh` compares it
   against `trustc tir` character for character, names included. A function
   it does not lower yet is left out rather than lowered wrongly, which is
   what keeps the comparison honest while the slice is still small.

   What is left is aggregates, `match`, and the drops Ch. 3 §1.4 puts at the
   end of a scope, and the first of those is bigger than it looks. This is
   what one costs:

   ```
   struct P { x: t27, y: t9 }
   fn make(a: t27) -> P { P { x: a, y: 1 } }
   fn read(p: P) -> t27 { p.x }
   ```
   ```
   fn @make(%sret: ptr, %a: t27) {          ← the answer is a parameter
       …
       store t27 %v.2, %sret
       %p.3 = offset %sret, const t27 3     ← Ch. 2's offsets, in the TIR
       store t9 const t9 1, %p.3
       ret
   }
   fn @read(%p: ptr) -> t27 {               ← and so is an aggregate argument
       %p.slot.1 = slot tryte[6]
       …                                     ← copied in, field by field
   }
   ```

   Three things arrive at once: a function whose answer is an aggregate takes
   the storage for it as a parameter (`sret`); an aggregate parameter is a
   `ptr` and is copied into a slot of its own on entry, field by field; and
   every field offset is Ch. 2's, which `bootstrap/sizes.tr` already computes
   and agrees about — so that half is done and this is where it gets used.

   There is a fourth, and it is only visible in the counter. In `fn both(a)
   { let p = P { … }; … }` the numbers run 1, 3, 4, 5 and **2 is missing**:
   the aggregate was built in a temporary and the binding *renamed* that
   temporary rather than copying out of it, which is what makes `let p = P {
   … }` cost nothing. A second implementation has to make the same
   temporary, and skip the same number.

   *All four are done*, and so are `match` and the drops. What the Trust
   lowering emits now is the whole of the language it reads, minus generics:
   scalars and arithmetic, all six comparisons, calls, `if`, `while`,
   aggregates by value and by pointer, `match` with its payload bindings,
   and the destructor calls Ch. 3 §1.4 puts at the end of a scope — with the
   `impl Drop` body emitted as the `@drop.T` it becomes.

   *Modules are done too.* `bootstrap/program.tr` reads a bundle, runs Ch. 6
   §4's three passes, lowers the flat list they produce, and cuts it down to
   what `main` reaches — a function nothing calls is a function the program
   does not contain, and the two implementations agree about which those are.

   The prelude is **handed over**, not copied: `trust bundle … --prelude`
   sends it as a section under `#prelude`, a name no module path can be, and
   `bootstrap/program.tr` merges it as Ch. 6 §3.3 says — the prelude's items
   first, and an item the program defines replaces the one of that name.
   That is one copy of the library's source rather than two, and the price
   is that this compiler is handed the library it compiles against for the
   same reason it is handed the files.

   The operators are all there now, and so are `impl` methods: a method is
   a function named after the type it is written on, its `&self` is an
   address rather than a copy, and a call passes the receiver's storage as
   the first argument. `Drop` is the one impl whose method is named after
   the *type* instead.

   **Generics are done** — §2.5's monomorphization, which is what the last
   of the ordinary language needed. The shape, written down before it was
   written and unchanged by writing it:

   ```
   fn id<T>(x: T) -> T { x }
   fn main() -> t27 { let a = id(1); let b = id(0t); … }
   ```
   ```
   %r.1 = call @id.t27(const t27 1) -> t27
   %r.3 = call @id.trit(const t1 0) -> t1
   …
   fn @id.trit(%x: t1) -> t1 { … }     ← the instantiations, after `main`
   fn @id.t27(%x: t27) -> t27 { … }    ← and in the order they were queued
   ```

   The five pieces, as they were built:

   - a `Family` per generic function — its type parameters, its parameters'
     written types, and its own — so a call can work out which instantiation
     it means;
   - a `Job` queue on the emitter: the function's name and the written types
     its parameters turned out to be. It is **append-only**, and what
     deduplicates is that a key already asked for is never asked for again;
   - inference at the call: match each declared parameter type against the
     argument's written type, which is the same matching the checker already
     does for a variant's payload;
   - `function()` taking a **key** — the type arguments in declaration order
     — which it substitutes for the type parameters as it reads the
     signature *and the body*, and appends to the name;
   - `lower::instantiations()` draining the queue after the ordinary
     functions and the impls, which is where the instantiations go.

   Two things about the draining turned out to be part of what agrees, and
   neither is visible in a design that only says "drain the queue".

   The queue is a **stack**: the instantiation emitted next is the one asked
   for most recently, so the instantiations come out in the reverse of the
   order the calls asked for them, and one a generic body asks for while it
   is being emitted comes out ahead of everything that was already waiting.
   And the deduplication is at the moment a key is *asked for*, not at the
   moment it is emitted — those two give different orders as soon as two
   functions ask for the same two keys in opposite orders, and only the
   first is the other implementation's.

   Four more, all of them the same rule about names: an instantiation is
   named `id.trit` by Ch. 6 §4, and the argument is the *written* type
   (`trit`, not `t1`) — which is why the emitter had to stop calling a `bool`
   and a `trit` both `t1` before any of this could be right; the key is the
   argument types and a body is emitted once per distinct key; a name across
   a module is the path, so `util::id` instantiates as `@util.id.t27`; and
   the type a call instantiates at is what Ch. 4 §2.3 infers from the
   arguments.

   **What is refused**, and each is refused rather than emitted wrongly:

   - a type parameter the parameters do not name. `fn narrow<T>(x: t27) -> T`
     is chosen by what the *binding* wants, and this infers from arguments
     only, so a call to one is left out;
   - a `const` parameter (§2.4). A key here is type arguments and nothing
     else, and a family whose members are told apart by a value needs a key
     that can say so, so such a family is not one this instantiates at all;
   - a type argument TIR has no spelling for — an aggregate — because an
     aggregate is passed as a pointer and answered through an `sret`, and a
     key that stood for one would name a function with a different shape;
   - two arguments written where one type parameter was named that are not
     the same type — `max(1, 0t)`. There is no key that is the right one,
     and Ch. 4 §2.3 is what refuses it; `trustc` refuses it in the checker,
     and this refuses it where it would otherwise have picked the first and
     emitted a call whose second argument is the wrong width;
   - a generic type as a key, for the same reason an aggregate is not one.

   Generic **methods** are done and are the same machinery: a family under
   `C.with`, whose receiver is an argument the call did not write and which
   therefore infers nothing. That one is worth naming because it was found
   the way G9.55 and G9.56 were — by writing the first program that uses it
   before writing the feature — and it failed the same way: not by
   refusing, but by emitting `call @C.plus(…)` under a name nothing
   defines.

   **Generic types are done too**, and they were cheaper than the functions
   because an instantiation shows in no name: `fn make(x) -> Pair<t27>` is
   `@make`, and `Pair.t27` is a size and a set of offsets Ch. 2 already
   decides — which is why neither implementation lays out what is *written*
   for a generic type, and `bootstrap/layouts/05.tr` holds them to that. A
   name is needed in one place only: a method of an `impl<T> Pair<T>` is
   `@Pair.first.t27`, because the impl's parameter is the first of the
   method's key and is read off whatever the receiver turned out to be.
   `Holder.Pair.t27` cannot be read back into a head and its arguments, so
   what made each mangled name is remembered rather than parsed.

   Three things had to come with them, all older than they were and all
   found by writing the program before the feature (G9.60): an aggregate
   temporary was named `%agg.N` where `trustc` names it `%tmp.slot.N`; a
   field that is itself an aggregate emitted `store ?` instead of a
   field-by-field copy; and `bootstrap/build.tr` did not cut a file down to
   what `main` reaches while `bootstrap/program.tr` did. Both drivers call
   one `lower::module` now, which makes the class of mistake impossible
   rather than the instance of it.

   **Enum construction** is done as well — the payload where the layout puts
   that variant's fields, then the tag — and with it `self` by value, which
   is what an `unwrap` is: an aggregate parameter whose written type is
   `Self`, and inside an instantiation `Self` *is* the instantiation. A
   variant with no payload says nothing about its arguments, so `Opt::None`
   is told which member it is by the binding it is going into, which is the
   one direction of Ch. 4 §2.3 this follows.

   A **niche-encoded** enum is refused: it says which variant it is by
   writing a value the payload could not have had (§6), and working out
   which value that is is not done here. The side that reads a tag refuses
   a niche too, so the two refuse together — which is why
   `bootstrap/lowered/12.tr` has an `Opt<t27>` and an `Opt<t9>` and not an
   `Opt<trit>`.

   What stands between here and the prelude is now the **parser**:
   `Option<Self::Item>` in `trait Iterator` stops it at the third
   declaration in the file. An associated type is a Ch. 4 §1.7 feature the
   Trust side has not read yet, and until it can, the library it is handed
   is a library it cannot parse.

   The limit that was stated in advance — that substituting the signature is
   not enough for a body that names its own type parameters — was met by
   substituting in the body too: the emitter carries the key, so `let y: T`
   and `x as T` are read under it. What is left of that limit is the list
   above, and every item on it is a refusal.
4. **Run the double compile.** `scripts/ddc.sh`: build `stage1` with
   `trustc`, build `stage2` with `stage1`, demand `stage2 == stage1`. Report
   the two hashes whether or not they match, because a number that is only
   printed when it is right is a number nobody checks.
5. **Write down what it proves.** A `DDC` section in `README.md` stating the
   claim, its parent, and its limits — including §3.4. A corroboration
   presented as a proof is worse than none.

---

## 5. What this changes about the work already planned

Nothing is dropped, but three things acquire a second reason:

- **`bootstrap/` must compile, not only read.** Reading Trust makes it a
  second opinion; compiling Trust makes it `sA`. The differential invariant
  is satisfied by the first, and DDC needs the second.
- **Canonical output matters more than it did.** `trustc fmt`'s canonical
  form was for reading diffs; it becomes the thing equality is *defined* on.
  Anything that makes two equivalent modules print differently — a name from
  a counter that depends on visit order, a set printed unsorted — is now a
  correctness bug rather than an untidiness.
- **The specification is the artifact that lets someone else write `cP`.**
  Every gap recorded in `docs/spec-gaps.md` is a place where a third
  implementation would have to guess, and a guess is a divergence that would
  read as a failed DDC. The gap file was for honesty; it is now also the
  work list for making the language implementable by a stranger.
