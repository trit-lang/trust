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
    // The machine side runs TIR §6's whole pipeline; the oracle above runs
    // the module as written. So a canonicalization that changes an answer
    // shows up here, in every one of these tests.
    let m = &tir::canonicalize_module(m);
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
    let m = tir::canonicalize_module(&parse(src));
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

#[test]
fn a_vtable_and_an_indirect_call_agree_on_both_engines() {
    // TIR §1.2 and §3.7, the two extensions dynamic dispatch needs: an
    // initializer that holds the address of a function, and a call through
    // it. The interpreter resolves the address through provenance; the
    // machine resolves it as a relocation and a `jalr`. They must agree.
    differential(
        r#"tir 0.1 target "tritium"

global @vtable : tryte[6] = [addr @twice, addr @thrice]

fn @twice(%x: t27) -> t27 {
^entry:
    %r = mul.trap t27 %x, const t27 2
    ret %r
}

fn @thrice(%x: t27) -> t27 {
^entry:
    %r = mul.trap t27 %x, const t27 3
    ret %r
}

fn @dispatch(%which: t27, %x: t27) -> t27 {
^entry:
    %p = offset @vtable, %which
    %f = load ptr %p
    %r = call %f(%x) -> t27
    ret %r
}
"#,
        "dispatch",
        &[&[0, 7], &[3, 7], &[0, -5], &[3, -5]],
    );
}

/// The assembly a module compiles to, for tests about the shape of the code
/// rather than the answer it gives.
fn compile_asm(src: &str, entry: &str) -> String {
    let m = tir::canonicalize_module(&parse(src));
    let legalized = tir::legalize_module(&m, &TargetDesc::tritium())
        .unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    codegen::compile(&legalized, entry).unwrap_or_else(|e| panic!("code generation failed: {e:?}"))
}

/// The body of a function, without the surrounding module.
fn body_of<'a>(asm: &'a str, name: &str) -> &'a str {
    let start = asm
        .find(&format!("\nf.{name}:"))
        .unwrap_or_else(|| panic!("`f.{name}` is not in\n{asm}"));
    let rest = &asm[start + 1..];
    match rest[1..].find("\n\n") {
        Some(end) => &rest[..end + 1],
        None => rest,
    }
}

#[test]
fn a_constant_operand_goes_in_the_immediate_field() {
    // Every operation but `wrap` has an immediate form (TRISC-27 §4.1), and
    // the field reaches ±2 391 484. A constant that fits belongs there: the
    // alternative is `li` into a scratch register, an instruction spent on a
    // number the encoding had room for. Measured on `examples/trust/HPL.tr`,
    // this fold was 10% of everything the program executed.
    let src = r#"tir 0.1 target "tritium"

fn @plain(%x: t27) -> t27 {
^entry:
    %r = add.trap t27 %x, const t27 5
    ret %r
}

fn @onleft(%x: t27) -> t27 {
^entry:
    %r = mul.trap t27 const t27 3, %x
    ret %r
}

fn @notcommutative(%x: t27) -> t27 {
^entry:
    %r = sub.trap t27 const t27 3, %x
    ret %r
}

fn @toobig(%x: t27) -> t27 {
^entry:
    %r = add.trap t27 %x, const t27 43046721
    ret %r
}

fn @flagged(%x: t27) -> t27 {
^entry:
    %r, %o = add.flag t27 %x, const t27 5
    ret %r
}
"#;
    let asm = compile_asm(src, "plain");

    let plain = body_of(&asm, "plain");
    assert!(plain.contains("addi.trap"), "{plain}");
    assert!(
        !plain.contains("li "),
        "a constant was materialized:\n{plain}"
    );

    // Addition and multiplication commute, so a constant on the left reaches
    // the immediate field just as readily.
    let onleft = body_of(&asm, "onleft");
    assert!(onleft.contains("muli.trap"), "{onleft}");
    assert!(!onleft.contains("li "), "{onleft}");

    // Subtraction does not, and the constant stays in a register.
    let sub = body_of(&asm, "notcommutative");
    assert!(sub.contains("li "), "{sub}");
    assert!(sub.contains("sub.trap"), "{sub}");

    // 3¹⁶ does not fit fourteen trits, so it goes back through `li`.
    // (`addi.trap sp, sp, …` is the prologue and says nothing either way.)
    let big = body_of(&asm, "toobig");
    assert!(big.contains("li "), "{big}");
    assert!(big.contains("add.trap"), "{big}");

    // `alui` has no field for the flag flavor's second destination, so the
    // flag form keeps the register form whatever its operands are.
    let flagged = body_of(&asm, "flagged");
    assert!(flagged.contains("add.flag"), "{flagged}");
    assert!(flagged.contains("li "), "{flagged}");

    // And every one of them still computes what the interpreter computes.
    for entry in ["plain", "onleft", "notcommutative", "toobig", "flagged"] {
        differential(src, entry, &[&[0], &[1], &[-1], &[7], &[-3_812_798]]);
    }
}

#[test]
fn a_branch_names_the_block_it_can_reach_and_a_stub_otherwise() {
    // `br3` carries three displacements of seven trits each — ±1093 words
    // (TRISC-27 §3.2) — so it can usually name its targets itself. It used to
    // name three adjacent stubs instead, each holding one jump, which cost a
    // jump on every branch the program took. On examples/trust/HPL.tr that
    // was 6% of everything executed.
    let near = r#"tir 0.1 target "tritium"

fn @near(%x: t27) -> t27 {
^entry:
    %c = cmp t27 %x, const t27 0
    br3 %c, ^neg, ^zero, ^pos
^neg:
    ret const t27 -1
^zero:
    ret const t27 0
^pos:
    ret const t27 1
}
"#;
    let asm = compile_asm(near, "near");
    let branch = asm
        .lines()
        .find(|l| l.trim_start().starts_with("br3"))
        .expect("a branch");
    assert!(branch.contains("f.near.neg"), "{branch}");
    assert!(branch.contains("f.near.zero"), "{branch}");
    assert!(branch.contains("f.near.pos"), "{branch}");
    differential(near, "near", &[&[-7], &[0], &[7]]);

    // A target with block arguments keeps its stub, because binding the
    // parameters is code and the edge is the only place it belongs.
    let args = r#"tir 0.1 target "tritium"

fn @withargs(%x: t27) -> t27 {
^entry:
    %c = cmp t27 %x, const t27 0
    br3 %c, ^join(const t27 -1), ^join(const t27 0), ^join(const t27 1)
^join(%s: t27):
    %r = mul.wrap t27 %s, %x
    ret %r
}
"#;
    let asm = compile_asm(args, "withargs");
    let branch = asm
        .lines()
        .find(|l| l.trim_start().starts_with("br3"))
        .expect("a branch");
    assert!(
        !branch.contains("f.withargs.join"),
        "an edge with arguments needs its stub: {branch}"
    );
    differential(args, "withargs", &[&[-7], &[0], &[7]]);

    // And a target genuinely out of reach falls back to a stub too. `^far`
    // sits beyond a block long enough that no plausible error in the word
    // count this pass keeps could bring it inside ±1093 words.
    let mut pad = String::new();
    for i in 0..1500 {
        let prev = if i == 0 {
            "%x".to_string()
        } else {
            format!("%v{}", i - 1)
        };
        pad.push_str(&format!("    %v{i} = add.wrap t27 {prev}, const t27 1\n"));
    }
    let far = format!(
        r#"tir 0.1 target "tritium"

fn @far(%x: t27) -> t27 {{
^entry:
    %c = cmp t27 %x, const t27 0
    br3 %c, ^far, ^near, ^near
^near:
{pad}    ret %v1499
^far:
    ret const t27 42
}}
"#
    );
    let asm = compile_asm(&far, "far");
    let branch = asm
        .lines()
        .find(|l| l.trim_start().starts_with("br3"))
        .expect("a branch");
    assert!(
        branch.contains("f.far.near"),
        "the near arm reaches: {branch}"
    );
    assert!(
        !branch.contains("f.far.far"),
        "the far arm cannot reach: {branch}"
    );
    // The assembler is the judge of whether the estimate was right: a `br3`
    // that does not reach is an error there, not a slower program here.
    differential(&far, "far", &[&[-1], &[0], &[1]]);
}

#[test]
fn a_value_read_more_than_once_across_a_call_earns_a_saved_register() {
    // TRISC-27 §6.1: `s0`…`s6` survive a call, at the cost of a save in the
    // prologue and a restore in the epilogue — once per invocation, while a
    // spill costs once per use. So the allocator takes one only for a value
    // used more than once, which was measured rather than assumed (G8.3):
    // handing one to every call-crossing value made a benchmark 6% slower.
    //
    // The rule belongs to code generation, so it is stated in code
    // generation's own input language. From Trust source a local is storage,
    // and its reads are loads that carry their own displacement.
    let src = r#"tir 0.1 target "tritium"

fn @twice(%x: t27) -> t27 {
^entry:
    %r = mul.wrap t27 %x, const t27 2
    ret %r
}

fn @thrice(%a: t27) -> t27 {
^entry:
    %b = call @twice(%a) -> t27
    %c = call @twice(%b) -> t27
    %d = mul.wrap t27 %b, %c
    %e = add.wrap t27 %d, %b
    ret %e
}

fn @once(%a: t27) -> t27 {
^entry:
    %b = call @twice(%a) -> t27
    %c = call @twice(%b) -> t27
    ret %c
}
"#;
    let asm = compile_asm(src, "thrice");

    // `%b` is read three times, and a call falls between its definition and
    // its last read.
    let thrice = body_of(&asm, "thrice");
    let saved = thrice.matches("st.word s").count();
    assert!(
        saved > 0,
        "a value read three times across a call:\n{thrice}"
    );
    assert_eq!(
        saved,
        thrice.matches("ld.word s").count(),
        "every save has its restore:\n{thrice}"
    );

    // Nothing here is read twice: `%a` reaches one call and `%b` the next.
    // Neither is worth a save and a restore, which cost more than the one
    // spill they would replace.
    let once = body_of(&asm, "once");
    assert_eq!(
        once.matches("st.word s").count(),
        0,
        "a value read once does not earn one:\n{once}"
    );

    for entry in ["thrice", "once"] {
        differential(src, entry, &[&[0], &[1], &[-5], &[1000]]);
    }
}

#[test]
fn an_access_carries_its_own_displacement() {
    // `ld` and `st` have a fourteen-trit displacement field (TRISC-27 §3.2)
    // and code generation was leaving it at zero, emitting an `add` for every
    // address the encoding already had room for. Two shapes fold: `slot`,
    // whose value is a fixed displacement from `sp`, and `offset` by a
    // constant.
    let src = r#"tir 0.1 target "tritium"

fn @frame(%x: t27) -> t27 {
^entry:
    %p = slot tryte[9]
    store t27 %x, %p
    %q = offset %p, const t27 3
    store t27 const t27 5, %q
    %a = load t27 %p
    %b = load t27 %q
    %r = add.wrap t27 %a, %b
    ret %r
}

fn @escapes(%x: t27) -> t27 {
^entry:
    %p = slot tryte[9]
    store t27 %x, %p
    %r = call @reads(%p) -> t27
    ret %r
}

fn @reads(%p: ptr) -> t27 {
^entry:
    %v = load t27 %p
    ret %v
}
"#;
    let asm = compile_asm(src, "frame");

    // Nothing computes an address: every access names `sp` and its own
    // displacement, and the frame is opened and closed once each.
    let frame = body_of(&asm, "frame");
    assert_eq!(
        frame.matches("addi.trap sp, sp,").count(),
        2,
        "an address was computed:\n{frame}"
    );
    assert!(!frame.contains(", 0(sp)"), "a zero displacement:\n{frame}");

    // An address that escapes the block — here into a call — still has to be
    // computed, because something other than an access needs it.
    let escapes = body_of(&asm, "escapes");
    assert!(
        escapes
            .lines()
            .any(|l| l.trim_start().starts_with("addi.trap") && !l.contains("addi.trap sp,")),
        "an address a call takes must exist:\n{escapes}"
    );

    for entry in ["frame", "escapes"] {
        differential(src, entry, &[&[0], &[7], &[-7], &[1_000_000]]);
    }
}

#[test]
fn a_leaf_function_keeps_its_parameters_where_they_arrived() {
    // Parameters arrive in `a0`…`a7` (TRISC-27 §6.1) and were being stored
    // to frame slots by the prologue and loaded back by every use. In the
    // entry block of a function that makes no call, nothing can clobber
    // them, so they stay. And with every value in a register there is no
    // frame to open: `@leaf` compiles to its arithmetic and a `ret`.
    let src = r#"tir 0.1 target "tritium"

fn @leaf(%a: t27, %b: t27) -> t27 {
^entry:
    %p = mul.wrap t27 %a, %b
    %q = add.wrap t27 %p, %a
    ret %q
}

fn @calls(%a: t27) -> t27 {
^entry:
    %r = call @leaf(%a, %a) -> t27
    %s = add.wrap t27 %r, %a
    ret %s
}
"#;
    let asm = compile_asm(src, "calls");

    let leaf = body_of(&asm, "leaf");
    assert!(!leaf.contains("sp"), "no frame and no spill:\n{leaf}");
    assert!(leaf.contains("a0") && leaf.contains("a1"), "{leaf}");

    // A block that calls cannot keep them there — setting up the call's own
    // arguments is what overwrites them.
    let calls = body_of(&asm, "calls");
    assert!(
        calls.contains("(sp)"),
        "a parameter live across a call needs somewhere else to be:\n{calls}"
    );

    for entry in ["leaf", "calls"] {
        differential(src, entry, &[&[0, 0], &[3, 5], &[-7, 11], &[9841, -3]]);
    }
}

#[test]
fn a_parameter_live_across_the_first_call_is_not_left_where_a_call_clobbers_it() {
    // A parameter is live from before the entry block's first instruction.
    // When that instruction is a call, the parameter is live *across* it, and
    // an interval that began at the call rather than before it did not say
    // so — the allocator handed `%b` a caller-saved register, and
    // `examples/trust/HPL.tr` printed a table with no padding in it.
    //
    // This is what `Positions` gives a block's entry a number of its own for.
    let src = r#"tir 0.1 target "tritium"

fn @clobber(%x: t27) -> t27 {
^entry:
    %a = mul.wrap t27 %x, const t27 3
    %b = add.wrap t27 %a, const t27 7
    %c = mul.wrap t27 %b, const t27 5
    %d = add.wrap t27 %c, %a
    %e = sub.wrap t27 %d, %b
    %f = add.wrap t27 %e, %c
    ret %f
}

fn @f(%a: t27, %b: t27) -> t27 {
^entry:
    %r = call @clobber(%a) -> t27
    %s = add.wrap t27 %b, %r
    ret %s
}
"#;
    let asm = compile_asm(src, "f");
    let f = body_of(&asm, "f");
    // `%b` arrives in a1. Whatever happens to it, it may not be moved into a
    // register the call is entitled to destroy (TRISC-27 §6.1).
    for r in [
        "t4", "t5", "t6", "t7", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7",
    ] {
        assert!(
            !f.contains(&format!("add.wrap {r}, a1, zero")),
            "`%b` is live across the call and {r} does not survive one:\n{f}"
        );
    }
    differential(src, "f", &[&[0, 0], &[1, 100], &[-3, 9], &[7, -7]]);
}

#[test]
fn a_loop_counter_stays_in_a_register_across_the_back_edge() {
    // What cross-block allocation is for. The counter is defined in one
    // block, tested in another and incremented in a third; nothing here is
    // block-local, so the old allocator left every one of them in memory —
    // two accesses per iteration for the counter alone.
    let src = r#"tir 0.1 target "tritium"

fn @sum(%n: t27) -> t27 {
^entry:
    br ^head(const t27 0, const t27 0)
^head(%i: t27, %acc: t27):
    %c = cmp t27 %i, %n
    br3 %c, ^body, ^done(%acc), ^done(%acc)
^body:
    %i2 = add.wrap t27 %i, const t27 1
    %a2 = add.wrap t27 %acc, %i
    br ^head(%i2, %a2)
^done(%r: t27):
    ret %r
}

fn @count(%n: t27) -> t27 {
^entry:
    %z = cmp t27 %n, const t27 0
    br3 %z, ^done, ^done, ^loop
^loop:
    %v = load t27 @cell
    %w = add.wrap t27 %v, const t27 1
    store t27 %w, @cell
    %d = sub.wrap t27 %n, const t27 1
    store t27 %d, @cell
    br ^done
^done:
    %o = load t27 @cell
    ret %o
}

global @cell : tryte[3] = [0, 0, 0]
"#;
    // Block parameters keep their transfer slots — agreeing on a register for
    // a value with a different definition on each incoming edge is the
    // parallel-copy problem, and the frontend emits none of them.
    differential(src, "sum", &[&[0], &[1], &[5], &[100]]);
    differential(src, "count", &[&[0], &[3]]);
}

#[test]
fn the_register_a_long_interval_holds_can_be_taken_from_it() {
    // More values live at once than there are registers. The easy answer is
    // to spill whichever interval arrives when the pool is empty; the better
    // one is to take the register from whichever interval runs *longest*,
    // because a long interval holds its register over more instructions and
    // saves fewer of them per instruction held.
    //
    // Twenty-six values are defined at the top and read once each at the bottom.
    // Between them, `%hot` is defined and read eight times in a row: it can
    // only be in a register if something gives one up.
    let mut body = String::new();
    for i in 0..26 {
        body.push_str(&format!(
            "    %long{i} = mul.wrap t27 %x, const t27 {}\n",
            i + 2
        ));
    }
    body.push_str("    %hot = add.wrap t27 %x, const t27 1\n");
    let mut acc = String::from("%hot");
    for i in 0..8 {
        body.push_str(&format!("    %h{i} = mul.wrap t27 {acc}, %hot\n"));
        acc = format!("%h{i}");
    }
    for i in 0..26 {
        body.push_str(&format!("    %acc{i} = add.wrap t27 {acc}, %long{i}\n"));
        acc = format!("%acc{i}");
    }
    let src = format!(
        "tir 0.1 target \"tritium\"\n\nfn @pressure(%x: t27) -> t27 {{\n^entry:\n{body}    ret {acc}\n}}\n"
    );

    let asm = compile_asm(&src, "pressure");
    let f = body_of(&asm, "pressure");
    let traffic = f.matches("(sp)").count();
    // 48 without the eviction rule, because `%hot` arrives to find nothing
    // free and pays a load on each of its eight reads; 34 with it. The
    // assertion is an upper bound, so an allocator that does better still
    // passes.
    assert!(
        traffic <= 36,
        "{traffic} frame accesses under register pressure:\n{f}"
    );
    differential(&src, "pressure", &[&[0], &[1], &[-3], &[100]]);
}

#[test]
fn a_swap_on_an_edge_is_a_swap_and_not_two_overwrites() {
    // Block parameters are in registers now, so binding them on an edge is a
    // parallel copy: every argument is read as it was before any parameter
    // was written. A back edge that exchanges two values is the case that
    // makes the difference visible — done in the obvious order it produces
    // two copies of one value.
    //
    // `@fib` swaps on every iteration; `@rotate` cycles three, which needs
    // the scratch to be freed and taken again.
    let src = r#"tir 0.1 target "tritium"

fn @fib(%n: t27) -> t27 {
^entry:
    br ^head(%n, const t27 0, const t27 1)
^head(%i: t27, %a: t27, %b: t27):
    %c = cmp t27 %i, const t27 0
    br3 %c, ^done(%a), ^done(%a), ^step(%i, %a, %b)
^step(%si: t27, %sa: t27, %sb: t27):
    %sum = add.wrap t27 %sa, %sb
    %i2 = sub.wrap t27 %si, const t27 1
    br ^head(%i2, %sb, %sum)
^done(%r: t27):
    ret %r
}

fn @rotate(%n: t27) -> t27 {
^entry:
    br ^head(%n, const t27 1, const t27 2, const t27 3)
^head(%i: t27, %x: t27, %y: t27, %z: t27):
    %c = cmp t27 %i, const t27 0
    br3 %c, ^done(%x), ^done(%x), ^step(%i, %x, %y, %z)
^step(%si: t27, %sx: t27, %sy: t27, %sz: t27):
    %i2 = sub.wrap t27 %si, const t27 1
    br ^head(%i2, %sy, %sz, %sx)
^done(%r: t27):
    ret %r
}
"#;
    // 0 1 1 2 3 5 8 13 21 34 55 …
    differential(src, "fib", &[&[0], &[1], &[2], &[10], &[20], &[-1]]);
    // 1 2 3 1 2 3 …
    differential(src, "rotate", &[&[0], &[1], &[2], &[3], &[4], &[7]]);
}
