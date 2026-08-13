//! End-to-end tests: TIR → legalization → TRISC-27 assembly → image → machine.
//!
//! Each one runs the same function two ways — on the TIR interpreter, which
//! executes AM semantics directly, and on the machine, through every pass of
//! the pipeline — and demands the same answer. That is the TIR specification's
//! own correctness criterion applied to the whole toolchain rather than to one
//! pass: a transformation is correct iff it preserves observable AM behavior.

use trit_core::{FaultCode, Tint};
use trustc::codegen;
use trustc::tir::{self, Halt, Interp, TargetDesc, Val};

/// What a run produced: a value, or the fault that stopped it.
type Outcome = Result<i128, FaultCode>;

fn parse(src: &str) -> tir::Module {
    let m = tir::parse_module(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
    let errs = tir::verify(&m);
    assert!(errs.is_empty(), "input does not verify: {errs:?}");
    m
}

/// Run on the TIR interpreter — the oracle.
fn interpret(m: &tir::Module, entry: &str, args: &[i128]) -> Outcome {
    let f = m.function(entry).expect("entry exists");
    let vals: Vec<Val> = f
        .sig
        .params
        .iter()
        .zip(args)
        .map(|((_, t), &a)| {
            Val::Int(Tint::new(t.width().expect("integer parameter"), a).expect("fits"))
        })
        .collect();
    match Interp::new(m).call(entry, &vals) {
        Ok(Some(Val::Int(i))) => Ok(i.to_i128().expect("fits")),
        Ok(other) => panic!("expected an integer result, got {other:?}"),
        Err(Halt::Fault(f)) => Err(f.code),
        Err(other) => panic!("interpreter stopped: {other}"),
    }
}

/// Run through the whole pipeline on the machine.
fn compile_and_run(m: &tir::Module, entry: &str, args: &[i128]) -> Outcome {
    let legalized = tir::legalize_module(m, &TargetDesc::tritium())
        .unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    assert!(
        tir::verify(&legalized).is_empty(),
        "legalized module does not verify"
    );

    let asm = codegen::compile(&legalized, entry)
        .unwrap_or_else(|e| panic!("code generation failed: {e:?}"));
    let image = tritium::assemble(&asm).unwrap_or_else(|e| panic!("assembly failed: {e:?}\n{asm}"));

    let mut vm = tritium::Vm::with_default_memory();
    vm.load_image(&image);
    // `_start` passes a0…a7 straight through to the entry function.
    for (i, &a) in args.iter().enumerate() {
        vm.set_reg(
            tritium::Reg::from_name(&format!("a{i}")).expect("arg register"),
            a,
        );
    }
    match vm.run(50_000_000) {
        tritium::Stop::Halted(v) => Ok(v),
        tritium::Stop::Fault(code, _) => Err(code),
        other => panic!("machine stopped: {other}\n{asm}"),
    }
}

/// The whole point: both ways, same answer.
fn differential(src: &str, entry: &str, cases: &[&[i128]]) {
    let m = parse(src);
    for args in cases {
        let want = interpret(&m, entry, args);
        let got = compile_and_run(&m, entry, args);
        assert_eq!(
            want, got,
            "@{entry}{args:?} differs between interpreter and machine"
        );
    }
}

/// Compile, append the hand-written runtime, assemble and run — returning
/// what the program wrote. There is no linker (TRISC-27 §8), so "linking" is
/// concatenation.
fn compile_link_and_run(src: &str, entry: &str) -> (i128, String) {
    let m = parse(src);
    let legalized = tir::legalize_module(&m, &TargetDesc::tritium())
        .unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    let mut asm = codegen::compile(&legalized, entry)
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

#[test]
fn hello_world_compiles_from_tir_and_prints() {
    // The whole stack, for the smallest interesting program: TIR through
    // legalization, code generation, assembly and the machine, with a
    // hand-written `putchar` standing in for the device access TIR cannot
    // express (§5 has no integer-to-pointer cast).
    let (status, out) = compile_link_and_run(include_str!("../../examples/tir/hello.tir"), "main");
    assert_eq!(status, 0);
    assert_eq!(out, "Hello, world!\n");
}

#[test]
fn the_tir_appendix_example_compiles_and_runs() {
    differential(
        include_str!("../../examples/tir/steps_toward.tir"),
        "steps_toward",
        &[&[0, -5], &[0, 0], &[0, 7], &[100, 100], &[-9841, 9841]],
    );
}

#[test]
fn a_loop_with_block_parameters_compiles_and_runs() {
    differential(
        include_str!("../../examples/tir/factorial.tir"),
        "factorial",
        &[&[0], &[1], &[5], &[9], &[12], &[-1], &[20]],
    );
}

#[test]
fn globals_loads_and_address_arithmetic_compile_and_run() {
    differential(
        include_str!("../../examples/tir/sum_global.tir"),
        "sum_data",
        &[&[]],
    );
}

#[test]
fn the_flag_flavor_survives_the_whole_pipeline() {
    differential(
        include_str!("../../examples/tir/wide_add.tir"),
        "add54_hi",
        &[
            &[3_812_798_742_493, 0, 3_812_798_742_493, 0],
            &[-3_812_798_742_493, 0, -3_812_798_742_493, 0],
            &[1, 4, 2, 5],
        ],
    );
}

#[test]
fn calls_and_recursion_compile_and_run() {
    differential(
        r#"
tir 0.1 target "tritium"
fn @fib(%n: t27) -> t27 {
^entry:
    %c = cmp t27 %n, const t27 2
    br3 %c, ^base, ^rec, ^rec
^base:
    ret %n
^rec:
    %a = sub.trap t27 %n, const t27 1
    %b = sub.trap t27 %n, const t27 2
    %x = call @fib(%a) -> t27
    %y = call @fib(%b) -> t27
    %s = add.trap t27 %x, %y
    ret %s
}
"#,
        "fib",
        &[&[0], &[1], &[7], &[15]],
    );
}

#[test]
fn every_arithmetic_operation_agrees_with_the_interpreter() {
    let src = r#"
tir 0.1 target "tritium"
fn @arith(%a: t27, %b: t27) -> t27 {
^entry:
    %q = div t27 %a, %b
    %r = rem t27 %a, %b
    %m = tmin t27 %q, %r
    %x = tmax t27 %m, %a
    %y = tmul t27 %x, %b
    %z = neg t27 %y
    %w = add.wrap t27 %z, %a
    %v = mul.wrap t27 %w, %b
    %u = sub.wrap t27 %v, %r
    ret %u
}
"#;
    let cases: Vec<Vec<i128>> = [
        (7i128, 2i128),
        (8, 3),
        (-8, 3),
        (9841, -1),
        (100, 7),
        (3_812_798_742_493, 2),
        (0, 5),
    ]
    .iter()
    .map(|(a, b)| vec![*a, *b])
    .collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, "arith", &refs);
}

#[test]
fn faults_arrive_at_the_machine_with_the_same_code() {
    // Division by zero, a trapping overflow, an out-of-range shift and a
    // deliberate trap must all stop the machine the way they stop the
    // interpreter.
    let src = r#"
tir 0.1 target "tritium"
fn @faulty(%which: t27, %a: t27, %b: t27) -> t27 {
^entry:
    %c = cmp t27 %which, const t27 0
    br3 %c, ^divzero, ^overflow, ^shift
^divzero:
    %q = div t27 %a, %b
    ret %q
^overflow:
    %s = add.trap t27 %a, %b
    ret %s
^shift:
    %t = shl.trap t27 %a, %b
    ret %t
}
"#;
    let max = 3_812_798_742_493i128;
    differential(
        src,
        "faulty",
        &[
            &[-1, 5, 0],  // F_DIVZERO
            &[-1, 5, 2],  // fine
            &[0, max, 1], // F_OVERFLOW
            &[0, 1, 1],   // fine
            &[1, 1, 27],  // F_SHIFT
            &[1, 1, 3],   // fine
        ],
    );
}

#[test]
fn narrow_widths_are_legalized_and_still_agree() {
    // t9 and t1 arithmetic have no machine instruction; legalization promotes
    // them to the word and renormalizes with `wrap`, and the answers must not
    // move.
    let src = r#"
tir 0.1 target "tritium"
fn @narrow(%a: t9, %b: t9) -> t9 {
^entry:
    %s = add.wrap t9 %a, %b
    %m = mul.wrap t9 %s, %b
    %c = cmp t9 %m, const t9 0
    %w = widen t1 %c -> t9
    %r = add.wrap t9 %m, %w
    ret %r
}
"#;
    let cases: Vec<Vec<i128>> = [
        (9841i128, 1i128),
        (9841, 9841),
        (-9841, -9841),
        (0, 0),
        (99, 99),
        (1234, -4321),
    ]
    .iter()
    .map(|(a, b)| vec![*a, *b])
    .collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, "narrow", &refs);
}

#[test]
fn stack_slots_and_memory_compile_and_run() {
    let src = r#"
tir 0.1 target "tritium"
fn @spill(%v: t27) -> t27 {
^entry:
    %p = slot tryte[6]
    store t27 %v, %p
    %q = offset %p, const t27 3
    store t27 %v, %q
    %x = load t27 %p
    %y = load t27 %q
    %s = add.wrap t27 %x, %y
    ret %s
}
"#;
    differential(
        src,
        "spill",
        &[&[0], &[1], &[-1], &[1_000_000], &[3_812_798_742_493]],
    );
}
