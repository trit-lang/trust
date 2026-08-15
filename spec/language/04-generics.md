# Language Specification, Chapter 4 — Generics, Traits and Closures

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), Language Ch. 0 (*Syntax*), Ch. 1 (*Types*), Ch. 2 (*Composites*), Ch. 3 (*References*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

This is the chapter the previous four kept deferring to. It defines traits,
generic parameters, trait objects and closures, and in doing so it discharges
every promise made in its name:

| Made in | Discharged in |
|---|---|
| Ch. 1 §5 — ordering traits whose core method returns `trit` | §5.3 |
| Ch. 1 §6 — `From`/`TryFrom` style conversion | §5.6 |
| Ch. 2 §8 — trait objects are dynamically sized, with a fat pointer | §3 |
| Ch. 2 appendix — `TritSlab<9>`, a type parameterized by a number | §2.4 |
| Ch. 3 §1.2 — the structural copy rule restated as a derived trait | §5.1 |
| Ch. 3 §1.4 — `fn drop(self: T)` restated as `impl Drop for T` | §5.2 |
| Ch. 3 §3.3 — the third elision rule, for `&self` | §1.4 |
| Ch. 3 §3.4 — `T: 'a` bounds, `where` clauses, `for<'a>` | §2.6 |
| Ch. 0 §5.5 — `for` loops, because iteration is a trait | §5.7 |

Three things are not inherited from Rust unchanged, and each is a place where
the absence of something — floating point, an allocator — or the presence of
the radix changes the answer:

- **There is no `PartialOrd` above `Ord`** (§5.3). Rust's ordering hierarchy
  is shaped by `NaN`; this language has no floating point, so the total order
  is the base and partial order is the rare opt-in. It costs one tryte.
- **A derived comparison is branchless** (§5.3.3), because the ordering type is
  `trit` and `sel3` combines two of them in one instruction.
- **Trait objects have half the address space as niches** (§3.5), for the same
  reason Ch. 3 §2.5 gives for ordinary references, and now for two pointers
  instead of one.

> **Design note (informative).** As in Ch. 3, the default was to inherit. A
> trait system is a large surface, its failure modes are well documented, and
> a novel variation would spend a reader's existing understanding on something
> unrelated to the radix. The three departures above are each forced: two by
> what the language does not have, one by what the machine does.

---

## 1. Traits

### 1.1 Declaring a trait

A **trait** is a named set of requirements a type may satisfy.

```
trait Shape {
    fn area(&self) -> t27;
    fn name(&self) -> &[t9];
}
```

A trait body contains **required methods** (a signature with no body),
**provided methods** (a signature with a body, §1.5), **associated types**
(§1.4) and **associated constants** (§1.4).

A required method is written exactly as Ch. 0 §3.1's bodyless function
declaration. That chapter observed the collision and accepted it: "a required
trait method will look identical when Ch. 4 arrives; that is a context a
reader and a compiler can both distinguish." The context is the enclosing
`trait` item, and there is no other.

### 1.2 Implementations

An **impl block** attaches methods to a type. There are two kinds.

```
// inherent: methods that belong to the type itself
impl Circle {
    fn unit() -> Circle { Circle { r: 1 } }
    fn diameter(&self) -> t27 { self.r * 2 }
}

// trait: the type satisfies a trait
impl Shape for Circle {
    fn area(&self) -> t27 { self.r * self.r * 3 }
    fn name(&self) -> &[t9] { &CIRCLE }
}
```

An inherent impl's type must be declared in the same file. A trait impl must
supply every required method, may override any provided method, and may supply
nothing else — a method in a trait impl that the trait does not declare is an
error, and the diagnostic says which.

`Self` inside an impl block names the type being implemented. Inside a trait
declaration it names the implementing type, whichever it turns out to be.

### 1.3 Method call syntax

`receiver.method(args)` resolves in this order:

1. inherent methods of the receiver's type;
2. methods of traits the receiver's type implements;
3. the same two, after inserting as many dereferences as needed (Ch. 3 §2.3);
4. the same two, after taking `&receiver` or `&mut receiver`.

If two traits in scope supply the same method name and both apply, the call is
ambiguous and must be written out as `Trait::method(receiver, args)`. Draft
0.1 has no imports, so "in scope" means "declared in this file"; the rule is
stated now because modules will not change it.

An inherent method always wins over a trait method of the same name. This is
Rust's rule, and it is what lets a type provide a faster specialized version of
something it also offers through a trait.

### 1.4 `self`, and the third elision rule

The first parameter of a method may be written in one of four shortened forms:

| Written | Means |
|---|---|
| `self` | `self: Self` — by value, so the receiver is moved or copied |
| `&self` | `self: &Self` |
| `&mut self` | `self: &mut Self` |
| `&'a self` | `self: &'a Self` |

A function whose first parameter is one of these is a **method**; one without
is an **associated function**, called as `Type::function(args)`.

Ch. 3 §3.3 deferred Rust's third elision rule to this chapter. It is:

> 3. If a method takes `&self` or `&mut self`, the lifetime of that borrow is
>    assigned to every elided lifetime in the return position.

Rule 3 is applied before rule 2, and where it applies rule 2 does not. So

```
impl Window {
    fn data(&self) -> &[t27] { self.data }          // borrows from `self`
    fn pick(&self, other: &[t27]) -> &[t27] { … }   // also borrows from `self`
}
```

both compile without a written lifetime, and in the second the returned slice
borrows from `self` and *not* from `other` — which is the rule's entire point,
and the reason a reader must be able to see it here rather than infer it.

Ch. 3 §1.4's `fn drop(self: Buffer)` is the long form of `fn drop(self)` in an
`impl Drop for Buffer` block. §5.2 completes that promise.

### 1.5 Default bodies

A trait method may have a body, which an implementation may but need not
override:

```
trait Shape {
    fn area(&self) -> t27;
    fn is_degenerate(&self) -> bool { self.area() == 0 }
}
```

A default body may call other methods of the same trait, including required
ones. It may not assume anything about `Self` beyond the trait's own
requirements and its supertraits.

### 1.6 Supertraits

`trait A: B` requires that every type implementing `A` also implements `B`, and
makes `B`'s methods available in `A`'s default bodies and to `A`'s users.

```
trait Ord: Eq { … }
```

Supertrait cycles are an error.

### 1.7 Associated types and constants

```
trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}

trait Bounded {
    const MIN: Self;
    const MAX: Self;
}
```

An associated type is chosen by the implementation, once per implementing
type. A trait's **type parameters** (§2.1) are chosen by the *user*, and a type
may implement such a trait many times over. That is the whole distinction, and
it is why the language has both:

```
trait From<T> { fn from(x: T) -> Self; }   // many impls per type: parameter
trait Iterator { type Item; … }            // one impl per type: associated
```

An associated type may carry bounds (`type Iter: Iterator;`) and may be
constrained at a use site by an **associated type binding**:

```
fn sum<I: Iterator<Item = t27>>(it: I) -> t27 { … }
```

### 1.8 Coherence

Two rules, so that "which impl applies" always has one answer.

- **Overlap.** Two impls of the same trait for overlapping sets of types are
  an error, whether or not any program uses both.
- **Orphan.** An impl is permitted only if the trait or the self type is
  declared in the same crate.

Draft 0.1 has one file and therefore one crate, so the orphan rule is
satisfied by everything and rejects nothing. It is written now, and normative
now, so that the arrival of modules is not also a language change.

There is no specialization: a more specific impl does not override a more
general one, it collides with it.

---

## 2. Generics

### 2.1 Parameters

Functions, structs, enums, traits and impl blocks may take parameters, written
between `<` and `>` — the list Ch. 0 §3.5 reserved and Ch. 3 §3.2 began using
for lifetimes:

```
fn largest<T: Ord>(xs: &[T]) -> &T { … }

struct Pair<T, U> { first: T, second: U }

impl<T: Ord> Pair<T, T> {
    fn min(&self) -> &T { … }
}
```

A parameter list may contain lifetime parameters (Ch. 3 §3.2), type
parameters, and const parameters (§2.4), in that order. `<'a, T>` is
well-formed, which is exactly what Ch. 3 §3.2 said it would be.

A parameter of an impl block must appear in the type being implemented or in
the trait being implemented; an unconstrained parameter is an error, because
nothing could ever determine it.

### 2.2 Bounds

A bound restricts a parameter to types implementing a trait. It may be written
inline, or in a `where` clause when the inline form crowds the signature:

```
fn show<T: Shape + Copy>(x: T) { … }

fn merge<I, J>(a: I, b: J) -> t27
where
    I: Iterator<Item = t27>,
    J: Iterator<Item = t27>,
{ … }
```

Within a generic body, a parameter has **only** what its bounds give it. A
generic function that compiles is correct for every instantiation, and an
instantiation that fails a bound is rejected at the call site, not inside the
body. This is Rust's discipline rather than C++'s, and it is inherited for the
reason Ch. 3 gives for inheriting: its failure modes are understood.

`impl Trait` may appear in **argument** position, as sugar for an anonymous
type parameter:

```
fn apply(xs: &mut [t27], f: impl Fn(t27) -> t27)      // means: <F: Fn(t27) -> t27>
```

The difference from a named parameter is that an anonymous one cannot be given
explicitly with `::<>`. `impl Trait` in **return** position is reserved: its
principal use is returning a closure, which §4.5 explains this draft cannot do
anyway.

### 2.3 Explicit instantiation

`f::<t27>(x)` gives type arguments explicitly, in declaration order. Arguments
may be omitted from the right; omitted ones are inferred. Lifetime arguments
are never required and are usually not written, since §3.1 of Ch. 3 erases
them.

### 2.4 Const parameters

Ch. 2's appendix names a library type `TritSlab<9>` — a type parameterized by
a *number*. Const parameters are therefore not optional:

```
struct TritSlab<const N: taddr> { … }

fn zeroed<T, const N: taddr>() -> [T; N] { … }
```

A const parameter's type must be one of Ch. 1's integer types or `bool`. Its
argument is a constant expression, evaluated exactly and in balanced ternary
by the same rule Ch. 0 §3.2 gives for `const` initializers.

`[T; N]` accepts a const parameter as its length, since Ch. 0 §3.5 already
requires the length to be a constant expression and a const parameter is one.

Arithmetic on const parameters in type position — `[T; N + 1]` — is
**reserved**. It requires deciding when two such expressions are the same
type, which is a research question with a poor track record, and nothing in
this draft needs it.

### 2.5 `Sized`

`Sized` is an automatic trait: a type implements it exactly when its size is
known at compile time. Ch. 2 §8's dynamically sized types — `[T]` and
`dyn Trait` — do not.

Every type parameter has an implicit `Sized` bound. `?Sized` removes it:

```
fn len_of<T: ?Sized>(x: &T) -> taddr { … }
```

A `?Sized` parameter may be used only behind a reference, which is the same
restriction Ch. 3 §5.1 already places on `[T]` and for the same reason.

`Sized` cannot be implemented or negated by hand. It is a fact about a type,
not a claim about it.

### 2.6 Lifetime bounds

Ch. 3 §3.4 deferred `T: 'a` here, having no type parameters to write it about.

`T: 'a` states that every reference inside `T` outlives `'a`. `T: 'static`
therefore means `T` contains no references at all, or only `'static` ones.
The bound is written wherever any other bound is:

```
struct Holder<'a, T: 'a> { value: &'a T }
```

In most positions it is inferred, and a written `T: 'a` is needed only where
inference has nothing to go on.

**Higher-ranked bounds** — `for<'a> F: Fn(&'a t27) -> &'a t27` — quantify over
every lifetime rather than one chosen by the caller. The only place this draft
needs them is a closure taking a reference, and there Ch. 3 §3.3's elision
already produces the higher-ranked form: `Fn(&t27) -> &t27` *means*
`for<'a> Fn(&'a t27) -> &'a t27`. The explicit syntax exists so that the
implicit form has something to be short for.

### 2.7 How generics are compiled

Generics are compiled by **monomorphization**: each distinct set of type and
const arguments produces its own copy of the code, and lifetimes — erased
before TIR by Ch. 3 §3.1 — produce nothing. TIR has no generic construct of
any kind, which is not an omission in TIR §1 but a consequence of this
sentence.

Two obligations follow, and they are normative:

- **Instantiation must terminate.** A generic function that instantiates
  itself at a strictly larger type — polymorphic recursion — produces
  infinitely many copies. An implementation must reject it rather than
  diverge, and may use a depth limit to do so, reporting the chain that
  exceeded it.
- **Identical instantiations are one function.** Two instantiations differing
  only in lifetimes are the same code (Ch. 3 §3.1 already says so); an
  implementation may also merge instantiations that lower to identical TIR.

Monomorphization is observable in exactly two ways: code size, and the fact
that a generic function's body is checked once, against its bounds, rather
than once per use. Neither is a semantic difference, which is why this section
is at the end of §2 rather than the start.

---

## 3. Trait objects

### 3.1 `dyn Trait`

`dyn Trait` is the type of *some* value implementing `Trait`, not known until
run time. It is dynamically sized (§2.5), so like `[T]` it appears only behind
a reference:

```
fn total(shapes: &[&dyn Shape]) -> t27 {
    let mut sum: t27 = 0;
    let mut i: taddr = 0;
    while i < shapes.len() { sum += shapes[i].area(); i += 1; }
    sum
}
```

`&dyn Trait` and `&mut dyn Trait` are the only forms draft 0.1 can write.
`Box<dyn Trait>` needs an allocator and arrives with the library chapter; the
representation below is already the one it will use.

A trait object carries a lifetime bound, elided to the reference's own:
`&'a dyn Shape` means `&'a (dyn Shape + 'a)`.

### 3.2 The fat pointer

Ch. 2 §8 promised this chapter would give the representation.

| Field | Offset | Type |
|---|---|---|
| data pointer | 0 | address |
| vtable pointer | 3 | address |

Size 6 trytes, alignment 3 — the same shape as Ch. 3 §5.2's slice reference,
and with the data pointer first for the same reason: every fat pointer in the
language agrees with every thin one on its first word.

### 3.3 The vtable

A vtable is a compiler-generated constant in read-only memory, one per
(type, trait) pair actually used as an object.

| Slot | Offset | Contents |
|---|---|---|
| size | 0 | `size_of` the concrete type, in trytes |
| align | 3 | its alignment |
| drop | 6 | its destructor (§5.2), or 0 if it has none |
| methods | 9, 12, … | one address per object-safe method, supertrait methods first, then the trait's own in declaration order |

A vtable's address is the address of an ordinary object, and is therefore
strictly positive (Ch. 3 §2.4). A drop slot of 0 is unambiguous because
**ISA §2.2 reserves the first word of memory**: no function's entry may be at
address 0, so zero cannot be mistaken for a destructor.

> **Note (informative).** That reservation was an open question in the ISA
> until this slot needed it. Ch. 3 §2.4 says no *value a reference can name*
> may occupy address 0, which says nothing about function addresses — and a
> vtable holds function addresses. Without the reservation a destructor that
> the layout happened to place first would be indistinguishable from having
> none. The ISA now reserves the word, at a cost of one `nop`, and the
> language's non-null invariant becomes hardware-checkable as a side effect.

The `size` and `align` slots are what a future `Box<dyn Trait>` needs in order
to free what it points at; they are specified now because adding a slot later
would change the layout of something programs will already have written.

### 3.4 Object safety

A trait may be used as `dyn Trait` only if every method is **object-safe**, or
is explicitly excluded by a `where Self: Sized` bound. A method is object-safe
when:

- it takes `self`, `&self` or `&mut self`;
- neither its parameters nor its return type mention `Self` except as that
  receiver;
- it has no type or const parameters of its own.

The reason is one fact: a trait object has erased its type, so any signature
that would need the type back cannot be called through it. `Ord::cmp` takes
`&Self` as its second argument and is therefore not object-safe — a fact worth
naming here, because it is the first one a reader will meet.

A trait with a non-object-safe method may still be used as an object if that
method carries `where Self: Sized`, which makes it callable only where the
type is known.

### 3.5 The niche budget of a trait object

Both words of §3.2 are strictly positive addresses. By Ch. 3 §2.5's argument,
each of them makes every value less than or equal to zero available as a
niche, so:

1. `Option<&dyn Trait>` is 6 trytes, not 9.
2. So is an enum with one `&dyn Trait` variant and up to (3²⁷+1)/2 − 1
   fieldless ones.
3. The compiler uses the **data** pointer's niches and leaves the vtable
   pointer's alone, so that a niche-carrying enum and a plain trait object
   agree on their second word as well as their first.

Guarantee 3 is a choice, not a consequence, and it is made in favour of
uniformity: there is no shortage of niches to economize.

---

## 4. Closures

### 4.1 Syntax

```
|x| x * 2
|x: t27| -> t27 { x * 2 }
|| 0
```

Parameter types and the return type may be omitted and are inferred from the
context the closure appears in. A closure with no parameters begins with two
`|` characters; where an expression may begin, `||` is read as two parameter
delimiters rather than as the logical-or operator of Ch. 0 §2.1. This is the
re-examination of `|` that Ch. 0 §7 anticipated, and it is the whole of it.

### 4.2 What a closure is

Each closure expression has its own anonymous type, generated by the compiler,
which is a struct holding its captures and which implements one or more of the
traits in §4.3. Two closures written differently never have the same type,
even if they look identical.

### 4.3 `Fn`, `FnMut`, `FnOnce`

```
trait FnOnce<Args> { type Output; fn call_once(self, args: Args) -> Self::Output; }
trait FnMut<Args>: FnOnce<Args> { fn call_mut(&mut self, args: Args) -> Self::Output; }
trait Fn<Args>: FnMut<Args> { fn call(&self, args: Args) -> Self::Output; }
```

These are written in the sugared form everywhere a program uses them:
`Fn(t27) -> t27`, `FnMut(&t9)`, `FnOnce() -> bool`. The unsugared spelling
above is given so that the hierarchy is visible; a program may not write it,
and the traits may not be implemented by hand in draft 0.1.

Which one a closure implements follows from how it uses its captures:

| The body… | implements |
|---|---|
| only reads its captures | `Fn`, and therefore `FnMut` and `FnOnce` |
| writes a capture | `FnMut` and `FnOnce` |
| moves a capture out | `FnOnce` only |

A plain function's name coerces to all three, so `apply(xs, double)` and
`apply(xs, |x| x * 2)` both work.

The bound is written in either of two places, and they mean the same thing:

```
fn twice(f: impl Fn(t27) -> t27, x: t27) -> t27 { f(f(x)) }   // §2.2
fn twice<F: Fn(t27) -> t27>(f: F, x: t27) -> t27 { f(f(x)) }  // the same
```

`impl Fn(…)` invents a name for the parameter and gives it this bound, so the
second form is the first with the name written down. Nothing after the
grammar can tell them apart.

**A `Fn` bound settles the parameters its signature names.** A closure has one
signature, so given the closure, the bound says what everything in it is:

```
impl<I: Iterator, B, F: Fn(I::Item) -> B> Iterator for Map<I, F> {
    type Item = B;
    …
}
```

`Map` takes two arguments and the impl has three parameters. `B` appears in no
argument of the self type; it is determined by `F`, and `F` is the closure
that was passed. This is the rule that lets an adaptor's `Item` be the
closure's result rather than a fixed type — without it, §5.7's `Map` can only
be written for one output type, which is not `Map`.

The same applies where a *method* names the parameter:

```
fn map<B, F: Fn(Self::Item) -> B>(self, f: F) -> Map<Self, F>
```

`Self` is substituted in a method's bounds as it is in its types, and the
bound is checked under the environment of the call that instantiated it,
because `B` belongs to that call.

> **Not implemented.** A method with type parameters of its own, inside a
> *generic* impl. `map` above is fine on `impl Iterator for Upto` and not on
> `impl<I, B, F> Iterator for Map<I, F>`, so a chain of one adaptor works and
> a chain of two does not: settling the impl's parameters and the method's
> needs one instantiation to happen in two stages, and instantiation is a
> single step today.

### 4.4 Capture

A closure captures each place it mentions, by the weakest form that suffices:
by shared reference if it only reads, by exclusive reference if it writes, by
value if it moves. Capture is by **place**, not by variable: a closure that
uses `p.x` borrows `p.x`, leaving `p.y` free.

Every capture is checked by Ch. 3 §4's borrow checker, unchanged. A closure
holding a shared borrow of `p` is subject to §2.2's aliasing rule for as long
as the closure lives, which is what makes the following rejected rather than
merely surprising:

```
let mut p = Point { x: 1, y: 2 };
let f = |v: t27| p.x + v;       // shared borrow of `p.x`
p.x = 9;                        // rejected while `f` is live
let a = f(1);
```

Rust's `move` keyword, which forces capture by value, is **not** provided.
Ch. 0 §1.3 reserves the word, and §4.5 explains why nothing needs it yet.

### 4.5 What draft 0.1 cannot write

A closure may be passed, called and stored in a local. It may **not** be
returned, because returning one means returning a value of an anonymous type,
which requires either `impl Trait` in return position (§2.2, reserved) or
`Box<dyn Fn…>` (needs an allocator). Both arrive together, with the library
chapter.

A closure therefore never outlives the frame that created it, and a capture by
reference is always sound without the escape analysis that would otherwise be
needed. That is why `move` is not missed: its purpose is to let a closure
outlive its captures' scope, and no closure in this draft can.

---

## 5. The language's own traits

The traits below are **lang items**: the language itself refers to them, so
they are defined here rather than in the library chapter. Their methods are
here; the conveniences built on them are not.

### 5.1 `Copy`

```
trait Copy { }
```

`Copy` has no methods. It is a claim, and the claim is Ch. 3 §1.2's:

> A type is copyable if it has no destructor and every type it contains is
> copyable.

That chapter's note promised restating this as a derived trait would change
which programs compile "in no way at all", so `Copy` is **implemented
automatically** for every type meeting the rule. It is not opted into. A Rust
programmer will notice the difference; a Trust program written against Ch. 3
will not.

> **Note (informative).** Rust makes `Copy` opt-in for a reason worth naming:
> it is an API promise. A type that becomes copyable by accident stops being
> copyable the day someone adds a field with a destructor, and every caller
> changes from copying to moving without having asked for it — a breaking
> change that fires at a distance. Rust makes the author write `Copy` down so
> that the author owns it.
>
> That hazard cannot fire in draft 0.1, which has one file and therefore no
> caller the author cannot see. It will fire once modules exist, and the
> answer then is that a *published* type's copyability is part of its
> signature and can be required to be written there — a rule that can be added
> without changing any program that predates modules. Making `Copy` opt-in
> now, on the other hand, would break every program written against Ch. 3 §1.2
> today.

Ch. 3 §1.2 also observed that draft 0.1 had no way to opt *out*, and that
opting out "needs a trait to opt out of". Here it is:

```
impl !Copy for Handle { }
```

A type with a negative `Copy` impl moves rather than copies, and so does
anything containing it. This is the only negative impl the language has, and
it exists because the alternative — making `Copy` opt-in, as Rust does — would
break the promise in the paragraph above.

`Copy` may not be implemented positively by hand: a type either meets the
structural rule or it does not, and asserting otherwise would be asserting
that a destructor need not run.

### 5.2 `Drop`

```
trait Drop { fn drop(self); }
```

`impl Drop for Buffer { fn drop(self) { … } }` is what Ch. 3 §1.4 wrote as
`fn drop(self: Buffer)`, and that chapter's rules carry over unchanged: at
most one per type, the type must be declared in the same file, the body runs
before the fields are dropped, and a field the body moved out of is not
dropped again.

The receiver is `self` by value rather than Rust's `&mut self`. Ch. 3 §1.4's
note explains why the recursion this normally causes does not occur here:
dropping the fields is not a drop of `self`, so the destructor cannot invoke
itself.

Three restrictions, all inherited:

- `drop` may not be called by hand. Dropping a value early is done by moving
  it into a function that consumes it, which the library chapter will name.
- `Drop` may not be used as a bound. `T: Drop` would be true of too little to
  be useful — a type whose *fields* have destructors does not implement `Drop`
  itself — and every use a reader expects it to have is served by knowing
  whether `T: Copy`.
- A type may not implement both `Drop` and `Copy`, which §5.1 already makes
  impossible.

### 5.3 `Eq` and `Ord`

#### 5.3.1 The two traits

```
trait Eq  { fn eq(&self, other: &Self) -> bool; }
trait Ord: Eq { fn cmp(&self, other: &Self) -> trit; }
```

`==` and `!=` call `eq`. `<=>`, `<`, `<=`, `>` and `>=` call `cmp`, the four
two-way forms being projections of it exactly as Ch. 1 §5 requires. Ch. 1 §5
counts `==` and `!=` among the projections too, which holds for every type
where both traits are implemented, by the agreement required below.

`cmp` returns `trit` and not an `Ordering` enum. Ch. 1 §5 states this and
gives the reason — "the hardware type *is* the ordering type" — and
`Ordering` correspondingly does not exist.

An implementation of both must satisfy `a.eq(b) == (a.cmp(b) == 0t)`, and
`cmp` must be a total order: antisymmetric, transitive, total. Neither is
checked. A type that breaks them produces wrong answers from correct code,
which is the same bargain Rust strikes and for the same reason — the
alternative is proof obligations the language cannot discharge.

#### 5.3.2 Partial orders

Rust splits `PartialOrd` from `Ord` for one reason: floating point, where
`NaN` compares unequal to itself. This language has no floating point, so the
split buys nothing and is not made. `Ord` is the base.

Genuinely partial orders — set inclusion, divisibility, a dependency relation
— do exist, and they get their own trait, unrelated to `Ord` by inheritance:

```
trait PartialOrd { fn partial_cmp(&self, other: &Self) -> Option<trit>; }
```

The fourth state, *incomparable*, is `None`. It is free: `trit` uses three of
a tryte's 19 683 patterns (Ch. 2 §6), so `Option<trit>` is **one tryte**, as
Ch. 2's appendix already records. A binary machine pays a whole extra byte, or
an awkward encoding, for the same four states.

No operator calls `partial_cmp`. `<` on a partially ordered type is an error
that says so, because the alternative is deciding what `<` means for
incomparable operands, and every answer to that is a lie.

#### 5.3.3 A derived comparison is branchless

`cmp` on a struct whose fields are all Ch. 1 scalars compares them
lexicographically in declaration order (§6). The combination of two field
results is *"the first one that is nonzero"*, and on TRISC-27 that is one
instruction:

```
sel3 rd, rt, rn, rz, rp     // rd ← rn / rz / rp by the sign of rt
sel3 rd, c1, c1, c2, c1     // rd ← c1 if c1 ≠ 0, else c2
```

So comparing a struct of *n* scalar fields is *n* comparisons and *n*−1
selects, with **no branch at all**, and feeding the result to `match` adds one
`br3`. This is a codegen guarantee of the same kind as Ch. 1 §5's, and it is
required for derived `cmp` on scalar fields:

> A derived `cmp` whose fields are all Ch. 1 scalar types must compile to
> straight-line code.

It is evaluated eagerly — every field is compared even when the first decides
— which is observable only in time, and is faster than the branch it replaces.
Where a field's `cmp` is a user-written call, an implementation may instead
short-circuit with a branch, since the call may be arbitrarily expensive.

### 5.4 Operator traits

```
trait Add { type Output; fn add(self, rhs: Self) -> Self::Output; }
trait Sub { type Output; fn sub(self, rhs: Self) -> Self::Output; }
trait Mul { type Output; fn mul(self, rhs: Self) -> Self::Output; }
trait Div { type Output; fn div(self, rhs: Self) -> Self::Output; }
trait Rem { type Output; fn rem(self, rhs: Self) -> Self::Output; }
trait Neg { type Output; fn neg(self) -> Self::Output; }
trait Not { type Output; fn not(self) -> Self::Output; }
trait Shl { type Output; fn shl(self, k: t27) -> Self::Output; }
trait Shr { type Output; fn shr(self, k: t27) -> Self::Output; }

trait Index<I>    { type Output; fn index(&self, i: I) -> &Self::Output; }
trait IndexMut<I>: Index<I> { fn index_mut(&mut self, i: I) -> &mut Self::Output; }
```

`a + b` where either operand is a user type means `Add::add(a, b)`, and
likewise throughout. The compound forms of Ch. 0 §2.2 (`+=`, `*=`, …) are
sugar for `a = a op b` and need no separate traits; `AddAssign` and its
relatives do not exist.

Three restrictions:

- **The overflow flavors of Ch. 1 §4 do not enter the trait system.**
  `a +.wrap b`, `a +.trap b` and `a +.flag b` apply to Ch. 1's integer types
  and to nothing else. A flavor written on a user type is an error whose
  message says why: `wrap` is meaningful because a machine word has a width,
  and a `Money` has no width to wrap into. A user type that wants three
  behaviours writes three methods, and names them honestly.
- **Built-in indexing does not go through `Index`.** Arrays and slices are
  indexed by the language, with the bounds check Ch. 3 §5.5 makes normative.
  Routing that through a library trait would demote a guarantee to a
  convention. `Index` exists for user types, and a user `index` is
  responsible for its own checking.
- **Comparison is `Eq`/`Ord`, not an operator trait.** There is no `PartialEq`
  to implement in its place.

> **A consequence of having no `AddAssign` (informative).** `c[i] += x` on a
> user type that implements `Index` desugars to `c[i] = c[i] + x`, and
> `Index::index` yields `&Self::Output` — so the right-hand side reads through
> a reference, which is only legal when `Output` is copyable. For a
> non-copyable `Output` the compound form is not writable and the long form
> is not either; the type must offer a method. This is exactly the case Rust
> keeps `AddAssign` for. Built-in arrays and slices are unaffected: the
> language indexes them itself (§5.4's second restriction), and reading an
> element is a read of a place rather than of a borrow. §7 records the gap
> rather than leaving it to be discovered.

### 5.5 `Clone`

```
trait Clone { fn clone(&self) -> Self; }
```

For a copyable type, cloning and copying are the same thing and `clone` is
derivable and rarely written. `Clone` earns its keep only for types with
destructors, where duplicating the value means duplicating whatever the
destructor releases — and draft 0.1 has no such resources (Ch. 3 §1.5). The
trait is defined here so the library chapter is not also a language change.

### 5.6 Conversion

Ch. 1 §6 promised these.

```
trait From<T>     { fn from(x: T) -> Self; }
trait Into<T>     { fn into(self) -> T; }
trait TryFrom<T>  { type Error; fn try_from(x: T) -> Result<Self, Self::Error>; }
```

A blanket impl gives `Into` for free wherever `From` exists:

```
impl<T, U: From<T>> Into<U> for T {
    fn into(self) -> U { U::from(self) }
}
```

Implement `From`; get `Into`. This is Rust's arrangement, inherited whole.

**`Into` may not be implemented by hand.** The blanket impl above covers every
type, so any hand-written `impl Into<Foo> for Bar` overlaps it, and §1.8 makes
overlapping impls an error. Rather than leave that as a collision a reader
discovers by hitting it, the trait is closed to hand implementation in the
same way §4.3 closes the `Fn` family. `TryInto` does not exist for the same
reason and would be added the same way.

These traits do **not** change Ch. 1 P2: there are still no implicit numeric
conversions. `x.into()` is as explicit as `x as t27`; it is written, it is
visible, and it converts nothing the reader did not ask to convert. `as`
remains the spelling for Ch. 1's built-in numeric conversions, and `From` the
one for everything else, because `as` has fixed meanings for the scalar cases
that a user impl must not be able to change.

### 5.7 `Iterator`, `IntoIterator`, and `for`

```
trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}

trait IntoIterator {
    type Item;
    type IntoIter: Iterator<Item = Self::Item>;
    fn into_iter(self) -> Self::IntoIter;
}
```

`Iterator` implements `IntoIterator` for itself, by blanket impl.

Ch. 0 §5.5 reserved `for` with the words "iteration is a trait, and traits are
Ch. 4". It is now defined, as sugar and nothing more:

```
for x in e { body }
```

means

```
{
    let mut it = IntoIterator::into_iter(e);
    loop {
        match it.next() {
            Some(x) => { body }
            None => break,
        }
    }
}
```

The desugaring uses only Ch. 0 constructs, which is the point: `for` adds no
control flow the language did not have. Ch. 0 §5.5 said "a loop over an array
is a `while` and an index, which is also what it lowers to" — that remains
true for slice iteration, whose `next` is an index and a bounds comparison the
optimizer is expected to fuse with the loop's own.

`break` and `continue` work in a `for` as in any loop. `break` with a value
does not: Ch. 0 §5.5 restricts that to `loop`, and the desugaring above puts
the user's `break` inside a `loop` whose own `break` carries nothing.

The adaptors — `map`, `filter`, `take`, `zip`, `enumerate` — are provided
methods on `Iterator`, and they belong to the library chapter. What this
chapter fixes is that they *can* be written: they need a closure (§4) and an
associated type (§1.7), and both now exist.

### 5.8 `Option` and `Result`

Ch. 2 §6 states niche guarantees about `Option<T>`, and §5.7 above needs it
normatively, so the two types are defined here rather than left to the library:

```
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
```

They are ordinary enums, laid out by Ch. 2's rules with no special case. Every
guarantee about them — `Option<&T>` is pointer-sized (Ch. 3 §2.5),
`Option<trit>` is one tryte (§5.3.2), `Option<&dyn Trait>` is two words
(§3.5) — is a consequence of niche optimization applying to a type the
compiler did not have to be told about.

Their methods (`unwrap`, `map`, `ok_or`, `?`) are the library's. The `?`
operator in particular is **reserved**: it needs a trait to describe what it
propagates, and that trait is worth designing once there is error handling to
design it against.

---

## 6. Derivation

```
#[derive(Ord, Clone)]
struct Point { x: t27, y: t27 }
```

**Deriving `Ord` derives `Eq`.** §1.6 makes `Eq` a supertrait of `Ord`, so an
impl of one without the other is ill-formed, and a derive that produced an
ill-formed impl would be a trap rather than a convenience. Writing both is
permitted and means the same thing. (Draft 0.1's first version of this
example wrote `#[derive(Ord, Clone)]` while requiring `Eq` separately, which
was ill-formed by this chapter's own §1.6.)

`derive` is the second attribute the language defines; Ch. 0 §3.4 defines
`repr` and reserves the rest, so this section un-reserves exactly one name.

Derivable: `Eq`, `Ord`, `Clone`. Each generates the impl a reader would write:

- `Eq` — every field equal, in declaration order.
- `Ord` — lexicographic by declaration order for a struct; for an enum, by
  discriminant first (Ch. 2 §5.1, so an explicit negative discriminant orders
  before a positive one), then by payload.
- `Clone` — field-wise.

A derived impl carries the bound `T: Trait` for every type parameter `T` of
the type. This is Rust's rule, it is occasionally too strong, and the escape
is to write the impl out.

`Copy` and `Sized` are **not** derivable, because they are automatic (§5.1,
§2.5). `Drop` is not derivable, because a destructor is exactly the thing no
one else can write for you.

---

## 7. What this chapter deliberately does not define

- **Returning a closure, `Box<dyn Trait>`, `impl Trait` in return position.**
  All three need an allocator or a decision this draft cannot make; §4.5.
- **Specialization.** §1.8. Overlapping impls collide; they do not order.
- **`const` arithmetic in type position.** §2.4.
- **Async, generators, `Pin`.** Not anticipated, and Ch. 3 §6 already declines
  `Pin` for its own reasons.
- **`Send`, `Sync`, and auto traits generally.** Reserved with AM §2.4, as
  Ch. 3 §4.4 says. `Sized` is the only automatic trait this draft has, and it
  is not a marker for a property of the concurrency model.
- **The `?` operator and an error-propagation trait.** §5.8.
- **`Hash`, `Debug`, `Display`, `Default`.** Library traits; two of them need
  text, which Ch. 0 §1.4 defers.
- **Trait method resolution across modules.** §1.3 states the rule that
  modules will inherit; the imports themselves are still reserved.
- **Negative impls beyond `!Copy`.** §5.1 defines the one the language needs
  and does not generalize it.
- **`AddAssign` and its relatives.** §5.4 explains what their absence costs: a
  compound assignment through a user `Index` whose `Output` does not copy. The
  trait is not defined here because adding it later changes no program that
  compiles today, and defining it now would double every operator impl.

---

## Appendix A (informative) — worked examples

```rust
// A trait, two implementations, and both dispatch forms.

trait Shape {
    fn area(&self) -> t27;
    fn is_degenerate(&self) -> bool { self.area() == 0 }   // provided
}

struct Circle { r: t27 }
struct Rect { w: t27, h: t27 }

impl Shape for Circle {
    fn area(&self) -> t27 { self.r * self.r * 3 }
}

impl Shape for Rect {
    fn area(&self) -> t27 { self.w * self.h }
}

// Static: monomorphized, one copy per type, direct call.
fn describe<S: Shape>(s: &S) -> t27 { s.area() }

// Dynamic: one copy, indirect call through the vtable.
fn total(shapes: &[&dyn Shape]) -> t27 {
    let mut sum: t27 = 0;
    for s in shapes { sum += s.area(); }
    sum
}

// A derived comparison: two `cmp`s and one `sel3`, no branch.
#[derive(Eq, Ord)]
struct Version { major: t27, minor: t27 }

fn newer(a: &Version, b: &Version) -> bool { a > b }

// A closure, passed but not returned.
fn map_in_place(xs: &mut [t27], f: impl Fn(t27) -> t27) {
    let mut i: taddr = 0;
    while i < xs.len() {
        xs[i] = f(xs[i]);
        i += 1;
    }
}

fn main() -> t27 {
    let c = Circle { r: 2 };
    let r = Rect { w: 3, h: 4 };
    let shapes: [&dyn Shape; 2] = [&c, &r];

    let k: t27 = 3;
    let mut ys: [t27; 3] = [1, 2, 3];
    map_in_place(&mut ys, |x| x * k);       // captures `k` by shared reference

    describe(&c) + total(&shapes) + ys[2]
}
```

## Appendix B (informative) — bug classes removed by construction

Continuing the scorecard:

| Binary-world bug class | Why it cannot occur here |
|---|---|
| a template error reported inside a library's body | bounds are checked at the call site (§2.2) |
| two libraries implementing the same trait for the same type | coherence (§1.8) |
| a `NaN` making `<` and `==` disagree | there is no partial order under `<` (§5.3.2) |
| an `Ordering` enum allocated, matched, and discarded | the ordering type is `trit` (§5.3.1) |
| a lexicographic comparison as a chain of branches | one `sel3` per field (§5.3.3) |
| `Option<&dyn Trait>` costing a word more than `&dyn Trait` | niches (§3.5) |
| a `dyn` call on a type whose method needed `Self` | object safety (§3.4) |
| a closure outliving the variable it captured | it cannot be returned (§4.5) |
| forgetting `#[derive(Copy)]` and getting a move | `Copy` is automatic (§5.1) |

## Appendix C (informative) — keywords this chapter takes

Ch. 0 §1.3 reserves a list "for chapters that do not exist yet". This chapter
is one of them, and it claims:

| Word | Use |
|---|---|
| `trait` | §1.1 |
| `impl` | §1.2, §2.2 |
| `Self` | §1.2 |
| `type` | §1.7 |
| `where` | §2.2 |
| `dyn` | §3.1 |
| `for` | §1.2 (`impl … for …`), §2.6 (`for<'a>`), §5.7 (`for … in …`) |
| `in` | §5.7 |

Still reserved, and not this chapter's: `crate`, `mod`, `move`, `pub`, `ref`,
`static`, `union`, `unsafe`, `use`.

`for` now has three meanings. They are distinguished by what precedes them —
an `impl` header, a bound, or a statement position — and no grammar ambiguity
arises. Rust carries the same overload for the same historical reason, and
splitting it would cost a reader more than it saves.
