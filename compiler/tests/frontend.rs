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

#[test]
fn the_traits_example_runs_the_whole_way() {
    let (status, out) = run(include_str!("../../examples/trust/traits.tr"));
    assert_eq!(status, 28);
    assert_eq!(out, "C: 12\nR: 12\n");
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
    // A *runtime* narrowing, which constant folding cannot hide: `t9` is not
    // a legal register width on this target, so the value is promoted to a
    // word and must come back down to be stored (TIR §6.2).
    assert_eq!(
        run("fn main() -> t27 { let n: t27 = 9842; let d: t9 = n as t9; d as t27 }").0,
        -9841
    );
    assert_eq!(
        run("fn main() -> t27 { let mut a: [t9; 4] = [0; 4]; let i: taddr = 1; \
             a[i] += 10; a[i] as t27 }")
            .0,
        10
    );
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
    assert!(error("fn main() -> t27 { 1 ^ 2 }").contains("tmul"));
    // Reserved for a chapter nobody has written…
    assert!(error("fn main() -> t27 { mod m; 0 }").contains("not written yet"));
    // …as against one that is written and only partly built.
    assert!(error("fn main() -> t27 { let x: dyn Shape = 0; 0 }").contains("Ch. 4"));
    assert!(error("fn main() -> t27 { for x in y { } 0 }").contains("§5.7"));
    // References are Chapter 3 and traits are Chapter 4, and both now work;
    // generic parameters are the next piece of Chapter 4.
    tir_of("fn f(x: &t27) -> t27 { *x }");
    tir_of("trait Shape { fn area(&self) -> t27; }");
    assert!(error("fn f<T>(x: T) -> T { x }").contains("Chapter 4"));
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

// ------------------------------------------- references and slices (Ch. 3)

#[test]
fn the_references_example_runs() {
    // 25 + 4 + (1+2+3+4)·10
    assert_eq!(run(include_str!("../../examples/trust/refs.tr")).0, 129);
}

#[test]
fn a_shared_reference_reads_and_auto_dereferences() {
    assert_eq!(
        run("struct P { x: t27, y: t27 } \
             fn get(p: &P) -> t27 { p.x + p.y } \
             fn main() -> t27 { let a = P { x: 3, y: 4 }; get(&a) }")
        .0,
        7
    );
    // `*r` is available too, and reads the same place.
    assert_eq!(
        run("fn main() -> t27 { let a: t27 = 5; let r = &a; *r + 1 }").0,
        6
    );
}

#[test]
fn an_exclusive_reference_writes_through() {
    assert_eq!(
        run("struct P { x: t27 } \
             fn bump(p: &mut P) { p.x += 1; } \
             fn main() -> t27 { let mut a = P { x: 7 }; bump(&mut a); bump(&mut a); a.x }")
        .0,
        9
    );
    assert_eq!(
        run("fn set(r: &mut t27, v: t27) { *r = v; } \
             fn main() -> t27 { let mut a: t27 = 1; set(&mut a, 42); a }")
        .0,
        42
    );
}

#[test]
fn a_shared_reference_cannot_be_written_through() {
    // Ch. 3 §2.1: a shared reference permits reading, and that is all.
    let e = error("fn set(r: &t27) { *r = 1; } fn main() -> t27 { 0 }");
    assert!(e.contains("shared reference"), "{e}");
    let e = error("struct P { x: t27 } fn set(p: &P) { p.x = 1; } fn main() -> t27 { 0 }");
    assert!(e.contains("shared reference"), "{e}");
}

#[test]
fn an_array_reference_coerces_to_a_slice() {
    // Ch. 3 §5.3, and the length comes from the type.
    assert_eq!(
        run("fn total(xs: &[t27]) -> t27 { xs[0] + xs[1] + xs[2] } \
             fn main() -> t27 { let a: [t27; 3] = [10, 20, 30]; total(&a) }")
        .0,
        60
    );
    // A slice carries its length, so a shorter array works with the same
    // function.
    assert_eq!(
        run("fn head(xs: &[t9]) -> t27 { xs[0] as t27 } \
             fn main() -> t27 { let a: [t9; 2] = [7, 8]; head(&a) }")
        .0,
        7
    );
}

#[test]
fn a_slice_write_goes_through_an_exclusive_reference() {
    assert_eq!(
        run("fn zero(xs: &mut [t27]) { xs[0] = 0; } \
             fn main() -> t27 { let mut a: [t27; 2] = [5, 6]; zero(&mut a); a[0] + a[1] }")
        .0,
        6
    );
}

#[test]
fn slice_indices_are_bounds_checked_against_the_length() {
    // Ch. 3 §5.5: out of bounds faults and never proceeds, and a negative
    // index is out of bounds rather than end-relative.
    for index in ["3", "0 - 1"] {
        let src = format!(
            "fn get(xs: &[t27], i: taddr) -> t27 {{ xs[i] }} \
             fn main() -> t27 {{ let a: [t27; 3] = [1, 2, 3]; get(&a, {index}) }}"
        );
        let module = tir_of(&src);
        let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium()).unwrap();
        let asm = trustc::codegen::compile(&legalized, "main").unwrap();
        let image = tritium::assemble(&asm).unwrap();
        let mut vm = tritium::Vm::with_default_memory();
        vm.load_image(&image);
        assert!(
            matches!(vm.run(1_000_000), tritium::Stop::Fault(..)),
            "index {index} should fault"
        );
    }
}

#[test]
fn a_reference_survives_being_stored_and_copied() {
    // TIR could not hold a pointer in memory until this chapter needed it;
    // provenance travels with the value (docs/spec-gaps.md G6.7).
    assert_eq!(
        run("struct Holder { r: t27 } \
             fn main() -> t27 { \
                 let a: t27 = 41; \
                 let r = &a; \
                 let s = r; \
                 *s + 1 \
             }")
        .0,
        42
    );
}

#[test]
fn a_reference_may_be_returned_under_elision() {
    // Ch. 3 §3.3 rule 2: one reference among the parameters, so the returned
    // reference borrows from it.
    assert_eq!(
        run("fn first(xs: &[t27]) -> &t27 { &xs[0] } \
             fn main() -> t27 { let a: [t27; 3] = [7, 8, 9]; *first(&a) }")
        .0,
        7
    );
    // A reference into a local would dangle.
    let e = error("fn bad() -> &t27 { let a: t27 = 1; &a } fn main() -> t27 { 0 }");
    assert!(e.contains("borrows from nothing"), "{e}");
    // Two candidates, and elision cannot choose between them (§3.3).
    let e = error("fn pick(a: &t27, b: &t27) -> &t27 { a } fn main() -> t27 { 0 }");
    assert!(e.contains("elision cannot choose"), "{e}");
    // A reference as a parameter, a local, or a field of a local is fine.
    tir_of("fn f(x: &t27) -> t27 { *x }");
    tir_of("fn main() -> t27 { let a: t27 = 1; let r = &a; *r }");
}

#[test]
fn a_returned_reference_keeps_its_caller_borrow_alive() {
    // The loan on the argument must live as long as the result does, or the
    // borrow checker would let the referent be written under it.
    let e = error(
        "struct P { x: t27 } fn get(p: &P) -> &t27 { &p.x } \
         fn main() -> t27 { let mut p = P { x: 1 }; let r = get(&p); p.x = 9; *r }",
    );
    assert!(e.contains("cannot be written to"), "{e}");
}

#[test]
fn the_aliasing_rule_is_enforced() {
    // Ch. 3 §2.2: one exclusive borrow, or any number of shared ones.
    let pre = "struct P { x: t27 } fn main() -> t27 { let mut p = P { x: 1 }; ";
    tir_of(&format!("{pre} let r = &p; let s = &p; r.x + s.x }}"));
    let e = error(&format!("{pre} let r = &p; let s = &mut p; r.x }}"));
    assert!(e.contains("borrow"), "{e}");
    let e = error(&format!("{pre} let r = &mut p; let s = &mut p; r.x }}"));
    assert!(e.contains("borrow"), "{e}");
    // The referent may not be written while it is borrowed…
    let e = error(&format!("{pre} let r = &p; p.x = 9; r.x }}"));
    assert!(e.contains("cannot be written to"), "{e}");
    // …nor moved out of.
    let e = error(
        "struct B { x: t27 } fn drop(self: B) { } fn take(b: B) { } \
         fn main() -> t27 { let a = B { x: 1 }; let r = &a; take(a); r.x }",
    );
    assert!(e.contains("moved"), "{e}");
}

#[test]
fn a_borrow_lives_to_its_last_use_and_no_further() {
    // Ch. 3 §4.2's own example, which a lexical rule would reject.
    assert_eq!(
        run("struct P { x: t27, y: t27 } \
             fn main() -> t27 { \
                 let mut p = P { x: 1, y: 2 }; \
                 let r = &p; \
                 let a = r.x; \
                 p.x = 9; \
                 a + p.x \
             }")
        .0,
        10
    );
    // A borrow taken and used inside a loop body is fine.
    tir_of(
        "struct P { x: t27 } \
         fn main() -> t27 { let p = P { x: 1 }; let mut i: t27 = 0; \
                            while i < 3 { let r = &p; i += r.x; } 0 }",
    );
}

#[test]
fn lifetimes_parse_and_are_erased() {
    // Ch. 3 §3.1: no lifetime reaches TIR.
    let m = tir_of("fn f<'a>(x: &'a t27) -> t27 { *x }");
    let printed = tir::print_module(&m);
    assert!(
        !printed.contains('\''),
        "a lifetime reached TIR:\n{printed}"
    );
}

#[test]
fn a_slice_type_needs_a_reference() {
    // Ch. 3 §5.1: `[T]` is dynamically sized and is never the type of a
    // place. It reaches the frontend only behind `&`.
    let e = error("fn f(xs: [t27]) -> t27 { 0 }");
    assert!(!e.is_empty(), "a bare slice parameter should be rejected");
}

// -------------------------------------------------- ownership (Ch. 3 §1)

#[test]
fn the_ownership_example_runs() {
    let (status, out) = run(include_str!("../../examples/trust/ownership.tr"));
    assert_eq!(status, 0);
    // "BA" in reverse declaration order, then C dropped inside `consume`,
    // then D dropped at the end because the branch did not take it.
    assert_eq!(out, "BA-C-\nD");
}

#[test]
fn a_type_with_a_destructor_moves_rather_than_copies() {
    // Ch. 3 §1.2: a type with a destructor is not copyable, and every type
    // containing one moves too.
    let e = error(
        "struct B { x: t27 } fn drop(self: B) { } \
         fn main() -> t27 { let a = B { x: 1 }; let b = a; let c = a; 0 }",
    );
    assert!(e.contains("moved out of"), "{e}");
    // A type without one is copied, so the same shape is fine.
    tir_of("struct P { x: t27 } fn main() -> t27 { let a = P { x: 1 }; let b = a; let c = a; 0 }");
}

#[test]
fn a_move_on_one_path_does_not_poison_the_other() {
    // The branches are checked from the same state, and joined afterwards.
    tir_of(
        "struct B { x: t27 } fn drop(self: B) { } fn take(b: B) { } \
         fn main() -> t27 { let a = B { x: 1 }; if true { take(a); } else { take(a); } 0 }",
    );
    // …and a value moved on some path cannot be used after the join.
    let e = error(
        "struct B { x: t27 } fn drop(self: B) { } fn take(b: B) { } \
         fn main() -> t27 { let a = B { x: 1 }; if true { take(a); } take(a); 0 }",
    );
    assert!(e.contains("may have been moved"), "{e}");
}

#[test]
fn a_value_cannot_be_moved_out_of_inside_a_loop() {
    let e = error(
        "struct B { x: t27 } fn drop(self: B) { } fn take(b: B) { } \
         fn main() -> t27 { let a = B { x: 1 }; let mut i: t27 = 0; \
                            while i < 2 { take(a); i += 1; } 0 }",
    );
    assert!(e.contains("loop may reach this again"), "{e}");
}

#[test]
fn a_destructor_is_checked_when_it_is_declared() {
    assert!(error("fn drop(x: t27) { }").contains("named `self`"));
    assert!(error("struct B { x: t27 } fn drop(self: B) -> t27 { 0 }").contains("returns nothing"));
    assert!(
        error("struct B { x: t27 } fn drop(self: B) { } fn drop(self: B) { }")
            .contains("more than one destructor")
    );
    // A destructor's `self` is not dropped as a whole, or it would call
    // itself for ever (Ch. 3 §1.4).
    let (_, out) = run(
        "fn putchar(c: t9); struct B { t: t9 } fn drop(self: B) { putchar(self.t); } \
         fn main() -> t27 { let a = B { t: 88 }; 0 }",
    );
    assert_eq!(out, "X");
}

#[test]
fn drops_nest_through_fields() {
    // A struct containing a droppable field is itself droppable, and its
    // fields are dropped after its own destructor (Ch. 3 §1.4).
    let (_, out) = run("fn putchar(c: t9); \
         struct Inner { t: t9 } fn drop(self: Inner) { putchar(self.t); } \
         struct Outer { a: Inner, b: Inner } \
         fn main() -> t27 { let o = Outer { a: Inner { t: 49 }, b: Inner { t: 50 } }; 0 }");
    assert_eq!(out, "12"); // declaration order within the struct
}

// ------------------------------------------------- Chapter 4: traits, impls

#[test]
fn inherent_methods_and_associated_functions() {
    // Ch. 4 §1.2: an impl block attaches methods to a type; §1.4: a function
    // without a `self` receiver is called on the type.
    assert_eq!(
        run("struct Point { x: t27, y: t27 } \
             impl Point { \
                 fn origin() -> Point { Point { x: 0, y: 0 } } \
                 fn magnitude_squared(&self) -> t27 { self.x * self.x + self.y * self.y } \
                 fn translate(&mut self, dx: t27, dy: t27) { self.x += dx; self.y += dy; } \
             } \
             fn main() -> t27 { \
                 let mut p = Point::origin(); \
                 p.translate(3, 4); \
                 p.magnitude_squared() \
             }")
        .0,
        25
    );
}

#[test]
fn a_trait_is_implemented_and_dispatched_statically() {
    // §1.1, §1.2, §1.5: required methods, two impls, and a default body the
    // second impl does not override.
    let src = "trait Shape { \
                   fn area(&self) -> t27; \
                   fn is_degenerate(&self) -> bool { self.area() == 0 } \
               } \
               struct Circle { r: t27 } \
               struct Rect { w: t27, h: t27 } \
               impl Shape for Circle { fn area(&self) -> t27 { self.r * self.r * 3 } } \
               impl Shape for Rect { fn area(&self) -> t27 { self.w * self.h } } \
               fn main() -> t27 { \
                   let c = Circle { r: 2 }; \
                   let r = Rect { w: 3, h: 0 }; \
                   let mut n: t27 = c.area(); \
                   if r.is_degenerate() { n += 100; } \
                   n \
               }";
    assert_eq!(run(src).0, 112);
}

#[test]
fn a_supertrait_supplies_its_methods() {
    // §1.6, and the default body of `B` calling a required method of `A`.
    assert_eq!(
        run("trait A { fn a(&self) -> t27; } \
             trait B: A { fn b(&self) -> t27 { self.a() * 2 } } \
             struct S { n: t27 } \
             impl A for S { fn a(&self) -> t27 { self.n } } \
             impl B for S { } \
             fn main() -> t27 { let s = S { n: 21 }; s.b() }")
        .0,
        42
    );
    assert!(error("trait B: Nope { } fn main() -> t27 { 0 }").contains("not a trait in scope"));
}

#[test]
fn a_receiver_auto_dereferences_and_writes_through() {
    // Ch. 3 §2.3 applies to method receivers: `r.get()` through a reference.
    assert_eq!(
        run("struct P { x: t27 } \
             impl P { fn get(&self) -> t27 { self.x } fn set(&mut self, v: t27) { self.x = v; } } \
             fn read(p: &P) -> t27 { p.get() } \
             fn write(p: &mut P) { p.set(9); } \
             fn main() -> t27 { let mut p = P { x: 1 }; write(&mut p); read(&p) }")
        .0,
        9
    );
    // A method on a built-in type: the orphan rule permits it because the
    // trait is local (§1.8).
    assert_eq!(
        run("trait Double { fn double(&self) -> t27; } \
             impl Double for t27 { fn double(&self) -> t27 { *self * 2 } } \
             fn main() -> t27 { (21).double() }")
        .0,
        42
    );
}

#[test]
fn elision_rule_three_lends_from_self() {
    // Ch. 4 §1.4: a `&self` method lends to its result, and rule 2 does not
    // get to complain about the other reference parameters.
    assert_eq!(
        run("struct W { d: [t27; 3] } \
             impl W { fn first(&self, _other: &t27) -> &t27 { &self.d[0] } } \
             fn main() -> t27 { let w = W { d: [9, 8, 7] }; let k: t27 = 1; *w.first(&k) }")
        .0,
        9
    );
    // And the caller's borrow lives as long as the result does.
    let e = error(
        "struct P { x: t27 } \
         impl P { fn get(&self) -> &t27 { &self.x } } \
         fn main() -> t27 { let mut p = P { x: 1 }; let r = p.get(); p.x = 9; *r }",
    );
    assert!(e.contains("cannot be written to"), "{e}");
}

#[test]
fn drop_is_a_trait_and_means_what_chapter_three_said() {
    // Ch. 4 §5.2: `impl Drop for B` is Ch. 3 §1.4's `fn drop(self: B)`, and
    // both make the type non-copyable and get called at end of scope.
    let both = [
        "struct B { x: t27 } impl Drop for B { fn drop(self) { } }",
        "struct B { x: t27 } fn drop(self: B) { }",
    ];
    for decl in both {
        let m = tir_of(&format!(
            "{decl} fn main() -> t27 {{ let b = B {{ x: 7 }}; b.x }}"
        ));
        let printed = tir::print_module(&m);
        assert!(printed.contains("drop.B"), "{printed}");
        // Moving it out means it is not dropped, and not usable either.
        let e = error(&format!(
            "{decl} fn take(b: B) -> t27 {{ b.x }} \
             fn main() -> t27 {{ let a = B {{ x: 1 }}; let n = take(a); let m = a.x; n }}"
        ));
        assert!(e.contains("moved out of"), "{e}");
    }
    // §5.2's restrictions.
    assert!(
        error("struct S { } impl S { fn drop(self) { } } fn main() -> t27 { 0 }")
            .contains("impl Drop for T")
    );
    let e = error("struct S { } impl Drop for S { fn drop(&self) { } } fn main() -> t27 { 0 }");
    assert!(e.contains("by value"), "{e}");
}

#[test]
fn a_trait_impl_must_match_its_trait() {
    let s = "trait T { fn f(&self) -> t27; } struct S { } ";
    // Missing a required method.
    assert!(error(&format!("{s} impl T for S {{ }} fn main() -> t27 {{ 0 }}")).contains("missing"));
    // Supplying one the trait does not declare (§1.2).
    let e = error(&format!(
        "{s} impl T for S {{ fn f(&self) -> t27 {{ 1 }} fn g(&self) -> t27 {{ 2 }} }} \
         fn main() -> t27 {{ 0 }}"
    ));
    assert!(e.contains("no method `g`"), "{e}");
    // A signature that disagrees.
    let e = error(&format!(
        "{s} impl T for S {{ fn f(&self) -> t9 {{ 1 }} }} fn main() -> t27 {{ 0 }}"
    ));
    assert!(e.contains("does not match"), "{e}");
    // Two methods of the same name on one type (§1.3).
    let e = error(
        "struct S { } impl S { fn f(&self) -> t27 { 1 } } \
         impl S { fn f(&self) -> t27 { 2 } } fn main() -> t27 { 0 }",
    );
    assert!(e.contains("already has a method"), "{e}");
    // A method that does not exist.
    let e = error("struct S { } fn main() -> t27 { let s = S { }; s.nope() }");
    assert!(e.contains("no method `nope`"), "{e}");
}

#[test]
fn chapter_ones_methods_are_the_languages_and_are_not_overridden() {
    // §5.4's principle applied to Ch. 1: the built-in methods resolve first,
    // so an impl cannot change what `tmin` means.
    assert_eq!(value("0t111.tmin(0t1T0)"), 6);
    assert_eq!(
        run("trait Bad { fn tmin(&self, o: t27) -> t27; } \
             impl Bad for t27 { fn tmin(&self, o: t27) -> t27 { 999 } } \
             fn main() -> t27 { 0t111.tmin(0t1T0) }")
        .0,
        6
    );
}
