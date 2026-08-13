//! TIR tests: textual round-tripping, verification, and interpretation.

use trit_core::{Bt, FaultCode, Tint};
use trustc::tir::{self, Halt, Interp, Val, verify::VerifyError};

fn parse(src: &str) -> tir::Module {
    tir::parse_module(src).unwrap_or_else(|e| panic!("parse failed: {e}\n{src}"))
}

fn checked(src: &str) -> tir::Module {
    let m = parse(src);
    let errs = tir::verify(&m);
    assert!(errs.is_empty(), "unexpected verification errors: {errs:?}");
    m
}

fn errors(src: &str) -> Vec<VerifyError> {
    tir::verify(&parse(src))
}

fn run(src: &str, entry: &str, args: &[i128]) -> Result<Option<Val>, Halt> {
    let m = checked(src);
    let widths: Vec<u32> = m
        .function(entry)
        .expect("entry exists")
        .sig
        .params
        .iter()
        .map(|(_, t)| t.width().expect("integer parameter"))
        .collect();
    let vals: Vec<Val> = args
        .iter()
        .zip(widths)
        .map(|(&a, w)| Val::Int(Tint::new(w, a).expect("argument in range")))
        .collect();
    Interp::new(&m).call(entry, &vals)
}

fn run_int(src: &str, entry: &str, args: &[i128]) -> i128 {
    match run(src, entry, args) {
        Ok(Some(Val::Int(i))) => i.to_i128().expect("result fits"),
        other => panic!("expected an integer result, got {other:?}"),
    }
}

// ------------------------------------------------------------------ examples

const EXAMPLES: &[(&str, &str)] = &[
    (
        "steps_toward",
        include_str!("../../examples/tir/steps_toward.tir"),
    ),
    (
        "factorial",
        include_str!("../../examples/tir/factorial.tir"),
    ),
    (
        "sum_global",
        include_str!("../../examples/tir/sum_global.tir"),
    ),
    ("wide_add", include_str!("../../examples/tir/wide_add.tir")),
];

#[test]
fn examples_verify_and_round_trip() {
    for (name, src) in EXAMPLES {
        let m = checked(src);
        let printed = tir::print_module(&m);
        let reparsed = parse(&printed);
        assert_eq!(m, reparsed, "`{name}` did not survive a print/parse cycle");
        assert_eq!(
            printed,
            tir::print_module(&reparsed),
            "`{name}` printing is not idempotent"
        );
    }
}

#[test]
fn spec_appendix_example_behaves_three_way() {
    // The whole point of the example: one comparison, three outcomes.
    let src = EXAMPLES[0].1;
    assert_eq!(run_int(src, "steps_toward", &[0, -5]), 1);
    assert_eq!(run_int(src, "steps_toward", &[0, 0]), 0);
    assert_eq!(run_int(src, "steps_toward", &[0, 7]), -1);
}

#[test]
fn factorial_loops_and_traps() {
    let src = EXAMPLES[1].1;
    assert_eq!(run_int(src, "factorial", &[0]), 1);
    assert_eq!(run_int(src, "factorial", &[5]), 120);
    assert_eq!(run_int(src, "factorial", &[9]), 362_880);
    // 20! does not fit in t27, and `.trap` makes that a fault, not a lie.
    assert_eq!(
        run(src, "factorial", &[20]),
        Err(Halt::Fault(trit_core::Fault::new(FaultCode::Overflow)))
    );
    // A deliberate `trap` for the negative case.
    assert_eq!(
        run(src, "factorial", &[-1]),
        Err(Halt::Fault(trit_core::Fault::new(FaultCode::Trap)))
    );
}

#[test]
fn globals_loads_and_offsets() {
    assert_eq!(run_int(EXAMPLES[2].1, "sum_data", &[]), 45);
}

#[test]
fn flag_flavor_carries_between_parts() {
    // MAX + MAX in the low part carries +1 into the high part (TIR §6).
    let src = EXAMPLES[3].1;
    let max = 3_812_798_742_493i128;
    assert_eq!(run_int(src, "add54_hi", &[max, 0, max, 0]), 1);
    assert_eq!(run_int(src, "add54_lo", &[max, max]), -1);
    assert_eq!(run_int(src, "add54_hi", &[-max, 0, -max, 0]), -1);
    assert_eq!(run_int(src, "add54_hi", &[1, 4, 2, 5]), 9);
}

// ----------------------------------------------------------------- the format

#[test]
fn version_mismatch_is_rejected_outright() {
    // TIR §8: the version stamp is a compatibility check, not a promise.
    let e = tir::parse_module("tir 0.2 target \"tritium\"").unwrap_err();
    assert!(e.message.contains("0.2"), "{e}");
}

#[test]
fn comments_and_all_three_radices_parse() {
    let m = checked(
        r#"
tir 0.1 target "tritium"      ; a comment
fn @f() -> t27 {
^entry:
    %a = add.wrap t27 const t27 0t1T0, const t27 0hDDDDDDDDE  ; 6 + 1
    %b = add.wrap t27 %a, const t27 -3_000
    ret %b
}
"#,
    );
    let mut interp = Interp::new(&m);
    assert_eq!(
        interp.call("f", &[]).unwrap(),
        Some(Val::Int(Tint::new(27, 6 + 1 - 3000).unwrap()))
    );
}

#[test]
fn br2_is_sugar_for_a_br3_with_two_identical_arms() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%t: t1) -> t9 {
^entry:
    br2 %t, ^then, ^else
^then:
    ret const t9 1
^else:
    ret const t9 0
}
"#;
    let m = checked(src);
    // It parses into a br3 and prints back as br2.
    assert!(tir::print_module(&m).contains("br2 %t, ^then, ^else"));
    let mut interp = Interp::new(&m);
    for (sel, want) in [(-1i128, 0i128), (0, 0), (1, 1)] {
        let arg = Val::Int(Tint::new(1, sel).unwrap());
        assert_eq!(
            interp.call("f", &[arg]).unwrap(),
            Some(Val::Int(Tint::new(9, want).unwrap()))
        );
    }
}

#[test]
fn the_type_may_be_omitted_when_a_constant_supplies_it() {
    // TIR §1 writes `%s = cmp %x, const t27 0` with no type on the mnemonic.
    let m = checked(
        r#"
tir 0.1 target "tritium"
fn @clamp_sign(%x: t27) -> t1 {
^entry:
    %s = cmp %x, const t27 0
    ret %s
}
"#,
    );
    let mut interp = Interp::new(&m);
    let arg = Val::Int(Tint::new(27, -42).unwrap());
    assert_eq!(
        interp.call("clamp_sign", &[arg]).unwrap(),
        Some(Val::Int(Tint::new(1, -1).unwrap()))
    );
}

// -------------------------------------------------------------- the verifier

fn assert_rejects(src: &str, needle: &str) {
    let errs = errors(src);
    assert!(
        errs.iter().any(|e| e.message.contains(needle)),
        "expected an error mentioning `{needle}`, got {errs:?}"
    );
}

#[test]
fn ssa_uses_must_be_dominated_by_their_definition() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%c: t1) -> t9 {
^entry:
    br3 %c, ^a, ^b, ^b
^a:
    %x = add.wrap t9 const t9 1, const t9 1
    br ^b
^b:
    ret %x
}
"#,
        "does not dominate",
    );
}

#[test]
fn values_are_defined_once() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %x = add.wrap t9 const t9 1, const t9 1
    %x = add.wrap t9 const t9 2, const t9 2
    ret %x
}
"#,
        "defined more than once",
    );
}

#[test]
fn mixed_widths_are_rejected() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%a: t9, %b: t27) -> t9 {
^entry:
    %x = add.wrap t9 %a, %b
    ret %x
}
"#,
        "expected t9",
    );
}

#[test]
fn entry_block_may_not_be_a_branch_target() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    br ^entry
}
"#,
        "entry block may not be a branch target",
    );
}

#[test]
fn block_arguments_must_match_block_parameters() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    br ^next(const t9 1, const t9 2)
^next(%a: t9):
    ret %a
}
"#,
        "takes 1 block parameters",
    );
}

#[test]
fn flag_flavor_defines_two_results() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%a: t9) -> t9 {
^entry:
    %x = add.flag t9 %a, %a
    ret %x
}
"#,
        "defines 1 results, expected 2",
    );
}

#[test]
fn conversions_must_go_the_way_they_say() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%a: t27) -> t9 {
^entry:
    %x = widen t27 %a -> t9
    ret %x
}
"#,
        "does not widen",
    );
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%a: t9) -> t27 {
^entry:
    %x = trunc t9 %a -> t27
    ret %x
}
"#,
        "does not narrow",
    );
}

#[test]
fn calls_are_checked_against_the_signature() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %x = call @g() -> t9
    ret %x
}
"#,
        "neither declared nor defined",
    );
    // A declaration — signature with no body — is enough to call.
    checked(
        r#"
tir 0.1 target "tritium"
fn @g(%a: t9) -> t9
fn @f() -> t9 {
^entry:
    %x = call @g(const t9 1) -> t9
    ret %x
}
"#,
    );
}

#[test]
fn a_return_must_match_the_signature() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    ret
}
"#,
        "`ret` with no value",
    );
}

#[test]
fn widths_are_bounded() {
    assert_rejects(
        r#"
tir 0.1 target "tritium"
fn @f(%a: t244) -> t244 {
^entry:
    %x = add.wrap t244 %a, %a
    ret %x
}
"#,
        "exceeds the module maximum",
    );
}

// ------------------------------------------------------- faults and UB (§4)

fn halt(src: &str) -> Halt {
    run(src, "f", &[]).unwrap_err()
}

#[test]
fn division_by_zero_faults() {
    assert_eq!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %x = div t9 const t9 1, const t9 0
    ret %x
}
"#
        ),
        Halt::Fault(trit_core::Fault::new(FaultCode::DivZero))
    );
}

#[test]
fn out_of_range_shift_faults() {
    assert_eq!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %x = shr t9 const t9 1, const t9 9
    ret %x
}
"#
        ),
        Halt::Fault(trit_core::Fault::new(FaultCode::Shift))
    );
}

#[test]
fn unreachable_is_ub() {
    assert!(matches!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    unreachable
}
"#
        ),
        Halt::Ub(_)
    ));
}

#[test]
fn escaping_provenance_is_ub() {
    // TIR §5: a pointer derived from an allocation may address only that
    // allocation's trytes plus one past the end.
    assert!(matches!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %p = slot tryte[3]
    %q = offset %p, const t27 4
    %v = load t9 %q
    ret %v
}
"#
        ),
        Halt::Ub(_)
    ));
    // One-past-the-end is a legal address, but not a legal access.
    assert!(matches!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %p = slot tryte[3]
    %q = offset %p, const t27 3
    %v = load t9 %q
    ret %v
}
"#
        ),
        Halt::Ub(_)
    ));
}

#[test]
fn natural_alignment_follows_the_am_table() {
    // AM §2.3: 1…9 trits align to 1 tryte, 10…27 trits to 3.
    use trustc::tir::interp::{align_trytes, size_trytes};
    for w in 1..=9 {
        assert_eq!(align_trytes(w), 1, "t{w}");
        assert_eq!(size_trytes(w), 1, "t{w}");
    }
    for w in 10..=27 {
        assert_eq!(align_trytes(w), 3, "t{w}");
    }
    // A t18 occupies two trytes but still aligns to three — the table is
    // stated per *width*, not per size.
    assert_eq!(size_trytes(18), 2);
    assert_eq!(align_trytes(18), 3);
    // Past the word the AM stops; the rule continues to the next power of
    // three (docs/spec-gaps.md G4.1).
    assert_eq!(align_trytes(28), 9);
    assert_eq!(align_trytes(81), 9);
    assert_eq!(align_trytes(82), 27);
}

#[test]
fn a_t18_access_needs_three_tryte_alignment() {
    for offset in [1i32, 2] {
        let src = format!(
            r#"
tir 0.1 target "tritium"
fn @f() -> t18 {{
^entry:
    %p = slot tryte[9]
    %q = offset %p, const t27 {offset}
    %v = load t18 %q
    ret %v
}}
"#
        );
        assert!(matches!(halt(&src), Halt::Ub(_)), "offset {offset}");
    }
}

#[test]
fn misaligned_access_is_ub() {
    assert!(matches!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t27 {
^entry:
    %p = slot tryte[6]
    %q = offset %p, const t27 1
    %v = load t27 %q
    ret %v
}
"#
        ),
        Halt::Ub(_)
    ));
}

#[test]
fn branching_on_poison_is_ub() {
    // Reading uninitialized slot storage yields poison; branching on it is UB.
    assert!(matches!(
        halt(
            r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    %p = slot tryte[1]
    %t = load t1 %p
    br3 %t, ^a, ^a, ^a
^a:
    ret const t9 0
}
"#
        ),
        Halt::Ub(_)
    ));
}

#[test]
fn stores_and_loads_round_trip_little_trytean() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%v: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %v, %p
    %lo = load t9 %p
    %q = offset %p, const t27 1
    %mid = load t9 %q
    %r = offset %p, const t27 2
    %hi = load t9 %r
    ; Reassemble: lo + 3^9*mid + 3^18*hi.
    %m1 = shl.wrap t27 const t27 1, const t27 9
    %m2 = shl.wrap t27 const t27 1, const t27 18
    %lo27 = widen t9 %lo -> t27
    %mid27 = widen t9 %mid -> t27
    %hi27 = widen t9 %hi -> t27
    %a = mul.wrap t27 %mid27, %m1
    %b = mul.wrap t27 %hi27, %m2
    %c = add.wrap t27 %lo27, %a
    %d = add.wrap t27 %c, %b
    ret %d
}
"#;
    for v in [
        0i128,
        1,
        -1,
        9841,
        -9841,
        3_812_798_742_493,
        -3_812_798_742_493,
    ] {
        assert_eq!(run_int(src, "f", &[v]), v, "storing {v}");
    }
}

#[test]
fn recursion_works() {
    let src = r#"
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
"#;
    assert_eq!(run_int(src, "fib", &[10]), 55);
}

#[test]
fn tritwise_and_arithmetic_ops_agree_with_the_core() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9, %b: t9) -> t9 {
^entry:
    %x = tmin t9 %a, %b
    %y = tmax t9 %a, %b
    %z = tmul t9 %x, %y
    %w = neg t9 %z
    ret %w
}
"#;
    for (a, b) in [(5i128, -7i128), (0, 0), (9841, -9841), (100, 3)] {
        let (x, y) = (Tint::new(9, a).unwrap(), Tint::new(9, b).unwrap());
        let want = x.tmin(&y).tmul(&x.tmax(&y)).neg();
        assert_eq!(run_int(src, "f", &[a, b]), want.to_i128().unwrap());
    }
}

#[test]
fn a_global_initializer_must_fit_its_trytes() {
    let errs = errors(
        r#"
tir 0.1 target "tritium"
global @g : tryte[1] = [9842]
"#,
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("does not fit in one tryte")),
        "{errs:?}"
    );
}

#[test]
fn constants_are_range_checked_at_parse_time() {
    // Types Ch. 1 §3: a literal that does not fit is an error, never a wrap.
    let e = tir::parse_module(
        r#"
tir 0.1 target "tritium"
fn @f() -> t9 {
^entry:
    ret const t9 9842
}
"#,
    )
    .unwrap_err();
    assert!(e.message.contains("does not fit"), "{e}");
    assert_eq!(Bt::from_i128(9842).trit_len(), 10);
}
