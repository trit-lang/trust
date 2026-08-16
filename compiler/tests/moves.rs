//! Moving out of a reference (Ch. 3 §1.2), which was not refused (G9.27).
//!
//! Each of these compiled before the fix, and each dropped one value more
//! than once. They are here rather than in `frontend.rs` because what they
//! test is a *refusal*, and a refusal that quietly stops refusing is the
//! failure mode this file exists to catch.

use trustc::lang;

/// The message a program is refused with, or `None` if it compiled.
fn refusal(src: &str) -> Option<String> {
    lang::compile(src)
        .err()
        .map(|errs| errs.iter().map(|e| e.message.clone()).collect())
}

/// A type with a destructor, so that dropping it twice is observable and
/// copying it is not allowed.
const OWNED: &str = "struct Held { n: t27 }\n\
                     fn drop(self: Held) { print_int(self.n); }\n\
                     fn take(h: Held) -> t27 { h.n }\n";

#[test]
fn a_value_behind_a_reference_cannot_be_taken() {
    let why = refusal(&format!(
        "{OWNED}fn through(r: &Held) -> t27 {{ take(*r) }}\n\
         fn main() -> t27 {{ let h = Held {{ n: 7 }}; through(&h) }}\n"
    ))
    .expect("refused");
    assert!(why.contains("cannot move out of a reference"), "{why}");
}

#[test]
fn a_closure_capture_cannot_be_taken_either() {
    // A capture is a reference (Ch. 4 §4.2) and is rewritten to `*self.h`,
    // so this is the same hole wearing a hat. It dropped the value three
    // times: once per call, and once when the scope ended.
    let why = refusal(&format!(
        "{OWNED}fn twice(f: impl Fn() -> t27) -> t27 {{ f() + f() }}\n\
         fn main() -> t27 {{ let h = Held {{ n: 7 }}; twice(|| take(h)) }}\n"
    ))
    .expect("refused");
    assert!(why.contains("cannot move out of a reference"), "{why}");
}

#[test]
fn a_copy_through_a_reference_is_still_a_copy() {
    // Without a destructor the struct is copyable, and reading one through a
    // reference copies it — there is no second drop to be wrong about.
    let src = "struct P { n: t27 }\n\
               fn take(p: P) -> t27 { p.n }\n\
               fn through(r: &P) -> t27 { take(*r) }\n\
               fn main() -> t27 { let p = P { n: 7 }; through(&p) + p.n }\n";
    assert_eq!(refusal(src), None, "a copy is not a move");
}

#[test]
fn a_box_still_gives_up_what_it_owns() {
    // A `Box` is not a reference: reading through one moves the box, which
    // *is* the owner, so there is one drop and it happens there. The first
    // attempt at this refusal broke exactly this.
    let src = "enum Tree { Leaf, Node(Box<Tree>, t27) }\n\
               fn depth(t: Tree) -> t27 {\n\
                   match t { Tree::Leaf => 0, Tree::Node(l, n) => n + depth(*l) }\n\
               }\n\
               fn main() -> t27 { depth(Tree::Node(Box::new(Tree::Leaf), 1)) }\n";
    assert_eq!(refusal(src), None, "moving out of a box is moving the box");
}

#[test]
fn mutual_recursion_compiles_at_all() {
    // The inliner unrolled `is_even`/`is_odd` into `main` without bound and
    // the compiler never returned (G9.28). A compiler written in this
    // language would be nothing but call cycles.
    let src = "fn is_even(n: t27) -> bool { if n == 0 { true } else { is_odd(n - 1) } }\n\
               fn is_odd(n: t27) -> bool { if n == 0 { false } else { is_even(n - 1) } }\n\
               fn main() -> t27 { if is_even(10) { 1 } else { 0 } }\n";
    let module = lang::compile(src).expect("compiles");
    let module = trustc::tir::canonicalize_module(&module);
    let mut module = trustc::tir::inline_module(&module);
    trustc::tir::drop_uncalled(&mut module, &["main"]);
    assert!(module.function("is_even").is_some());
    assert!(module.function("is_odd").is_some());
}
