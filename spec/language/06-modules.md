# Language Specification, Chapter 6 — Modules

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | Language Ch. 0 (*Syntax*), Ch. 4 (*Generics and traits*), Ch. 5 (*Library*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

Every chapter before this one describes a program that fits in a file. This
one is written because the next thing to be built in this language is its own
compiler, and that does not.

Ch. 0 §1.3 reserved `mod`, `pub` and `use` for "the module system". This is
that system, and it is deliberately the smallest one that answers the two
questions a program too large for a file actually asks: **where does the rest
of it live**, and **what may reach into it**.

> **Design note (informative).** The second question is not organisational.
> A `Vec` is a pointer, a length and a capacity, and every bounds check in
> Ch. 3 §5.5 is sound only because nothing may write the length. Ch. 5 §2
> states that as a property of the type without saying what enforces it,
> because until this chapter *nothing did* — a program could set the length
> past the capacity and index into memory it does not own. Privacy is not
> tidiness here. It is the mechanism a safety claim rests on, and that is why
> it is in this chapter rather than deferred with the rest of the ergonomics.

---

## 1. A module is a file

A **module** is a source file. There is no other kind.

### 1.1 The root

The file named to the compiler is the **root module**. It has no name of its
own: nothing can refer to it, because everything else is inside it.

### 1.2 Declaring a module

```
mod lex;
```

declares that a module named `lex` exists and that its source is a file the
compiler will find by §1.3. A module that is not declared is not compiled;
a file beside the root that nothing declares is not part of the program.

A `mod` declaration is an item (Ch. 0 §3) and may appear wherever an item may,
which is to say at the top level of any module. It is not an expression and
may not appear inside a function.

> **Why a declaration at all**, when the file is right there: because a
> directory is not a program. Which files are compiled is a fact about the
> program and belongs in it, and a build that changes when an unrelated file
> is created is a build nobody can reason about.

### 1.3 Where the file is

For a module `m` declared in the root, the file is `m.tr`, in the directory
holding the root.

For a module `m` declared in a module whose file is `<dir>/p.tr`, the file is
`<dir>/p/m.tr`.

That is the whole rule. A module with submodules is a directory named after
it, holding a file per submodule, beside the file that declares them.

```
main.tr           mod lang;
lang.tr           mod lex;  mod parse;
lang/lex.tr
lang/parse.tr
```

> **Informative.** There is no second spelling — no `mod.rs`, no `lang/lang.tr`.
> One path is computable from one declaration, and a reader who knows the
> rule can find any module in the program without a search.

### 1.4 Each module is compiled once

A module declared twice, whether in one file or in two, is an error. A module
is not a copy of its source: it is compiled once, and every path that names it
names the same items.

Two modules may name each other. There is no ordering constraint of any kind,
here or anywhere else in this language: Ch. 0 §3 says every item in a file is
visible to every other whatever the order, and the same is true across files.

---

## 2. Visibility

### 2.1 Private by default

An item is visible in the module that defines it and in every module inside
it. It is not visible anywhere else.

"Inside" is by declaration: a module declared by `m` is inside `m`.

### 2.2 `pub`

An item written `pub` is visible wherever its module is.

```
pub struct Span { pub lo: taddr, pub hi: taddr }
pub fn width(s: &Span) -> taddr { s.hi - s.lo }
```

`pub` is written on an item, and separately on each field of a `struct` and
each variant payload field it should apply to. A `pub struct` with private
fields is a type others may hold and name and pass, and whose insides are
this module's business.

A `pub` on a `trait`'s method, on an `enum`'s variant, or on a method of an
`impl`, is an error: a trait's methods are as visible as the trait, a variant
as its enum, and an `impl`'s methods as the `impl` (§5). There is no way to
write half an enum, and no way to write a method more private than the type
it is on.

> **Informative.** Rust allows `pub fn` in an inherent `impl` and means
> something by it: a `pub` type may have methods its module keeps to itself.
> The cost of not having it is a real one — a helper that briefly breaks an
> invariant is as callable as the type — and it is accepted here because the
> invariant of §2.3 does not rest on methods. It rests on *fields*, and a
> method that can only reach them through the same `pub` surface everyone
> else has is not a way around anything. A private method is an
> organisational tool, and §2.4 has already said which of those this draft
> takes.

### 2.3 What privacy is for

A module's items may hold an invariant no combination of its `pub` items can
break. That is the only claim privacy makes, and it is enough for Ch. 5 §2:

```
pub struct Vec<T> { ptr: taddr, len: taddr, cap: taddr }
```

With the fields private, `len <= cap` and "`ptr` points at `cap` elements this
`Vec` owns" hold for every value of the type that can exist, because the only
code that can write them is the code that maintains them. With the fields
public they hold for none, and every bounds check that trusts `len` is
checking a number anybody could have written.

> **Informative.** This is why the answer to "does this language need
> privacy" is not a matter of taste. Ch. 1 §1 inherits the Rust discipline,
> and a large part of that discipline is that safety is a property of
> *modules*: `unsafe` code inside one is sound because the module's `pub`
> surface cannot be driven into a state it does not expect. Trust has no
> `unsafe` yet (Ch. 0 §1.3), but it has a `Vec` whose implementation is the
> compiler's, and the argument is the same one.

### 2.4 What `pub` does not have

There is one degree of visibility and it is `pub`. There is no `pub(crate)`,
no `pub(super)` and no `pub(in path)`.

There is no re-export: `pub use` is reserved and is an error. A name is
reached by the path to where it was defined.

> **Informative.** Both are omitted for the same reason, and it is not
> simplicity for its own sake. A re-export makes one item reachable by two
> paths, and a graded visibility makes "who can see this" a question the
> reader answers by searching. Draft 0.1 would rather be told it is missing
> them than be unable to say what is visible from where.

---

## 3. Paths and `use`

### 3.1 A path

A path names an item through the modules that hold it, separated by `::`:

```
lang::lex::Span
```

A path **in an expression or a type** is resolved from the module it is
written in. Its first segment names either a module declared in that module,
or a name brought in by a `use` (§3.2), or an item.

A path **in a `use`** is resolved from the **root** (§1.1). That is the one
place a module can name something outside itself, and it is why there is no
`crate::`, `super::` or `self::` prefix: every path that leaves a module is a
`use`, and every `use` starts from the same place.

> **Informative.** Draft 0.1 declined all three prefixes and said a module
> "reaches what is outside it by a `use` written at its top" — while leaving
> the `use` itself relative, so it could not name the outside either. Two
> sibling modules could not see each other at all, which a compiler written
> in this language found on its second file. One rule fixes it and it is the
> rule that was meant: a `use` is absolute.
>
> `super::super::thing` remains declined for the reason it always was — a
> path whose meaning depends on where its file sits changes meaning when the
> file moves. An absolute `use` does not.

### 3.2 `use`

```
use lang::lex::Span;
use lang::lex;
```

`use` binds the **last segment** of a path as a name in the module it is
written in, for the whole module regardless of where in it the `use` sits.
Its path is absolute (§3.1): `use lang::lex::Span;` names `Span` in
`lang::lex` from wherever it is written, and a module names its *sibling* the
same way it names anything else outside it.

A `use` is an item and appears at the top level of a module. What it binds is
a name for something already visible: `use` grants no access. A `use` of a
private item of another module is an error, and it is the same error as
naming it by its full path.

Two `use`s binding the same name are an error, as are a `use` and an item of
that name in the same module. There is no shadowing between them and no
"last one wins".

There is no `use a::{b, c}`, no `use a::*` and no `as` renaming in a `use`.
Each is one line.

### 3.3 The prelude

Ch. 5's prelude is in scope in **every** module, with no `use`. It is not a
module and has no path.

An item **the program** defines shadows a prelude item of the same name,
everywhere in the program — not only in the module defining it. A prelude
name means one thing in a program.

> **Informative.** Per-module shadowing was considered and declined. The
> prelude has no path, so a module cannot say "the prelude's `print`, not
> mine"; with per-module shadowing, `print` in one module and `print` in
> another would be different functions with no way to write the difference,
> and a reader moving a line between files would change what it calls.
> Program-wide is the rule that can be stated in one sentence and checked by
> reading one program.

An item **a module** defines is its own, and does not collide with an item of
the same name in another module: `m::go` and the root's `go` are two
functions (§4).

---

## 4. Below the language

A module is a **naming** construct and nothing else. It has no run-time
representation, no initializer and no order.

The symbol a function or global becomes in TIR is its path with `::` written
as `.` — `lang::lex::Span::width` becomes `lang.lex.Span.width` — because TIR
§1 admits `.` in an identifier and not `:`. A program in which two items would
become the same TIR symbol is an error, reported by name.

A **declaration** — a signature with no body (Ch. 0 §3.1) — is the exception,
and keeps the name it was written with wherever it was written. It does not
name an item of this program: it names something the program is linked
against, which was written before this program existed and knows nothing
about its modules. `putchar` is a symbol in a runtime; `io.putchar` is not.

> **Informative.** Two modules may therefore declare the same external
> function, and they name the same one. That is the intent — a declaration
> is a claim about the outside, and two identical claims are one claim — and
> it is why a declaration is not subject to the paragraph above.

A **monomorphization** is named the same way: the head and its arguments,
with `.` between them, so `Option<taddr>` is `Option.taddr`. The scheme does
not say where one argument ends and the next begins, and does not need to —
Ch. 4 §2.1 gives a type one arity, so `Holder.Option.t27` can only be read
one way. Anything that changed that would make two types one name, and is a
reason to change the scheme rather than the program.

Monomorphization, vtables and drop glue are unchanged: they are described in
Ch. 4 and Ch. 3 in terms of items, and an item's module is part of its name
and of nothing else.

---

## 5. Grammar

Added to Ch. 0 §6:

```
item        := attr* ( mod | use | fn | struct | enum | const | trait | impl )
mod         := 'pub'? 'mod' ident ';'
use         := 'use' path ';'
path        := ident ( '::' ident )*
```

and `pub` is admitted before `fn`, `struct`, `enum`, `const`, `trait`, a type
alias (Ch. 0 §3.6) and a `struct`'s or a variant's field.

`impl` takes no `pub`: an `impl` block is as visible as the more private of
the type and the trait, which is a rule rather than a spelling.

---

## 6. What this chapter deliberately does not define

- **`crate`, `super`, `self` as path prefixes.** §3.1; reserved.
- **`pub use`, and every other re-export.** §2.4; reserved.
- **`pub(crate)` and friends.** §2.4; reserved.
- **Grouped, glob and renaming `use`.** §3.2; reserved.
- **Inline `mod name { … }`.** A module is a file (§1). Grouping within a
  file is what an `impl` block and a heading comment are for, and two
  spellings of one idea is what Ch. 0 §7 declines elsewhere.
- **Separate compilation, and a unit larger than a program.** There is no
  `crate`, no library, and no linking of one program to another. Every
  program is compiled from its root in one go. What a package is, and what it
  would mean to depend on one, is not this chapter's and not draft 0.1's.
- **A file that is not UTF-8, and a path that is not ASCII.** Ch. 0 §1.1
  gives the source encoding; what a filesystem does with a name is the
  filesystem's.

---

## Appendix (informative) — the smallest program with two files

`main.tr`:

```
mod greet;

fn main() -> t27 {
    greet::hello();
    0
}
```

`greet.tr`:

```
pub fn hello() {
    println("hello");
}
```

`hello` is `pub`, so `main` may name it. `println` is the prelude's and needs
no path. The TIR symbol is `greet.hello`.
