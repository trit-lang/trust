# Language Specification, Chapter 5 — The Library

| | |
|---|---|
| **Status** | Draft 0.1 |
| **Depends on** | `spec/00-abstract-machine.md` (*AM*), Language Ch. 0 (*Syntax*), Ch. 1 (*Types*), Ch. 2 (*Composites*), Ch. 3 (*References*), Ch. 4 (*Generics*) |
| **Language** | **Trust** (see `spec/01-naming.md`) |

Every chapter before this one deferred something to "the library chapter".
This is that chapter, and its first job is to discharge those:

| Made in | Discharged in |
|---|---|
| AM §5 — text encoding | §1 |
| Ch. 0 §1.4 — character and string literals, reserved | §1.4 |
| Ch. 2 §6 — `kleene`, the lifting of three-valued logic | §6.2 |
| Ch. 2 §8 — a recursive type, written against `Box` | §2.3 |
| Ch. 3 §1.5 — a destructor that releases a real resource | §2.3 |
| Ch. 3 §6 — `Box`, and interior mutability | §2, §5 |
| Ch. 4 §3.1 — `Box<dyn Trait>` | §2.4 |
| Ch. 4 §5.2 — the function that drops a value early | §6.1 |
| Ch. 4 §5.7 — the `Iterator` adaptors | §3 |
| ISA assembly §9 — `.string` | §1.5 |

Four things here are not inherited from Rust, and each is forced by something
this machine or this language has:

- **Text is fixed-width, one character per word** (§1). UTF-8's variable width
  is a compromise made for a machine whose smallest addressable unit holds 256
  values. A tryte holds 19 683, and on this machine the compromise buys back
  less than it costs.
- **`s.len()` is a number of characters and `s[i]` is a character** (§1.3),
  because the encoding makes both O(1). In Rust neither is available, and the
  reason is UTF-8.
- **There is no `Try` trait** (§4.2). `?` is two rules over `Result` and
  `Option`. Rust's `Try`/`FromResidual` exists to make `?` extensible; the
  extension has few users and the trait is one most readers cannot state.
- **`expect` does not exist** (§4.3). A message needs somewhere to go, and AM
  §4 gives a fault a code and nothing else. `unwrap` is defined, and it traps.

> **Design note (informative).** Chapters 3 and 4 inherited from Rust by
> default, because their failure modes are documented and a novel variation
> spends a reader's understanding on something unrelated to the radix. This
> chapter departs more often, and the reason is that a library is where a
> machine's actual units show through. Ownership does not care how wide a
> tryte is. Text does.

---

## 1. Text

### 1.1 Why not UTF-8

UTF-8 is a variable-width encoding, and every property that makes it the right
answer on a binary machine follows from one fact: a byte holds 256 values,
which is not enough for a character, so *something* must be variable. Given
that, UTF-8 makes the best of it — ASCII stays one unit, the encoding is
self-synchronizing, and no code unit is ever mistaken for another.

A tryte holds **19 683** values. That is still not enough for a character
(Unicode has 1 112 064 of them), so a single tryte cannot be the unit either.
But the next unit up — a word, 27 trits, 7 625 597 484 987 values — is enough
several thousand times over, and a word is a width this machine loads and
stores in one instruction (AM §2.2).

So the choice is between a variable-width encoding in trytes and a fixed-width
one in words, and the storage cost is not what a reader raised on UTF-8
expects:

| Code points | UTF-8, one code unit per tryte | One character per word | |
|---|---|---|---|
| U+0000 … U+007F (ASCII) | 1 tryte | 3 | 3× |
| U+0080 … U+07FF (Greek, Cyrillic, Arabic) | 2 | 3 | 1.5× |
| U+0800 … U+FFFF (CJK, most of the BMP) | 3 | 3 | **1×** |
| U+10000 … (astral planes) | 4 | 3 | **0.75×** |

Fixed width costs three times as much for ASCII, the same for CJK, and less
for anything above the BMP. What it buys is that `s[i]` and `s.len()` are
what a reader thinks they are.

This chapter therefore defines **one character per word**, and UTF-8 becomes
what it is for a machine like this: an interchange format, converted at the
boundary (§1.5).

> **Erratum against AM §5 (normative).** AM §5 states that the library chapter
> specifies "a tryte-based UTF-8 carrier format as the interop default; a
> native ternary text encoding is a reserved appendix". That is the reverse of
> this chapter, and this chapter governs: the native encoding is the storage
> format, UTF-8 is the conversion. A *denser* native encoding is what is
> reserved, in Appendix A. AM §5 is corrected accordingly.

### 1.2 `char`

A **`char`** is a Unicode scalar value: an integer in `0 ..= 0x10FFFF`
excluding the surrogate range `0xD800 ..= 0xDFFF`, which is what Unicode
reserves for UTF-16 and which is not a character.

`char` is a distinct nominal type, one **word** wide, word-aligned. It is
`Copy`, `Eq` and `Ord`, and its order is the order of its scalar values.

**Why a word and not two trytes.** Two trytes (18 trits, 387 420 489 values)
would hold every scalar value with room to spare, and would be denser. But the
machine has exactly two access widths, tryte and word (AM §2.2, ISA §4.4);
18 trits is neither, so every read of an 18-trit character would be two loads
and an assembly, and every write two stores. The width that costs one
instruction is the one that gets used.

**Conversions.** `char as t27` is the scalar value. `t27 as char` does not
exist: not every word is a character, and Ch. 1 P2 does not permit a
conversion that can be wrong to be silent. `char::try_from(x: t27) -> Option<char>`
is the checked form, and `char::from_digit`/`to_digit` are the two the radix
makes worth naming.

**Niche.** A `char` occupies 1 112 064 of a word's 7 625 597 484 987 values,
so Ch. 2 §6's niche rule applies with enormous headroom: `Option<char>`,
`Option<Option<char>>` and any tower of them are one word.

### 1.3 `str`, `String`

**`str`** is `[char]` — a slice of characters, and a dynamically sized type in
exactly the sense of Ch. 2 §8. `&str` is the fat pointer that chapter
describes: an address and a length, two words. The length is a number of
**characters**.

Because the encoding is fixed width:

| | Trust | Rust |
|---|---|---|
| `s.len()` | characters | bytes |
| `s[i]` | a `char`, O(1) | does not compile |
| `&s[a..b]` | O(1), always valid | O(1), panics on a non-boundary |
| iterating characters | `s.iter()` | `s.chars()`, O(n) to reach the *i*-th |

There is no distinction between a "byte index" and a "character index",
because there is only one index. This removes a class of bug rather than
documenting it.

**`String`** is the growable owned form: `Vec<char>` with the string methods
(§2.6). It needs the heap and is defined with it.

A string is **not** normalized, and `char`-wise equality is not
canonical equivalence. §1.7 says what that means.

### 1.4 Literals

Ch. 0 §1.4 reserves character and string literals. This chapter unreserves
them.

```
'a'        'é'      '一'      '\n'      '\u{1F600}'
"hello"    "一二三"   ""
```

A **character literal** is one `char` between single quotes. A **string
literal** is a `&str` between double quotes, whose characters live in static
storage for the whole program and are therefore `&'static str`.

The escapes are `\n`, `\r`, `\t`, `\\`, `\'`, `\"`, `\0`, and `\u{…}` with
one to six **hexadecimal** digits naming a scalar value.

Hexadecimal, in a language whose own numeric literals are decimal, `0t` and
`0h`, and the exception is deliberate. `\u{…}` does not name a number in this
language: it names a code point in an external standard, and that standard,
every character table, and every editor's "insert character" dialog write it
one way. An escape that had to be transcribed into heptavintimal before it
could be checked against the reference it came from would be unreadable
against the thing it names. `0h` remains the spelling for numbers this
language owns.

**There is no `\x` escape.** `\xNN` names a byte, and this chapter's text has
no bytes in it. A program that wants a particular scalar value writes
`\u{…}`; a program that wants a particular *byte* is building an interchange
buffer and writes a `[t9]`, which is what it is.

The source file itself is UTF-8. That is a fact about files, not about the
language's text: the lexer decodes the source and stores what it read as
characters.

### 1.5 Interchange

UTF-8 is where text meets everything that is not this machine, and the
conversion is a library function, not a representation:

```
fn char.to_utf8(self) -> ([t9; 4], taddr);          // units, and how many
fn char.from_utf8(units: &[t9]) -> Option<(char, taddr)>;
fn str.to_utf8(&self, out: &mut [t9]) -> Option<taddr>;
fn str.from_utf8(units: &[t9]) -> Option<String>;
fn str.chars(&self) -> Chars;                       // an Iterator<Item = char>
                                                    // `for c in s` is this
```

A `[t9]` here holds one UTF-8 code unit per element, values `0 ..= 255`. Every
one of these is fallible where the input can be wrong, and none of them is
fallible where it cannot: `to_utf8` on a `char` always succeeds, because a
`char` is always encodable.

The reference target's character ports (ISA §2.2) are UTF-8 code-unit ports
and stay that way. That is a property of the device, and **the library encodes
on the way out** — `print` and `println` are in the library, written in Trust:

```
fn putchar(c: t9);                  // no body: it reaches a device

fn print(s: &str) {
    for c in s {
        let (units, n) = c.to_utf8();
        let mut i: taddr = 0;
        while i < n { putchar(units[i]); i += 1; }
    }
}

fn print_char(c: char) { … }        // the same, for one character
fn println(s: &str) { print(s); putchar(10); }
```

`print_char` is there because the asymmetry was real: a library that prints a
string and not a character leaves a program holding a character it *computed*
— as against one it wrote down — with nothing to call but `putchar`, doing its
own encoding at exactly the boundary this section says the library owns.

So `putchar` is a **required target function**, the third after `alloc` and
`free` (§2.1) and there for the same reason: it reaches a memory-mapped device
and TIR has no way to name one. Everything above it is ordinary Trust, and a
program that never prints emits none of it.

This is the one place the "I/O is per target" rule of AM §5 is qualified, and
the qualification is narrow: the *character ports* are part of what a target
must offer, and nothing else about I/O is. A target with no console supplies a
`putchar` that discards, the way a target with no heap supplies an `alloc`
that fails.

A program may declare `putchar` itself; its item shadows the prelude's, and
both resolve to the target's one symbol.

The assembler's `.string` directive (assembly §9) emits the **native**
encoding: one word per character, so that a `&'static str` built by the
assembler and one built by the compiler are the same thing. A directive that
emits UTF-8 code units is `.utf8`, and it emits trytes.

### 1.6 Numbers out

```
fn print_int(n: t27);      // decimal, with a sign where there is one
fn println_int(n: t27);
```

One argument and no composition, which is what keeps this outside §7: a
`Display` or a `format!` needs variadic arguments or a macro, and this needs
neither. It is the digit loop §7 says every program writes, written once.

The loop is not the one a binary machine would write. `%` is the **symmetric**
remainder (Ch. 1 §4), so `n % 10` runs −9 ..= 9 and a negative digit borrows
from the next place. That is the same rounding rule that makes `x >> 11`
exactly division, and it costs one `if` here.

There is no `print_ternary`, and there could be: balanced ternary is the
machine's own notation and `0t` is its literal (Ch. 1 §3). It is left out
because nothing has needed it that a program could not write in six lines,
and because the digit that has no character — `T` for −1 — is a choice this
section would have to make.

### 1.7 What a character is not

A `char` is a Unicode scalar value. It is not:

- **A user-perceived character.** `é` may be one scalar value (U+00E9) or two
  (U+0065 U+0301), and a flag emoji is two. Grapheme cluster segmentation is
  **not defined** in draft 0.1.
- **A normalization form.** `"é".len()` may be 1 or 2 depending on which
  sequence produced it, and the two do not compare equal. Normalization is
  **not defined**.
- **A collation key.** `Ord` on `str` is scalar-value order, which is not
  anyone's alphabetical order. Locale-aware collation is **not defined**.

These are reserved rather than approximated. Each of them is a table-driven
problem whose tables are larger than this specification, and a language that
shipped a wrong answer would be worse than one that shipped none: a program
that needs them can be written against a library that has them, and a program
that does not need them pays nothing.

---

## 2. The heap

### 2.1 What draft 0.1 can and cannot say

An allocator's job is to turn a size into an address. TIR §5 has no
integer-to-pointer conversion, deliberately, and Ch. 3 §6 reserves `unsafe`
along with raw pointers. So **an allocator cannot be written in this language
as it stands**, and neither can `Box`.

Draft 0.1 therefore says the smallest true thing:

- **`Box<T>` is a language item**, like `Drop` glue: the compiler knows it,
  and its inside is not Trust.
- **There is one allocator, and the target supplies it.** Its interface to a
  program is two declared functions with no body, exactly as `putchar` is
  (Ch. 0 §3.1) — and `putchar` is declared in the library alongside them
  (§1.5), so a target owes exactly three functions and no more.
- **`trait Allocator`, and a `Box` parameterized by one, are reserved** to the
  `unsafe` chapter. Writing them is the operation this language does not have.

```
fn alloc(size: taddr, align: taddr) -> taddr;    // 0 if it cannot
fn free(at: taddr, size: taddr, align: taddr);
```

These are the *target's* interface, not a program's: a program calls neither.
They are written down because a target implementer needs to know what to
supply, and because naming them is what makes `Box` a definition rather than
magic.

### 2.2 Where the memory is

The AM says an address space is `0 ..< A` and nothing else (AM §2.1), so
"where the heap is" is a target question. For the reference target
(TRISC-27 §2):

- The image is loaded at 0 and ends at an address the assembler publishes as
  the symbol **`_end`**.
- The stack begins at `MEM_SIZE` and grows down (ISA §6.2).
- The heap begins at `_end`, rounded up to a word, and grows up.

The two meet in the middle, and nothing detects the collision: a program that
exhausts memory gets `alloc` returning 0 (§2.5) or a stack that runs into the
heap, and the second of those is not diagnosed. That is the same situation
every machine without a memory management unit is in, and draft 0.1 does not
pretend otherwise.

`_end` is defined by the assembler, which is the only thing that knows where
the image stops (assembly §3.4).

### 2.3 `Box<T>`

```
struct Box<T: ?Sized>;    // language item

impl<T> Box<T> {
    fn new(value: T) -> Box<T>;                 // traps if it cannot
    fn try_new(value: T) -> Option<Box<T>>;
    fn into_inner(b: Box<T>) -> T;
}
```

A `Box<T>` owns one `T` on the heap. It is:

- **One word** for a sized `T`, holding the address — so Ch. 3 §2.4's
  representation facts apply unchanged, and `Option<Box<T>>` is one word
  because 0 is not the address of anything.
- **Not `Copy`.** It owns; copying it would give two owners of one allocation.
- **Dropped** by dropping the `T` and then freeing, in that order.
- **Dereferenced** with `*b`, which is a place, so `b.field` and `&mut *b`
  work as Ch. 3 §2.3 says.

This is what Ch. 2 §8's recursive-type example was written against, and it is
now writable:

```
enum Tree {
    Leaf,
    Node(Box<Tree>, t27, Box<Tree>),
}
```

The type is finite because `Box<Tree>` is one word whatever `Tree` is, which
is the whole reason indirection makes recursion possible.

And it is the first real resource Ch. 3 §1.5 said the destructor mechanism was
specified in advance for. Nothing about `Drop` changes to accommodate it.

### 2.4 `Box<dyn Trait>`

Two words: the fat pointer of Ch. 4 §3.2, in the representation that section
already fixed for this purpose. `Box<dyn Trait>` owns the value, so dropping
it calls the destructor through the vtable's drop slot (Ch. 4 §3.3) and then
frees `size` trytes — which is why that slot and the size field are in the
vtable at all.

This is what lets a trait object outlive the frame that made it, and it is
therefore what makes `fn shapes() -> Vec<Box<dyn Shape>>` writable where
Ch. 4 §4.5's "cannot return a closure" restriction still stands for `&dyn`.

### 2.5 Failure

`alloc` returns 0 when it cannot satisfy a request. What a program sees
depends on which constructor it used:

- **`Box::new` traps** — `F_TRAP`, no unwinding, no message.
- **`Box::try_new` returns `None`.**

The trapping default is the same decision Ch. 2 §3 makes for an
out-of-bounds index and Ch. 1 §4 makes for a `.trap` overflow: a failure the
program did not say what to do about stops the program. There is no third
option, because unwinding needs a mechanism AM §4 does not have — a fault
halts, and has no handler.

### 2.6 `Vec<T>` and `String`

```
struct Vec<T>;      // pointer, length, capacity — three words
struct String;      // Vec<char>, with the methods of §1.3
```

`Vec<T>` is a growable array. It **was** a language item for `Box`'s reason
and one more — the room beyond the length is memory that is *not yet a `T`*,
and this language had no way to say that — and §2.7 now says exactly that
much, so `Vec` and `String` are **library code**: Trust source, compiled like
any other, resting on one item instead of being one. What follows is
therefore a description of what the library must do rather than of what the
compiler must generate.

| | |
|---|---|
| `Vec::new()` | three zero words; no allocation |
| `push(x)` | appends, growing if it must |
| `pop() -> Option<T>` | moves the last element out; `None` when empty |
| `len()`, `capacity()`, `is_empty()` | the second word, the third, and a test on the second |
| `reserve(n)` | room for `n` **more** than there are |
| `with_capacity(n)` | empty, with room for `n` |
| `clear()` | drops every element; keeps the allocation |
| `insert(i, x)`, `remove(i)` | move a run of elements by one place |
| `v[i]` | bounds-checked against the **length** |

`insert` accepts `i == len`, which is `push`; `remove` returns the element,
read before the shift so that the shift does not drop it. Both shifts are
copies of storage and therefore moves of every element in them (Ch. 3 §1.2),
and `insert`'s runs **downwards**, because a forward copy moving a block up
overwrites what it has yet to read.

A slice's own `copy_within` — the same operation where a program can call it —
is §7's, and is not defined.

`v[i]` is `index`/`index_mut` by Ch. 2 §3.1, which is the rule that lets a
library type be indexed at all.

Indexing is bounds-checked against the **length**, not the capacity — the room
beyond it holds nothing a program may read. `capacity` is the only method that
can see that room, and it can see only how much of it there is.

Growth **doubles** the capacity, from a first allocation of 4 elements. The
factor is specified rather than left open because a program that pushes *n*
elements is entitled to know that it did O(*n*) work in total, and the
amortized argument is a property of the factor. `reserve` does **not** double:
a program that says how much it wants has said something the guess cannot
improve on, so it gets `len + n` exactly.

There is no `realloc` (§7), so growing is three steps — allocate, copy, free —
and the copy is a copy of storage, which is what a move is (Ch. 3 §1.2).

`&Vec<T>` **coerces to `&[T]`**: the allocation and the length, which are a
`Vec`'s first two words and a slice's only two. The capacity is left behind,
and that is the whole of the conversion — a slice may read what is there and a
`Vec` may grow, and only one of those needs to know how much room is left.
This and `&Concrete` to `&dyn Trait` (Ch. 4 §3.2) are the only implicit
conversions in the language, and both convert a *representation* rather than a
value.

`String` **is** `Vec<char>` — not a wrapper around one — so it needs almost no
rules of its own: `String::new`, `with_capacity` and `push(char)` are `Vec`'s,
`&String` becomes `&str` by the coercion above, and every method §1.3 gives a
string applies to one the moment it does. `push_str(&str)` is the one method
that is a `String`'s alone: a loop around `push`, written in the library as
`impl Vec<char>` — an impl for one instantiation (Ch. 4 §2.1).

---

### 2.7 `Raw<T>` — room that is not yet a `T`

§2.6 gives two reasons `Vec` cannot be written in this language: it needs an
address, and **the room beyond the length is memory that is not yet a `T`**.
The second is the harder one, and it is the only thing standing between the
collections and the library.

Draft 0.1 names exactly that much and no more.

```
struct Raw<T>;    // language item — an address and a count, two words

impl<T> Raw<T> {
    fn new() -> Raw<T>;                     // no room; no allocation
    fn alloc(n: taddr) -> Raw<T>;           // room for n; traps if it cannot
    fn try_alloc(n: taddr) -> Option<Raw<T>>;
    fn room(&self) -> taddr;                // how many positions there are
    fn read(&self, i: taddr) -> T;          // the i-th, as a T
    fn write(&mut self, i: taddr, value: T);
}
```

A `Raw<T>` **owns room** and knows nothing about what is in it. It is:

- **Two words** — the address and the count — because a position is checked
  against the count and a checked index needs the count to be there. That is
  the same shape as a slice (Ch. 2 §4), and for the same reason.
- **Not `Copy`**, for `Box`'s reason: it owns an allocation.
- **Dropped** by freeing the room and **dropping nothing**, because it does
  not know which positions hold a `T`. Whatever does know must drop them
  first — which is exactly what `Vec`'s own destructor is for.
- **Bounds-checked**: `read` and `write` trap on `i >= room()`, by Ch. 2 §3's
  rule for an index, because there is no reason for this to be the one place
  that does not check.

`alloc` asks the target's allocator for `n` times the size of a `T`, aligned
as a `T` is, and traps on 0 exactly as `Box::new` does (§2.5).

**What this chapter does not define** is the one thing left: `read` at a
position nothing has written. It is not a fault and not an error — the trits
are whatever the allocator left there — and it is not a `T` either, so what a
program does with it is outside this chapter. A caller that reads only what
it wrote never asks the question, and `Vec` is written that way: it reads
below its length and its length counts writes.

That is the whole of the unsoundness, and it is worth saying plainly what has
been bought with it. `Raw<T>` is a **language item** and so is not derivable
from these chapters — but it is five operations whose lowering is a
sentence each, where `Vec` was a dozen methods with a growth policy and two
shifting loops inside them. Everything in §2.6 that is a *decision* — the
doubling, the first allocation of four, `reserve` not doubling, `insert`
shifting downwards, the bounds rule being the length and not the capacity —
is Trust source in the library now, written once and compiled by whoever is
compiling. A second implementation has to agree about `Raw`, and can derive
the rest.

`Box<T>` is `Raw<T>` with a count of one and a destructor that drops what is
there — and is kept as its own item because §2.3's guarantees about
`Option<Box<T>>` and the fat pointer of §2.4 are about `Box` and not about
room.

---

## 3. Iterators

Ch. 4 §5.7 defines `Iterator` and `IntoIterator` and desugars `for`. What that
section deferred is the adaptors, and what it fixed is that they *can* be
written: each needs a closure (Ch. 4 §4) and an associated type (Ch. 4 §1.7),
and both exist. Nothing in this section is a language feature; every one of
these is a struct and an `impl` a reader could have written.

### 3.1 The lazy adaptors

Each returns a struct that implements `Iterator` and does nothing until it is
asked for a value.

| | |
|---|---|
| `map(f)` | `Item` becomes `f`'s result |
| `filter(p)` | keeps the items `p` accepts |
| `take(n)`, `skip(n)` | a prefix, and everything after one |
| `take_while(p)`, `skip_while(p)` | the same, bounded by a predicate |
| `zip(other)` | pairs, ending with the shorter |
| `enumerate()` | `(taddr, Item)`, counting from 0 |
| `chain(other)` | one after the other; both `Item`s must agree |
| `peekable()` | adds `peek(&mut self) -> Option<&Item>` |

Each is a **provided method** on `Iterator`, so an implementation gets all of
them by writing `next` alone (Ch. 4 §1.5), and each returns a struct whose
`Item` follows the iterator underneath it:

```
let it = Upto { n: 10, at: 0 };
sum(it.map(|x| x * x).filter(|v| v % 2 == 0))
```

`map`'s struct is the shape all of them have:

```
struct Map<I, F> { inner: I, f: F }

impl<I: Iterator, B, F: Fn(I::Item) -> B> Iterator for Map<I, F> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        match self.inner.next() {
            Option::Some(x) => Option::Some((self.f)(x)),
            Option::None => Option::None,
        }
    }
}
```

**`rev` is not here.** Reversing needs an iterator that can be advanced from
either end, which is a second trait (`DoubleEndedIterator`) and a second
method on every adaptor that could support it. It is **reserved**; a program
that wants to walk a slice backwards writes the index loop, which is what the
adaptor would have become.

### 3.2 What laziness costs

An adaptor chain is a tower of `next` calls, one per stage per item. Nothing
in this specification makes them free; what makes them free is inlining, and
that is a compiler's business rather than a language's.

Ch. 4 §5.7 already states the position for slice iteration — "`next` is an
index and a bounds comparison the optimizer is expected to fuse with the
loop's own" — and it extends unchanged. This chapter states the *intent*
that an adaptor chain compile to the loop a reader would have written, and
records that the intent is not a guarantee. A program in a hot loop that
cannot afford to find out writes the loop.

### 3.3 The consuming methods

```
count()  sum()  product()  fold(init, f)  for_each(f)
all(p)   any(p) find(p)    position(p)    min()  max()
last()   nth(n) collect()
```

`sum` and `product` need `Add` and `Mul` (Ch. 4 §5.4); `min` and `max` need
`Ord` (Ch. 4 §5.3). Each is a `while let` over `next` and nothing more.

The ones that work for **any** item are provided bodies on the trait, so an
implementation gets them by writing `next` alone (Ch. 4 §1.5). The ones that
need arithmetic or an order are free functions taking the bound on the
*iterator* — `fn sum<I: Iterator<Item = t27>>(it: I) -> t27` — because a bound
on an associated type is not written in draft 0.1 and this says the same
requirement where the language can say it.

`collect` is the one that needs a trait, because what it collects *into* is
chosen by the context:

```
trait FromIterator {
    type Elem;
    fn from_iter<J: Iterator<Item = Self::Elem>>(it: J) -> Self;
}

fn Iterator.collect<C: FromIterator<Elem = Self::Item>>(self) -> C;
```

The element is an **associated type, not a parameter**. A type is collected
into from one element type and no other: there is no "collect a `Vec<T>` from
an iterator of `&T`" here, because there is nothing to clone with, and
`String` *is* `Vec<char>` rather than a second thing collecting characters. So
the element is an output of the implementation, which is what an associated
type is for. This needed Ch. 4 §1.7 either way.

`Vec<T>` implements it, and `String` therefore does. `Option<Vec<T>>` — how a
sequence of fallible steps collects into one result — is **not** implemented.

### 3.4 `Range`

```
struct Range<T> { start: T, end: T }

impl<T> Iterator for Range<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start < self.end {
            let v = self.start;
            self.start += 1;
            Option::Some(v)
        } else {
            Option::None
        }
    }
}
```

That is the whole of it, and it is what `a..b` means (Ch. 0 §5.5). It needs no
bound: `<` and `+= 1` are Ch. 1's on every type that has them, and a `Range`
of anything else does not compile at the point it is used.

**What it costs.** `for i in 0..n` is measurably dearer than the `while` and
an index it replaces — 54 085 instructions against 7 010 for a thousand
iterations, at the time of writing. The reason is not the range: it is that
`next` returns an `Option<T>`, an `Option<t27>` has no niche and so is two
words, and two words go through memory. Every iterator pays this per item;
the range is only where it is easiest to see.

---

## 4. Errors

### 4.1 `?`

Ch. 0 reserves `?`. This chapter defines it, as **two rules and no trait**.

In a function whose result is `Result<U, F>`:

```
let v = e?;
```

evaluates `e` to a `Result<T, E>`; if it is `Ok(v)` the expression is `v`, and
if it is `Err(err)` the function returns `Err(F::from(err))` immediately. `F`
must implement `From<E>` (Ch. 4 §5.6), and where `F` *is* `E` the blanket
identity impl makes the conversion nothing.

In a function whose result is `Option<U>`:

```
let v = e?;
```

evaluates `e` to an `Option<T>`; `Some(v)` is `v`, and `None` returns `None`.

**The two do not mix.** `?` on an `Option` in a function returning `Result` is
an error, and so is the reverse. Converting between them is `ok_or(err)` and
`ok()`, which are written where they happen.

`?` returns from the function, so every value the frame owns is dropped on the
way out exactly as `return` drops them (Ch. 3 §1.4). It is not a new control
flow; it is `match` and `return`, spelled shorter:

```
let v = match e {
    Result::Ok(v) => v,
    Result::Err(err) => return Result::Err(F::from(err)),
};
```

### 4.2 Why there is no `Try` trait

Rust defines `?` through `Try` and `FromResidual` so that a user type can
participate. Two reasons not to inherit that here:

- **The extension has almost no users.** `Result` and `Option` are the two
  types `?` is for; a third is rare enough that the machinery costs more than
  it returns.
- **The trait is one most readers cannot state.** `FromResidual` exists to
  express "the failure case of one type converted into the failure case of
  another" without naming either, and the result is a signature that has to be
  looked up every time. A language whose whole argument is that a reader can
  tell what code does should not have a control-flow operator defined by a
  trait its users cannot recite.

A user type that wants `?` converts to `Result` first, which is one method
call and is visible.

`Try` is **reserved**, not rejected: if a third type earns it, the operator's
definition can move behind a trait without changing what any existing program
means.

### 4.3 `unwrap`, and the message that has nowhere to go

```
fn Option.unwrap(self) -> T;        // traps on None
fn Result.unwrap(self) -> T;        // traps on Err
fn Option.unwrap_or(self, d: T) -> T;
fn Option.unwrap_or_else(self, f: impl Fn() -> T) -> T;
```

`unwrap` traps with `F_TRAP`. There is no unwinding and no handler (AM §4),
so it is the same kind of stop as an out-of-bounds index — and it is written
in the language, because Ch. 1 §6's `trap()` has type `!` and an arm with no
value is an arm of any type:

```
fn unwrap(self) -> T {
    match self { Option::Some(v) => v, Option::None => trap() }
}
```

**`expect(msg)` does not exist.** Its whole value is the message, and a
message needs somewhere to go: AM §4 gives a fault a code and nothing else,
and a target that could print would be printing from inside a failure, on a
port the failing program may itself have been using. Draft 0.1 declines to
have a diagnostic that is a message on some targets and nothing on others.

This is a real loss and it is recorded as one. It comes back with whatever
mechanism carries a panic message, which is the same mechanism a debugger
would need, and neither exists yet.

---

## 5. Interior mutability

Ch. 3 §6 lists these as "library types built on `unsafe`". `unsafe` is still
reserved, so they are **language items** for the same reason `Box` is: what
they do — hand out a `&mut` where the aliasing rule (Ch. 3 §2.2) would not —
cannot be written in this language, and pretending otherwise would be worse
than naming it.

### 5.1 `Cell<T>`

```
struct Cell<T: Copy>;                 // language item

fn Cell.new(v: T) -> Cell<T>;
fn Cell.get(&self) -> T;
fn Cell.set(&self, v: T);
fn Cell.replace(&self, v: T) -> T;
```

`Cell` hands out **no references at all**: `get` copies out and `set` copies
in. There is therefore nothing to check at run time and nothing to go wrong,
and that is why it is restricted to `T: Copy` — a non-`Copy` `T` could not be
returned by `get` without moving out of a shared reference.

`Cell<T>` is the same size and alignment as `T`. It has no niche of its own,
and does not remove `T`'s.

### 5.2 `RefCell<T>`

```
struct RefCell<T>;                    // language item

fn RefCell.new(v: T) -> RefCell<T>;
fn RefCell.borrow(&self) -> Ref<T>;         // traps if exclusively borrowed
fn RefCell.borrow_mut(&self) -> RefMut<T>;  // traps if borrowed at all
fn RefCell.try_borrow(&self) -> Option<Ref<T>>;
fn RefCell.try_borrow_mut(&self) -> Option<RefMut<T>>;
```

`RefCell` moves Ch. 3 §2.2's rule from compile time to run time, and keeps the
rule identical: any number of shared borrows, or one exclusive, never both.
`Ref<T>` and `RefMut<T>` are guards whose destructors release the borrow, so
the borrow lasts exactly as long as the guard — which makes the common bug
visible in the source, since a guard that lives too long is a binding that
lives too long.

**The state is a trit and a count.** Ch. 3 §6 observes that the state is
three-valued — unborrowed, shared, exclusive — "though a shared borrow count
needs more than a trit and so the representation is not as neat as it first
looks". This chapter makes it exact: the state is **one word**, holding

| Value | Meaning |
|---|---|
| 0 | unborrowed |
| *n* > 0 | *n* shared borrows |
| −1 | exclusively borrowed |

which is a single word whose *sign* is the three-valued part and whose
magnitude is the count. `cmp` against zero answers "which state" in one
instruction, and that is the neatness Ch. 3 §6 was reaching for: it is not in
a trit, it is in a sign.

Violating the rule **traps**. `try_borrow` is the form for a program that
wants to decide.

---

## 6. What the other chapters named

### 6.1 `drop`

```
fn drop<T>(x: T) {}
```

Ch. 4 §5.2 forbids calling a destructor by hand and says that dropping a value
early is done by moving it into a function that consumes it, "which the
library chapter will name". This is that function, and its body is empty: the
value is moved in, nothing is done with it, and it is dropped at the end of
the call because that is what happens to a value nobody moved out.

Nothing about it is special. It is the smallest possible demonstration that
the ownership rules already do this job.

### 6.2 `kleene`

Ch. 2 §6 observes that `Option<bool>` *is* the value space of three-valued
logic and promises the lifting under this name. `Option<bool>` is one tryte
(Ch. 2 §6, guarantee 2), and the three operations are the AM's own:

```
fn kleene.and(a: Option<bool>, b: Option<bool>) -> Option<bool>;   // tmin
fn kleene.or (a: Option<bool>, b: Option<bool>) -> Option<bool>;   // tmax
fn kleene.not(a: Option<bool>)                  -> Option<bool>;   // neg
```

with `None` as *unknown*, `Some(false)` as −1 and `Some(true)` as +1. Under
that mapping each of the three is **one machine instruction** (AM §3.4), which
is the point Ch. 2 was making: a binary machine emulates Kleene logic with a
table, and this one has it in the ALU.

The mapping is a property of the layout Ch. 2 fixes, so this is a library
function whose implementation the compiler is expected to see through, not a
language rule.

### 6.3 Moving out of a reference

```
fn mem.swap<T>(a: &mut T, b: &mut T);
fn mem.replace<T>(dst: &mut T, v: T) -> T;
fn mem.take<T: Default>(dst: &mut T) -> T;
```

Ch. 3 §2.2 forbids moving out of a place while a reference to it is live,
because the owner does not know. These three are how a program
does it anyway, and each works by putting something back: `swap` exchanges,
`replace` supplies the replacement, `take` uses `Default`'s.

`trait Default { fn default() -> Self; }` is defined here for `take`'s sake
and is derivable (Ch. 4 §6) field by field.

---

## 7. What this chapter deliberately does not define

- **Grapheme clusters, normalization, and collation.** §1.7. Each is a
  table-driven problem whose tables are larger than this document.
- **A dense native text encoding.** Appendix A, reserved.
- **`trait Allocator`, and a `Box` parameterized by one.** §2.1. An
  allocator's own body needs `unsafe`, which Ch. 3 §6 reserves.
- **Reallocation in place, and an allocator interface that can grow a block.**
  `Vec`'s growth allocates and copies. A `realloc` would be a third target
  function, and nothing in draft 0.1 can measure whether it earns its place.
- **`DoubleEndedIterator`, and `rev`.** §3.1.
- **`Try`.** §4.2, reserved rather than rejected.
- **`expect`, and any panic message.** §4.3.
- **Formatting.** There is no `Display`, no `format!`, and no `println!`. Each
  needs either variadic arguments or a macro system; Ch. 0 §7 reserves macros
  and Ch. 0 §1.5 reserves the `name!(…)` position they would use, so the door
  is held open and nothing walks through it here. A program prints a string
  with §1.5's `print` and a number with §1.6's `print_int`; what is missing is
  putting the two in one call with the text around them.
- **Maps, sets, and any collection with a hash.** A hash function is a choice
  with security consequences, and nothing here needs one yet. One requirement
  is fixed in advance, because it cannot be added afterwards: **a map's
  iteration order is unspecified**, and a revision that adds one must say so
  from its first sentence. Once programs exist that depend on an order, the
  order is part of the interface whether or not it was meant to be.
- **Sorting.** `[T]::sort` needs either an allocation for a merge sort or a
  statement about worst cases for a quicksort, and neither is decided.
- **`[T]::copy_within`, and the slice-to-slice copies beside it.** `Vec`'s
  `insert` and `remove` do this operation inside themselves, where it is the
  compiler's and not a program's; offering it on a slice is a separate
  question, because a program can then overlap two ranges of its own
  choosing.
- **Time, files, processes, environment.** The AM is pure and I/O is per
  target (AM §5). Anything beyond the character ports is a target's business.
- **`Rc`, `Arc`, and any shared ownership.** `Rc` is writable once `Cell`
  exists, and is not written here because the first thing a reader asks of it
  is what happens to cycles, and the answer — they leak — deserves the section
  it would take.

---

## Appendix A (informative) — a denser native encoding, reserved

§1.1 chooses one character per word because fixed width is what makes `s[i]`
mean what it says. The cost is 3× for ASCII, and a reader who mostly writes
ASCII will want it back.

There is a variable-width native encoding that is worth reserving, and it is
more ternary than UTF-8 is binary. A tryte is **signed**: −9841 … 9841. So the
*sign trit* can be the continuation marker, at no cost in code space:

| Leading tryte | Meaning |
|---|---|
| 0 … 9841 | the scalar value itself, one tryte |
| < 0 | its magnitude is the high part; one more tryte follows |

That gives 9 842 single-tryte characters — all of ASCII, Latin, Greek,
Cyrillic, Hebrew, Arabic, and the general punctuation — and two trytes for
everything up to 9842 + 9841 × 19683 ≈ 193 million, which is all of Unicode
several times over. It is self-synchronizing in the same way UTF-8 is: a
continuation tryte can be told from a leading one by its sign, so a scan can
start anywhere and find the next boundary in one step.

What it costs is what UTF-8 costs: `s[i]` is O(*i*), and a slice boundary can
be wrong. That is why it is not the storage format. It is the right format for
a *file*, and a future revision that needs one has this appendix.

---

## Appendix B (informative) — worked examples

**Text, with the index meaning what it says.**

```
fn first_upper(s: &str) -> Option<char> {
    let mut i: taddr = 0;
    while i < s.len() {
        let c = s[i];
        if c >= 'A' && c <= 'Z' { return Option::Some(c); }
        i += 1;
    }
    Option::None
}
```

No byte index, no boundary check, no second length.

**A tree, which Ch. 2 §8 wrote and could not run.**

```
enum Tree { Leaf, Node(Box<Tree>, t27, Box<Tree>) }

fn insert(t: Tree, v: t27) -> Tree {
    match t {
        Tree::Leaf => Tree::Node(Box::new(Tree::Leaf), v, Box::new(Tree::Leaf)),
        Tree::Node(l, x, r) => match v <=> x {
            -1t => Tree::Node(Box::new(insert(*l, v)), x, r),
             1t => Tree::Node(l, x, Box::new(insert(*r, v))),
             0t => Tree::Node(l, x, r),
        },
    }
}
```

One `<=>`, three arms, no `else`. The tree is dropped by dropping its
`Box`es, which drop their `Tree`s, which drop theirs — the recursion the
destructor mechanism has had since Ch. 3 §1.4 and had nothing recursive to
apply itself to.

**`?`, which is `match` and `return`.**

```
fn parse_pair(s: &str) -> Result<(t27, t27), ParseError> {
    let comma = find(s, ',').ok_or(ParseError::NoComma)?;
    let a = parse(&s[0..comma])?;
    let b = parse(&s[comma + 1..s.len()])?;
    Result::Ok((a, b))
}
```

`ok_or` is where an `Option` becomes a `Result`, written at the place it
happens, because §4.1 will not do it silently.

**Kleene logic, in the ALU.**

```
fn known_true(a: Option<bool>, b: Option<bool>) -> bool {
    kleene.and(a, b) == Option::Some(true)
}
```

`kleene.and` is `tmin` — one instruction on a value that is one tryte.
