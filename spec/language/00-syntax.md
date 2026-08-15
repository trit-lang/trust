# Language Specification, Chapter 0 — Surface Syntax

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), `spec/01-naming.md` (*Naming*), Language Ch. 1 (*Types*), Ch. 2 (*Composites*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

This chapter defines how a Trust program is written: its lexical structure,
its items, its expressions and its statements. It is Chapter 0 because every
other chapter's examples are written in it — Ch. 1 §4 defers "the rest of
surface syntax" to exactly this document, and the TIR specification's appendix
labels its sample "syntax provisional" pending it.

It defines **no semantics**. Where a construct has meaning, that meaning is
already fixed elsewhere: the numeric rules are the AM's, the type rules are
Ch. 1's, the layout rules are Ch. 2's. This chapter says only what the text
looks like and how it groups.

> **Design note (informative).** The shape is Rust's, deliberately and almost
> everywhere, because the parts of Rust's syntax this language would change
> are not the parts that follow from the radix. Where the radix *does* change
> something — `<=>` as a primitive, trit literals, three-way `match` — the
> spelling is already fixed by Ch. 1. What is left is the ordinary business of
> where the braces go, and inventing novelty there would cost every reader
> something and buy nothing.
>
> Four questions were genuinely open, and all four are answered here in the
> direction that keeps the language smaller: `& | ^` stay reserved (§2.5), a
> function without a body is a declaration (§3.1), comments are `//` (§1.2),
> and three-way dispatch is `match` and nothing else (§5.6).

---

## 1. Lexical structure

### 1.1 Source text

A source file is UTF-8 text with the extension `.tr` (Naming §2). Line
terminators are U+000A, optionally preceded by U+000D. Horizontal whitespace
separates tokens and is otherwise insignificant: indentation carries no
meaning.

### 1.2 Comments

```
// to the end of the line
/* to the closing marker, and these /* nest */ */
```

`;` is **not** a comment marker here, though it is one in every other document
of this project — in the abstract machine's listings, in TIR, and in assembly.
The reason is that a Rust-shaped statement ends in `;`, and one character
cannot both end a statement and begin a comment. The inconsistency is real and
is accepted knowingly: statement terminators are used far more often than the
cross-layer habit is worth.

A comment is whitespace. There is no doc-comment form in draft 0.1;
`///` and `//!` are **reserved**.

### 1.3 Identifiers and keywords

An identifier begins with an ASCII letter or `_` and continues with letters,
digits and `_`. Identifiers are case-sensitive. A lone `_` is not an
identifier; it is the wildcard pattern (§4).

Keywords, all of which are reserved and none of which may be an identifier:

```
as      break   const   continue   else    enum    false   fn
if      let     loop    match      mut     return  struct  true
while
```

Reserved for chapters that do not exist yet, and equally unusable as
identifiers:

```
crate   dyn     for     impl    in      mod     move    pub
ref     self    Self    static  trait   type    union   unsafe
use     where
```

`union` is reserved by Ch. 2 §7 with its semantics deferred; the rest belong
to Chapters 3 and 4, to the module system, and to the library chapter.

### 1.4 Literals

Numeric literals are exactly Ch. 1 §3, unchanged: decimal, `0t` for balanced
ternary and `0h` for heptavintimal, with `_` permitted as a separator in all
three. `trit` literals carry the `t` suffix — `-1t`, `0t`, `1t` — and `bool`
literals are `true` and `false`.

Character and string literals were reserved until a text encoding existed,
and Ch. 5 §1 now fixes one: a `char` is a Unicode scalar value in one word,
and a string is a sequence of them. `'a'` is a `char` and `"hello"` a
`&'static str`; Ch. 5 §1.4 gives the escapes and says why there is no `\x`.

A program that wants *bytes* rather than text still writes them as what they
are, which is an array of `t9` — that is what an interchange buffer is, and
Ch. 5 §1.5 is where text becomes one.

### 1.5 Punctuation and operators

```
(  )  [  ]  {  }
,  ;  :  ::  .  ->  =>  #  _
=  ==  !=  <  <=  >  >=  <=>
+  -  *  /  %  <<  >>
+= -= *= /= %= <<= >>=
&&  ||  !
&
```

`&` introduces a reference type and a borrow (Ch. 3). `|` appears only in
or-patterns (§4). `^`, `~`, and the bitwise `&`/`|` operators are
**reserved and rejected** — see §2.5.

`!` is logical negation (§2.4) and the marker of a negative implementation
(Ch. 4 §5.1). One further position is **reserved**: an identifier immediately
followed by `!`, as in `name!(…)`, is the invocation form macros will take if
they arrive (§7). It is a syntax error in draft 0.1, and nothing else may
claim it. The reservation costs nothing now and is what keeps a formatting
facility possible later, since every spelling of one needs either macros or
variadic arguments and this language has no plan for the second.

---

## 2. Expressions

### 2.1 Precedence

Tightest first. Operators on one line have equal precedence and associate
left to right unless marked otherwise.

| | Operators | |
|---|---|---|
| 1 | paths, literals, `(…)`, array and struct literals, blocks, `if`, `match`, `loop`, `while` | primary |
| 2 | `f(…)`, `x.field`, `x.0`, `x.method(…)`, `x[i]` | postfix |
| 3 | `-x`, `!x`, `&x` | unary |
| 4 | `x as T` | |
| 5 | `*` `/` `%` | |
| 6 | `+` `-` | |
| 7 | `<<` `>>` | |
| 8 | `<=>` | **non-associative** |
| 9 | `==` `!=` `<` `<=` `>` `>=` | **non-associative** |
| 10 | `&&` | |
| 11 | `\|\|` | |
| 12 | `=` `+=` `-=` `*=` `/=` `%=` `<<=` `>>=` | right-associative |

`<=>` binds tighter than the two-way predicates, so `a <=> b == 0t` is
`(a <=> b) == 0t` — the reading anyone writing it intends. Both comparison
levels are non-associative: `a < b < c` and `a <=> b <=> c` are syntax errors
rather than surprises.

### 2.2 Operands and arithmetic

`+ - * / %`, unary `-`, and `<< >>` are Ch. 1 §4's operators with Ch. 1 §4's
semantics — one division, rounding to nearest with ties away from zero, and
shifts by powers of three. Operands must have the same type (Ch. 1, P2); there
are no implicit conversions and mixed-width arithmetic is a compile-time
error.

The compound forms are pure sugar: `a op= b` is `a = a op b`, with `a`
evaluated once.

### 2.3 Comparison

`a <=> b` yields a `trit` (Ch. 1 §5). The six two-way predicates yield `bool`
and are projections of it; a compiler is required to treat them as such, so a
chained `if a < b … else if a > b …` costs one comparison.

### 2.4 Boolean operators

`&&` and `||` take `bool` and short-circuit; `!` negates a `bool`. They are
the only boolean connectives, and they are two-valued because `bool` is
(Ch. 1 §2). Three-valued logic is `Option<bool>` and lives in the library
(Ch. 2 §6).

### 2.5 What `& | ^` are not

Ch. 1 §4 left open whether `& | ^` should be repurposed as `tmin`, `tmax` and
`tmul`. **They are not.** The trit-wise operations remain the methods Ch. 1
§4 names:

```
a.tmin(b)    a.tmax(b)    a.tmul(b)    a.tneg()
```

Two reasons. The named forms say which operation they are, and a reader who
has not yet internalized that `tmin` is the AND analogue is not helped by a
symbol that says AND. And `|` is worth more as the or-pattern separator
(§4) than as a second spelling of `tmax`. A future revision may still take the
operators; nothing here forecloses it, which is the point of deciding this way
round.

`^` and `~` are reserved and produce a diagnostic naming the methods.

### 2.6 Calls, fields and indexing

```
f(a, b)          x.method(a)       p.x        t.0        xs[i]
```

Indexing takes a `taddr` and bounds-checks (Ch. 2 §3). A negative index is
out of bounds, not end-relative — Ch. 2 §3 claims this explicitly so that no
library invents the convention.

### 2.7 Conversions

`x as T` is Ch. 1 §6's conversion: widening is value-preserving, narrowing
wraps, and `trit` ↔ `bool` has no `as` path by design.

### 2.8 Literals of composite types

```
[1, 2, 3]                 // array
[0; 9]                    // array repeat: nine zeros
(a, b)                    // tuple
()                        // unit
Point { x: 1, y: 2 }      // struct
Trip(1, 2, 3)             // tuple struct
Sign::Neg                 // enum variant
Shape::Line(4)            // enum variant with a payload
```

A struct literal is ambiguous with a block in the condition position of `if`,
`while` and `match`, and is **not permitted there** without parentheses — the
same rule, and the same reason, as Rust's.

---

## 3. Items

An item is a top-level definition. Draft 0.1 has one file and no module
system: `mod`, `use` and `pub` are reserved (§1.3), and every item in the file
is visible to every other regardless of order.

### 3.1 Functions

```
fn steps_toward(target: t27, pos: t27) -> t27 {
    match pos <=> target {
        -1t =>  1,
         0t =>  0,
         1t => -1,
    }
}
```

Parameters are `name: Type`, comma-separated, with an optional trailing comma.
The return type is `-> T`; its absence means `()`.

**A function without a body is a declaration**: its signature is defined here
and its body is external.

```
fn putchar(c: t9);
```

This is the same rule TIR §1 states for its own declarations ("function
declarations — signature only, body external"), and using one rule in both
places means a `.tr` declaration and the TIR declaration it lowers to are the
same thing spelled twice rather than two mechanisms. The cost is that a
required trait method will look identical when Ch. 4 arrives; that is a
context a reader and a compiler can both distinguish.

Calling a declaration is how a program reaches anything the language cannot
express — a memory-mapped device port, most immediately, since TIR §5 has no
integer-to-pointer cast and so cannot name an address at all.

### 3.2 Constants

```
const LIMIT: t27 = 3_812_798;
const MESSAGE: [t9; 5] = [72, 101, 108, 108, 111];
```

A constant's type is written out; it is not inferred. Its initializer is
evaluated at compile time, exactly, in balanced ternary — the same evaluation
the assembler performs (ISA Assembly §4.2), and for the same reason: a
constant that meant one thing at compile time and another at run time would be
a defect no test could catch.

### 3.3 Structs and enums

Exactly the forms Ch. 2 §4 and §5 already show:

```
struct Point { x: t27, y: t27 }
struct Trip(t9, t9, t9);
struct Marker;

enum Sign { Neg, Zero, Pos }
enum Shape { Dot, Line(t27), Rect { w: t27, h: t27 } }
enum E { A = -1, B, C = 1 }
```

An explicit discriminant is `= expr` after the variant name and may be
negative (Ch. 2 §5.1). An unassigned discriminant continues from the previous
variant — which is what Ch. 2's own appendix requires, since it lists
`enum { A=-1, B, C=1 }` as trit-shaped and that holds only if `B` is 0.

### 3.4 Attributes

```
#[repr(linear)]
struct Header { magic: t27, len: t9 }
```

An attribute is `#[` name `]` or `#[` name `(` arguments `)` `]`, and attaches
to the item that follows it. Draft 0.1 defines exactly one: `repr`, taking
`lang` or `linear` (Ch. 2 §1). Every other attribute name is reserved.

Attributes appear on items only. In particular the overflow profile of Ch. 1
§4 — trapping in checked builds, wrapping in release — is a property of the
build and not of an item, and `#[overflow(…)]` is reserved against the day
that changes.

### 3.5 Type syntax

```
trit    bool    t9    t27    taddr    ()
[T; N]            // array, N a constant expression
(T, U)            // tuple
&T                // reference (Ch. 3)
Name              // a struct or enum
Name<T>           // reserved: generics are Ch. 4
```

`&T` and `Name<T>` are written here so the grammar is complete; their meaning
is Chapters 3 and 4's, and a draft 0.1 compiler may reject them.

---

## 4. Patterns

Patterns appear in `match` arms and in `let`.

```
_                      // wildcard: matches anything, binds nothing
x                      // binding
-1t   0t   1t          // trit literals
42    0t1T0   0hDDE    // integer literals
true  false            // bool literals
(a, b)                 // tuple
Sign::Neg              // fieldless variant
Shape::Line(n)         // variant with a payload
Shape::Rect { w, h }   // variant with named fields
Point { x, y }         // struct
p @ Sign::Neg          // binding the whole while matching
-1t | 1t               // or-pattern
```

An or-pattern's alternatives must bind the same names at the same types.
`|` is available for this because §2.5 declined to spend it on `tmax`.

A `match` must be exhaustive (Ch. 2 §5). Over a `trit` that means three arms,
or fewer plus a wildcard; over an enum it means every variant. Exhaustiveness
is checked, not assumed.

Range patterns, `ref`, and `if let` are reserved.

---

## 5. Statements and control flow

### 5.1 Blocks

A block is a sequence of statements in braces, optionally ending in an
expression. That trailing expression, if there is one, is the block's value;
otherwise the block's value is `()`. A block is an expression and may appear
wherever one may.

```
let y = {
    let t = x * x;
    t + 1              // the block's value
};
```

### 5.2 Statements

```
let x: t27 = 0;        // binding, with a type
let x = 0;             // binding, type inferred
let mut i: taddr = 0;  // mutable binding
let (a, mut b) = p;    // binding a tuple, element by element
expr;                  // expression statement
item                   // items may appear in blocks
```

The tuple form is **sugar**: it binds the whole tuple to a name no program can
write and then binds each element to a field of it, so nothing below the
grammar learns a new shape and the ownership rules need no case of their own —
moving out of one element leaves the others usable, which Ch. 3 §1.3 already
says. `mut` is per element. It is the only pattern a `let` takes; anything
richer is a `match`.

A binding is immutable unless it says `mut`. Ch. 1's numeric types are all
`Copy`-shaped scalars, so nothing about ownership is visible here yet; Ch. 3
is where that changes.

**A block-shaped expression in statement position ends its statement.** `if`,
`match`, `loop`, `while` and a bare `{ … }` may stand as statements without a
`;`, and where one does, it ends at its closing brace: an operator on the next
line begins a new statement rather than continuing the old one.

```
if a > 3 { n = 100; }
(a) * 2 + n              // a second statement, not a call of the `if`
```

This is the rule Rust has, and it is here for the same reason: without it the
two lines above parse as one, the `if`'s value is `()`, and the diagnostic is
that `()` is not callable — true, and no help at all. The cost is that a
method on a `match` in statement position needs parentheses, which is a cost
paid where it is visible.

In **tail** position nothing changes: a block-shaped expression is the block's
value there, so `fn pick(c: bool) -> t27 { if c { 1 } else { 2 } }` means what
it looks like.

Literal type inference is Ch. 1 §3's: unconstrained integer literals default
to `t27`, and a literal that does not fit its inferred type is an error rather
than a wrap.

### 5.3 `if`

```
if c { … } else if d { … } else { … }
```

The condition is a `bool` — not a `trit`, and not "anything nonzero" (Ch. 1
§2). An `if` used for its value must have an `else`, and both branches must
agree in type.

### 5.4 `match`

```
match pos <=> target {
    -1t =>  1,
     0t =>  0,
     1t => -1,
}

match shape {
    Shape::Dot            => 0,
    Shape::Line(n)        => n,
    Shape::Rect { w, h }  => w * h,
}
```

An arm is `pattern => expression`, separated by commas; a trailing comma is
optional and an arm whose body is a block needs no comma. An arm may carry a
guard: `pattern if condition => expression`. A guarded arm never counts
toward exhaustiveness.

### 5.5 Loops

```
loop { … }                 // until `break`
while c { … }              // while the bool condition holds
break;    break expr;      // `break` with a value applies to `loop`
continue;
```

`for` is reserved: iteration is a trait, and traits are Ch. 4. Until then a
loop over an array is a `while` and an index, which is also what it lowers to.

### 5.6 There is no three-way conditional

`match a <=> b` is the three-way dispatch, and Ch. 1 §5 already calls it the
idiomatic one. A dedicated construct — `cmp3`, `if3`, or a three-armed `if` —
was considered and declined: it would be a second way to write something the
language already writes well, and `match` carries exhaustiveness checking that
a bespoke form would have to re-earn.

That `match` lowers to a single `br3` is a codegen guarantee (Ch. 1 §5, TIR
§3.6), not a syntactic one. The syntax does not need to advertise the machine.

### 5.7 `return`

`return expr;` leaves the enclosing function. It is not needed for the last
expression of a function body, which is already its value, and is expected to
be rare.

---

## 6. Grammar summary

Informative; the normative content is §§1–5. `A*` is zero or more, `A?`
optional, `A,*` a comma-separated list with an optional trailing comma.

This summary covers Chapters 0, 3 and 4. It is *this* chapter's job to be
complete about the surface syntax, so productions the later chapters
introduce — references, generics, traits, closures — are here even though
their meaning is not.

```
file        := item*
item        := attr* ( fn | struct | enum | const | trait | impl )
attr        := '#' '[' ident '(' ident,* ')' ']'

generics    := ( '<' ( lifetime | ident bounds? | 'const' ident ':' type ),* '>' )?
bounds      := ':' ( lifetime | ident ) ( '+' ( lifetime | ident ) )*
where       := ( 'where' ( ( lifetime | ident ) bounds ),* )?
lifetime    := "'" ident ( ':' lifetime ( '+' lifetime )* )?

fn          := 'fn' ident generics '(' param,* ')' ( '->' type )? where
               ( block | ';' )
param       := self_param | ident ':' type
self_param  := 'self' | '&' lifetime? 'mut'? 'self'          -- Ch. 4 §1.4
struct      := 'struct' ident generics
               ( '{' field,* '}' | '(' type,* ')' ';' | ';' )
field       := ident ':' type
enum        := 'enum' ident generics '{' variant,* '}'
variant     := ident ( '(' type,* ')' | '{' field,* '}' )? ( '=' expr )?
const       := 'const' ident ':' type '=' expr ';'

trait       := 'trait' ident bounds? '{' assoc_item* '}'    -- Ch. 4 §1.1
impl        := 'impl' generics '!'? ident ( 'for' ident targs )? where
               '{' assoc_item* '}'                          -- Ch. 4 §§1.2, 5.1
assoc_item  := fn | 'type' ident bounds? ( '=' type )? ';'
             | 'const' ident ':' type ( '=' expr )? ';'      -- Ch. 4 §1.7

targs       := ( '<' ( type | lifetime ),* '>' )?
type        := 'trit' | 'bool' | 't9' | 't27' | 'taddr' | 'char' | '!' | 'Self'
             | '(' type,* ')' | '[' type ( ';' expr )? ']'
             | '&' lifetime? 'mut'? type                     -- Ch. 3 §2.1
             | 'dyn' ident                                   -- Ch. 4 §3.1
             | 'impl' ident '(' type,* ')' ( '->' type )?    -- Ch. 4 §2.2
             | ident targs ( '::' ident )*                   -- Ch. 4 §§2.1, 1.7

block       := '{' stmt* expr? '}'
stmt        := 'let' 'mut'? pattern ( ':' type )? '=' expr ';'
             | expr ';' | item

expr        := assign
assign      := or ( ( '=' | '+=' | … ) assign )?
or          := and ( '||' and )*
and         := compare ( '&&' compare )*
compare     := spaceship ( ( '==' | '!=' | '<' | '<=' | '>' | '>=' ) spaceship )?
spaceship   := shift ( '<=>' shift )?
shift       := sum ( ( '<<' | '>>' ) sum )*
sum         := product ( ( '+' | '-' ) product )*
product     := cast ( ( '*' | '/' | '%' ) cast )*
cast        := unary ( 'as' type )*
unary       := ( '-' | '!' | '&' 'mut'? | '*' )* postfix
postfix     := primary ( '(' expr,* ')' | '.' ident | '.' int
                       | '.' ident '(' expr,* ')' | '[' expr ']' )*
primary     := literal | path | '(' expr,* ')' | array | struct_lit
             | block | if | match | loop | while | for | closure
             | 'break' expr? | 'continue' | 'return' expr?

for         := 'for' ident 'in' expr block                   -- Ch. 4 §5.7
closure     := ( '||' | '|' ( ident ( ':' type )? ),* '|' )
               ( '->' type block | expr )                    -- Ch. 4 §4.1

literal     := int | trit | 'true' | 'false' | char_lit    -- §1.4, Ch. 5 §1.4
char_lit    := "'" ( char | escape ) "'"
escape      := '\\' ( 'n' | 'r' | 't' | '\\' | "'" | '"' | '0'
                    | 'u' '{' hex{1,6} '}' )

array       := '[' expr,* ']' | '[' expr ';' expr ']'
struct_lit  := path '{' ( ident ( ':' expr )? ),* '}'
if          := 'if' expr block ( 'else' ( if | block ) )?
match       := 'match' expr '{' arm,* '}'
arm         := pattern ( 'if' expr )? '=>' expr
loop        := 'loop' block
while       := 'while' expr block

pattern     := alt ( '|' alt )*
alt         := '_' | literal | ident ( '@' alt )? | path
             | path '(' pattern,* ')' | path '{' field_pat,* '}'
             | '(' pattern,* ')'
path        := ident ( '::' ( ident | '<' type,* '>' ) )*    -- Ch. 4 §2.3
```

> **Note on keeping this current (informative).** Draft 0.1 shipped this
> summary describing §§1–5 only, while claiming in §7 that "the type grammar
> admits `Name<T>` so that adding them changes no other rule" — which it did
> not, since the production had no such alternative. It also omitted `&mut`,
> which Ch. 3 §2.1 makes one of the two reference forms. A grammar summary
> that lags the chapters citing it is worse than none, because a reader
> checking whether something is writable will believe it. Naming §6's sweep
> rule applies here.

---

## 7. What this chapter deliberately does not define

- **Strings and characters.** §1.4; defined by Ch. 5 §1.
- **Generics, traits, `impl`.** Their meaning is Chapter 4's; their syntax is
  in §6, because a reader asking "can I write this?" should not have to read
  four chapters to find out.
- **References and borrowing beyond `&T`'s spelling.** Chapter 3.
- **Modules, visibility, multiple files.** `mod`, `use` and `pub` are
  reserved.
- **Closures.** Reserved; `|` would need re-examining if they arrive, which
  §2.5 already leaves room for.
- **Macros.** Reserved. `println!` does not exist and will not until there is
  something for it to print.
- **`for` loops.** §5.5.
- **Per-item overflow control.** §3.4.

---

## Appendix (informative) — a whole program

Everything below is defined by this chapter, and nothing in it waits on an
unwritten one.

```rust
// hello.tr — the smallest interesting program in Trust.
//
// The message is an array of UTF-8 code units, one per t9, because §1.4 has
// no string literal to offer and will not pretend otherwise.

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
```

And the three-way dispatch this language exists for, which is one comparison
and one branch:

```rust
enum Step { Back = -1, Stay = 0, Forward = 1 }

fn step_toward(target: t27, pos: t27) -> Step {
    match pos <=> target {
        -1t => Step::Forward,
         0t => Step::Stay,
         1t => Step::Back,
    }
}
```
