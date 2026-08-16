//! Macros (Language Ch. 7): one rule, variadic, hygienic.

use trustc::lang;

fn refusal(src: &str) -> Option<String> {
    lang::compile(src)
        .err()
        .map(|errs| errs.iter().map(|e| e.message.clone()).collect())
}

const VEC: &str = "macro vec($($x),*) {\n\
                   \x20   {\n\
                   \x20       let mut v: Vec<t27> = Vec::new();\n\
                   \x20       $( v.push($x); )*\n\
                   \x20       v\n\
                   \x20   }\n}\n";

#[test]
fn a_macros_arity_is_part_of_the_call() {
    // What no function signature can say, and the only reason draft 0.1 has
    // macros at all (Ch. 7 intro).
    let src = format!(
        "{VEC}fn main() -> t27 {{\n\
         \x20   let a = vec!();\n\
         \x20   let b = vec!(1);\n\
         \x20   let c = vec!(1, 2, 3);\n\
         \x20   (a.len() + b.len() + c.len()) as t27\n}}\n"
    );
    assert_eq!(refusal(&src), None, "{src}");
}

#[test]
fn a_body_binding_does_not_capture_the_callers() {
    // §4, and the decision the chapter turns on. Both the caller and the
    // body write `tmp`; the swap is the swap.
    let src = "macro swap($a, $b) { { let tmp = $a; $a = $b; $b = tmp; } }\n\
               fn main() -> t27 {\n\
               \x20   let mut tmp = 1;\n\
               \x20   let mut x = 2;\n\
               \x20   swap!(tmp, x);\n\
               \x20   tmp * 10 + x\n}\n";
    assert_eq!(refusal(src), None);
    // And it runs to 21 rather than to whatever a capture would give.
    let module = lang::compile(src).expect("compiles");
    assert!(module.function("main").is_some());
}

#[test]
fn two_expansions_of_one_macro_do_not_collide() {
    let src = "macro hold($x) { { let n = $x; n + n } }\n\
               fn main() -> t27 { hold!(1) + hold!(2) }\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn an_argument_is_substituted_as_an_expression() {
    // §2.1: written twice, evaluated twice — and usable where a value would
    // not be, which is the reason.
    let src = "macro twice($x) { $x + $x }\n\
               macro setb($p) { { $p = 7; } }\n\
               fn main() -> t27 {\n\
               \x20   let mut b = 0;\n\
               \x20   setb!(b);\n\
               \x20   twice!(b)\n}\n";
    assert_eq!(refusal(src), None);
}

#[test]
fn the_ways_an_invocation_can_be_wrong() {
    let one = "macro one($x) { $x }\n";
    let why = refusal(&format!("{one}fn main() -> t27 {{ one!(1, 2) }}\n")).expect("refused");
    assert!(why.contains("takes 1 argument(s), 2 given"), "{why}");

    let why =
        refusal("macro two($a, $b) { $a + $b }\nfn main() -> t27 { two!(1) }\n").expect("refused");
    assert!(why.contains("takes 2 argument(s), 1 given"), "{why}");

    let why = refusal("fn main() -> t27 { nope!(1) }\n").expect("refused");
    assert!(why.contains("is not a macro in scope"), "{why}");

    // §3.1: no recursion, direct or through another.
    let why =
        refusal("macro a($x) { b!($x) }\nmacro b($x) { a!($x) }\nfn main() -> t27 { a!(1) }\n")
            .expect("refused");
    assert!(why.contains("expands into itself"), "{why}");
}

#[test]
fn a_dollar_means_nothing_outside_a_macro() {
    let why = refusal("fn main() -> t27 { $x }\n").expect("refused");
    assert!(why.contains("appears only in a macro"), "{why}");
}

#[test]
fn a_repetition_needs_something_to_repeat() {
    let why = refusal("macro m($x) { { $( print($x); )* 0 } }\nfn main() -> t27 { m!(1) }\n")
        .expect("refused");
    assert!(why.contains("has no repetition"), "{why}");

    let why = refusal("macro m($x) { $y }\nfn main() -> t27 { m!(1) }\n").expect("refused");
    assert!(why.contains("is not a parameter of this macro"), "{why}");

    let why = refusal("macro m($x, $x) { $x }\nfn main() -> t27 { m!(1, 2) }\n").expect("refused");
    assert!(why.contains("twice"), "{why}");
}

#[test]
fn a_macro_is_an_item_and_may_be_pub() {
    // Ch. 6 §2 applies to a macro like anything else.
    let src = "pub macro id($x) { $x }\nfn main() -> t27 { id!(1) }\n";
    assert_eq!(refusal(src), None);
}
