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

impl<I, F> Iterator for Map<I, F> {
    type Item = t27;
    fn next(&mut self) -> Option<t27> {
        match self.inner.next() {
            Option::Some(x) => Option::Some((self.f)(x)),
            Option::None => Option::None,
        }
    }
}

impl<I, P> Iterator for Filter<I, P> {
    type Item = t27;
    fn next(&mut self) -> Option<t27> {
        loop {
            match self.inner.next() {
                Option::Some(x) => { if (self.p)(x) { return Option::Some(x); } },
                Option::None => { return Option::None; },
            }
        }
    }
}

impl<I> Iterator for Take<I> {
    type Item = t27;
    fn next(&mut self) -> Option<t27> {
        if self.left == 0 { Option::None } else {
            self.left -= 1;
            self.inner.next()
        }
    }
}

impl<I> Iterator for Skip<I> {
    type Item = t27;
    fn next(&mut self) -> Option<t27> {
        while self.left > 0 {
            self.left -= 1;
            self.inner.next();
        }
        self.inner.next()
    }
}

impl<I> Iterator for Enumerate<I> {
    type Item = (taddr, t27);
    fn next(&mut self) -> Option<(taddr, t27)> {
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

impl<A, B> Iterator for Zip<A, B> {
    type Item = (t27, t27);
    fn next(&mut self) -> Option<(t27, t27)> {
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

    // Write this character's UTF-8 code units into `out`, returning how many.
    // `None` if `out` is too short: the length is knowable in advance with
    // `utf8_len`, so a caller that does not want the check can avoid it.
    fn to_utf8(self, out: &mut [t9]) -> Option<taddr> {
        let n = self.utf8_len();
        if out.len() < n { return Option::None; }
        let c = self as t27;
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
        Option::Some(n)
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
            let c = self[i];
            let n = c.utf8_len();
            if at + n > out.len() { return Option::None; }
            let mut j: taddr = 0;
            let mut unit: [t9; 4] = [0; 4];
            match c.to_utf8(&mut unit) {
                Option::Some(k) => {
                    while j < k {
                        out[at + j] = unit[j];
                        j += 1;
                    }
                    at += k;
                },
                Option::None => { return Option::None; },
            }
            i += 1;
        }
        Option::Some(at)
    }
}
"#;

/// Compile a source file to a TIR module.
pub fn compile(src: &str) -> Result<crate::tir::Module, Vec<SyntaxError>> {
    let prelude_lines = PRELUDE.lines().count() as u32;
    let file = parse::parse(&format!("{PRELUDE}{src}")).map_err(|mut e| {
        e.line = e.line.saturating_sub(prelude_lines);
        vec![e]
    })?;
    let module = lower::lower(&file).map_err(|mut es| {
        for e in &mut es {
            e.line = e.line.saturating_sub(prelude_lines);
        }
        es
    })?;

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

/// The functions the prelude defines, by the names they lower to.
fn prelude_functions() -> std::collections::HashSet<String> {
    let file = parse::parse(PRELUDE).expect("the prelude parses");
    let mut names = std::collections::HashSet::new();
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
