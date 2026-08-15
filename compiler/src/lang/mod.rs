//! The Trust frontend: source text to TIR.
//!
//! Implements Language Ch. 0 (syntax), Ch. 1's type rules, Ch. 2's
//! composites and layout, Ch. 3's ownership and borrowing, and Ch. 4's
//! traits, generics, trait objects and closures. What it does not cover is
//! what the specification defers: strings and everything else that waits for
//! the library chapter.

pub mod ast;
pub mod lex;
pub mod lower;
pub mod parse;

pub use lex::SyntaxError;

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

// A character below 128 is one code unit and *is* that unit — the test
// `utf8_len` makes first anyway. Taking the branch here rather than in
// `to_utf8` saves building a four-element buffer and returning it, which is
// most of the cost of printing text that is all ASCII.
//
// This is `print_char`'s body written out rather than called, and it is the
// one place in this file that repeats itself. There is no inliner, so a
// one-line function is a call, and calling one per character costs 1.3% of
// HPL. When there is an inliner this loop becomes `print_char(s[i])`.
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
"#;

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
    let mut file = parse::parse(PRELUDE).expect("the prelude parses");
    let user = parse::parse(src).map_err(|e| vec![e])?;

    let defined: std::collections::HashSet<&str> =
        user.items.iter().filter_map(item_name).collect();
    file.items
        .retain(|i| item_name(i).is_none_or(|n| !defined.contains(n)));
    file.items.extend(user.items.iter().cloned());

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
                line: 0,
                message: format!("the frontend produced ill-formed TIR: {e}"),
            })
            .collect());
    }

    let mut module = module;
    keep_reachable(&mut module, &prelude_functions());
    Ok(module)
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
        ast::Item::Impl(_) => None,
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
                for m in &i.methods {
                    names.insert(format!("{}.{}", i.self_ty, m.name));
                }
                if let Some(t) = &i.trait_name
                    && let Some(ms) = trait_methods.get(t.as_str())
                {
                    for m in ms {
                        names.insert(format!("{}.{}", i.self_ty, m));
                    }
                }
            }
            _ => {}
        }
    }
    names
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
                .filter(|n| !prelude.contains(n)),
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
}
