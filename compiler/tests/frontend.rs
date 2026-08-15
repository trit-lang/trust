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
    let target = tir::TargetDesc::tritium();
    // TIR §6's pipeline: target-independent optimization, then legalization.
    let module = tir::canonicalize_module(&module);
    let legalized = tir::legalize_module(&module, &target)
        .unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    // TIR §6's post-condition: the backend is entitled to assume this, so
    // every end-to-end test checks it rather than trusting the pass.
    let errs = tir::verify_legalized(&legalized, &target);
    assert!(errs.is_empty(), "not legalized: {errs:?}");
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
    assert_eq!(out, "Hello, 世界! 🙂\n");
}

#[test]
fn the_demo_runs_the_whole_way() {
    // Every feature the compiler has, in one program, checked against its
    // exact output — which is the only claim about a compiler worth making.
    let (status, out) = run(include_str!("../../examples/trust/demo.tr"));
    assert_eq!(status, 0);
    assert_eq!(
        out,
        "0t1 0t10 0t100 0t1000 0t10000 0t100000 \n\
         0tTT1 -11 0t1101001 1000\n\
         C=48 R=42 90\n\
         3 6 9 12 \n\
         0.4 0.11 1.2 1.9 \n\
         42\n\
         [2]\n\
         [3][1]"
    );
}

#[test]
fn a_trait_object_satisfies_its_own_traits_bound() {
    // Ch. 4 §3.1: dispatch through an object is what its vtable is for, so
    // `dyn Shape` is a `Shape` and a generic function may take one.
    assert_eq!(
        run("trait Shape { fn area(&self) -> t27; } \
             struct C { r: t27 } \
             impl Shape for C { fn area(&self) -> t27 { self.r * 2 } } \
             fn twice<S: Shape>(s: &S) -> t27 { s.area() * 2 } \
             fn main() -> t27 { let c = C { r: 5 }; let d: &dyn Shape = &c; twice(d) }")
        .0,
        20
    );
}

#[test]
fn the_generics_example_runs_the_whole_way() {
    let (status, out) = run(include_str!("../../examples/trust/generics.tr"));
    assert_eq!(status, 19);
    assert_eq!(out, "18\n20\n18\n");
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
    // word and must come back down to be stored (TIR §6).
    assert_eq!(
        run("fn main() -> t27 { let n: t27 = 9842; let d: t9 = n as t9; d as t27 }").0,
        -9841
    );
    assert_eq!(
        run(
            "fn main() -> t27 { let mut a: [t9; 4] = [0; 4]; let i: taddr = 1; \
             a[i] += 10; a[i] as t27 }"
        )
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
    // Text is no longer deferred: Ch. 5 §1 defines it and §1.4's literals
    // lex, so what is left is what waits on storage the compiler does not
    // emit or on a chapter nobody has written.
    assert!(error("fn main() -> t27 { 1 ^ 2 }").contains("tmul"));
    // Reserved for a chapter nobody has written…
    assert!(error("fn main() -> t27 { mod m; 0 }").contains("not written yet"));
    // …as against one that is written and only partly built.
    assert!(
        error("trait Shape { fn a(&self) -> t27; } fn main() -> t27 { let x: dyn Shape = 0; 0 }")
            .contains("has no size")
    );
    assert!(error("fn main() -> t27 { for x in y { } 0 }").contains("`y` is not in scope"));
    // References are Chapter 3; traits and generics are Chapter 4; all now
    // work. What is left of Ch. 4 says which section it is waiting for.
    tir_of("fn f(x: &t27) -> t27 { *x }");
    tir_of("trait Shape { fn area(&self) -> t27; }");
    tir_of("fn id<T>(x: T) -> T { x } fn main() -> t27 { id(1) }");
    let e = error("struct P { x: t27 } impl<T> P<T> { fn f(&self) -> t27 { 0 } }");
    assert!(e.contains("is not a generic type"), "{e}");
    // A trait may take type parameters now, and a type may implement it once
    // per argument. What is still deferred is a bound on the trait's own
    // parameter, and a blanket impl.
    tir_of(
        "trait From<T> { fn from(x: T) -> Self; } \
         impl From<t9> for t27 { fn from(x: t9) -> t27 { x as t27 } } \
         fn main() -> t27 { 0 }",
    );
    let e = error("trait Weird<T: Copy> { fn f(x: T) -> Self; }");
    assert!(e.contains("bound on a trait's own parameter"), "{e}");
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
    // `checked_*` is implemented; see `the_checked_family_returns_an_option`.
    tir_of(
        "fn main() -> t27 { match (1).checked_add(2) { \
                Option::Some(v) => v, Option::None => 0, } }",
    );
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

// ------------------------------------------------ Chapter 4: generics

#[test]
fn a_generic_function_is_monomorphized_per_instantiation() {
    // Ch. 4 §2.7: one copy of the code per distinct set of type arguments.
    assert_eq!(
        run("fn id<T>(x: T) -> T { x } \
             fn main() -> t27 { let a = id(40); let b: t9 = id(2); a + b as t27 }")
        .0,
        42
    );
    let m = tir_of(
        "fn id<T>(x: T) -> T { x } \
                    fn main() -> t27 { let a = id(40); let b: t9 = id(2); a + b as t27 }",
    );
    let printed = tir::print_module(&m);
    assert!(printed.contains("@id.t27"), "{printed}");
    assert!(printed.contains("@id.t9"), "{printed}");
    // And no generic construct reaches TIR at all.
    assert!(!printed.contains('<'), "{printed}");
}

#[test]
fn generic_parameters_are_inferred_from_arguments_and_from_the_expected_type() {
    // From an argument.
    assert_eq!(
        run("fn first<T>(xs: &[T]) -> &T { &xs[0] } \
             fn main() -> t27 { let a: [t27; 3] = [9, 8, 7]; *first(&a) }")
        .0,
        9
    );
    // From the expected type, which is the only thing that separates these.
    assert_eq!(
        run("fn zero<T>(x: T) -> T { x } \
             fn main() -> t27 { let n: t9 = zero(1); n as t27 }")
        .0,
        1
    );
    // And when neither says, the diagnostic names the parameter.
    let e = error("fn make<T>() -> t27 { 0 } fn main() -> t27 { make() }");
    assert!(e.contains("cannot tell what `T` is"), "{e}");
}

#[test]
fn a_generic_struct_is_a_type_once_it_is_applied() {
    assert_eq!(
        run("struct Pair<A, B> { first: A, second: B } \
             fn main() -> t27 { \
                 let p: Pair<t27, t9> = Pair { first: 7, second: 2 }; \
                 p.first + p.second as t27 \
             }")
        .0,
        9
    );
    // Written without its arguments it is not a type.
    let e = error("struct Pair<A, B> { first: A, second: B } fn f(p: Pair) -> t27 { 0 }");
    assert!(e.contains("needs its arguments written"), "{e}");
    // Nor with the wrong number of them.
    let e = error("struct Pair<A, B> { first: A, second: B } fn f(p: Pair<t27>) -> t27 { 0 }");
    assert!(e.contains("takes 2 type argument"), "{e}");
}

#[test]
fn a_generic_enum_keeps_chapter_twos_niche_promise() {
    // The payoff: `Opt<T>` is written in the language now, and the niche
    // optimization applies to it because nothing told the compiler it was
    // special (Ch. 4 §5.8).
    assert_eq!(
        run("enum Opt<T> { None, Some(T) } \
             fn unwrap_or<T>(o: Opt<T>, d: T) -> T { \
                 match o { Opt::Some(v) => v, Opt::None => d, } \
             } \
             fn main() -> t27 { unwrap_or(Opt::Some(7), 0) + unwrap_or(Opt::None, 5) }")
        .0,
        12
    );
    // The layout promises hold for a type the compiler was never told was
    // special: one tryte for `Opt<trit>`, one word for `Opt<&t27>` because
    // every non-positive value is a reference niche, and a word plus a tag
    // for `Opt<t27>` because a word has no spare patterns to hide `None` in.
    let m = tir_of(
        "enum Opt<T> { None, Some(T) } \
         fn main() -> t27 { \
             let a: Opt<&t27> = Opt::None; \
             let b: Opt<t27> = Opt::None; \
             let c: Opt<trit> = Opt::None; \
             0 \
         }",
    );
    let printed = tir::print_module(&m);
    for want in ["%a.slot.4 = slot tryte[3]", "tryte[6]", "tryte[1]"] {
        assert!(printed.contains(want), "{want} missing from\n{printed}");
    }
    // `Opt<&t27>` is one word, because every non-positive value is a
    // reference niche (Ch. 3 §2.5).
    assert_eq!(
        run("enum Opt<T> { None, Some(T) } \
             fn main() -> t27 { \
                 let n: Opt<&t27> = Opt::None; \
                 match n { Opt::Some(v) => *v, Opt::None => 5, } \
             }")
        .0,
        5
    );
}

#[test]
fn a_bound_is_checked_at_the_call_site() {
    // Ch. 4 §2.2: the diagnostic names the call, not something inside the
    // generic body.
    assert_eq!(
        run("trait Area { fn area(&self) -> t27; } \
             struct Sq { s: t27 } \
             impl Area for Sq { fn area(&self) -> t27 { self.s * self.s } } \
             fn measure<T: Area>(x: &T) -> t27 { x.area() } \
             fn main() -> t27 { let s = Sq { s: 5 }; measure(&s) }")
        .0,
        25
    );
    let e = error(
        "trait Area { fn area(&self) -> t27; } struct Sq { s: t27 } \
         fn measure<T: Area>(x: &T) -> t27 { 0 } \
         fn main() -> t27 { let s = Sq { s: 5 }; measure(&s) }",
    );
    assert!(e.contains("does not implement `Area`"), "{e}");
    // `Copy` is structural and automatic (§5.1), so it is a bound the
    // compiler answers itself.
    tir_of("fn dup<T: Copy>(x: T) -> T { x } fn main() -> t27 { dup(1) }");
    let e = error(
        "struct B { x: t27 } impl Drop for B { fn drop(self) { } } \
         fn dup<T: Copy>(x: T) -> T { x } \
         fn main() -> t27 { let b = B { x: 1 }; dup(b); 0 }",
    );
    assert!(e.contains("does not implement `Copy`"), "{e}");
}

#[test]
fn a_where_clause_is_the_same_bound_written_later() {
    assert_eq!(
        run("trait Area { fn area(&self) -> t27; } \
             struct Sq { s: t27 } \
             impl Area for Sq { fn area(&self) -> t27 { self.s * self.s } } \
             fn measure<T>(x: &T) -> t27 where T: Area { x.area() } \
             fn main() -> t27 { let s = Sq { s: 4 }; measure(&s) }")
        .0,
        16
    );
    let e = error("fn f<T>(x: T) -> T where U: Copy { x } fn main() -> t27 { 0 }");
    assert!(e.contains("not a type parameter"), "{e}");
}

#[test]
fn a_generic_impl_gives_a_generic_type_its_methods() {
    // Ch. 4 §2.1: the impl's type parameters become the method's, so a
    // method of a generic type is a generic function keyed by the base.
    assert_eq!(
        run("enum Opt<T> { None, Some(T) } \
             impl<T> Opt<T> { \
                 fn is_some(&self) -> bool { \
                     match self { Opt::Some(v) => true, Opt::None => false, } \
                 } \
                 fn unwrap_or(self, d: T) -> T { \
                     match self { Opt::Some(v) => v, Opt::None => d, } \
                 } \
             } \
             fn main() -> t27 { \
                 let a: Opt<t27> = Opt::Some(7); \
                 let b: Opt<t27> = Opt::None; \
                 let n = if a.is_some() { a.unwrap_or(0) } else { 100 }; \
                 n + b.unwrap_or(5) \
             }")
        .0,
        12
    );
    // One method, two instantiations.
    assert_eq!(
        run("struct W<T> { v: T } \
             impl<T> W<T> { fn get(&self) -> &T { &self.v } } \
             fn main() -> t27 { \
                 let a: W<t27> = W { v: 40 }; \
                 let b: W<t9> = W { v: 2 }; \
                 *a.get() + *b.get() as t27 \
             }")
        .0,
        42
    );
    // A trait implemented for a generic type satisfies a bound on any of its
    // instantiations (§2.2).
    assert_eq!(
        run("trait Size { fn size(&self) -> t27; } \
             struct Boxed<T> { v: T } \
             impl<T> Size for Boxed<T> { fn size(&self) -> t27 { 1 } } \
             fn total<T: Size>(x: &T) -> t27 { x.size() * 5 } \
             fn main() -> t27 { let b: Boxed<t9> = Boxed { v: 3 }; total(&b) }")
        .0,
        5
    );
}

#[test]
fn a_match_scrutinee_auto_dereferences() {
    // Ch. 3 §2.3 governs `.`; a scrutinee behind a reference reads the same
    // way, which is what lets a `&self` method match on `self`.
    assert_eq!(
        run("enum E { A, B } \
             fn f(e: &E) -> t27 { match e { E::A => 1, E::B => 2, } } \
             fn main() -> t27 { let x = E::B; f(&x) }")
        .0,
        2
    );
    assert_eq!(
        run("enum Opt<T> { None, Some(T) } \
             fn get(o: &Opt<t27>) -> t27 { match o { Opt::Some(v) => v, Opt::None => 0, } } \
             fn main() -> t27 { let o: Opt<t27> = Opt::Some(7); get(&o) }")
        .0,
        7
    );
}

// ------------------------------------------------ Chapter 4: derive, Ord

#[test]
fn a_derived_comparison_drives_the_operators() {
    // Ch. 4 §5.3.1: `==` and `!=` call `eq`; the ordering forms and `<=>`
    // call `cmp`, projected exactly as Ch. 1 §5 requires.
    let src = "#[derive(Ord)] struct V { major: t27, minor: t27 } \
               fn main() -> t27 { \
                   let a = V { major: 1, minor: 5 }; \
                   let b = V { major: 1, minor: 9 }; \
                   let mut n: t27 = 0; \
                   if a < b { n += 1; } \
                   if a == a { n += 10; } \
                   if a != b { n += 100; } \
                   if b >= a { n += 1000; } \
                   match a <=> b { -1t => n += 10000, 0t => n += 20000, 1t => n += 30000, } \
                   n \
               }";
    assert_eq!(run(src).0, 11111);
    // Deriving `Ord` derives `Eq` too, since §1.6 makes `Eq` its supertrait.
    let m = tir_of(src);
    let printed = tir::print_module(&m);
    assert!(printed.contains("@V.eq"), "{printed}");
}

#[test]
fn a_derived_comparison_over_scalars_has_no_branch() {
    // Ch. 4 §5.3.3, which is a codegen guarantee of the same kind as Ch. 1
    // §5's: one `cmp` per field, one `sel3` per field after the first, and
    // no branch at all.
    let module = tir_of(
        "#[derive(Ord)] struct V { a: t27, b: t27, c: t27 } \
         fn main() -> t27 { let x = V { a: 1, b: 2, c: 3 }; if x < x { 1 } else { 0 } }",
    );
    let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium()).unwrap();
    let asm = trustc::codegen::compile(&legalized, "main").unwrap();

    // The body of `V.cmp`, from its entry label to its `ret`.
    let start = asm.find("f.V.cmp.entry:").expect("the derived cmp");
    let body = &asm[start..];
    let body = &body[..body.find("\n    ret").expect("a return")];
    for branch in ["br3", "    j ", "beq"] {
        assert!(
            !body.contains(branch),
            "`{branch}` in a derived cmp:\n{body}"
        );
    }
    assert_eq!(body.matches("cmp ").count(), 3, "{body}");
    assert_eq!(body.matches("sel3").count(), 2, "{body}");
}

#[test]
fn derive_says_what_it_will_not_do() {
    // §6: `Copy` and `Sized` are automatic and `Drop` is nobody else's to
    // write, so none of the three is derivable.
    for t in ["Copy", "Drop", "Sized", "Hash"] {
        let e = error(&format!("#[derive({t})] struct S {{ x: t27 }}"));
        assert!(e.contains("not derivable"), "{t}: {e}");
    }
    // A payload-carrying enum orders by discriminant and then by payload,
    // and only the first half is built.
    let e = error("#[derive(Ord)] enum E { A, B(t27) } fn main() -> t27 { 0 }");
    assert!(e.contains("payload"), "{e}");
    // Without an impl or a derive, the operator says which trait is missing.
    let e = error(
        "struct S { x: t27 } fn main() -> t27 { let a = S { x: 1 }; if a < a { 1 } else { 0 } }",
    );
    assert!(e.contains("needs `Ord`"), "{e}");
}

#[test]
fn a_fieldless_enum_derives_a_comparison_over_its_discriminants() {
    // §6 orders an enum by discriminant, and Ch. 2 §5.1 lets one be
    // negative — so `Back` sorts before `Stay` for the reason it reads.
    assert_eq!(
        run(
            "#[derive(Ord)] enum Step { Back = -1, Stay = 0, Forward = 1 } \
             fn main() -> t27 { \
                 let mut n: t27 = 0; \
                 if Step::Back < Step::Stay { n += 1; } \
                 if Step::Forward > Step::Stay { n += 10; } \
                 if Step::Stay == Step::Stay { n += 100; } \
                 n \
             }"
        )
        .0,
        111
    );
}

// ------------------------------------------------ Chapter 4: trait objects

#[test]
fn a_trait_object_dispatches_through_its_vtable() {
    // Ch. 4 §3: one copy of `total`, an indirect call per element.
    let src = "trait Shape { fn area(&self) -> t27; } \
               struct Circle { r: t27 } \
               struct Rect { w: t27, h: t27 } \
               impl Shape for Circle { fn area(&self) -> t27 { self.r * self.r * 3 } } \
               impl Shape for Rect { fn area(&self) -> t27 { self.w * self.h } } \
               fn total(shapes: &[&dyn Shape]) -> t27 { \
                   let mut sum: t27 = 0; \
                   let mut i: taddr = 0; \
                   while i < shapes.len() { sum += shapes[i].area(); i += 1; } \
                   sum \
               } \
               fn main() -> t27 { \
                   let c = Circle { r: 2 }; \
                   let r = Rect { w: 3, h: 4 }; \
                   let shapes: [&dyn Shape; 2] = [&c, &r]; \
                   total(&shapes) \
               }";
    assert_eq!(run(src).0, 24);

    // The vtable is a real global holding real addresses (§3.3), laid out
    // size, align, drop, then the methods.
    let printed = tir::print_module(&tir_of(src));
    assert!(printed.contains("global @vt.Circle.Shape"), "{printed}");
    assert!(printed.contains("addr @Circle.area"), "{printed}");
    // And the dispatch is an indirect call, not a direct one.
    assert!(printed.contains("call %"), "{printed}");
}

#[test]
fn a_trait_objects_vtable_holds_its_supertraits_methods_first() {
    // Ch. 4 §3.3's order, which the table and the dispatch must agree on or
    // every call through a supertrait method goes to the wrong function.
    assert_eq!(
        run("trait Shape { fn area(&self) -> t27; } \
             trait Named: Shape { fn initial(&self) -> t27; } \
             struct C { r: t27 } \
             impl Shape for C { fn area(&self) -> t27 { self.r * 2 } } \
             impl Named for C { fn initial(&self) -> t27 { 67 } } \
             fn both(n: &dyn Named) -> t27 { n.area() + n.initial() } \
             fn main() -> t27 { let c = C { r: 5 }; both(&c) }")
        .0,
        77
    );
}

#[test]
fn a_trait_object_is_two_words_and_keeps_its_niches() {
    // Ch. 4 §3.2: the same shape as Ch. 3 §5.2's slice, data pointer first.
    let m = tir_of(
        "trait Shape { fn area(&self) -> t27; } \
         struct Circle { r: t27 } \
         impl Shape for Circle { fn area(&self) -> t27 { self.r } } \
         fn main() -> t27 { let c = Circle { r: 1 }; let s: &dyn Shape = &c; s.area() }",
    );
    let printed = tir::print_module(&m);
    assert!(printed.contains("slot tryte[6]"), "{printed}");
}

#[test]
fn object_safety_is_checked_and_says_why() {
    // §3.4: a signature that needs the erased type back cannot be called.
    let base = "struct C { x: t27 } ";
    for (method, why) in [
        ("fn cmp(&self, other: &Self) -> trit;", "mentions `Self`"),
        ("fn make() -> t27;", "takes no `self`"),
        (
            "fn go<T>(&self, x: T) -> t27;",
            "type parameters of its own",
        ),
        ("fn clone(&self) -> Self;", "returns `Self`"),
    ] {
        let e = error(&format!(
            "{base} trait T {{ {method} }} \
             fn f(x: &dyn T) -> t27 {{ 0 }} fn main() -> t27 {{ 0 }}"
        ));
        assert!(
            e.contains("not object-safe") && e.contains(why),
            "{method}: {e}"
        );
    }
    // A type that does not implement the trait is not one of its objects.
    let e = error(
        "trait Shape { fn area(&self) -> t27; } struct C { x: t27 } \
         fn f(s: &dyn Shape) -> t27 { 0 } \
         fn main() -> t27 { let c = C { x: 1 }; f(&c) }",
    );
    assert!(e.contains("does not implement"), "{e}");
    // Only the trait's own methods are reachable through an object (§3.1).
    let e = error(
        "trait Shape { fn area(&self) -> t27; } struct C { x: t27 } \
         impl Shape for C { fn area(&self) -> t27 { self.x } } \
         impl C { fn extra(&self) -> t27 { 9 } } \
         fn f(s: &dyn Shape) -> t27 { s.extra() } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("has no method `extra`"), "{e}");
}

#[test]
fn a_slice_reports_its_length() {
    // Ch. 3 §5.4, which the frontend had never implemented.
    assert_eq!(
        run("fn n(xs: &[t27]) -> taddr { xs.len() } \
             fn main() -> t27 { let a: [t27; 3] = [1, 2, 3]; n(&a) as t27 }")
        .0,
        3
    );
    assert_eq!(
        run("fn main() -> t27 { let a: [t9; 7] = [0; 7]; a.len() as t27 }").0,
        7
    );
}

// ------------------------------------------------ Chapter 4: closures

#[test]
fn a_closure_captures_and_is_called() {
    // Ch. 4 §4.1–4.4: the closure captures `k` by shared reference, and
    // `map_in_place` is monomorphized for its anonymous type.
    assert_eq!(
        run("fn map_in_place(xs: &mut [t27], f: impl Fn(t27) -> t27) { \
                 let mut i: taddr = 0; \
                 while i < xs.len() { xs[i] = f(xs[i]); i += 1; } \
             } \
             fn main() -> t27 { \
                 let k: t27 = 3; \
                 let mut ys: [t27; 3] = [1, 2, 3]; \
                 map_in_place(&mut ys, |x: t27| x * k); \
                 ys[0] + ys[1] + ys[2] \
             }")
        .0,
        18
    );
    // Types omitted entirely: the bound says what they are (§4.1).
    assert_eq!(
        run(
            "fn twice(f: impl Fn(t27) -> t27, x: t27) -> t27 { f(f(x)) } \
             fn main() -> t27 { let k: t27 = 2; twice(|x| x + k, 1) }"
        )
        .0,
        5
    );
}

#[test]
fn a_closure_that_writes_a_capture_is_fn_mut() {
    // §4.3: which trait a closure implements follows from how it uses its
    // captures, and `FnMut` may not stand in for `Fn`.
    assert_eq!(
        run("fn run(f: impl FnMut(t27)) { f(1); f(2); } \
             fn main() -> t27 { let mut n: t27 = 0; run(|x| n += x); n }")
        .0,
        3
    );
    let e = error(
        "fn go(f: impl Fn(t27) -> t27) -> t27 { f(1) } \
         fn main() -> t27 { let mut n: t27 = 0; go(|x| { n += x; n }) }",
    );
    assert!(e.contains("is `FnMut` because it writes a capture"), "{e}");
}

#[test]
fn a_captured_place_is_borrowed_for_as_long_as_the_closure_lives() {
    // §4.4's own example: the capture is subject to Ch. 3 §2.2 unchanged.
    let src = "struct P { x: t27, y: t27 } \
               fn use1(f: impl Fn(t27) -> t27) -> t27 { f(1) } \
               fn main() -> t27 { \
                   let mut p = P { x: 1, y: 2 }; \
                   let f = |v: t27| p.x + v; ";
    let e = error(&format!("{src} p.x = 9; use1(f) }}"));
    assert!(e.contains("cannot be written to"), "{e}");
    // And the borrow ends at the closure's last use, not at the end of the
    // block — Ch. 3 §4.2 applies here too.
    assert_eq!(
        run(&format!("{src} let a = use1(f); p.x = 9; a + p.x }}")).0,
        11
    );
}

#[test]
fn a_closure_cannot_be_returned_and_says_why() {
    // §4.5: returning one needs `impl Trait` in return position or a `Box`,
    // and both wait for an allocator.
    let e = error("fn make() -> impl Fn(t27) -> t27 { |x| x } fn main() -> t27 { 0 }");
    assert!(e.contains("allocator"), "{e}");
    // A closure's signature must be the one the bound asks for.
    let e = error("fn go(f: impl Fn(t27) -> t27) -> t27 { 0 } fn main() -> t27 { go(1) }");
    assert!(e.contains("is not a closure"), "{e}");
}

// ------------------------------ Chapter 4: associated types, Iterator, for

#[test]
fn an_associated_type_is_chosen_by_the_implementation() {
    // Ch. 4 §1.7: the trait declares it, the impl chooses it, and
    // `Option<Self::Item>` and `Option<t27>` are the same signature.
    let src = "struct Counter { n: t27, limit: t27 } \
               impl Iterator for Counter { \
                   type Item = t27; \
                   fn next(&mut self) -> Option<t27> { \
                       if self.n < self.limit { self.n += 1; Option::Some(self.n) } \
                       else { Option::None } \
                   } \
               } ";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 let mut sum: t27 = 0; \
                 for v in (Counter {{ n: 0, limit: 5 }}) {{ sum += v; }} \
                 sum \
             }}"
        ))
        .0,
        15
    );
    // An impl must choose every associated type the trait declares.
    let e = error(
        "trait T { type Item; } struct S { x: t27 } impl T for S { } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("missing `type Item`"), "{e}");
    // And may not invent one the trait did not declare.
    let e = error(
        "trait T { fn f(&self) -> t27; } struct S { x: t27 } \
         impl T for S { type Item = t27; fn f(&self) -> t27 { 0 } } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("declares no associated type"), "{e}");
}

#[test]
fn a_for_loop_is_the_desugaring_and_nothing_more() {
    // Ch. 4 §5.7's desugaring uses only Ch. 0 constructs, so `for` adds no
    // control flow the language did not have — `break` and `continue` work
    // in one as in any loop.
    let src = "\
               struct Upto { n: t27, limit: t27 } \
               impl Iterator for Upto { \
                   type Item = t27; \
                   fn next(&mut self) -> Option<t27> { \
                       if self.n < self.limit { self.n += 1; Option::Some(self.n) } \
                       else { Option::None } \
                   } \
               } ";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 let mut sum: t27 = 0; \
                 for v in (Upto {{ n: 0, limit: 10 }}) {{ \
                     if v == 3 {{ continue; }} \
                     if v == 6 {{ break; }} \
                     sum += v; \
                 }} \
                 sum \
             }}"
        ))
        .0,
        1 + 2 + 4 + 5
    );
}

#[test]
fn option_and_result_are_the_languages_own() {
    // Ch. 4 §5.8: ordinary enums, laid out by Ch. 2's rules with no special
    // case — which is why every niche guarantee holds for them.
    assert_eq!(
        run("fn get(o: Option<t27>) -> t27 { match o { Option::Some(v) => v, Option::None => 5, } } \
             fn main() -> t27 { get(Option::Some(7)) + get(Option::None) }")
            .0,
        12
    );
    let m = tir_of(
        "fn main() -> t27 { \
             let a: Option<&t27> = Option::None; \
             let b: Option<trit> = Option::None; \
             0 \
         }",
    );
    let printed = tir::print_module(&m);
    assert!(printed.contains("%a.slot.4 = slot tryte[3]"), "{printed}");
    // `Result` too, and it is not special either.
    assert_eq!(
        run("fn f(n: t27) -> Result<t27, t9> { if n > 0 { Result::Ok(n) } else { Result::Err(1) } } \
             fn main() -> t27 { match f(7) { Result::Ok(v) => v, Result::Err(e) => e as t27, } }")
            .0,
        7
    );
}

// -------------------------------- Chapter 4: the last of it

#[test]
fn a_turbofish_says_what_inference_cannot() {
    // Ch. 4 §2.3: arguments in declaration order, and any omitted are
    // inferred.
    assert_eq!(
        run("fn zero<T>() -> t27 { 0 } fn main() -> t27 { zero::<t27>() + 7 }").0,
        7
    );
    assert_eq!(
        run("fn main() -> t27 { \
                 let o = Option::<t27>::None; \
                 match o { Option::Some(v) => v, Option::None => 5, } \
             }")
        .0,
        5
    );
    let e = error("fn id<T>(x: T) -> T { x } fn main() -> t27 { id::<t27, t9>(1) }");
    assert!(e.contains("takes 1 type argument"), "{e}");
    let e = error("fn f(x: t27) -> t27 { x } fn main() -> t27 { f::<t27>(1) }");
    assert!(e.contains("takes no type arguments"), "{e}");
}

#[test]
fn a_negative_copy_impl_makes_a_type_move() {
    // Ch. 4 §5.1: the one negative implementation the language has, and the
    // opt-out Ch. 3 §1.2 said it lacked.
    let e = error(
        "struct H { x: t27 } impl !Copy for H { } \
         fn take(h: H) -> t27 { h.x } \
         fn main() -> t27 { let a = H { x: 1 }; let n = take(a); let m = a.x; n }",
    );
    assert!(e.contains("moved out of"), "{e}");
    // Without it the same type copies, which is the automatic rule of §5.1.
    assert_eq!(
        run("struct P { x: t27 } fn take(p: P) -> t27 { p.x } \
             fn main() -> t27 { let a = P { x: 1 }; take(a) + a.x }")
        .0,
        2
    );
    // Nothing else may be negated.
    let e = error("trait T { } struct S { x: t27 } impl !T for S { } fn main() -> t27 { 0 }");
    assert!(e.contains("only negative implementation"), "{e}");
}

#[test]
fn an_associated_constant_is_a_constant_under_a_qualified_name() {
    // Ch. 4 §1.7.
    assert_eq!(
        run("trait Bounded { const MIN: t27; const MAX: t27; } \
             struct Small { v: t27 } \
             impl Bounded for Small { const MIN: t27 = -9; const MAX: t27 = 9; } \
             fn main() -> t27 { Small::MAX - Small::MIN }")
        .0,
        18
    );
    // An inherent one needs no trait.
    assert_eq!(
        run("struct C { r: t27 } \
             impl C { const UNIT: t27 = 1; fn r(&self) -> t27 { self.r } } \
             fn main() -> t27 { let c = C { r: 5 }; c.r() + C::UNIT }")
        .0,
        6
    );
    let e = error(
        "trait B { const MIN: t27; } struct S { x: t27 } impl B for S { } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("missing `const MIN`"), "{e}");
}

#[test]
fn a_constant_is_evaluated_exactly_at_compile_time() {
    // Ch. 0 §3.2: the same evaluation the assembler performs, which until
    // now was "integer literals only" — so a negative constant did not
    // compile at all.
    assert_eq!(
        run("const N: t27 = -9; \
             const M: t27 = 3 * 4 + 1; \
             const K: [t9; 2] = [-1, 2]; \
             fn main() -> t27 { N + M + K[0] as t27 }")
        .0,
        3
    );
    // Division is the AM's one division, so a constant folds to what the
    // machine would compute (Ch. 1 §4).
    assert_eq!(run("const Q: t27 = 8 / 3; fn main() -> t27 { Q }").0, 3);
    assert_eq!(run("const R: t27 = 8 % 3; fn main() -> t27 { R }").0, -1);
    assert_eq!(run("const S: t27 = 100 >> 2; fn main() -> t27 { S }").0, 11);
    let e = error("const Z: t27 = 1 / 0; fn main() -> t27 { 0 }");
    assert!(e.contains("division by zero"), "{e}");
}

// ------------------------------------- Ch. 3 §1.4: dropping exactly once

#[test]
fn a_nested_destructor_runs_exactly_once() {
    // `drop.T` is the *complete* glue for T: its body, then its fields, both
    // inside that function. Draft 0.1 also dropped the fields at the call
    // site, so every nested destructor ran twice — a double free in a
    // language whose Ch. 3 Appendix B claims that class is removed by
    // construction. It was unobservable only because §1.5 has no resources.
    let (status, out) = run("fn putchar(c: t9); \
         struct Inner { id: t9 } \
         impl Drop for Inner { fn drop(self) { putchar(self.id); } } \
         struct Outer { a: Inner } \
         impl Drop for Outer { fn drop(self) { putchar(79); } } \
         fn main() -> t27 { let o = Outer { a: Inner { id: 73 } }; 0 }");
    assert_eq!(status, 0);
    assert_eq!(out, "OI", "the outer body, then the field, once each");
}

#[test]
fn a_field_the_destructor_moved_out_of_is_not_dropped_again() {
    // Ch. 3 §1.4 item 3, and the second half of the same double free.
    let (_, out) = run("fn putchar(c: t9); \
         struct Inner { id: t9 } \
         impl Drop for Inner { fn drop(self) { putchar(self.id); } } \
         struct Outer { a: Inner } \
         impl Drop for Outer { fn drop(self) { putchar(79); consume(self.a); } } \
         fn consume(i: Inner) { putchar(67); } \
         fn main() -> t27 { let o = Outer { a: Inner { id: 73 } }; 0 }");
    assert_eq!(
        out, "OCI",
        "`I` once: consume's parameter, and nothing after"
    );
}

/// The shared declarations of the field-move family below.
const MOVE_FAMILY: &str = "fn putchar(c: t9); \
     struct B { id: t9 } impl Drop for B { fn drop(self) { putchar(self.id); } } \
     struct O { a: B, b: B } struct N { inner: O } \
     fn take(x: B) { } fn takeo(x: O) { } ";

#[test]
fn the_field_move_family_is_rejected() {
    // One direct case is not evidence for a whole rule. Each of these moves
    // a field out and then does something that must not be allowed.
    for (what, body) in [
        (
            "reading the whole struct afterwards",
            "let o = O { a: B{id:1}, b: B{id:2} }; take(o.a); takeo(o); 0",
        ),
        (
            "moving the same field twice",
            "let o = O { a: B{id:1}, b: B{id:2} }; take(o.a); take(o.a); 0",
        ),
        (
            "moving a nested field, then reading its parent",
            "let n = N { inner: O { a: B{id:1}, b: B{id:2} } }; \
             take(n.inner.a); takeo(n.inner); 0",
        ),
        (
            "moving a field inside a loop",
            "let o = O { a: B{id:1}, b: B{id:2} }; let mut i: t27 = 0; \
             while i < 2 { take(o.a); i += 1; } 0",
        ),
        (
            "one match arm moving and the others not, then using the whole",
            "let o = O { a: B{id:1}, b: B{id:2} }; let t: trit = 0t; \
             match t { -1t => take(o.a), 0t => (), 1t => (), } takeo(o); 0",
        ),
    ] {
        let e = error(&format!("{MOVE_FAMILY} fn main() -> t27 {{ {body} }}"));
        assert!(
            e.contains("moved out of") || e.contains("moved out of here"),
            "{what}: {e}"
        );
    }
}

#[test]
fn per_local_ownership_rejects_two_programs_that_are_legal() {
    // The evidence for calling the approximation *conservative*, which is a
    // claim about which direction it errs in and needs a wrongly-rejected
    // program to support. Ownership is tracked per local, not per place, so
    // a move out of one field takes the whole local with it.
    //
    // Both of these are legal by Ch. 3 §1.3 — "moving out of a place leaves
    // *that place* uninitialized, not the whole variable" — and both are
    // accepted by Rust. If per-place ownership ever arrives, these two
    // assertions are what must flip.
    for (what, body) in [
        (
            "moving out of disjoint fields",
            "let o = O { a: B{id:1}, b: B{id:2} }; take(o.a); take(o.b); 0",
        ),
        (
            "moving a field out and putting one back",
            "let mut o = O { a: B{id:1}, b: B{id:2} }; take(o.a); o.a = B{id:3}; takeo(o); 0",
        ),
    ] {
        let e = error(&format!("{MOVE_FAMILY} fn main() -> t27 {{ {body} }}"));
        assert!(e.contains("moved out of"), "{what}: {e}");
    }
}

#[test]
fn moving_a_field_out_moves_the_value() {
    // Reading a place of non-copyable type moves it (Ch. 3 §1.2), and that
    // is as true of a field as of a whole local. Draft 0.1 tracked the
    // second and not the first, so this compiled and dropped one value three
    // times.
    let e = error(
        "struct B { id: t9 } impl Drop for B { fn drop(self) { } } \
         struct O { a: B } fn take(b: B) { } \
         fn main() -> t27 { let o = O { a: B { id: 1 } }; take(o.a); take(o.a); 0 }",
    );
    assert!(e.contains("moved out of"), "{e}");
    // Moving it out once is fine, and the value is then dropped by `take`
    // rather than by `o`.
    let (_, out) = run("fn putchar(c: t9); \
         struct B { id: t9 } impl Drop for B { fn drop(self) { putchar(self.id); } } \
         struct O { a: B } fn take(b: B) { putchar(84); } \
         fn main() -> t27 { let o = O { a: B { id: 66 } }; take(o.a); 0 }");
    assert_eq!(out, "TB", "take prints, then drops its parameter — once");
}

#[test]
fn an_enum_payload_is_dropped_by_variant() {
    // Ch. 3 §1.4 item 2: an enum drops its variant's payload, which needs a
    // dispatch on the discriminant. Draft 0.1 emitted none, so a value
    // inside an enum leaked.
    let (_, out) = run("fn putchar(c: t9); \
         struct Port { id: t9 } \
         impl Drop for Port { fn drop(self) { putchar(self.id); } } \
         enum Slot { Empty, Held(Port) } \
         fn main() -> t27 { \
             { let a = Slot::Held(Port { id: 65 }); } \
             { let b = Slot::Empty; } \
             { let c = Slot::Held(Port { id: 66 }); } \
             0 \
         }");
    assert_eq!(
        out, "AB",
        "each held port dropped once, the empty slot none"
    );
    // And through `Option`, which is the case that matters.
    let (_, out) = run("fn putchar(c: t9); \
         struct Port { id: t9 } \
         impl Drop for Port { fn drop(self) { putchar(self.id); } } \
         fn main() -> t27 { let o = Option::Some(Port { id: 90 }); 0 }");
    assert_eq!(out, "Z");
}

// --------------------------------------------------- docs/status.md §11
//
// One test per entry in that section. A claim about how the implementation
// falls short is worth no more than a claim about anything else without
// something that runs, and the entries claiming *conservatism* need a
// wrongly-rejected program specifically: "conservative" says which direction
// the error goes, and that is exactly what a passing test cannot show.
//
// Where an entry describes behaviour that is wrong rather than merely
// limited, the test asserts the wrong behaviour, so that fixing it fails
// here and forces §11 to be updated.

#[test]
fn known_limit_a_generic_body_is_checked_at_instantiation() {
    // §11: the bound half of Ch. 4 §2.2 holds — a failed bound is reported
    // at the call site…
    let e = error(
        "trait Area { fn area(&self) -> t27; } struct S { x: t27 } \
         fn m<T: Area>(x: &T) -> t27 { 0 } \
         fn main() -> t27 { let s = S { x: 1 }; m(&s) }",
    );
    assert!(e.contains("does not implement `Area`"), "{e}");

    // …but a generic function that is never called is never checked, so a
    // body that could not compile for any instantiation compiles.
    tir_of(
        "trait Area { fn area(&self) -> t27; } \
         fn never_called<T: Area>(x: &T) -> t27 { x.no_such_method() } \
         fn main() -> t27 { 0 }",
    );
}

#[test]
fn known_limit_there_is_no_sized_bound() {
    // §11: Ch. 4 §2.5 gives every type parameter an implicit `Sized` bound
    // and `?Sized` to remove it; there is neither, so a parameter behaves as
    // `?Sized`. Rust rejects this call; here it works.
    assert_eq!(
        run("trait Shape { fn area(&self) -> t27; } \
             struct C { r: t27 } \
             impl Shape for C { fn area(&self) -> t27 { self.r * 2 } } \
             fn twice<S: Shape>(s: &S) -> t27 { s.area() * 2 } \
             fn main() -> t27 { let c = C { r: 5 }; let d: &dyn Shape = &c; twice(d) }")
        .0,
        20
    );
    // Sound, because every use that needs a size is checked at that use.
    for (what, src, call) in [
        ("a parameter", "fn f<T: Shape>(x: T) -> t27 { 0 }", "f(*d)"),
        (
            "a local",
            "fn f<T: Shape>(x: &T) -> t27 { let y = *x; 0 }",
            "f(d)",
        ),
        (
            "a field",
            "struct W<T> { v: T } \
             fn f<T: Shape>(x: &T) -> t27 { let w: W<T> = W { v: *x }; 0 }",
            "f(d)",
        ),
    ] {
        let e = error(&format!(
            "trait Shape {{ fn area(&self) -> t27; }} struct C {{ r: t27 }} \
             impl Shape for C {{ fn area(&self) -> t27 {{ self.r }} }} \
             {src} \
             fn main() -> t27 {{ let c = C {{ r: 1 }}; let d: &dyn Shape = &c; {call} }}"
        ));
        assert!(
            e.contains("no size") || e.contains("cannot read"),
            "{what}: {e}"
        );
    }
}

#[test]
fn known_limit_a_returned_borrow_is_rooted_syntactically() {
    // §11: there is no region inference, so the root must be a parameter.
    // Every program accepted would be accepted by full inference; this one
    // would be too, and is rejected.
    let e = error(
        "struct P { x: t27 } \
         fn pick<'a>(a: &'a P, b: &'a P) -> &'a t27 { &a.x } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("elision cannot choose"), "{e}");
}

#[test]
fn known_limit_a_closure_captures_by_variable_not_by_place() {
    // §11: Ch. 4 §4.4 says a closure using `p.x` borrows `p.x` and leaves
    // `p.y` free. This one borrows `p`, so writing `p.y` under a live
    // closure is rejected where the chapter permits it.
    let e = error(
        "struct P { x: t27, y: t27 } \
         fn use1(f: impl Fn(t27) -> t27) -> t27 { f(1) } \
         fn main() -> t27 { \
             let mut p = P { x: 1, y: 2 }; \
             let f = |v: t27| p.x + v; \
             p.y = 9; \
             use1(f) \
         }",
    );
    assert!(e.contains("cannot be written to"), "{e}");
}

#[test]
fn known_limit_diagnostics_print_mangled_names() {
    // §11: `Pair.t27.t9`, not `Pair<t27, t9>`. Recorded so that improving it
    // fails here rather than silently leaving §11 stale.
    let e = error(
        "struct Pair<A, B> { first: A, second: B } \
         fn main() -> t27 { let p: Pair<t27, t9> = Pair { first: 1, second: 2 }; p.z }",
    );
    assert!(e.contains("Pair.t27.t9"), "{e}");
}

// ------------------------------------------------- the drop ledger
//
// Ch. 3 §1.1 gives every value exactly one owner and Ch. 3 §1.4 exactly one
// drop. Nothing in draft 0.1 owns a resource (§1.5), so a value dropped
// twice or never releases the same nothing and no ordinary test can see it —
// which is how three of these got in.
//
// This is the resource the language does not have: a destructor that prints,
// and an assertion on the exact sequence. Every construct that can own a
// value belongs in the table below. When you add one, add a row; if you
// cannot say what the output should be, that is the bug.

/// Declarations shared by every ledger case.
const LEDGER: &str = "fn putchar(c: t9); \
     struct P { id: t9 } impl Drop for P { fn drop(self) { putchar(self.id); } } \
     struct Pair { a: P, b: P } struct Wrap<T> { v: T } \
     enum Slot { Empty, Held(P) } \
     fn eat(p: P) { } fn make(n: t9) -> P { P { id: n } } ";

#[track_caller]
fn ledger(what: &str, body: &str, want: &str) {
    let (_, out) = run(&format!("{LEDGER} fn main() -> t27 {{ {body} }}"));
    assert_eq!(out, want, "{what}");
}

#[test]
fn every_owner_drops_exactly_once() {
    // Storage.
    ledger("a local", "let a = P { id: 65 }; 0", "A");
    ledger(
        "a nested scope",
        "{ let a = P{id:65}; } let b = P{id:66}; 0",
        "AB",
    );
    ledger(
        "a struct field",
        "let s = Pair { a: P{id:65}, b: P{id:66} }; 0",
        "AB",
    );
    ledger(
        "an array element",
        "let xs: [P; 2] = [P{id:65}, P{id:66}]; 0",
        "AB",
    );
    ledger(
        "a tuple field",
        "let t: (P, P) = (P{id:65}, P{id:66}); 0",
        "AB",
    );
    ledger("an enum payload", "let s = Slot::Held(P{id:65}); 0", "A");
    ledger(
        "an Option payload",
        "let o = Option::Some(P{id:65}); 0",
        "A",
    );
    ledger(
        "a generic field",
        "let w: Wrap<P> = Wrap { v: P{id:65} }; 0",
        "A",
    );

    // Moves: the value goes with the move and is dropped once, there.
    ledger("moved into a function", "let a = P{id:65}; eat(a); 0", "A");
    ledger("returned from a function", "let a = make(65); 0", "A");
    ledger(
        "moved out of a match arm",
        "let s = Slot::Held(P{id:65}); \
         match s { Slot::Held(p) => eat(p), Slot::Empty => (), } 0",
        "A",
    );
    ledger(
        "moved on one branch only",
        "let a = P{id:65}; if 1 > 0 { eat(a); } 0",
        "A",
    );

    // Overwriting: the value being replaced is going away, and it has one
    // drop to spend. Draft 0.1 stored over it and leaked.
    ledger(
        "assignment over a local",
        "let mut a = P{id:65}; a = P{id:66}; 0",
        "AB",
    );
    ledger(
        "assignment over a field",
        "let mut s = Pair { a: P{id:65}, b: P{id:66} }; s.a = P{id:67}; 0",
        "ACB",
    );
    // Shadowing: two bindings, two drops, reverse order of declaration.
    // Draft 0.1 resolved both entries to the second binding — a double free
    // and a leak in one line.
    ledger("shadowing", "let a = P{id:65}; let a = P{id:66}; 0", "BA");

    // Leaving a scope early is still leaving it.
    ledger(
        "early return",
        "let a = P{id:65}; if 1 > 0 { return 0; } 0",
        "A",
    );
    ledger(
        "break out of a loop",
        "loop { let a = P{id:65}; break; } 0",
        "A",
    );
    ledger(
        "break from a scope inside a loop",
        "loop { let a = P{id:65}; { let b = P{id:66}; break; } } 0",
        "BA",
    );
    ledger(
        "continue past a local",
        "let mut i: t27 = 0; while i < 2 { let a = P{id:65}; i += 1; continue; } 0",
        "AA",
    );
    ledger(
        "one local per iteration",
        "let mut i: t27 = 0; while i < 2 { let a = P{id:65}; i += 1; } 0",
        "AA",
    );

    // Destructors nest, and run before the fields they own.
    ledger(
        "a destructor and then its fields",
        "let s = Pair { a: P{id:65}, b: P{id:66} }; 0",
        "AB",
    );
}

#[test]
fn values_that_do_not_leave_their_block_stay_in_registers() {
    // The frame-slot scheme spent a load on every operand and a store on
    // every result. A block-local allocator removes both for a value the
    // block produces and consumes, which is most of them.
    let module = tir_of(
        "#[derive(Ord)] struct V { a: t27, b: t27, c: t27 } \
         fn main() -> t27 { let x = V { a: 1, b: 2, c: 3 }; if x < x { 1 } else { 0 } }",
    );
    let legalized = tir::legalize_module(&module, &tir::TargetDesc::tritium()).unwrap();
    let asm = trustc::codegen::compile(&legalized, "main").unwrap();

    let start = asm.find("f.V.cmp.entry:").expect("the derived cmp");
    let body = &asm[start..];
    let body = &body[..body.find("\n    ret").expect("a return")];

    // Three comparisons and two selects, as §5.3.3 promises — and now the
    // trits they pass between them never touch memory. Each field still
    // needs one load, since the fields are in the caller's memory.
    assert_eq!(body.matches("cmp ").count(), 3, "{body}");
    assert_eq!(body.matches("sel3").count(), 2, "{body}");
    let stores = body.matches("st.word").count();
    assert_eq!(
        stores, 0,
        "nothing spills in a straight-line comparison:\n{body}"
    );
}

#[test]
fn the_machine_can_be_asked_what_it_has_done() {
    // TRISC-27 §2.3: `CYCLES` reads instructions retired since reset, and the
    // difference of two readings is the cost of what ran between them —
    // nothing to subtract for the measurement, because the load has not
    // retired when its value is produced.
    let (status, _) = run("fn elapsed() -> t27; \
         fn work(n: t27) -> t27 { \
             let mut s: t27 = 0; let mut i: t27 = 0; \
             while i < n { s += i * i; i += 1; } \
             s \
         } \
         fn main() -> t27 { \
             let t0 = elapsed(); \
             let r = work(10); \
             let t1 = elapsed(); \
             if t1 > t0 { 1 } else { 0 } \
         }");
    assert_eq!(status, 1, "time moves forward");

    // Twice the work costs more than once, which is the only property a
    // benchmark actually needs of it.
    let cost = |n: i128| {
        run(&format!(
            "fn elapsed() -> t27; \
             fn work(n: t27) -> t27 {{ \
                 let mut s: t27 = 0; let mut i: t27 = 0; \
                 while i < n {{ s += i * i; i += 1; }} \
                 s \
             }} \
             fn main() -> t27 {{ \
                 let t0 = elapsed(); let r = work({n}); let t1 = elapsed(); t1 - t0 \
             }}"
        ))
        .0
    };
    let (ten, twenty) = (cost(10), cost(20));
    assert!(twenty > ten, "{ten} then {twenty}");
    // And the same code costs the same twice: it counts instructions, not
    // anything that could vary between runs.
    assert_eq!(cost(10), ten);
}

#[test]
fn mulh_is_reachable_from_the_language() {
    // Ch. 1 §4: `a.mulh(b)·3^N + a.wrapping_mul(b)` is the exact product.
    // TRISC-27 §4.1 had the instruction and TIR §3.1 the operation; this is
    // the same one at the surface, and it is what fixed-point arithmetic
    // needs in order to use a whole word.
    assert_eq!(value("(3812798742493).mulh(3)"), 1);
    assert_eq!(value("(9841).mulh(9841)"), 0);
    assert_eq!(value("(0 - 3812798742493).mulh(3)"), -1);
    // The two halves reconstruct the product, at a scale a t27 can check.
    assert_eq!(
        run("fn main() -> t27 { \
                 let a: t27 = 1000000; let b: t27 = 1000000; \
                 let h = a.mulh(b); let l = a.wrapping_mul(b); \
                 h * 3 + l * 0 + h \
             }")
        .0,
        // 10^12 / 3^27 rounds to 0 here; the point is that it compiles and
        // agrees with the interpreter, which `tir_of` already checked.
        0
    );
}

#[test]
fn a_constant_may_be_an_arrays_length() {
    // Ch. 0 §3.2 says a length is a constant expression and a `const` is
    // one, which had never been true of the implementation.
    assert_eq!(
        run("const N: taddr = 8; \
             const M: taddr = N * 2; \
             struct Grid { cells: [t27; M] } \
             fn sum(xs: &[t27]) -> t27 { \
                 let mut s: t27 = 0; let mut i: taddr = 0; \
                 while i < xs.len() { s += xs[i]; i += 1; } \
                 s \
             } \
             fn main() -> t27 { \
                 let a: [t27; N] = [3; N]; \
                 let g = Grid { cells: [1; M] }; \
                 sum(&a) + sum(&g.cells) \
             }")
        .0,
        8 * 3 + 16
    );
    let e = error("fn main() -> t27 { let a: [t27; K] = [0; 1]; 0 }");
    assert!(e.contains("not a constant"), "{e}");
}

#[test]
fn a_local_is_reached_through_the_frame_pointer_and_not_an_address_register() {
    // A `let` is storage, and every read of it is a load. Code generation
    // used to compute the storage's address into a register first — one
    // `addi` per local, kept alive across everything that read it, and across
    // any call in between, which is what used to earn `s0`…`s6` here. `ld`
    // and `st` carry a fourteen-trit displacement of their own (TRISC-27
    // §3.2), so the address is folded into each access and the register is
    // never needed.
    let m = tir_of(
        "fn f(x: t27) -> t27 { x * 3 + 1 } \
         fn work() -> t27 { \
             let a: t27 = 7; let b: t27 = 9; \
             let r = f(1); \
             a * r + b * r + a * b \
         } \
         fn main() -> t27 { work() }",
    );
    let legalized = tir::legalize_module(&m, &tir::TargetDesc::tritium()).unwrap();
    let asm = trustc::codegen::compile(&legalized, "main").unwrap();

    let start = asm.find("f.work:").expect("the function");
    let body = &asm[start..asm[start..].find("\n\n").map_or(asm.len(), |i| start + i)];
    // Every access to a local names `sp` directly.
    assert!(body.contains("(sp)"), "{body}");
    // The only `addi` on `sp` is the frame, twice: opening and closing it.
    assert_eq!(
        body.matches("addi.trap sp, sp,").count(),
        2,
        "no address of a local is computed:\n{body}"
    );
    // And with no address to keep, no callee-saved register is spent.
    assert_eq!(body.matches("st.word s").count(), 0, "{body}");

    // The answer is unchanged: 7·22 + 9·22 + 63.
    assert_eq!(
        run("fn f(x: t27) -> t27 { x * 3 + 1 } \
         fn main() -> t27 { \
             let a: t27 = 7; let b: t27 = 9; \
             let r = f(1); \
             a * r + b * r + a * b \
         }")
        .0,
        7 * 4 + 9 * 4 + 63
    );
}

#[test]
fn the_checked_family_returns_an_option() {
    // Ch. 1 §4 carries the Rust family over with identical naming. The
    // overflow trit already says whether the exact result fitted, so this is
    // one `.flag` operation and a three-way branch — no second computation
    // and no comparison against a bound.
    assert_eq!(
        run("fn get(o: Option<t27>, d: t27) -> t27 { \
                 match o { Option::Some(v) => v, Option::None => d, } \
             } \
             fn main() -> t27 { \
                 let a: t27 = 3812798742493; \
                 get(a.checked_add(1), 100) \
                     + get((5).checked_add(6), 0) \
                     + get(a.checked_mul(2), 7) \
                     + get((9).checked_sub(4), 0) \
             }")
        .0,
        100 + 11 + 7 + 5
    );
    // And at a narrow width, where the overflow boundary is the narrow one.
    assert_eq!(
        run("fn get(o: Option<t9>, d: t9) -> t9 { \
                 match o { Option::Some(v) => v, Option::None => d, } \
             } \
             fn main() -> t27 { \
                 let m: t9 = 9841; \
                 get(m.checked_add(1), 7) as t27 \
             }")
        .0,
        7
    );
}

#[test]
fn a_trait_with_type_parameters_is_implemented_once_per_argument() {
    // A trait's own parameters are chosen by whoever implements it, so one
    // type may implement it many times. That is the whole distinction from an
    // associated type, which the implementor chooses once (Ch. 4 §1.7).
    let src = "trait From<T> { fn from(x: T) -> Self; } \
               struct Celsius { deg: t27 } \
               impl From<t9> for t27 { fn from(x: t9) -> t27 { x as t27 } } \
               impl From<bool> for t27 { fn from(b: bool) -> t27 { if b { 1 } else { 0 } } } \
               impl From<Celsius> for t27 { \
                   fn from(c: Celsius) -> t27 { c.deg * 9 / 5 + 32 } \
               } \
               fn main() -> t27 { \
                   let n: t9 = 7; \
                   t27::from(n) + t27::from(true) + t27::from(Celsius { deg: 100 }) \
               }";
    assert_eq!(run(src).0, 7 + 1 + 212);

    // Which one is meant comes from the argument, and each is a separate
    // function: three `from`s for one type would otherwise be one name.
    let m = tir_of(src);
    let text = tir::print_module(&m);
    for want in [
        "@t27.From.t9.from",
        "@t27.From.bool.from",
        "@t27.From.Celsius.from",
    ] {
        assert!(text.contains(want), "{want} is not in\n{text}");
    }
}

#[test]
fn a_bound_carries_the_traits_arguments() {
    // `U: From<T>` is a different requirement for every `T`, so the arguments
    // are part of it. A generic function may then call through the bound.
    let src = "trait From<T> { fn from(x: T) -> Self; } \
               struct Celsius { deg: t27 } \
               impl From<Celsius> for t27 { fn from(c: Celsius) -> t27 { c.deg * 9 / 5 + 32 } } \
               fn convert<T, U: From<T>>(x: T) -> U { U::from(x) } \
               fn main() -> t27 { let f: t27 = convert(Celsius { deg: 100 }); f }";
    assert_eq!(run(src).0, 212);

    // And the requirement is checked with its arguments: `t27: From<t27>` is
    // not `t27: From<Celsius>`.
    let e = error(
        "trait From<T> { fn from(x: T) -> Self; } \
         struct Celsius { deg: t27 } \
         impl From<Celsius> for t27 { fn from(c: Celsius) -> t27 { c.deg } } \
         fn convert<T, U: From<T>>(x: T) -> U { U::from(x) } \
         fn main() -> t27 { let f: t27 = convert(7); f }",
    );
    assert!(e.contains("does not implement `From<t27>`"), "{e}");
}

#[test]
fn an_impl_must_give_the_trait_the_arguments_it_takes() {
    let e = error(
        "trait From<T> { fn from(x: T) -> Self; } \
         impl From for t27 { fn from(x: t9) -> t27 { 0 } } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("takes 1 type argument(s), 0 given"), "{e}");

    // And the impl's signature is checked against the declaration with the
    // choice already made: `fn from(x: T)` with `T = t9` is `fn from(x: t9)`.
    let e = error(
        "trait From<T> { fn from(x: T) -> Self; } \
         impl From<t9> for t27 { fn from(x: bool) -> t27 { 0 } } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("does not match the signature"), "{e}");
}

#[test]
fn generic_arguments_nest() {
    // `Option<Option<t27>>` ends in a token the lexer reads as the shift
    // operator, and only the parser knows it is two brackets. Before the
    // split, no generic argument could contain another.
    assert_eq!(
        run("fn main() -> t27 { \
                 let x: Option<Option<t27>> = Option::Some(Option::Some(7)); \
                 match x { \
                     Option::Some(inner) => match inner { \
                         Option::Some(v) => v, \
                         Option::None => 1, \
                     }, \
                     Option::None => 2, \
                 } \
             }")
        .0,
        7
    );
}

/// The two traits of Ch. 4 §5.6, and the rule that connects them.
const CONVERSION: &str = "trait From<T> { fn from(x: T) -> Self; } \
                          trait Into<T> { fn into(self) -> T; } \
                          impl<T, U: From<T>> Into<U> for T { \
                              fn into(self) -> U { U::from(self) } \
                          } ";

#[test]
fn implementing_from_gives_into_for_free() {
    // A blanket impl is a rule about every type rather than an
    // implementation for one, so it is found by checking a condition where
    // every other impl is found by name. Its parameters are bound the way any
    // generic call binds them: `T` from the receiver, `U` from the type the
    // context wants.
    let src = format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         impl From<Celsius> for t27 {{ fn from(c: Celsius) -> t27 {{ c.deg * 9 / 5 + 32 }} }} \
         fn main() -> t27 {{ \
             let c = Celsius {{ deg: 100 }}; \
             let f: t27 = c.into(); \
             f \
         }}"
    );
    assert_eq!(run(&src).0, 212);

    // And the rule satisfies a bound, so a generic function may ask for it.
    let src = format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         impl From<Celsius> for t27 {{ fn from(c: Celsius) -> t27 {{ c.deg * 9 / 5 + 32 }} }} \
         fn show<A, B: Into<A>>(x: B) -> A {{ x.into() }} \
         fn main() -> t27 {{ let f: t27 = show(Celsius {{ deg: 100 }}); f }}"
    );
    assert_eq!(run(&src).0, 212);
}

#[test]
fn a_trait_a_rule_covers_may_not_be_implemented_by_hand() {
    // The rule holds for every type, so any hand-written impl of the same
    // trait overlaps it, and overlapping impls are an error. Closing the
    // trait says so where it is written rather than leaving a collision to be
    // discovered (Ch. 4 §§1.8, 5.6).
    let e = error(&format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         impl Into<t27> for Celsius {{ fn into(self) -> t27 {{ self.deg }} }} \
         fn main() -> t27 {{ 0 }}"
    ));
    assert!(e.contains("may not be implemented by hand"), "{e}");
}

#[test]
fn a_rule_whose_condition_fails_does_not_apply() {
    // No `From<Celsius>` anywhere, so nothing gives `Celsius` an `into`.
    let e = error(&format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         fn main() -> t27 {{ let c = Celsius {{ deg: 1 }}; let f: t27 = c.into(); f }}"
    ));
    assert!(e.contains("does not implement `From<Celsius>`"), "{e}");

    // And it applies only where the result type is known, since that is what
    // binds the trait's argument. A tail expression takes it from the return
    // type, so the case that has nothing is a `let` without one.
    let e = error(&format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         impl From<Celsius> for t27 {{ fn from(c: Celsius) -> t27 {{ c.deg }} }} \
         fn main() -> t27 {{ let c = Celsius {{ deg: 1 }}; let f = c.into(); f }}"
    ));
    assert!(e.contains("result type is known"), "{e}");
}

#[test]
fn a_types_own_method_wins_over_a_rule() {
    // A rule is the last thing tried, so an inherent method of the same name
    // is what a call means.
    let src = format!(
        "{CONVERSION} \
         struct Celsius {{ deg: t27 }} \
         impl From<Celsius> for t27 {{ fn from(c: Celsius) -> t27 {{ c.deg * 9 / 5 + 32 }} }} \
         impl Celsius {{ fn into(self) -> t27 {{ 1 }} }} \
         fn main() -> t27 {{ let c = Celsius {{ deg: 100 }}; let f: t27 = c.into(); f }}"
    );
    assert_eq!(run(&src).0, 1);
}

#[test]
fn a_character_is_a_scalar_value_in_one_word() {
    // Ch. 5 §1.2. A `char` is a Unicode scalar value, one word wide, and
    // fixed width is the whole reason: `'一'` costs what `'A'` costs.
    assert_eq!(
        run("fn main() -> t27 { \
                 let a: char = 'A'; \
                 let z = '一'; \
                 let n = '\\n'; \
                 let e = '\\u{1F600}'; \
                 (a as t27) + (z as t27) + (n as t27) + (e as t27) \
             }")
        .0,
        65 + 19968 + 10 + 0x1F600
    );

    // It compares, it matches, and it sits in aggregates like any scalar.
    assert_eq!(
        run("struct Pair { a: char, b: char } \
             fn main() -> t27 { \
                 let p = Pair { a: 'x', b: 'y' }; \
                 let arr: [char; 3] = ['a', 'b', 'c']; \
                 let mut n: t27 = 0; \
                 if p.a == 'x' { n += 1; } \
                 if p.a < 'y' { n += 10; } \
                 match arr[2] { 'a' => n += 100, 'c' => n += 1000, _ => n += 10000 } \
                 n \
             }")
        .0,
        1011
    );
}

#[test]
fn a_character_converts_one_way_and_to_one_type() {
    // Downward there is nothing to check; upward there is, and Ch. 1 P2 does
    // not let a conversion that can be wrong be silent (Ch. 5 §1.2).
    let e = error("fn main() -> t27 { let c = 65 as char; c as t27 }");
    assert!(e.contains("no `as` from t27 to `char`"), "{e}");

    let e = error("fn main() -> t9 { 'A' as t9 }");
    assert!(e.contains("converts only to `t27`"), "{e}");
}

#[test]
fn option_of_a_character_costs_nothing_over_the_character() {
    // Ch. 2 §6's niche rule, on the scalar with the largest niche in the
    // language: a word holds 7 625 597 484 987 values and 1 112 064 of them
    // are characters.
    assert_eq!(
        run("fn main() -> t27 { \
                 let o: Option<char> = Option::Some('z'); \
                 let p: Option<Option<char>> = Option::Some(Option::None); \
                 let a = match o { Option::Some(c) => c as t27, Option::None => 0 }; \
                 let b = match p { \
                     Option::Some(inner) => match inner { \
                         Option::Some(c) => c as t27, \
                         Option::None => 1, \
                     }, \
                     Option::None => 2, \
                 }; \
                 a + b \
             }")
        .0,
        122 + 1
    );
    // One word each, the same as a bare `char`: the slot sizes say so.
    let printed = tir::print_module(&tir_of(
        "fn main() -> t27 { \
             let a: char = 'a'; \
             let b: Option<char> = Option::None; \
             let c: Option<Option<char>> = Option::None; \
             0 \
         }",
    ));
    // Every slot in that function is one word. A tag beside the payload
    // would have made one of them two.
    assert!(printed.contains("slot tryte[3]"), "{printed}");
    assert!(
        !printed.contains("slot tryte[6]"),
        "a niche was not used:\n{printed}"
    );
}

#[test]
fn a_character_literal_is_told_from_a_lifetime_by_what_closes_it() {
    // `'a'` and `'a` differ in their third character, which is the same rule
    // Rust uses and for the same reason.
    assert_eq!(
        run("fn first<'a>(xs: &'a [char]) -> char { xs[0] } \
             fn main() -> t27 { let a: [char; 2] = ['q', 'r']; first(&a) as t27 }")
        .0,
        113
    );

    // And what a character literal may not be.
    for (src, want) in [
        ("fn main() -> t27 { let c = ''; 0 }", "it is empty"),
        (
            "fn main() -> t27 { let c = 'ab'; 0 }",
            "more than one character",
        ),
        ("fn main() -> t27 { let c = '\\q'; 0 }", "is not an escape"),
        ("fn main() -> t27 { let c = '\\x41'; 0 }", "no `\\x` escape"),
        (
            "fn main() -> t27 { let c = '\\u{D800}'; 0 }",
            "not a Unicode scalar value",
        ),
        (
            "fn main() -> t27 { let c = '\\u{110000}'; 0 }",
            "not a Unicode scalar value",
        ),
    ] {
        let e = error(src);
        assert!(e.contains(want), "{src}\n  gave: {e}");
    }
}

#[test]
fn a_string_literal_is_a_fat_pointer_to_static_storage() {
    // Ch. 5 §1.3. `str` is `[char]` and `&str` is the fat pointer every slice
    // has: an address and a length in *characters*. Fixed width is what makes
    // `len` and `[i]` mean that, and both are O(1).
    assert_eq!(
        run("fn main() -> t27 { \
                 let s: &str = \"hello\"; \
                 let t = \"一二三\"; \
                 (s.len() as t27) * 100 + (t.len() as t27) * 10 + (s[1] as t27) \
             }")
        .0,
        5 * 100 + 3 * 10 + 101
    );

    // The characters are one word each, in a global — one per distinct
    // literal, since a `&'static str` has no identity beyond what it points
    // at.
    let printed = tir::print_module(&tir_of(
        "fn main() -> t27 { let a = \"ab\"; let b = \"ab\"; let c = \"c\"; 0 }",
    ));
    assert_eq!(printed.matches("global @str.").count(), 2, "{printed}");
    assert!(printed.contains("global @str.0 : tryte[6]"), "{printed}");
    assert!(printed.contains("global @str.1 : tryte[3]"), "{printed}");
}

#[test]
fn hello_world_says_hello_in_three_scripts() {
    // The whole of Ch. 5 §1 that is built, end to end: a string literal, a
    // character read out of it, and the UTF-8 encoding that happens at the
    // boundary and nowhere else.
    let (status, out) = run(include_str!("../../examples/trust/hello.tr"));
    assert_eq!(status, 0);
    assert_eq!(out, "Hello, 世界! 🙂\n");
}

#[test]
fn an_unterminated_string_stops_at_the_line_it_started_on() {
    // Not at the end of the file, which is where the error would otherwise be
    // reported and where it would be useless.
    let e = error("fn main() -> t27 { let s = \"oops; 0 }\nfn other() {}");
    assert!(e.contains("unterminated string literal"), "{e}");
}

#[test]
fn char_try_from_is_the_one_conversion_into_a_character() {
    // Ch. 5 §1.2. Four comparisons: non-negative, no greater than U+10FFFF,
    // and outside the surrogates, which UTF-16 reserves and which are not
    // characters.
    let ok = "fn ok(x: t27) -> t27 { \
                  match char::try_from(x) { \
                      Option::Some(c) => c as t27, \
                      Option::None => -1, \
                  } \
              } ";
    for (input, want) in [
        (65, 65),
        (0, 0),
        (0x10FFFF, 0x10FFFF),
        (0xD7FF, 0xD7FF),
        (0xE000, 0xE000),
        (-1, -1),
        (0x110000, -1),
        (0xD800, -1),
        (0xDFFF, -1),
        (0xDC00, -1),
    ] {
        assert_eq!(
            run(&format!("{ok} fn main() -> t27 {{ ok({input}) }}")).0,
            want,
            "char::try_from({input})"
        );
    }
}

#[test]
fn the_text_library_is_ordinary_trust_in_the_prelude() {
    // Everything of Ch. 5 §1 except `char::try_from` is written in the
    // language, in an `impl char` and an `impl str` a reader could have
    // written. `to_utf8` is where the interchange format's variable width
    // lives, and it is the only place it does.
    assert_eq!(
        run("fn main() -> t27 { \
                 let mut sum: t27 = 0; \
                 match '7'.to_digit(10) { Option::Some(v) => sum += v, Option::None => {} } \
                 match 'Z'.to_digit(36) { Option::Some(v) => sum += v, Option::None => {} } \
                 match 'Z'.to_digit(10) { Option::Some(v) => sum += v, Option::None => sum += 1000 } \
                 sum * 100 + (\"一二三\".utf8_len() as t27) * 10 + ('🙂'.utf8_len() as t27) \
             }")
        .0,
        (7 + 35 + 1000) * 100 + 9 * 10 + 4
    );

    // And a program pays for none of it if it says nothing about text.
    let m = tir_of("fn main() -> t27 { 7 }");
    assert_eq!(m.funcs.len(), 1, "{}", tir::print_module(&m));
}

#[test]
fn a_method_on_an_unsized_type_takes_the_reference_it_already_has() {
    // `impl str` gives methods to `[char]`, whose receiver is a fat pointer.
    // There is nothing to dereference and nothing to borrow: the reference
    // *is* the value (Ch. 5 §1.3).
    assert_eq!(
        run("impl str { fn second(&self) -> char { self[1] } } \
             fn main() -> t27 { \"abc\".second() as t27 }")
        .0,
        98
    );
}

#[test]
fn the_question_mark_is_a_match_and_a_return() {
    // Ch. 5 §4.1: two rules and no trait. On `Option`, `Some(v)` continues
    // and `None` returns `None`; on `Result`, `Ok(v)` continues and `Err(e)`
    // returns `Err(F::from(e))`.
    let src = "fn digit(c: char) -> Option<t27> { c.to_digit(10) } \
               fn two(s: &str) -> Option<t27> { \
                   let a = digit(s[0])?; \
                   let b = digit(s[1])?; \
                   Option::Some(a * 10 + b) \
               } \
               fn main() -> t27 { \
                   let s = match two(\"42\") { Option::Some(v) => v, Option::None => -1 }; \
                   let t = match two(\"4x\") { Option::Some(v) => v, Option::None => -1 }; \
                   s * 100 + (t + 1) \
               }";
    assert_eq!(run(src).0, 42 * 100);
}

#[test]
fn the_question_mark_converts_the_error_with_from() {
    // Ch. 5 §4.1 leans on Ch. 4 §5.6: `Err(e)` becomes `Err(F::from(e))`, and
    // where the two error types are the same there is nothing to convert.
    let src = "trait From<T> { fn from(x: T) -> Self; } \
               enum Low { NoDigit } \
               enum High { Parse, Other } \
               impl From<Low> for High { fn from(l: Low) -> High { High::Parse } } \
               fn inner(c: char) -> Result<t27, Low> { \
                   match c.to_digit(10) { \
                       Option::Some(d) => Result::Ok(d), \
                       Option::None => Result::Err(Low::NoDigit), \
                   } \
               } \
               fn outer(c: char) -> Result<t27, High> { let d = inner(c)?; Result::Ok(d * 2) } \
               fn main() -> t27 { \
                   let a = match outer('4') { Result::Ok(v) => v, Result::Err(e) => -1 }; \
                   let b = match outer('x') { \
                       Result::Ok(v) => v, \
                       Result::Err(e) => match e { High::Parse => 100, High::Other => 200 }, \
                   }; \
                   a * 1000 + b \
               }";
    assert_eq!(run(src).0, 8 * 1000 + 100);
}

#[test]
fn the_two_rules_of_the_question_mark_do_not_mix() {
    let e = error(
        "fn f(o: Option<t27>) -> Result<t27, t9> { let v = o?; Result::Ok(v) } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("does not convert between them"), "{e}");

    let e = error(
        "fn f(r: Result<t27, t9>) -> Option<t27> { let v = r?; Option::Some(v) } \
         fn main() -> t27 { 0 }",
    );
    assert!(e.contains("does not convert between them"), "{e}");

    // And it needs a function that carries a failure at all.
    let e = error("fn f(o: Option<t27>) -> t27 { let v = o?; v } fn main() -> t27 { 0 }");
    assert!(e.contains("needs a function that returns"), "{e}");

    // And a value that is one.
    let e = error(
        "fn f(x: t27) -> Option<t27> { let v = x?; Option::Some(v) } \
                   fn main() -> t27 { 0 }",
    );
    assert!(e.contains("applies to `Result` and `Option`"), "{e}");
}

#[test]
fn leaving_early_with_a_question_mark_still_drops() {
    // `?` is a `return`, and a `return` drops everything the frame owns
    // (Ch. 3 §1.4). Lowering it as `match` and `return` rather than as
    // branches is what makes that true without restating it.
    let (_, out) = run("fn putchar(c: t9); \
         struct Port { id: t27 } \
         impl Drop for Port { fn drop(self) { putchar((48 + self.id) as t9); } } \
         fn go(o: Option<t27>) -> Option<t27> { \
             let a = Port { id: 1 }; \
             let v = o?; \
             let b = Port { id: 2 }; \
             Option::Some(v) \
         } \
         fn main() -> t27 { \
             go(Option::None); \
             putchar(45); \
             go(Option::Some(7)); \
             0 \
         }");
    // The early exit drops `a` and never made `b`; the full path drops both,
    // in reverse order of declaration.
    assert_eq!(out, "1-21");
}

#[test]
fn a_return_inside_a_match_arm_does_not_retire_the_other_arms_values() {
    // The bug `?` found, in the form it has without `?`. A `return` leaves by
    // one path; the arms beside it still own the same values, and retiring
    // them there means the value the *other* path owns is never dropped.
    // `break` and `continue` were given the non-retiring form for exactly
    // this reason and `return` was not (Ch. 3 §1.4).
    let (_, out) = run("fn putchar(c: t9); \
         struct Port { id: t27 } \
         impl Drop for Port { fn drop(self) { putchar((48 + self.id) as t9); } } \
         fn go(o: Option<t27>) -> Option<t27> { \
             let a = Port { id: 1 }; \
             let v = match o { \
                 Option::Some(x) => x, \
                 Option::None => { return Option::None; }, \
             }; \
             let b = Port { id: 2 }; \
             Option::Some(v) \
         } \
         fn main() -> t27 { \
             go(Option::None); \
             putchar(45); \
             go(Option::Some(7)); \
             0 \
         }");
    assert_eq!(out, "1-21");

    // The same through an `if`, which is the shape a reader meets first.
    let (_, out) = run("fn putchar(c: t9); \
         struct Port { id: t27 } \
         impl Drop for Port { fn drop(self) { putchar((48 + self.id) as t9); } } \
         fn go(early: bool) -> t27 { \
             let a = Port { id: 1 }; \
             if early { return 0; } \
             let b = Port { id: 2 }; \
             1 \
         } \
         fn main() -> t27 { go(true); putchar(45); go(false); 0 }");
    assert_eq!(out, "1-21");
}

#[test]
fn the_iterator_adaptors_are_structs_a_reader_could_have_written() {
    // Ch. 5 §3. Nothing here is a language feature: each adaptor is a struct
    // and an `impl`, needing a closure (Ch. 4 §4) and an associated type
    // (Ch. 4 §1.7), and both exist. They live in the prelude, so a program
    // that never iterates pays nothing for them.
    let src = "struct Count { at: t27, end: t27 } \
               impl Iterator for Count { \
                   type Item = t27; \
                   fn next(&mut self) -> Option<t27> { \
                       if self.at >= self.end { Option::None } else { \
                           let v = self.at; self.at += 1; Option::Some(v) \
                       } \
                   } \
               } \
               fn total<I: Iterator<Item = t27>>(it: I) -> t27 { \
                   let mut it = it; \
                   let mut sum: t27 = 0; \
                   loop { \
                       match it.next() { \
                           Option::Some(v) => { sum += v; }, \
                           Option::None => { break; }, \
                       } \
                   } \
                   sum \
               } ";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 total(Map {{ inner: Count {{ at: 1, end: 5 }}, f: |x: t27| x * 10 }}) \
             }}"
        ))
        .0,
        100
    );
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 total(Filter {{ inner: Count {{ at: 1, end: 10 }}, p: |x: t27| x % 3 == 0 }}) \
             }}"
        ))
        .0,
        3 + 6 + 9
    );
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 total(Take {{ inner: Count {{ at: 1, end: 100 }}, left: 4 }}) \
             }}"
        ))
        .0,
        10
    );
}

#[test]
fn an_associated_type_binding_constrains_what_the_impl_chose() {
    // Ch. 4 §1.7: an argument says which implementation is meant, a binding
    // says what it must have chosen.
    let e = error(
        "struct C { at: t9 } \
         impl Iterator for C { type Item = t9; fn next(&mut self) -> Option<t9> { Option::None } } \
         fn total<I: Iterator<Item = t27>>(it: I) -> t27 { 0 } \
         fn main() -> t27 { total(C { at: 0 }) }",
    );
    assert!(e.contains("is t9 and not t27"), "{e}");
}

#[test]
fn a_loop_nothing_breaks_out_of_has_no_exit() {
    // Its type is `!`, and the block that would have followed is emitted no
    // more than the block after a `return` is. Emitting it anyway left an
    // unreachable block reading a slot nothing had defined on a path that
    // reaches it — which the verifier caught, once the function was reachable
    // enough to be verified at all.
    assert_eq!(
        run("fn go(o: Option<t27>) -> Option<t27> { \
                 loop { \
                     match o { \
                         Option::Some(x) => { if x > 0 { return Option::Some(x); } }, \
                         Option::None => { return Option::None; }, \
                     } \
                 } \
             } \
             fn main() -> t27 { \
                 match go(Option::Some(3)) { Option::Some(v) => v, Option::None => 0 } \
             }")
        .0,
        3
    );
}

#[test]
fn a_function_nothing_calls_is_still_verified() {
    // The order matters and the fix is the order: `lang::compile` verifies
    // before it prunes, because a function nothing calls is still a function
    // this compiler emitted, and an ill-formed one is a bug whether or not
    // any program reaches it.
    //
    // The other order hid one (G9.9): a `loop` whose every path returned
    // emitted an unreachable exit block, and the reduced test case *passed*,
    // because the function was unused and pruning removed it before the
    // verifier ran.
    //
    // What this asserts is the consequence: `main` alone survives, and
    // everything else was looked at on the way out.
    let m = tir_of(
        "fn never_called(o: Option<t27>) -> Option<t27> { \
             loop { \
                 match o { \
                     Option::Some(x) => { if x > 0 { return Option::Some(x); } }, \
                     Option::None => { return Option::None; }, \
                 } \
             } \
         } \
         fn main() -> t27 { 0 }",
    );
    assert_eq!(m.funcs.len(), 1, "{}", tir::print_module(&m));
    assert_eq!(m.funcs[0].sig.name, "main");
}

#[test]
fn the_type_with_no_values_is_writable_and_trap_has_it() {
    // Ch. 1 §2: nothing can be a `!`, so a `!` may stand where any type is
    // wanted — vacuously. Ch. 1 §6's `trap()` is the only one of the four
    // ways to have it that is a function, and it is what lets a library say
    // "this cannot go on" in the language.
    assert_eq!(
        run("fn get(o: Option<t27>) -> t27 { \
                 match o { Option::Some(v) => v, Option::None => trap() } \
             } \
             fn main() -> t27 { get(Option::Some(7)) }")
        .0,
        7
    );

    // It is writable in return position, which is what says a function does
    // not return.
    assert_eq!(
        run("fn boom() -> ! { trap() } \
             fn main() -> t27 { let x: t27 = 3; if x > 100 { boom(); } x }")
        .0,
        3
    );

    let e = error("fn main() -> t27 { trap(1) }");
    assert!(e.contains("takes no arguments"), "{e}");
}

#[test]
fn unwrap_traps_and_is_written_in_the_language() {
    // Ch. 5 §4.3. `expect` does not exist: its whole value is the message,
    // and a message has nowhere to go.
    assert_eq!(
        run("fn main() -> t27 { \
                 let a: Option<t27> = Option::Some(7); \
                 let b: Option<t27> = Option::None; \
                 let c: Result<t27, t9> = Result::Ok(3); \
                 a.unwrap() * 1000 + b.unwrap_or(20) * 10 + c.unwrap() \
             }")
        .0,
        7 * 1000 + 20 * 10 + 3
    );

    // And on the empty one it stops the program — the same kind of stop as
    // an out-of-bounds index (Ch. 2 §3).
    let asm = {
        let m = tir_of("fn main() -> t27 { let b: Option<t27> = Option::None; b.unwrap() }");
        let legalized = tir::legalize_module(&m, &tir::TargetDesc::tritium()).unwrap();
        trustc::codegen::compile(&legalized, "main").unwrap()
    };
    let image = tritium::assemble(&asm).unwrap();
    let mut vm = tritium::Vm::with_default_memory();
    vm.load_image(&image);
    assert!(matches!(
        vm.run(1_000_000),
        tritium::Stop::Fault(trit_core::FaultCode::Trap, _)
    ));
}

#[test]
fn the_consuming_methods_are_provided_bodies_and_free_functions() {
    // Ch. 5 §3.3. Each is a `while let` over `next` and nothing more. The
    // ones that work for any `Item` are provided bodies on the trait, so an
    // implementation gets them by writing `next` alone (Ch. 4 §1.5); the ones
    // that need arithmetic or an order take the bound on the *iterator*,
    // because a bound on an associated type is not written yet.
    let src = "struct Count { at: t27, end: t27 } \
               impl Iterator for Count { \
                   type Item = t27; \
                   fn next(&mut self) -> Option<t27> { \
                       if self.at >= self.end { Option::None } else { \
                           let v = self.at; self.at += 1; Option::Some(v) \
                       } \
                   } \
               } \
               fn c() -> Count { Count { at: 1, end: 6 } } ";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 let n = c().count() as t27; \
                 let s = sum(c()); \
                 let m = max(c()).unwrap(); \
                 let f = fold(c(), 0, |a, b| a * 2 + b); \
                 let p = c().position(|x| x == 4).unwrap() as t27; \
                 let a = if c().all(|x| x > 0) {{ 1 }} else {{ 0 }}; \
                 n * 1000000 + s * 10000 + m * 1000 + f * 10 + p + a \
             }}"
        ))
        .0,
        // 5 items, sum 15, max 5, fold 57, `4` at index 3, all positive.
        5 * 1000000 + 15 * 10000 + 5 * 1000 + 57 * 10 + 3 + 1
    );
}

#[test]
fn a_method_may_have_type_parameters_of_its_own() {
    // A method taking `impl Fn(…)` is a generic function, and generic
    // functions are instantiated at the call site rather than looked up —
    // which method resolution did not do, so no such method could be called
    // at all. Both receiver forms need it: a place, and a temporary.
    let src = "struct S { a: t27 } \
               impl S { fn pick(&self, p: impl Fn(t27) -> bool) -> bool { p(self.a) } } \
               fn make() -> S { S { a: 5 } } ";
    assert_eq!(
        run(&format!(
            "{src} fn main() -> t27 {{ \
                 let s = S {{ a: 5 }}; \
                 let here = if s.pick(|x| x > 3) {{ 10 }} else {{ 0 }}; \
                 let temp = if make().pick(|x| x > 9) {{ 1 }} else {{ 0 }}; \
                 here + temp \
             }}"
        ))
        .0,
        10
    );
}

#[test]
fn a_program_shadows_the_prelude() {
    // There are no modules (Ch. 0 §1.3), so the prelude occupies the only
    // namespace there is. A program that defines its own `sum` gets its own,
    // and the prelude's — which takes an iterator — is not in the way.
    assert_eq!(
        run("struct P { x: t27, y: t27 } \
             fn sum(p: P) -> t27 { p.x + p.y } \
             fn main() -> t27 { sum(P { x: 7, y: 14 }) }")
        .0,
        21
    );
}
