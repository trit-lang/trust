# Language Specification, Chapter 7 — Macros

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | Language Ch. 0 (*Syntax*), Ch. 3 (*Ownership*), Ch. 6 (*Modules*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

A macro is the language's answer to the one thing a function cannot be: a
name for a call whose **number of arguments is not fixed**. `vec![a, b, c]`
builds a vector from a list whose length is part of the call, and no
signature can be written for it — a function takes the arguments its type
says and no others.

That is the whole of what draft 0.1 needs one for, and this chapter is sized
to it.

This chapter defines the smallest macro that answers those, and two of its
properties are decisions the rest follows from:

- **A macro is hygienic** (§4). A binding its body introduces cannot capture
  a name at the call site, and cannot be captured by one.
- **A macro has one rule** (§2). It takes a parameter list, optionally ending
  in a repetition, and has one body. There is no matching between several
  shapes.

> **Design note (informative).** Hygiene is not an ergonomic preference
> here. Every other chapter refuses what it cannot check rather than
> accepting it and being sometimes wrong — an index that might be out of
> range, a move that might be a second one, an inference that might be a
> guess. A macro that captures is exactly that failure with no diagnostic
> attached: the program compiles, runs, and does something else. A language
> that refuses `take(*r)` because the value *might* be dropped twice cannot
> coherently accept `swap!(tmp, x)` doing nothing because the macro happened
> to write `tmp`.

---

## 1. A macro is an item

```
macro twice($x) {
    $x + $x
}
```

A macro is an item (Ch. 0 §3) and may appear wherever an item may. It is
visible by the rules of Ch. 6 §2 and may be `pub`.

It is **invoked** with `!`:

```
twice!(3)
```

An invocation is an **expression**, and expands to one. A macro that expands
to a statement or an item is reserved (§7).

A macro is not a value: `twice` alone names nothing, and there is no way to
pass one, store one or return one.

---

## 2. Parameters

A parameter is written `$name` and binds **one expression**:

```
macro min($a, $b) { if $a < $b { $a } else { $b } }
```

The last parameter may instead be a **repetition**, written
`$($name),*`, which binds zero or more expressions separated by commas:

```
macro sum($($x),*) { … }
```

A macro has at most one repetition and it is last. A call supplies exactly
the fixed parameters, then whatever the repetition takes.

Two parameters of one macro may not share a name.

> **Informative.** One repetition, at the end, is what makes an invocation
> unambiguous to read and to parse without lookahead: everything up to the
> last fixed parameter is positional, and the rest is the list. Rust's
> `macro_rules!` is more expressive and pays for it with a matching
> algorithm; §7 records what that would buy.

### 2.1 An argument is evaluated where it is written

Substitution is of the **expression**, not of its value: an argument written
twice in the body is evaluated twice.

```
macro twice($x) { $x + $x }
twice!(f())        // calls `f` twice
```

That is the same rule Rust has and for the same reason — the alternative is a
macro that cannot say `$x` in a place where a value would not be accepted, of
which `&$x` and `$x = 1` are two.

A macro is therefore a poor place to put an argument that is expensive or
that moves (Ch. 3 §1.2), and the diagnostic for a doubled move is the
ordinary one, reported at the call.

---

## 3. The body

The body is a **block** (Ch. 0 §5.1), and the expansion is that block as an
expression. It may name anything the macro's own module can name (Ch. 6 §3),
and the parameters.

A repetition in the body is written `$( … )*` and repeats its contents once
per argument the repetition bound:

```
macro vec($($x),*) {
    {
        let mut v = Vec::new();
        $( v.push($x); )*
        v
    }
}
```

A `$( … )*` group must mention the repetition's parameter, and a macro with
no repetition may not write one.

### 3.1 What the body may not do

- It may not invoke itself, directly or through other macros. Expansion is
  therefore finite without a depth limit, and a cycle is reported by name.
- It may not declare an item. A macro expands to an expression (§1).

---

## 4. Hygiene

A macro's body and its call site are **different scopes**, and the rule is
one sentence:

> A name the body binds is a new name; a name the body *uses* and does not
> bind is resolved where the macro was written.

Both halves matter:

```
macro swap($a, $b) {
    { let tmp = $a; $a = $b; $b = tmp; }
}

fn f() {
    let mut tmp = 1;
    let mut x = 2;
    swap!(tmp, x);          // swaps `tmp` and `x`
}
```

The body's `tmp` is not the caller's, so the swap is the swap. An
implementation gives the body's bindings names no program can write, which is
the same device Ch. 4 §4.2 already uses for a closure's captures.

Conversely, an argument's names are the **caller's**: `$a` is the caller's
`tmp`, and the body cannot rebind it or shadow it.

### 4.1 What hygiene does not cover

Hygiene is about **bindings**, not about items. A body that names `print`
names whatever `print` is where the macro was written (Ch. 6 §3.3), and a
caller that defines its own `print` does not change what the macro does. That
is the intent, and it is the same rule for a function.

There is no way to write a macro that *deliberately* introduces a name the
caller can see. Rust's `$crate` and its unhygienic escape hatches are §7's.

---

## 5. Expansion

Expansion happens once, before anything else looks at the program: after
modules are resolved (Ch. 6 §4) and before type aliases (Ch. 0 §3.6), so a
macro may expand to a use of an alias.

An expanded expression carries the span of the **invocation**, not of the
body, because that is the place a reader can act on. An error inside a body
therefore reports at the call, and names the macro.

A macro is expanded only where it is invoked. A macro nothing invokes is
compiled no further than being read, exactly as an uncalled function is.

---

## 6. Grammar

Added to Ch. 0 §6:

```
item        := attr* ( … | macro )
macro       := 'pub'? 'macro' ident '(' macro_params ')' block
macro_params:= ( '$' ident ,* ) ( ',' repetition )? | repetition?
repetition  := '$' '(' '$' ident ')' ',' '*'

primary     := … | macro_call
macro_call  := ident '!' '(' expr,* ')'
```

`$` is added to Ch. 0 §1.5's punctuation. It appears only inside a macro's
parameter list and body, and is an error elsewhere.

Within a body, `$( … )*` is a repetition group and `$name` is a parameter.

---

## 7. What this chapter deliberately does not define

- **Several rules per macro, and matching.** §2. A macro that behaves
  differently for `()` and for `($x)` is Rust's `macro_rules!`, and needs
  fragment kinds (`expr`, `ident`, `ty`, `pat`), a matching order and a
  recursion limit — three specifications, for an expressiveness draft 0.1 has
  not yet needed.
- **A parameter that is not an expression.** §2. `$t:ty` and `$p:pat` follow
  from the same machinery as matching, and wait with it. `matches!(e, p)`
  wants the second, and is the first thing this omission costs.
- **Stringification.** There is no `stringify!`, so `assert!` cannot report
  the text of what failed and is a function taking a `bool`.
- **Statement and item macros.** §1. `#[derive(…)]` (Ch. 4 §6) is the
  language's only item-generating construct and is not a macro.
- **Recursion.** §3.1.
- **An unhygienic escape.** §4.1. There is no `$crate`, no `gensym` and no
  way to introduce a name the caller can see.
- **`format!` and its family.** Ch. 5 §7 reserves formatting, and this
  chapter does not unreserve it: a format string is taken apart at compile
  time, which needs a parameter that is a *literal* and a way to read one —
  neither of which is here.

---

## Appendix (informative) — three macros worth having

```
macro vec($($x),*) {
    {
        let mut v = Vec::new();
        $( v.push($x); )*
        v
    }
}

macro say($($x),*) {
    {
        $( print($x); )*
        println("");
    }
}

macro swap($a, $b) {
    { let tmp = $a; $a = $b; $b = tmp; }
}
```

The first two could not be functions: their arity is part of the call. The
third could — `fn swap(a: &mut T, b: &mut T)` is writable — and is here for
what it shows about §4: `tmp` is the macro's, and `swap!(tmp, x)` is the swap
a reader expects rather than the one an unhygienic expansion would produce.
