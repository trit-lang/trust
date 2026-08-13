//! Frontend tests: Trust source through every pass to the machine.
//!
//! The end-to-end ones run the program the whole way — parse, check, lower to
//! TIR, legalize, generate code, assemble, execute — because that is the only
//! claim worth making about a compiler.

use trustc::lang;
use trustc::tir;

/// Compile source to TIR, or panic with the diagnostics.
fn tir_of(src: &str) -> tir::Module {
    let m = lang::compile(src).unwrap_or_else(|e| panic!("compilation failed: {e:?}\n{src}"));
    let errs = tir::verify(&m);
    assert!(errs.is_empty(), "frontend emitted ill-formed TIR: {errs:?}");
    m
}

/// The first diagnostic from a program that should not compile.
fn error(src: &str) -> String {
    match lang::compile(src) {
        Err(errs) => errs[0].message.clone(),
        Ok(_) => panic!("expected an error from:\n{src}"),
    }
}

/// Run a whole program on the machine, with the hand-written runtime linked
/// in, returning its exit status and its output.
fn run(src: &str) -> (i128, String) {
    let module = tir_of(src);
    let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium())
        .unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    let mut asm = trustc::codegen::compile(&legalized, "main")
        .unwrap_or_else(|e| panic!("code generation failed: {e:?}"));
    asm.push_str(include_str!("../../examples/trisc/runtime.t27"));

    let image = tritium::assemble(&asm).unwrap_or_else(|e| panic!("assembly failed: {e:?}\n{asm}"));
    let mut vm = tritium::Vm::with_default_memory();
    vm.load_image(&image);
    match vm.run(50_000_000) {
        tritium::Stop::Halted(v) => (v, String::from_utf8(vm.io.output().to_vec()).unwrap()),
        other => panic!("machine stopped: {other}"),
    }
}

/// Run an expression as a whole program and return its value.
fn value(expr: &str) -> i128 {
    run(&format!("fn main() -> t27 {{ {expr} }}")).0
}

// ------------------------------------------------------------- hello world

#[test]
fn hello_world_runs_the_whole_way() {
    let (status, out) = run(include_str!("../../examples/trust/hello.tr"));
    assert_eq!(status, 0);
    assert_eq!(out, "Hello, world!\n");
}

// ------------------------------------------------------------- expressions

#[test]
fn arithmetic_is_the_abstract_machines() {
    // One division, rounding to nearest with ties away from zero.
    assert_eq!(value("7 / 2"), 4);
    assert_eq!(value("8 / 3"), 3);
    assert_eq!(value("0 - 8 / 3"), -3);
    assert_eq!(value("8 % 3"), -1);
    // `>>` is division by a power of three.
    assert_eq!(value("100 >> 2"), 11);
    assert_eq!(value("4 << 3"), 108);
    assert_eq!(value("1 + 2 * 3"), 7);
    assert_eq!(value("(1 + 2) * 3"), 9);
    assert_eq!(value("-(3 * 4)"), -12);
}

#[test]
fn the_three_radices_are_one_notation() {
    assert_eq!(value("0t1T0"), 6);
    assert_eq!(value("0hDDE"), 1);
    assert_eq!(value("3_812_798"), 3_812_798);
}

#[test]
fn comparison_is_three_way_and_the_predicates_project_it() {
    assert_eq!(value("if 1 < 2 { 10 } else { 20 }"), 10);
    assert_eq!(value("if 2 <= 2 { 10 } else { 20 }"), 10);
    assert_eq!(value("if 3 == 4 { 10 } else { 20 }"), 20);
    assert_eq!(value("if 3 != 4 { 10 } else { 20 }"), 10);
    // `<=>` yields a trit, which `match` dispatches three ways.
    assert_eq!(
        value("match 5 <=> 9 { -1t => 100, 0t => 200, 1t => 300, }"),
        100
    );
    assert_eq!(
        value("match 9 <=> 9 { -1t => 100, 0t => 200, 1t => 300, }"),
        200
    );
    assert_eq!(
        value("match 9 <=> 5 { -1t => 100, 0t => 200, 1t => 300, }"),
        300
    );
}

#[test]
fn a_trit_match_is_one_three_way_branch() {
    // The payoff: no comparison chain, just `br3` (Ch. 1 §5).
    let m = tir_of("fn f(t: trit) -> t27 { match t { -1t => 1, 0t => 2, 1t => 3, } }");
    let printed = tir::print_module(&m);
    assert!(printed.contains("br3"), "{printed}");
    assert_eq!(
        printed.matches("cmp").count(),
        0,
        "no comparison needed:\n{printed}"
    );
}

#[test]
fn the_trit_wise_methods_and_projections_work() {
    assert_eq!(value("0t111.tmin(0t1T0)"), 6);
    assert_eq!(value("0t1T1.tmax(0tTT1)"), 7);
    assert_eq!(value("0t11T.tmul(0t1T1)"), 5);
    assert_eq!(value("(5).tneg()"), -5);
    assert_eq!(value("if sign(-7).is_neg() { 1 } else { 0 }"), 1);
    assert_eq!(value("if sign(0).is_zero() { 1 } else { 0 }"), 1);
    assert_eq!(value("if true.to_trit().is_pos() { 1 } else { 0 }"), 1);
}

#[test]
fn short_circuit_operators_short_circuit() {
    assert_eq!(value("if true || 1 / 0 == 0 { 1 } else { 0 }"), 1);
    assert_eq!(value("if false && 1 / 0 == 0 { 1 } else { 0 }"), 0);
    assert_eq!(value("if !false { 1 } else { 0 }"), 1);
}

#[test]
fn casts_are_explicit_and_follow_chapter_one() {
    assert_eq!(value("9841 as t9 as t27"), 9841);
    // Narrowing wraps into the narrow symmetric range.
    assert_eq!(value("9842 as t9 as t27"), -9841);
    assert_eq!(value("1t as t27"), 1);
}

// ------------------------------------------------------- control and state

#[test]
fn loops_and_mutation_work() {
    assert_eq!(
        run(
            "fn main() -> t27 { let mut i: t27 = 0; let mut s: t27 = 0; \
             while i < 10 { i += 1; s += i; } s }"
        )
        .0,
        55
    );
    assert_eq!(
        run("fn main() -> t27 { let mut n: t27 = 0; loop { n += 1; if n == 7 { break; } } n }").0,
        7
    );
    assert_eq!(
        run(
            "fn main() -> t27 { let mut i: t27 = 0; let mut s: t27 = 0; \
             while i < 10 { i += 1; if i == 5 { continue; } s += 1; } s }"
        )
        .0,
        9
    );
}

#[test]
fn functions_call_and_recurse() {
    assert_eq!(
        run(
            "fn fib(n: t27) -> t27 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } } \
             fn main() -> t27 { fib(15) }"
        )
        .0,
        610
    );
    assert_eq!(
        run("fn double(x: t27) -> t27 { x * 2 } \
             fn main() -> t27 { return double(21); }")
        .0,
        42
    );
}

#[test]
fn arrays_are_indexed_and_bounds_checked() {
    assert_eq!(
        run("const M: [t9; 3] = [4, 5, 6]; \
             fn main() -> t27 { M[2] as t27 }")
        .0,
        6
    );
    // Ch. 2 §3: an out-of-bounds index faults; it never proceeds.
    let module = tir_of("const M: [t9; 3] = [1, 2, 3]; fn main() -> t27 { M[3] as t27 }");
    let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium()).unwrap();
    let asm = trustc::codegen::compile(&legalized, "main").unwrap();
    let image = tritium::assemble(&asm).unwrap();
    let mut vm = tritium::Vm::with_default_memory();
    vm.load_image(&image);
    assert!(matches!(vm.run(1_000_000), tritium::Stop::Fault(..)));
}

#[test]
fn overflow_traps_in_the_default_profile() {
    // Ch. 1 P4: arithmetic operators trap on overflow in checked builds.
    let module = tir_of("fn main() -> t27 { let mut n: t27 = 3812798742493; n = n + 1; n }");
    let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium()).unwrap();
    let asm = trustc::codegen::compile(&legalized, "main").unwrap();
    let image = tritium::assemble(&asm).unwrap();
    let mut vm = tritium::Vm::with_default_memory();
    vm.load_image(&image);
    assert!(matches!(
        vm.run(1_000_000),
        tritium::Stop::Fault(trit_core::FaultCode::Overflow, _)
    ));
    // …and `wrapping_add` is always available.
    assert_eq!(
        run("fn main() -> t27 { let n: t27 = 3812798742493; n.wrapping_add(1) }").0,
        -3_812_798_742_493
    );
}

// ------------------------------------------------------------ diagnostics

#[test]
fn there_are_no_implicit_conversions() {
    assert!(
        error("fn main() -> t27 { let a: t9 = 1; let b: t27 = 2; a + b }").contains("expected t9")
    );
    assert!(error("fn main() -> t27 { if 1 { 2 } else { 3 } }").contains("expected bool"));
    assert!(error("fn main() -> t27 { 1t }").contains("expected t27"));
}

#[test]
fn a_match_must_be_exhaustive() {
    let e = error("fn f(t: trit) -> t27 { match t { -1t => 1, 0t => 2, } }");
    assert!(e.contains("not exhaustive"), "{e}");
    // A wildcard completes it.
    tir_of("fn f(t: trit) -> t27 { match t { -1t => 1, _ => 2, } }");
}

#[test]
fn immutable_bindings_cannot_be_assigned() {
    let e = error("fn main() -> t27 { let x: t27 = 1; x = 2; x }");
    assert!(e.contains("not mutable"), "{e}");
}

#[test]
fn the_deferred_features_say_what_they_are_waiting_for() {
    assert!(error("fn main() -> t27 { let s = \"hi\"; 0 }").contains("library chapter"));
    assert!(error("fn main() -> t27 { for x in y { } 0 }").contains("reserved"));
    assert!(error("fn main() -> t27 { 1 ^ 2 }").contains("tmul"));
    assert!(error("fn f(x: &t27) -> t27 { 0 }").contains("Chapter 3"));
}

#[test]
fn there_is_no_as_between_trit_and_bool() {
    // Ch. 1 §6: both mappings are plausible, so the language refuses to pick.
    let e = error("fn main() -> t27 { let b: bool = 1t as bool; 0 }");
    assert!(e.contains("is_pos"), "{e}");
}

#[test]
fn comparisons_do_not_chain() {
    assert!(error("fn main() -> t27 { if 1 < 2 < 3 { 1 } else { 0 } }").contains("chain"));
    assert!(error("fn main() -> t27 { let t = 1 <=> 2 <=> 3; 0 }").contains("chain"));
}

#[test]
fn a_declaration_has_no_body_and_lowers_to_one() {
    let m = tir_of("fn putchar(c: t9); fn main() -> t27 { putchar(65); 0 }");
    assert_eq!(m.decls.len(), 1);
    assert_eq!(m.decls[0].name, "putchar");
    assert!(m.function("putchar").is_none());
}

// ---------------------------------------------------- composites (Ch. 2)

#[test]
fn the_composites_example_runs() {
    let (status, out) = run(include_str!("../../examples/trust/shapes.tr"));
    // 25 + 30 + 7 + 1 − 1, and the three signs printed on the way.
    assert_eq!(status, 62);
    assert_eq!(out, "-0+\n");
}

#[test]
fn structs_and_tuples_have_fields() {
    assert_eq!(
        run("struct Point { x: t27, y: t27 } \
             fn main() -> t27 { let p = Point { x: 3, y: 4 }; p.x * 10 + p.y }")
        .0,
        34
    );
    assert_eq!(
        run("fn main() -> t27 { let t = (1, 2, 3); t.0 * 100 + t.1 * 10 + t.2 }").0,
        123
    );
    // A tuple struct's fields are positional.
    assert_eq!(
        run("struct Trip(t9, t9, t9); \
             fn main() -> t27 { let t = Trip(4, 5, 6); (t.0 as t27) + (t.2 as t27) }")
        .0,
        10
    );
}

#[test]
fn an_aggregate_binding_is_a_copy() {
    // Ch. 2's value semantics: `q` gets its own storage.
    assert_eq!(
        run("struct P { a: t27 } \
             fn main() -> t27 { let p = P { a: 1 }; let mut q = p; q = P { a: 2 }; p.a }")
        .0,
        1
    );
}

#[test]
fn a_trit_shaped_enum_dispatches_with_one_branch() {
    // Ch. 2 §5.2: three fieldless variants with discriminants −1, 0, +1 are
    // representation-identical to `trit`, and `match` lowers to one `br3`.
    let m = tir_of(
        "enum Sign { Neg = -1, Zero = 0, Pos = 1 } \
         fn f(s: Sign) -> t27 { match s { Sign::Neg => 1, Sign::Zero => 2, Sign::Pos => 3, } }",
    );
    let printed = tir::print_module(&m);
    assert!(printed.contains("br3"), "{printed}");
    assert_eq!(
        printed.matches("cmp").count(),
        0,
        "a trit-shaped enum needs no comparison:\n{printed}"
    );
}

#[test]
fn enum_payloads_are_bound_by_patterns() {
    let src = "enum Shape { Dot, Line(t27), Rect { w: t27, h: t27 } } \
               fn area(s: Shape) -> t27 { \
                   match s { \
                       Shape::Dot => 0, \
                       Shape::Line(n) => n, \
                       Shape::Rect { w, h } => w * h, \
                   } \
               }";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ area(Shape::Rect {{ w: 6, h: 7 }}) }}"
        ))
        .0,
        42
    );
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ area(Shape::Line(9)) }}"
        ))
        .0,
        9
    );
    assert_eq!(
        run(&format!("{src} fn main() -> t27 {{ area(Shape::Dot) }}")).0,
        0
    );
}

#[test]
fn a_niche_encoded_enum_costs_nothing_over_its_payload() {
    // Ch. 2 §6, guaranteed rather than merely observed: an enum wrapping a
    // `trit` with one extra fieldless variant is one tryte.
    use trustc::layout::{Repr, Ty, TypeDb, Variant, layout_of};
    let mut db = TypeDb::new();
    db.enum_(
        "Maybe",
        Repr::Lang,
        vec![Variant::unit("Nothing"), Variant::payload("Just", Ty::Trit)],
    );
    let l = layout_of(&db, &Ty::named("Maybe")).unwrap();
    assert_eq!((l.size, l.align), (1, 1));

    // And it still round-trips through the machine.
    let src = "enum Maybe { Nothing, Just(trit) } \
               fn get(m: Maybe, fallback: trit) -> trit { \
                   match m { Maybe::Nothing => fallback, Maybe::Just(t) => t, } \
               } \
               fn main() -> t27 { \
                   let a = get(Maybe::Just(1t), 0t) as t27; \
                   let b = get(Maybe::Nothing, -1t) as t27; \
                   a * 10 + b \
               }";
    assert_eq!(run(src).0, 9); // 1*10 + (−1)
}

#[test]
fn a_fieldless_enum_casts_to_its_discriminant() {
    // Ch. 2 §5.3, and there is no cast the other way.
    assert_eq!(
        run("enum E { A = -4, B = 0, C = 7 } fn main() -> t27 { (E::C as t27) - (E::A as t27) }").0,
        11
    );
    // There is no cast in the reverse direction: it is fallible, and library
    // `try_from` territory.
    let e = error("enum E { A } fn main() -> t27 { let x: E = 0 as E; 0 }");
    assert!(e.contains("no cast from") && e.contains("try_from"), "{e}");
}

#[test]
fn repr_linear_lays_fields_out_in_declaration_order() {
    // Ch. 2 §1: `repr(linear)` is a documented function of the declaration.
    use trustc::layout::{IntTy, Repr, Ty, TypeDb, layout_of};
    let mut db = TypeDb::new();
    db.struct_(
        "H",
        Repr::Linear,
        vec![("a", Ty::Int(IntTy::T9)), ("b", Ty::Int(IntTy::T27))],
    );
    let l = layout_of(&db, &Ty::named("H")).unwrap();
    assert_eq!(l.offsets, vec![0, 3]);
    assert_eq!((l.size, l.align), (6, 3));
    // The source form is accepted too.
    tir_of("#[repr(linear)] struct H { a: t9, b: t27 } fn main() -> t27 { 0 }");
}

#[test]
fn aggregates_cross_function_boundaries() {
    assert_eq!(
        run("struct P { x: t27, y: t27 } \
             fn make(a: t27) -> P { P { x: a, y: a * 2 } } \
             fn sum(p: P) -> t27 { p.x + p.y } \
             fn main() -> t27 { sum(make(7)) }")
        .0,
        21
    );
}

#[test]
fn an_enum_match_must_cover_every_variant() {
    let e = error("enum E { A, B, C } fn f(e: E) -> t27 { match e { E::A => 1, E::B => 2, } }");
    assert!(e.contains("not exhaustive"), "{e}");
    tir_of("enum E { A, B, C } fn f(e: E) -> t27 { match e { E::A => 1, _ => 2, } }");
}

#[test]
fn saturating_and_overflowing_are_available() {
    // Ch. 1 §4 carries the whole family over.
    let max = 3_812_798_742_493i128;
    assert_eq!(
        run(&format!(
            "fn main() -> t27 {{ let n: t27 = {max}; n.saturating_add(1) }}"
        ))
        .0,
        max
    );
    assert_eq!(
        run(&format!(
            "fn main() -> t27 {{ let n: t27 = -{max}; n.saturating_sub(1) }}"
        ))
        .0,
        -max
    );
    assert_eq!(
        run("fn main() -> t27 { let n: t27 = 5; n.saturating_add(1) }").0,
        6
    );
    // `overflowing_*` hands back the wrapped value and whether it wrapped.
    assert_eq!(
        run(&format!(
            "fn main() -> t27 {{ let n: t27 = {max}; let r = n.overflowing_add(1); \
             if r.1 {{ 1 }} else {{ 0 }} }}"
        ))
        .0,
        1
    );
    assert_eq!(
        run("fn main() -> t27 { let r = (5).overflowing_add(1); r.0 }").0,
        6
    );
    // `checked_*` needs Option, and so needs generics.
    assert!(error("fn main() -> t27 { (1).checked_add(2) }").contains("Chapter 4"));
}
