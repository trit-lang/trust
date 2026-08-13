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
