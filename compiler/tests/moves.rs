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

#[test]
fn a_vec_yields_its_elements() {
    // `for x in v` needed `Vec` to be turned into an iterator (Ch. 5 §3),
    // and it is not one itself: `next` would consume the vector, and a `Vec`
    // you have iterated once is a `Vec` you still have.
    let src = "fn main() -> t27 {\n\
               \x20   let mut v: Vec<t27> = Vec::new();\n\
               \x20   v.push(1); v.push(2); v.push(4);\n\
               \x20   let mut s = 0;\n\
               \x20   for x in v { s += x; }\n\
               \x20   s\n}\n";
    assert!(lang::compile(src).is_ok());
}

/// The source a nested-pattern test compiles.
fn nested(arms: &str) -> String {
    format!(
        "enum Inner {{ A(t27), B }}\n\
         enum Outer {{ One(Inner), Two(t27) }}\n\
         fn go(o: Outer) -> t27 {{ match o {{ {arms} }} }}\n\
         fn main() -> t27 {{ go(Outer::Two(1)) }}\n"
    )
}

#[test]
fn a_pattern_nests() {
    // A field's pattern used to have to be a binding. Now it may insist, and
    // an arm that insists falls through to the next when it is not there.
    let src = nested(
        "Outer::One(Inner::A(n)) => n, Outer::One(Inner::B) => 100, \
         Outer::Two(k) => k, _ => 0,",
    );
    assert_eq!(refusal(&src), None, "{src}");
}

#[test]
fn a_guard_decides_between_arms_of_one_variant() {
    let src = "enum E { Val(t27), Nil }\n\
               fn go(e: E) -> t27 {\n\
                   match e {\n\
                       E::Val(n) if n > 10 => 1,\n\
                       E::Val(n) if n > 0 => 2,\n\
                       E::Val(n) => 3,\n\
                       E::Nil => 4,\n\
                   }\n\
               }\n\
               fn main() -> t27 { go(E::Val(5)) }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn an_arm_that_can_fail_does_not_cover_its_variant() {
    // Whether two nested arms *together* cover a variant is a question about
    // patterns that draft 0.1 does not answer — so it is not assumed, and
    // the message says what to write instead.
    let src =
        nested("Outer::One(Inner::A(n)) => n, Outer::One(Inner::B) => 1, Outer::Two(k) => k,");
    let why = refusal(&src).expect("refused");
    assert!(why.contains("not exhaustive"), "{why}");
    assert!(why.contains("add a `_` arm"), "{why}");
}

#[test]
fn a_pattern_reaches_through_a_box() {
    // A compiler's own AST is mostly `Box`, so this is the shape that
    // matters: read two levels of a tree in one arm without taking it apart.
    let src = "enum E { Lit(t27), Add(Box<E>, Box<E>) }\n\
               fn eval(e: &E) -> t27 {\n\
                   match e {\n\
                       E::Lit(n) => n,\n\
                       E::Add(E::Lit(a), E::Lit(b)) => a + b,\n\
                       E::Add(l, r) => eval(&*l) + eval(&*r),\n\
                       _ => 0,\n\
                   }\n\
               }\n\
               fn main() -> t27 {\n\
                   let t = E::Add(Box::new(E::Lit(1)), Box::new(E::Lit(2)));\n\
                   eval(&t)\n\
               }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn what_a_pattern_takes_through_a_box_is_borrowed() {
    // The box still owns what is inside it and will drop it. A binding that
    // owned it too would be the second owner — which is G9.27's double free
    // written as a pattern.
    let src = "struct Held { n: t27 }\n\
               fn drop(self: Held) { print_int(self.n); }\n\
               fn take(h: Held) -> t27 { h.n }\n\
               enum Inner { Has(Held), Nothing }\n\
               enum E { One(Box<Inner>), None }\n\
               fn go(e: E) -> t27 {\n\
                   match e { E::One(Inner::Has(h)) => take(h), _ => 0 }\n\
               }\n\
               fn main() -> t27 { go(E::None) }\n";
    let why = refusal(src).expect("refused");
    assert!(why.contains("cannot be moved out of"), "{why}");
}

#[test]
fn an_exclusive_reference_is_accepted_where_a_shared_one_is_wanted() {
    // Ch. 3 §2.3a. Neither a dereference nor a conversion: the same
    // address, carrying less permission, and the permission goes down.
    let src = "struct P { n: t27 }\n\
               fn read(p: &P) -> t27 { p.n }\n\
               fn write(p: &mut P) -> t27 { p.n += 1; read(p) }\n\
               fn main() -> t27 {\n\
               \x20   let mut p = P { n: 1 };\n\
               \x20   let a = write(&mut p);\n\
               \x20   let b = read(&p);\n\
               \x20   a + b\n}\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_shared_reference_is_not_accepted_where_an_exclusive_one_is_wanted() {
    // The reverse is asking for a permission nobody has.
    let src = "struct P { n: t27 }\n\
               fn write(p: &mut P) { p.n += 1; }\n\
               fn main() -> t27 { let mut p = P { n: 1 }; let r = &p; write(r); p.n }\n";
    let why = refusal(src).expect("refused");
    assert!(why.contains("expected &mut P"), "{why}");
}

#[test]
fn a_diverging_first_arm_does_not_speak_for_the_others() {
    // `join_arm` folded each arm into what the ones before it left, and
    // "nothing has contributed yet" was represented by the first arm's own
    // state — so a first arm that *returned* put its state in as if it were
    // everyone's, and a loop then said the value was moved (G9.41).
    let src = "enum E { Done, More }\n\
               fn next() -> E { E::Done }\n\
               fn go() -> Vec<t27> {\n\
               \x20   let mut out: Vec<t27> = Vec::new();\n\
               \x20   loop {\n\
               \x20       match next() {\n\
               \x20           E::Done => return out,\n\
               \x20           E::More => {}\n\
               \x20       }\n\
               \x20       out.push(1);\n\
               \x20   }\n}\n\
               fn main() -> t27 { go().len() as t27 }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_value_cannot_be_moved_out_of_a_reference_by_indexing() {
    // G9.27 refused `take(*r)`; this is the same hole one projection along.
    // Reading `h.v[i]` through a `&mut Holder` moves the element, and the
    // local that gets marked is the one holding the *reference* — so the
    // owner still dropped it, and so did the caller (G9.45).
    let src = "struct Note { n: t27 }\n\
               impl Drop for Note { fn drop(self) { } }\n\
               struct Holder { v: Vec<Note>, at: taddr }\n\
               fn take_one(h: &mut Holder) -> Note { h.v[h.at] }\n\
               fn main() -> t27 { 0 }\n";
    assert!(
        refusal(src)
            .expect("a refusal")
            .contains("cannot move out of a reference"),
    );
}

#[test]
fn a_field_of_a_borrowed_struct_is_not_movable_either() {
    let src = "struct Note { n: t27 }\n\
               impl Drop for Note { fn drop(self) { } }\n\
               struct Pair { a: Note, b: Note }\n\
               fn steal(p: &Pair) -> Note { p.a }\n\
               fn main() -> t27 { 0 }\n";
    assert!(
        refusal(src)
            .expect("a refusal")
            .contains("cannot move out of a reference"),
    );
}

#[test]
fn a_copyable_field_of_a_borrowed_struct_is_read_and_not_moved() {
    let src = "struct Pair { a: t27, b: t27 }\n\
               fn sum(p: &Pair) -> t27 { p.a + p.b }\n\
               fn main() -> t27 { let p = Pair { a: 1, b: 2 }; sum(&p) }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_struct_can_be_taken_apart_by_matching_on_it() {
    // Ch. 0 §4 lists `Point { x, y }` among the patterns and draft 0.1 read
    // it nowhere: `match` refused a struct outright, and with ownership
    // tracked per local a struct with two non-`Copy` fields could not be
    // taken apart at all (G9.46).
    let src = "struct P { a: Vec<t27>, b: Vec<t27> }\n\
               fn swap(p: P) -> P { match p { P { a, b } => P { a: b, b: a } } }\n\
               fn main() -> t27 { let p = P { a: Vec::new(), b: Vec::new() }; \
               let q = swap(p); q.a.len() as t27 }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_struct_pattern_has_one_arm_because_a_struct_has_one_shape() {
    let src = "struct P { a: t27 }\n\
               fn f(p: P) -> t27 { match p { P { a } => a, P { a } => a } }\n\
               fn main() -> t27 { 0 }\n";
    assert!(refusal(src).expect("a refusal").contains("has one shape"));
}

#[test]
fn a_let_may_take_a_struct_apart() {
    let src = "struct P { a: Vec<t27>, b: Vec<t27> }\n\
               fn parts(p: P) -> taddr { let P { a, b } = p; a.len() + b.len() }\n\
               fn main() -> t27 { let p = P { a: Vec::new(), b: Vec::new() }; \
               parts(p) as t27 }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn a_tuple_of_values_that_own_can_be_taken_apart() {
    // The old desugaring read the fields out one by one, which moves the
    // whole on the first and refuses the second (G9.46).
    let src = "fn two() -> (Vec<t27>, Vec<t27>) { (Vec::new(), Vec::new()) }\n\
               fn main() -> t27 { let (a, b) = two(); (a.len() + b.len()) as t27 }\n";
    assert_eq!(refusal(src), None);
}
