//! The Trust frontend: source text to TIR.
//!
//! Implements Language Ch. 0 (syntax), Ch. 1's type rules, Ch. 2's
//! composites and layout, Ch. 3's ownership and borrowing, and Ch. 4's
//! traits, generics, trait objects and closures. What it does not cover is
//! what the specification defers: strings and everything else that waits for
//! the library chapter.

pub mod ast;
pub mod index;
pub mod lex;
pub mod lower;
pub mod macros;
pub mod modules;
pub mod parse;

pub use lex::{LineMap, Pos, Span, SyntaxError};

/// What every program is compiled with in front of it.
///
/// Two kinds of thing live here. `Option` and `Result` are types Ch. 4 §5.8
/// makes the language's own rather than the library's, because Ch. 2 §6 states
/// niche guarantees about `Option` and Ch. 4 §5.7 needs it to describe
/// `Iterator`; they are ordinary enums laid out by Ch. 2's rules with no
/// special case, and prepending their source is the most direct way to keep
/// that claim honest.
///
/// The rest is Chapter 5's library, written in Trust because it can be. Only
/// `char::try_from` is the compiler's, and only because producing a `char`
/// from a word is the one thing no `as` in this language does (Ch. 5 §1.2).
///
/// A program pays for none of what it does not call: `lower` drops every
/// function unreachable from `main`.
pub const PRELUDE: &str = r#"
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }

// --- Ch. 5 §4: what a failure does when nobody said ------------------------

// `unwrap` traps: there is no unwinding and no handler (AM §4), so it is the
// same kind of stop as an out-of-bounds index. `expect` does not exist,
// because its whole value is the message and a message has nowhere to go
// (Ch. 5 §4.3).

impl<T> Option<T> {
    fn unwrap(self) -> T {
        match self { Option::Some(v) => v, Option::None => trap() }
    }
    fn unwrap_or(self, d: T) -> T {
        match self { Option::Some(v) => v, Option::None => d }
    }
    fn is_some(&self) -> bool {
        match self { Option::Some(v) => true, Option::None => false }
    }
    fn is_none(&self) -> bool {
        match self { Option::Some(v) => false, Option::None => true }
    }
}

impl<T, E> Result<T, E> {
    fn unwrap(self) -> T {
        match self { Result::Ok(v) => v, Result::Err(e) => trap() }
    }
    fn unwrap_or(self, d: T) -> T {
        match self { Result::Ok(v) => v, Result::Err(e) => d }
    }
    fn is_ok(&self) -> bool {
        match self { Result::Ok(v) => true, Result::Err(e) => false }
    }
    fn is_err(&self) -> bool {
        match self { Result::Ok(v) => false, Result::Err(e) => true }
    }
}

// --- Ch. 5 §1: text -------------------------------------------------------

// Division that rounds *down*, and the matching remainder. Ch. 1 §4's `/`
// rounds to nearest, which is what arithmetic wants and what slicing a value
// into fields must not have: 19990 / 4096 is 5 to the nearest and 4 to the
// floor, and only one of those is 世.
fn floor_div(a: t27, b: t27) -> t27 {
    let q = a / b;
    if q * b > a { q - 1 } else { q }
}

fn floor_mod(a: t27, b: t27) -> t27 { a - floor_div(a, b) * b }

// --- Ch. 4 §5.7 and Ch. 5 §3: iteration -----------------------------------

trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;

    // The lazy adaptors (Ch. 5 §3.1). Each returns a struct that does
    // nothing until it is asked for a value, and each is a provided body —
    // so an implementation gets all of them by writing `next` alone.
    //
    // A method's own type parameters may not reuse the impl's, which is why
    // these are `R`, `G`, `Q`, `J` rather than the `B`, `F`, `P` the structs
    // below use: `Map`'s impl and `Map`'s `map` live in one environment.
    fn map<R, G: Fn(Self::Item) -> R>(self, g: G) -> Map<Self, G> {
        Map { inner: self, f: g }
    }

    fn filter<Q: Fn(Self::Item) -> bool>(self, q: Q) -> Filter<Self, Q> {
        Filter { inner: self, p: q }
    }

    fn take(self, n: taddr) -> Take<Self> {
        Take { inner: self, left: n }
    }

    fn skip(self, n: taddr) -> Skip<Self> {
        Skip { inner: self, left: n }
    }

    fn enumerate(self) -> Enumerate<Self> {
        Enumerate { inner: self, at: 0 }
    }

    fn zip<J>(self, other: J) -> Zip<Self, J> {
        Zip { a: self, b: other }
    }

    // `collect` is the one consuming method that needs a trait, because
    // what it collects *into* is chosen by the context (Ch. 5 §3.3).
    fn collect<C: FromIterator<Elem = Self::Item>>(self) -> C {
        C::from_iter(self)
    }

    // The consuming methods (Ch. 5 §3.3). Each is a `while let` over `next`
    // and nothing more, and each is a provided body, so an implementation
    // gets them by writing `next` alone (Ch. 4 §1.5).
    fn count(self) -> taddr {
        let mut it = self;
        let mut n: taddr = 0;
        loop {
            match it.next() {
                Option::Some(v) => { n += 1; },
                Option::None => { break; },
            }
        }
        n
    }

    fn last(self) -> Option<Self::Item> {
        let mut it = self;
        let mut seen: Option<Self::Item> = Option::None;
        loop {
            match it.next() {
                Option::Some(v) => { seen = Option::Some(v); },
                Option::None => { break; },
            }
        }
        seen
    }

    fn nth(self, n: taddr) -> Option<Self::Item> {
        let mut it = self;
        let mut left = n;
        loop {
            match it.next() {
                Option::Some(v) => {
                    if left == 0 { return Option::Some(v); }
                    left -= 1;
                },
                Option::None => { return Option::None; },
            }
        }
    }

    fn find(self, p: impl Fn(Self::Item) -> bool) -> Option<Self::Item> {
        let mut it = self;
        loop {
            match it.next() {
                Option::Some(v) => { if p(v) { return Option::Some(v); } },
                Option::None => { return Option::None; },
            }
        }
    }

    fn position(self, p: impl Fn(Self::Item) -> bool) -> Option<taddr> {
        let mut it = self;
        let mut at: taddr = 0;
        loop {
            match it.next() {
                Option::Some(v) => {
                    if p(v) { return Option::Some(at); }
                    at += 1;
                },
                Option::None => { return Option::None; },
            }
        }
    }

    fn all(self, p: impl Fn(Self::Item) -> bool) -> bool {
        let mut it = self;
        loop {
            match it.next() {
                Option::Some(v) => { if !p(v) { return false; } },
                Option::None => { return true; },
            }
        }
    }

    fn any(self, p: impl Fn(Self::Item) -> bool) -> bool {
        let mut it = self;
        loop {
            match it.next() {
                Option::Some(v) => { if p(v) { return true; } },
                Option::None => { return false; },
            }
        }
    }

    fn for_each(self, f: impl Fn(Self::Item)) {
        let mut it = self;
        loop {
            match it.next() {
                Option::Some(v) => { f(v); },
                Option::None => { break; },
            }
        }
    }
}

// `sum`, `min`, `max` and `fold` need arithmetic or an order on the item, and
// a bound on an associated type is not written yet — so they are free
// functions with the bound on the *iterator*, which is the same requirement
// spelled where the language can say it (Ch. 4 §1.7).

fn sum<I: Iterator<Item = t27>>(it: I) -> t27 {
    let mut it = it;
    let mut total: t27 = 0;
    loop {
        match it.next() {
            Option::Some(v) => { total += v; },
            Option::None => { break; },
        }
    }
    total
}

fn product<I: Iterator<Item = t27>>(it: I) -> t27 {
    let mut it = it;
    let mut total: t27 = 1;
    loop {
        match it.next() {
            Option::Some(v) => { total *= v; },
            Option::None => { break; },
        }
    }
    total
}

fn min<I: Iterator<Item = t27>>(it: I) -> Option<t27> {
    let mut it = it;
    let mut best: Option<t27> = Option::None;
    loop {
        match it.next() {
            Option::Some(v) => {
                best = match best {
                    Option::Some(b) => { if v < b { Option::Some(v) } else { Option::Some(b) } },
                    Option::None => Option::Some(v),
                };
            },
            Option::None => { break; },
        }
    }
    best
}

fn max<I: Iterator<Item = t27>>(it: I) -> Option<t27> {
    let mut it = it;
    let mut best: Option<t27> = Option::None;
    loop {
        match it.next() {
            Option::Some(v) => {
                best = match best {
                    Option::Some(b) => { if v > b { Option::Some(v) } else { Option::Some(b) } },
                    Option::None => Option::Some(v),
                };
            },
            Option::None => { break; },
        }
    }
    best
}

fn fold<I: Iterator<Item = t27>>(it: I, init: t27, f: impl Fn(t27, t27) -> t27) -> t27 {
    let mut it = it;
    let mut acc = init;
    loop {
        match it.next() {
            Option::Some(v) => { acc = f(acc, v); },
            Option::None => { break; },
        }
    }
    acc
}

// `a..b` — the half-open range, which is what `a..b` is sugar for. It is an
// ordinary struct with an ordinary `Iterator`: the range is not a language
// feature, only its spelling is (Ch. 5 §3.4).
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

// What a `for` loop takes (Ch. 4 §5.7). An iterator is one of these by the
// blanket impl below, and so is anything that can produce one.
trait IntoIterator {
    type Item;
    type IntoIter;
    fn into_iter(self) -> Self::IntoIter;
}

// An iterator is one of these already (Ch. 4 §5.7).
impl<I: Iterator> IntoIterator for I {
    type Item = I::Item;
    type IntoIter = I;
    fn into_iter(self) -> I { self }
}

// A string yields its characters, which is what `for c in s` means. The
// implementing type is the *reference*, because `str` is unsized and could
// not be passed by value to `into_iter` at all (Ch. 5 §1.3).
impl IntoIterator for &str {
    type Item = char;
    type IntoIter = Chars;
    fn into_iter(self) -> Chars { self.chars() }
}

// A `Vec` yields its elements, which is what `for x in v` means. It is not
// itself an iterator — `next` would consume the vector it is called on, and a
// `Vec` you have iterated once is a `Vec` you still have — so it is a
// separate type holding the vector and a position, exactly as `Chars` holds a
// string and one.
struct VecIter<T> {
    back: Vec<T>,
}

// `pop` is the only operation that hands an element over: it takes it *out*,
// so the vector no longer owns it and exactly one place does. Draft 0.1 read
// `self.v[self.at]` instead and left the vector owning what it had given
// away, which dropped every element twice (G9.45).
impl<T> Iterator for VecIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.back.pop()
    }
}

// Popping yields the last element first, so the vector is reversed once, on
// the way in — O(n) with no comparison and no allocation beyond the one, and
// the alternative is an iterator that cannot be written soundly at all.
impl<T> IntoIterator for Vec<T> {
    type Item = T;
    type IntoIter = VecIter<T>;
    fn into_iter(self) -> VecIter<T> {
        let mut from = self;
        let mut back: Vec<T> = Vec::new();
        loop {
            match from.pop() {
                Option::Some(x) => back.push(x),
                Option::None => return VecIter { back: back },
            }
        }
    }
}

// --- Ch. 5 §2: a map ------------------------------------------------------

// What a key must be able to do beyond being comparable: turn itself into a
// number. Equality comes from `Eq`, which every primitive has from Ch. 1 §4
// and every nominal type may derive (Ch. 4 §6) — so `Key` adds one method
// and claims one name.
trait Key: Eq {
    fn hash(&self) -> taddr;
}

// What `==` means for a type the language does not define it on (Ch. 4
// §5.3). `#[derive(Eq)]` writes one of these; so may a program.
trait Eq {
    fn eq(&self, other: &Self) -> bool;
}

// A `String` is a `Vec<char>` (Ch. 0 §3.6), and two are equal when they hold
// the same characters in the same order — which is what makes one a key.
impl Eq for Vec<char> {
    fn eq(&self, other: &Vec<char>) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut i: taddr = 0;
        while i < self.len() {
            if self[i] != other[i] {
                return false;
            }
            i += 1;
        }
        true
    }
}

impl Key for Vec<char> {
    fn hash(&self) -> taddr {
        // Multiply and add — the shift is by one trit, so a character moved
        // one place along changes the answer, which is the whole requirement.
        let mut h: taddr = 0;
        let mut i: taddr = 0;
        while i < self.len() {
            h = h.wrapping_mul(3).wrapping_add((self[i] as t27) as taddr);
            i += 1;
        }
        h
    }
}

impl Key for t27 {
    fn hash(&self) -> taddr { *self as taddr }
}

impl Key for char {
    // A `char` converts to `t27` and to nothing else (Ch. 5 §1.2), so the
    // widening to an address is written in two steps rather than assumed.
    fn hash(&self) -> taddr { (*self as t27) as taddr }
}

struct Entry<K, V> {
    k: K,
    v: V,
}

// The name is `HashMap` and not `Map` because the iterator adaptor of
// Ch. 5 §3.1 is a `Map` already — and the two coexisted silently until one
// of them was asked for a field the other had (G9.32).
//
// Open addressing would be fewer allocations and is what a mature one does.
// This is a **vector of buckets**, each a vector of entries, because it is
// the version whose correctness can be read off it: nothing here depends on
// a load factor, a probe sequence, or a tombstone.
//
// `BUCKETS` is fixed, so the map does not grow its table. That is a real
// limit and it is written here rather than discovered: a map with far more
// entries than buckets degrades to a linear scan of one of them.
struct HashMap<K, V> {
    buckets: Vec<Vec<Entry<K, V>>>,
    n: taddr,
}

const BUCKETS: taddr = 64;

impl<K: Key, V> HashMap<K, V> {
    fn new() -> HashMap<K, V> {
        let mut bs: Vec<Vec<Entry<K, V>>> = Vec::new();
        let mut i: taddr = 0;
        while i < BUCKETS {
            let empty: Vec<Entry<K, V>> = Vec::new();
            bs.push(empty);
            i += 1;
        }
        HashMap { buckets: bs, n: 0 }
    }

    /// How many keys it holds.
    fn len(&self) -> taddr {
        self.n
    }

    fn which(&self, k: &K) -> taddr {
        // `%` is **symmetric** (Ch. 1 §4): it rounds the quotient to nearest,
        // so `40 % 64` is −24 and not 40. A negative bucket is not a bucket,
        // and taking the sign off the *hash* first is not enough — the
        // remainder itself can still be negative. So the correction is after.
        let zero: taddr = 0;
        let h = k.hash();
        let a = if h < zero { zero - h } else { h };
        let r = a % BUCKETS;
        if r < zero { r + BUCKETS } else { r }
    }

    /// The value for `k`, or `None`.
    fn get(&self, k: &K) -> Option<&V> {
        let b = self.which(k);
        let mut i: taddr = 0;
        while i < self.buckets[b].len() {
            if self.buckets[b][i].k == *k {
                return Option::Some(&self.buckets[b][i].v);
            }
            i += 1;
        }
        Option::None
    }

    /// Whether `k` is in it.
    fn has(&self, k: &K) -> bool {
        match self.get(k) {
            Option::Some(v) => true,
            Option::None => false,
        }
    }

    /// Put `v` under `k`, replacing whatever was there.
    fn insert(&mut self, k: K, v: V) {
        let b = self.which(&k);
        let mut i: taddr = 0;
        while i < self.buckets[b].len() {
            if self.buckets[b][i].k == k {
                self.buckets[b][i].v = v;
                return;
            }
            i += 1;
        }
        self.buckets[b].push(Entry { k: k, v: v });
        self.n += 1;
    }
}

// What a sequence of values can be gathered into (Ch. 5 §3.3).
//
// The element is an *associated* type rather than a parameter: a type is
// collected into from one element type and no other — there is no "collect a
// `Vec<T>` from an iterator of `&T`" here, because there is nothing to clone
// with — so the element is an output of the implementation, which is what an
// associated type is for.
trait FromIterator {
    type Elem;
    fn from_iter<J: Iterator<Item = Self::Elem>>(it: J) -> Self;
}

impl<A> FromIterator for Vec<A> {
    type Elem = A;
    fn from_iter<J: Iterator<Item = A>>(it: J) -> Vec<A> {
        let mut out: Vec<A> = Vec::new();
        let mut j = it;
        loop {
            match j.next() {
                Option::Some(v) => { out.push(v); },
                Option::None => { break; },
            }
        }
        out
    }
}

// Each adaptor is a struct and an impl a reader could have written, which is
// the claim Ch. 5 §3 makes about them. Nothing here is a language feature:
// they need a closure (Ch. 4 §4) and an associated type (Ch. 4 §1.7), and
// both exist.

struct Map<I, F> { inner: I, f: F }
struct Filter<I, P> { inner: I, p: P }
struct Take<I> { inner: I, left: taddr }
struct Skip<I> { inner: I, left: taddr }
struct Enumerate<I> { inner: I, at: taddr }
struct Zip<A, B> { a: A, b: B }

// `Item` is the closure's result, not a fixed type: `B` is named by no
// argument of `Map` and is settled by `F`'s bound, because a closure has one
// signature and it is recorded (Ch. 4 §4.3, Ch. 5 §3.1).
impl<I: Iterator, B, F: Fn(I::Item) -> B> Iterator for Map<I, F> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        match self.inner.next() {
            Option::Some(x) => Option::Some((self.f)(x)),
            Option::None => Option::None,
        }
    }
}

impl<I: Iterator, P: Fn(I::Item) -> bool> Iterator for Filter<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        loop {
            match self.inner.next() {
                Option::Some(x) => { if (self.p)(x) { return Option::Some(x); } },
                Option::None => { return Option::None; },
            }
        }
    }
}

impl<I: Iterator> Iterator for Take<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.left == 0 { Option::None } else {
            self.left -= 1;
            self.inner.next()
        }
    }
}

impl<I: Iterator> Iterator for Skip<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        while self.left > 0 {
            self.left -= 1;
            self.inner.next();
        }
        self.inner.next()
    }
}

impl<I: Iterator> Iterator for Enumerate<I> {
    type Item = (taddr, I::Item);
    fn next(&mut self) -> Option<(taddr, I::Item)> {
        match self.inner.next() {
            Option::Some(x) => {
                let at = self.at;
                self.at += 1;
                Option::Some((at, x))
            },
            Option::None => Option::None,
        }
    }
}

impl<A: Iterator, B: Iterator> Iterator for Zip<A, B> {
    type Item = (A::Item, B::Item);
    fn next(&mut self) -> Option<(A::Item, B::Item)> {
        match self.a.next() {
            Option::Some(x) => match self.b.next() {
                Option::Some(y) => Option::Some((x, y)),
                Option::None => Option::None,
            },
            Option::None => Option::None,
        }
    }
}

impl char {
    // The value of a digit in a given radix, or `None` if it is not one.
    // Both the radix's own digits and this machine's are `0`-`9` and `A`-`Z`,
    // so heptavintimal (Ch. 1 §3.1) needs nothing special here.
    fn to_digit(self, radix: t27) -> Option<t27> {
        let c = self as t27;
        let v = if c >= 48 && c <= 57 {
            c - 48
        } else {
            if c >= 65 && c <= 90 {
                c - 55
            } else {
                if c >= 97 && c <= 122 { c - 87 } else { -1 }
            }
        };
        if v >= 0 && v < radix { Option::Some(v) } else { Option::None }
    }

    // How many UTF-8 code units this character encodes to (Ch. 5 §1.5).
    fn utf8_len(self) -> taddr {
        let c = self as t27;
        if c < 128 { 1 } else {
            if c < 2048 { 2 } else {
                if c < 65536 { 3 } else { 4 }
            }
        }
    }

    // This character's UTF-8 code units, and how many of them there are
    // (Ch. 5 §1.5). It returns no `Option` because it cannot fail: a `char`
    // is always encodable, and four units is always enough for one.
    fn to_utf8(self) -> ([t9; 4], taddr) {
        let n = self.utf8_len();
        let c = self as t27;
        let mut out: [t9; 4] = [0; 4];
        if n == 1 {
            out[0] = c as t9;
        } else {
            if n == 2 {
                out[0] = (192 + floor_div(c, 64)) as t9;
                out[1] = (128 + floor_mod(c, 64)) as t9;
            } else {
                if n == 3 {
                    out[0] = (224 + floor_div(c, 4096)) as t9;
                    out[1] = (128 + floor_mod(floor_div(c, 64), 64)) as t9;
                    out[2] = (128 + floor_mod(c, 64)) as t9;
                } else {
                    out[0] = (240 + floor_div(c, 262144)) as t9;
                    out[1] = (128 + floor_mod(floor_div(c, 4096), 64)) as t9;
                    out[2] = (128 + floor_mod(floor_div(c, 64), 64)) as t9;
                    out[3] = (128 + floor_mod(c, 64)) as t9;
                }
            }
        }
        (out, n)
    }
}

impl str {
    // The number of UTF-8 code units this string encodes to — what a caller
    // sizes a buffer with, and the one place a program sees the interchange
    // format's variable width.
    fn utf8_len(&self) -> taddr {
        let mut n: taddr = 0;
        let mut i: taddr = 0;
        while i < self.len() {
            n += self[i].utf8_len();
            i += 1;
        }
        n
    }

    fn to_utf8(&self, out: &mut [t9]) -> Option<taddr> {
        let mut at: taddr = 0;
        let mut i: taddr = 0;
        while i < self.len() {
            let (unit, n) = self[i].to_utf8();
            if at + n > out.len() { return Option::None; }
            let mut j: taddr = 0;
            while j < n {
                out[at + j] = unit[j];
                j += 1;
            }
            at += n;
            i += 1;
        }
        Option::Some(at)
    }

    // The characters, one at a time. `for c in s` would be this without the
    // call, and needs `impl IntoIterator for &str` — an impl whose self type
    // is a reference, which draft 0.1 cannot write (G9.17).
    fn chars(&self) -> Chars {
        Chars { s: self, at: 0 }
    }
}

// `String`'s one method of its own (Ch. 5 §2.6). An impl for a single
// instantiation, which is what `String` is: `Vec<char>` and no other.
// Owned, growable text (Ch. 5 §1).
//
// It is a *name* for `Vec<char>` and not a type of its own (Ch. 0 §3.6),
// which costs nothing and buys everything: `&String` is already `&[char]`,
// `&[char]` is already `&str`, and a `String` is already something `for c in`
// walks. A `str` here is one character per word and fixed width (Ch. 5 §1.1),
// so a `Vec<char>` is exactly the growable form of one — there is no encoding
// to agree on and no invariant to maintain.
type String = Vec<char>;

impl Vec<char> {
    fn push_str(&mut self, t: &str) {
        let mut i: taddr = 0;
        while i < t.len() {
            self.push(t[i]);
            i += 1;
        }
    }
}

impl str {
    /// An owned copy of this text.
    fn to_string(&self) -> Vec<char> {
        let mut out: Vec<char> = Vec::new();
        out.reserve(self.len());
        out.push_str(self);
        out
    }

    /// Whether this text begins with `p`.
    fn starts_with(&self, p: &str) -> bool {
        if p.len() > self.len() {
            return false;
        }
        let mut i: taddr = 0;
        while i < p.len() {
            if self[i] != p[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Whether `p` appears anywhere in this text.
    fn contains(&self, p: &str) -> bool {
        if p.len() > self.len() {
            return false;
        }
        let mut at: taddr = 0;
        while at + p.len() <= self.len() {
            let mut i: taddr = 0;
            let mut same = true;
            while i < p.len() {
                if self[at + i] != p[i] {
                    same = false;
                }
                i += 1;
            }
            if same {
                return true;
            }
            at += 1;
        }
        false
    }

    /// Whether this text is `other`, character for character.
    ///
    /// Named `same` and not `eq` for a reason worth knowing. A `String` is a
    /// `Vec<char>` (Ch. 0 §3.6) and has `Eq`'s `eq`, which takes another
    /// `Vec<char>`; a literal is a `&str`. If this were `eq` too, the one on
    /// `Vec<char>` would hide it and a `String` could not be compared with a
    /// literal at all — there is no overloading to sort them out. Under a
    /// name `Vec<char>` does not have, the fallback to `str`'s methods finds
    /// it, and text is compared with text however each side is held (G9.36).
    fn same(&self, other: &str) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut i: taddr = 0;
        while i < self.len() {
            if self[i] != other[i] {
                return false;
            }
            i += 1;
        }
        true
    }
}

struct Chars { s: &str, at: taddr }

impl Iterator for Chars {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if self.at < self.s.len() {
            let c = self.s[self.at];
            self.at += 1;
            Option::Some(c)
        } else {
            Option::None
        }
    }
}

// ------------------------------------------------------------------ output
//
// The character ports are the reference target's (ISA §2.2) and they take
// UTF-8 code units, so the encoding happens here and nowhere else: a program
// that prints never sees the interchange format (Ch. 5 §1.5).
//
// `putchar` has no body for the reason `alloc` has none — it reaches a
// memory-mapped device, and TIR has no way to name one (§2.1).

fn putchar(c: t9);

// This is `print_char`'s body written out rather than called, and it is the
// one place in this file that repeats itself. The inliner has a budget and
// `print_char` is over it — and raising the budget until it fits makes HPL
// *slower*, because a larger splice costs more in spills than it saves in
// calls. Measured: 3 247 122 instructions this way, 3 303 442 as a loop over
// `print_char`, and 3 315 827 with the budget raised far enough to take it.
fn print(s: &str) {
    let mut i: taddr = 0;
    while i < s.len() {
        let v = s[i] as t27;
        if v < 128 {
            putchar(v as t9);
        } else {
            let (units, n) = s[i].to_utf8();
            let mut j: taddr = 0;
            while j < n {
                putchar(units[j]);
                j += 1;
            }
        }
        i += 1;
    }
}

// One character. The library could print a string and not a character, which
// left a program with a character it had *computed* — as against one it had
// written down — reaching for `putchar` and doing its own encoding.
fn print_char(c: char) {
    let v = c as t27;
    if v < 128 {
        putchar(v as t9);
    } else {
        let (units, n) = c.to_utf8();
        let mut j: taddr = 0;
        while j < n {
            putchar(units[j]);
            j += 1;
        }
    }
}

fn println(s: &str) {
    print(s);
    putchar(10);
}

// A number, in decimal (Ch. 5 §1.6).
//
// One argument and no composition, which is what keeps it outside what §7
// reserves: `Display` and `format!` need variadics or a macro, and this needs
// neither. It is the digit loop §7 says every program writes, written once.
//
// `%` is the *symmetric* remainder, so `n % 10` is −9 ..= 9 and a negative
// digit borrows from the next place. That is Ch. 1 §4's rounding showing
// through, and it is why this is not the loop a binary machine would write.
fn print_int(value: t27) {
    let mut n = value;
    if n < 0 {
        print_char('-');
        n = -n;
    }
    if n == 0 {
        print_char('0');
        return;
    }
    // 3^27 is under 8·10^12, so fourteen digits is more than a word can need.
    let mut digits: [t9; 14] = [0; 14];
    let mut i: taddr = 0;
    while n > 0 {
        let mut d = n % 10;
        n = n / 10;
        if d < 0 {
            d += 10;
            n -= 1;
        }
        digits[i] = d as t9;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        print_char(char::try_from((digits[i] as t27) + 48).unwrap());
    }
}

fn println_int(value: t27) {
    print_int(value);
    putchar(10);
}
"#;

/// What one compilation learned about the file it was given.
///
/// `compile` answers "is this a program"; this answers "what is this file",
/// which is a different question and the one an editor asks. It reports
/// everything rather than the first thing, and it keeps what the frontend
/// worked out on the way — which is where a type comes from, since a type is
/// nowhere in the text of `let n = 1;`.
pub struct Analysis {
    /// Everything wrong with it.
    pub errors: Vec<SyntaxError>,
    /// The type of each expression in the file's own functions.
    pub types: lower::Noted,
}

/// Compile for an editor's sake.
///
/// Only the file's own functions are recorded: the prelude is parsed as its
/// own file, so its spans are offsets into *it* and would be read as places
/// in this one. `Noted` says the rest of what bounds it.
pub fn analyze(src: &str) -> Analysis {
    let (user, syntax) = parse::parse_recovering(src);
    let mine: std::collections::HashSet<String> = user
        .items
        .iter()
        .filter_map(item_name)
        .map(str::to_string)
        .collect();
    let mut file = merged(&user, 1);
    let mut alias = macros::expand(&mut file).err().unwrap_or_default();
    alias.extend(expand_aliases(&mut file).err().unwrap_or_default());
    let noted = std::cell::RefCell::new(lower::Noted::default());
    let found = lower::lower_noting(&file, &mine, Some(&noted))
        .err()
        .unwrap_or_default();
    Analysis {
        // A file that does not parse is not type-checked. What lowering
        // would say about the part that did parse is mostly about the part
        // that did not — every call to the function being typed is "not a
        // function in scope" — and a list of consequences buries the cause.
        // The types are kept, because a type is not a complaint.
        errors: if !syntax.is_empty() {
            syntax
        } else if !alias.is_empty() {
            alias
        } else {
            found
        },
        types: noted.into_inner(),
    }
}

/// The prelude with the program written over it: a name the program defines
/// replaces the prelude's, rather than colliding with it (Ch. 6 §3.3).
///
/// `at` is the file id the prelude's spans carry, which is one past the
/// program's own files so that nothing in it is read as a place in them.
fn merged(user: &ast::File, at: lex::File) -> ast::File {
    let mut file = parse::parse_in(PRELUDE, at).expect("the prelude parses");
    let defined: std::collections::HashSet<&str> =
        user.items.iter().filter_map(item_name).collect();
    // An `impl` goes with what it is written on. Dropping a shadowed item and
    // keeping the prelude's impls *for* it left `impl Key for t27` behind to
    // be checked against a program's own `Key` — which has different methods,
    // and said so in a message about a trait the program never saw (G9.34).
    file.items.retain(|i| match i {
        ast::Item::Impl(imp) => {
            !defined.contains(imp.self_ty.as_str())
                && imp
                    .trait_name
                    .as_deref()
                    .is_none_or(|t| !defined.contains(t))
        }
        other => item_name(other).is_none_or(|n| !defined.contains(n)),
    });
    file.items.extend(user.items.iter().cloned());
    file
}

/// What one build produced, and everything wrong with it.
///
/// A program is a tree of files (Ch. 6 §1), so an error names one: `errors`
/// carry a `Span` whose `file` indexes `program.sources`.
pub struct Build {
    /// The files, as loaded, for a diagnostic to place itself in.
    pub program: modules::Program,
    /// The module, if it got that far.
    pub module: Option<crate::tir::Module>,
    /// Everything wrong with it, in the order found.
    pub errors: Vec<SyntaxError>,
}

/// Compile a program from its root file, following its `mod` declarations.
pub fn build(root: &std::path::Path) -> Build {
    finish(modules::load(root))
}

/// Compile a program that is one file with nothing under it.
pub fn build_text(src: &str) -> Build {
    finish(modules::one(src))
}

fn finish(program: modules::Program) -> Build {
    // A file that does not parse is not resolved: what resolution would say
    // about the part that parsed is mostly about the part that did not.
    if !program.errors.is_empty() {
        let errors = program.errors.clone();
        return Build {
            program,
            module: None,
            errors,
        };
    }
    let (user, errors) = modules::resolve(&program);
    if !errors.is_empty() {
        return Build {
            program,
            module: None,
            errors,
        };
    }
    let mut file = merged(&user, program.sources.len() as lex::File);
    // Macros first, then aliases: a body may expand to a use of one
    // (Ch. 7 §5).
    if let Err(errors) = macros::expand(&mut file) {
        return Build {
            program,
            module: None,
            errors,
        };
    }
    if let Err(errors) = expand_aliases(&mut file) {
        return Build {
            program,
            module: None,
            errors,
        };
    }
    match lower::lower(&file) {
        Ok(module) => {
            let errs = crate::tir::verify(&module);
            if !errs.is_empty() {
                let errors = errs
                    .into_iter()
                    .map(|e| SyntaxError {
                        span: Span::NONE,
                        message: format!("the frontend produced ill-formed TIR: {e}"),
                    })
                    .collect();
                return Build {
                    program,
                    module: None,
                    errors,
                };
            }
            let mut module = module;
            keep_reachable(&mut module, &prelude_functions());
            Build {
                program,
                module: Some(module),
                errors: Vec::new(),
            }
        }
        Err(errors) => Build {
            program,
            module: None,
            errors,
        },
    }
}

/// Compile a source file to a TIR module.
///
/// The prelude is parsed separately from the program rather than pasted in
/// front of it, for two reasons. A program's error lines are then its own,
/// with nothing to subtract. And an item the program defines **shadows** the
/// prelude's of the same name — which is what a prelude is, and which matters
/// here more than elsewhere because there are no modules (Ch. 0 §1.3) and so
/// the prelude occupies the only namespace there is. A program that defines
/// its own `sum` gets its own `sum`.
pub fn compile(src: &str) -> Result<crate::tir::Module, Vec<SyntaxError>> {
    let user = parse::parse(src).map_err(|e| vec![e])?;
    let mut file = merged(&user, 1);
    macros::expand(&mut file)?;
    expand_aliases(&mut file)?;
    let module = lower::lower(&file)?;

    // Verified **before** pruning, and the order is the point. A function
    // nothing calls is still a function this compiler emitted, and if it is
    // ill-formed that is a bug whether or not any program reaches it.
    //
    // The other order hid one: a `loop` whose every path returned emitted an
    // unreachable exit block reading a slot nothing defined, and the reduced
    // test case *passed* — because the function was unused, so pruning
    // removed it before the verifier ran (G9.9). Dead-code elimination is
    // exactly the wrong thing to put in front of a checker.
    let errs = crate::tir::verify(&module);
    if !errs.is_empty() {
        return Err(errs
            .into_iter()
            .map(|e| SyntaxError {
                span: Span::NONE,
                message: format!("the frontend produced ill-formed TIR: {e}"),
            })
            .collect());
    }

    let mut module = module;
    keep_reachable(&mut module, &prelude_functions());
    Ok(module)
}

/// Replace every alias with what it names, and drop the aliases (Ch. 0
/// §3.6).
///
/// An alias is only a name, so this is the whole of what one is: after it,
/// nothing below the frontend has ever heard of `String`, and a diagnostic
/// that says `Vec<char>` is saying the same thing.
fn expand_aliases(file: &mut ast::File) -> Result<(), Vec<SyntaxError>> {
    let mut aliases: std::collections::HashMap<String, ast::Ty> = std::collections::HashMap::new();
    for item in &file.items {
        if let ast::Item::Alias(a) = item {
            aliases.insert(a.name.clone(), a.ty.clone());
        }
    }
    if aliases.is_empty() {
        return Ok(());
    }

    // An alias may name another, so each one is expanded until it names no
    // alias at all. A definition that reaches itself has no such form, and
    // saying so is better than not returning.
    let mut errs = Vec::new();
    let settled: std::collections::HashMap<String, ast::Ty> = aliases
        .keys()
        .map(|name| {
            let mut t = aliases[name].clone();
            let mut seen = vec![name.clone()];
            for _ in 0..aliases.len() {
                let ast::Ty::Name(n, at) = &t else { break };
                let Some(next) = aliases.get(n) else { break };
                if seen.contains(n) {
                    errs.push(SyntaxError {
                        span: *at,
                        message: format!("the type alias `{name}` names itself (Ch. 0 §3.6)"),
                    });
                    break;
                }
                seen.push(n.clone());
                t = next.clone();
            }
            (name.clone(), t)
        })
        .collect();
    if !errs.is_empty() {
        return Err(errs);
    }

    ast::for_each_ty(file, &mut |t| {
        // Only a bare name is an alias: an alias takes no parameters, so
        // `String<T>` names nothing and is left for the type checker to
        // refuse by name.
        let ast::Ty::Name(n, at) = t else { return };
        if let Some(to) = settled.get(n) {
            // The alias's span, not its definition's: a reader who wrote
            // `String` is asking about where they wrote it.
            *t = to.clone().spanning(*at);
        }
    });
    file.items.retain(|i| !matches!(i, ast::Item::Alias(_)));
    Ok(())
}

/// The name an item defines, for the shadowing rule. An `impl` block defines
/// none: it adds methods to a type, and two impls of different traits for one
/// type are not a collision.
fn item_name(i: &ast::Item) -> Option<&str> {
    match i {
        ast::Item::Fn(f) => Some(&f.name),
        ast::Item::Struct(s) => Some(&s.name),
        ast::Item::Enum(e) => Some(&e.name),
        ast::Item::Trait(t) => Some(&t.name),
        ast::Item::Const(c) => Some(&c.name),
        ast::Item::Alias(a) => Some(&a.name),
        ast::Item::Macro(m) => Some(&m.name),
        // An `impl` defines none: it adds methods to a type, and two impls
        // of different traits for one type are not a collision. A `mod` and
        // a `use` define a name in their own module and are resolved away
        // before this is asked (Ch. 6 §4).
        ast::Item::Impl(_) | ast::Item::Mod(_) | ast::Item::Use(_) => None,
    }
}

/// The functions the prelude defines, by the names they lower to.
fn prelude_functions() -> std::collections::HashSet<String> {
    let file = parse::parse(PRELUDE).expect("the prelude parses");
    let mut names = std::collections::HashSet::new();
    // A trait's methods, so that an impl can be credited with the ones it did
    // not write. A *provided* body is synthesized for each implementing type
    // (Ch. 4 §1.5), so `impl Iterator for Chars` produces `Chars.count` and
    // six more that appear in no impl block — and a name this set misses is
    // treated as a root, which is how the prelude's own iterator ended up in
    // the output of every program that never mentioned text.
    let mut trait_methods: std::collections::HashMap<&str, Vec<&str>> =
        std::collections::HashMap::new();
    for item in &file.items {
        if let ast::Item::Trait(t) = item {
            trait_methods.insert(
                t.name.as_str(),
                t.methods.iter().map(|m| m.name.as_str()).collect(),
            );
        }
    }
    for item in &file.items {
        match item {
            ast::Item::Fn(f) => {
                names.insert(f.name.clone());
            }
            // An impl block's methods lower to `Type.method` (Ch. 4 §1.2).
            ast::Item::Impl(i) => {
                // An impl for one instantiation keys its methods under the
                // instantiation's name, which is the base and its arguments.
                let owner = if i.generics.is_empty() && !i.self_args.is_empty() {
                    let mut n = i.self_ty.clone();
                    for a in &i.self_args {
                        let ast::Ty::Name(x, _) = a else {
                            n.clear();
                            break;
                        };
                        n.push('.');
                        n.push_str(x);
                    }
                    if n.is_empty() { i.self_ty.clone() } else { n }
                } else {
                    i.self_ty.clone()
                };
                for m in &i.methods {
                    names.insert(format!("{owner}.{}", m.name));
                }
                if let Some(t) = &i.trait_name
                    && let Some(ms) = trait_methods.get(t.as_str())
                {
                    for m in ms {
                        names.insert(format!("{owner}.{m}"));
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// Whether a name is the prelude's, allowing for instantiation.
///
/// A generic prelude function is emitted under a *mangled* name —
/// `Option.unwrap.char` for `Option.unwrap` at `char` — and the set knows
/// only the name as written. Mangling appends, so a prefix that is in the set
/// is the answer: without this, one `unwrap` inside `print_int` made every
/// program that never calls it emit one.
fn from_prelude(name: &str, prelude: &std::collections::HashSet<String>) -> bool {
    let mut at = Some(name);
    while let Some(n) = at {
        if prelude.contains(n) {
            return true;
        }
        at = n.rfind('.').map(|i| &n[..i]);
    }
    false
}

/// Drop the functions nothing can call.
///
/// Every program carries the prelude, and the prelude is where Chapter 5's
/// library lives; without this, a program that never mentions text would still
/// emit the UTF-8 encoder.
///
/// The roots are `main`, or — in a file that has none, which is what a test
/// and a library look like — every function the *program* defined, since
/// nothing else says which of them matter. Prelude functions are never roots,
/// which is the whole point.
///
/// One root is easy to forget: a function whose *address* appears in a global.
/// A vtable slot is an address, so a method reached only through `dyn Trait`
/// is reached only that way (Ch. 4 §3.3).
fn keep_reachable(module: &mut crate::tir::Module, prelude: &std::collections::HashSet<String>) {
    use crate::tir::{Callee, InitItem, InstKind};

    let mut queue: Vec<String> = Vec::new();
    for g in &module.globals {
        for item in g.init.iter().flatten() {
            if let InitItem::Addr(name) = item {
                queue.push(name.clone());
            }
        }
    }
    if module.funcs.iter().any(|f| f.sig.name == "main") {
        queue.push("main".to_string());
    } else {
        queue.extend(
            module
                .funcs
                .iter()
                .map(|f| f.sig.name.clone())
                .filter(|n| !from_prelude(n, prelude)),
        );
    }

    let mut live: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(name) = queue.pop() {
        if !live.insert(name.clone()) {
            continue;
        }
        let Some(f) = module.funcs.iter().find(|f| f.sig.name == name) else {
            continue;
        };
        for b in &f.blocks {
            for inst in &b.insts {
                if let InstKind::Call {
                    callee: Callee::Direct(callee),
                    ..
                } = &inst.kind
                {
                    queue.push(callee.clone());
                }
            }
        }
    }
    module.funcs.retain(|f| live.contains(&f.sig.name));

    // Declarations too. `needs_heap` is set while a body is lowered, and a
    // prelude body that uses the heap sets it whether or not anything calls
    // that body — so `alloc` and `free` were declared by every program the
    // moment the library gained a method that pushes. What a program owes
    // its target is what its *surviving* code calls.
    let called: std::collections::HashSet<String> = module
        .funcs
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.insts)
        .filter_map(|i| match &i.kind {
            InstKind::Call {
                callee: Callee::Direct(c),
                ..
            } => Some(c.clone()),
            _ => None,
        })
        .collect();
    module.decls.retain(|d| called.contains(&d.name));
}

#[cfg(test)]
mod analysis_tests {
    use super::*;

    /// The type of whatever is at `|` in the source.
    fn type_at(marked: &str) -> Option<String> {
        let offset = marked.chars().position(|c| c == '|').expect("a cursor") as u32;
        let src = marked.replace('|', "");
        analyze(&src).types.at(offset).map(str::to_string)
    }

    #[test]
    fn a_binding_with_no_written_type_has_one_anyway() {
        // The whole point: nothing in the text says `t27`, and lowering does.
        assert_eq!(
            type_at("fn f() -> t27 {\n    let |n = 1;\n    n\n}\n").as_deref(),
            Some("t27")
        );
        assert_eq!(
            type_at("fn f(a: &[t27]) -> taddr {\n    let |k = a.len();\n    k\n}\n").as_deref(),
            Some("taddr")
        );
    }

    #[test]
    fn an_expression_has_the_type_it_turned_out_to_be() {
        assert_eq!(
            type_at("fn f(a: t9, b: t9) -> t27 { (a as t27) |+ (b as t27) }\n").as_deref(),
            Some("t27")
        );
        // The smallest thing covering the cursor, so `a` and not the sum.
        assert_eq!(
            type_at("fn f(a: t9, b: t9) -> t9 { |a + b }\n").as_deref(),
            Some("t9")
        );
    }

    #[test]
    fn nothing_is_recorded_for_the_prelude() {
        // Every span here is an offset into *this* file. If the prelude were
        // recorded too, its spans would be read as places in this one, and
        // this file is far shorter than the prelude.
        let src = "fn f() -> t27 { 1 }\n";
        let types = analyze(src).types;
        let longest = src.chars().count() as u32;
        for offset in 0..longest {
            let _ = types.at(offset);
        }
        assert!(!types.is_empty());
        // `println` is in the prelude and calls things; none of it is here.
        assert!(types.len() < 20, "{} entries for one function", types.len());
    }

    #[test]
    fn a_file_that_does_not_compile_still_says_what_it_can() {
        // The second function is wrong; the first is not, and an editor asks
        // about a file in exactly this state all day.
        let src = "fn ok() -> t27 {\n    let n = 7;\n    n\n}\nfn bad() -> t27 { nope() }\n";
        let a = analyze(src);
        assert!(!a.errors.is_empty(), "the second function is wrong");
        let at = src.find("n = 7").expect("the binding") as u32;
        assert_eq!(a.types.at(at), Some("t27"));
    }

    #[test]
    fn a_generic_body_is_left_alone_rather_than_guessed_at() {
        // `id` is lowered once per instantiation, so its `x` is a `t9` and a
        // `t27` and neither. An editor shown one of them is shown a guess.
        let src = "fn id<T>(x: T) -> T { x }\n\
                   fn f() -> t27 { let a: t9 = id(1); let b: t27 = id(2); b }\n";
        let a = analyze(src);
        assert!(a.errors.is_empty(), "{:?}", a.errors);
        let at = src.find("x }").expect("the body") as u32;
        assert_eq!(a.types.at(at), None, "two instantiations, no one answer");
        // What is not generic is still answered.
        let at = src.find("b: t27 = id(2)").expect("the binding") as u32;
        assert_eq!(a.types.at(at), Some("t27"));
    }
}
