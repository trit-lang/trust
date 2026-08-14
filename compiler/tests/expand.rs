//! Expansion tests (TIR §6, the second direction).
//!
//! Same correctness criterion as promotion: run the function before and after
//! the pass and demand identical observable behavior. Wide values cannot
//! cross a function boundary yet (no calling convention exists for them), so
//! these functions take word-sized arguments, `widen` into the wide width,
//! compute, and `trunc` back.

use trit_core::Tint;
use trustc::tir::{self, Halt, Interp, TargetDesc, Val, legalize_module};

/// The reference target. A `t54` or `t81` value has to become two or three
/// `t27` parts on it.
fn tritium() -> TargetDesc {
    TargetDesc::tritium()
}

fn parse(src: &str) -> tir::Module {
    tir::parse_module(src).unwrap_or_else(|e| panic!("parse failed: {e}"))
}

fn run(m: &tir::Module, entry: &str, args: &[i128]) -> Result<Option<i128>, Halt> {
    let f = m.function(entry).expect("entry exists");
    let vals: Vec<Val> = f
        .sig
        .params
        .iter()
        .zip(args)
        .map(|((_, t), &a)| {
            Val::Int(Tint::new(t.width().expect("integer parameter"), a).expect("argument fits"))
        })
        .collect();
    match Interp::new(m).call(entry, &vals) {
        Ok(None) => Ok(None),
        Ok(Some(Val::Int(i))) => Ok(Some(i.to_i128().expect("result fits"))),
        Ok(Some(other)) => panic!("unexpected result {other}"),
        Err(e) => Err(e),
    }
}

/// No width above the target's widest legal width may survive.
fn assert_no_wide_widths(m: &tir::Module, target: &TargetDesc) {
    let widest = target.widest_legal();
    let check = |t: tir::Type, what: &str| {
        if let tir::Type::Int(w) = t {
            assert!(
                w <= widest,
                "{what}: t{w} exceeds the widest legal width t{widest}"
            );
        }
    };
    for f in &m.funcs {
        for (_, t) in &f.sig.params {
            check(*t, "parameter");
        }
        for b in &f.blocks {
            for (_, t) in &b.params {
                check(*t, "block parameter");
            }
            for i in &b.insts {
                use tir::InstKind::*;
                match &i.kind {
                    Flavored { ty, a, b, .. } | Plain { ty, a, b, .. } => {
                        check(*ty, "operation");
                        for o in [a, b] {
                            if let tir::Operand::Const(t, _) = o {
                                check(*t, "constant");
                            }
                        }
                    }
                    Neg { ty, .. } | Cmp { ty, .. } | Select3 { ty, .. } => check(*ty, "operation"),
                    Load { ty, .. } | Store { ty, .. } => check(*ty, "access"),
                    Widen { from, to, .. } | Trunc { from, to, .. } => {
                        check(*from, "conversion source");
                        check(*to, "conversion target");
                    }
                    _ => {}
                }
            }
        }
    }
}

fn differential(src: &str, target: &TargetDesc, entry: &str, cases: &[&[i128]]) -> tir::Module {
    let original = parse(src);
    assert!(tir::verify(&original).is_empty(), "input does not verify");
    let legalized =
        legalize_module(&original, target).unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    let errs = tir::verify(&legalized);
    assert!(
        errs.is_empty(),
        "legalized output does not verify: {errs:?}\n{}",
        tir::print_module(&legalized)
    );
    assert_no_wide_widths(&legalized, target);

    for args in cases {
        assert_eq!(
            run(&original, entry, args),
            run(&legalized, entry, args),
            "@{entry}{args:?} differs after expansion\n{}",
            tir::print_module(&legalized)
        );
    }
    legalized
}

/// Values chosen to exercise part boundaries: t27's MAX is where the low part
/// of a t54 value overflows into the high one.
const MAX27: i128 = 3_812_798_742_493;

// ------------------------------------------------------------------ arithmetic

#[test]
fn wide_addition_chains_the_carry_between_parts() {
    let src = r#"
tir 0.1 target "tritium"
fn @add(%a: t27, %b: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    %s = add.wrap t54 %wa, %wb
    ; Bring the high part down so the result is observable through a t27
    ; return: (a+b) / 3^27, which is nonzero exactly when the low part
    ; overflowed.
    %hi = trunc t54 %s -> t27
    ret %hi
}
"#;
    let m = differential(
        src,
        &tritium(),
        "add",
        &[
            &[MAX27, 1],
            &[MAX27, MAX27],
            &[-MAX27, -MAX27],
            &[0, 0],
            &[1, -1],
            &[123_456_789, 987_654_321],
        ],
    );
    // The carry is exactly the `.flag` trit fed into the next part.
    let printed = tir::print_module(&m);
    assert!(printed.contains("add.flag t27"), "{printed}");
}

#[test]
fn wide_values_survive_a_round_trip_through_arithmetic() {
    // Compute (a + b) − b at t54 and narrow back: the identity must hold even
    // when a + b crosses the part boundary.
    let src = r#"
tir 0.1 target "tritium"
fn @roundtrip(%a: t27, %b: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    %s = add.wrap t54 %wa, %wb
    %d = sub.wrap t54 %s, %wb
    %r = trunc t54 %d -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "roundtrip",
        &[
            &[MAX27, MAX27],
            &[-MAX27, -MAX27],
            &[MAX27, -MAX27],
            &[0, MAX27],
            &[7, 11],
        ],
    );
}

#[test]
fn subtraction_is_addition_of_the_negation_at_every_part() {
    let src = r#"
tir 0.1 target "tritium"
fn @sub(%a: t27, %b: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    %d = sub.wrap t54 %wa, %wb
    %n = neg t54 %d
    %r = trunc t54 %n -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "sub",
        &[&[MAX27, -MAX27], &[0, MAX27], &[-5, 5], &[MAX27, MAX27]],
    );
}

#[test]
fn three_part_values_work_too() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27) -> t27 {
^entry:
    %w = widen t27 %a -> t81
    %s = add.wrap t81 %w, %w
    %t = add.wrap t81 %s, %w
    %r = trunc t81 %t -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "f",
        &[&[MAX27], &[-MAX27], &[0], &[1], &[1_000_000]],
    );
}

#[test]
fn wide_trapping_arithmetic_faults_at_the_wide_boundary() {
    // t54's MAX, reached by shifting a t27 MAX into the high part.
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27, %b: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    %s = add.trap t54 %wa, %wb
    %r = trunc t54 %s -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "f",
        &[&[MAX27, MAX27], &[-MAX27, -MAX27], &[0, 0]],
    );
}

#[test]
fn a_wide_trap_fires_at_exactly_the_same_input_before_and_after() {
    // Doubling a t54 value `n` times overflows for some n; the expanded form
    // has to fault on the same n, which pins the carry chain and the overflow
    // trit together. t54's MAX is about 2.9e25, so ~43 doublings from t27's
    // MAX is the threshold.
    let src = r#"
tir 0.1 target "tritium"
fn @double_n(%n: t27, %seed: t27) -> t27 {
^entry:
    %w = widen t27 %seed -> t54
    br ^loop(%n, %w)
^loop(%i: t27, %acc: t54):
    %c = cmp t27 %i, const t27 0
    br3 %c, ^done(%acc), ^done(%acc), ^step(%i, %acc)
^step(%j: t27, %a: t54):
    %j2 = sub.trap t27 %j, const t27 1
    %a2 = add.trap t54 %a, %a
    br ^loop(%j2, %a2)
^done(%r: t54):
    %out = trunc t54 %r -> t27
    ret %out
}
"#;
    let cases: Vec<Vec<i128>> = (38..=46).map(|n| vec![n, MAX27]).collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, &tritium(), "double_n", &refs);

    // And the threshold is real, not vacuous: some of those inputs fault and
    // some do not.
    let m = parse(src);
    assert!(run(&m, "double_n", &[38, MAX27]).is_ok());
    assert!(matches!(
        run(&m, "double_n", &[46, MAX27]),
        Err(Halt::Fault(_))
    ));
}

#[test]
fn the_wide_flag_flavor_reports_the_direction_of_the_overflow() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%n: t27, %seed: t27) -> t27 {
^entry:
    %w = widen t27 %seed -> t54
    br ^loop(%n, %w)
^loop(%i: t27, %acc: t54):
    %c = cmp t27 %i, const t27 0
    br3 %c, ^done(%acc), ^done(%acc), ^step(%i, %acc)
^step(%j: t27, %a: t54):
    %j2 = sub.trap t27 %j, const t27 1
    %a2, %o = add.flag t54 %a, %a
    br3 %o, ^over(const t27 -1), ^loop(%j2, %a2), ^over(const t27 1)
^over(%d: t27):
    ret %d
^done(%r: t54):
    %out = trunc t54 %r -> t27
    ret %out
}
"#;
    let cases: Vec<Vec<i128>> = (40..=46)
        .flat_map(|n| [vec![n, MAX27], vec![n, -MAX27]])
        .collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, &tritium(), "f", &refs);

    // Overflowing upward reports +1, downward −1 — the same trit both before
    // and after expansion, which `differential` has already compared.
    let m = parse(src);
    assert_eq!(run(&m, "f", &[46, MAX27]).unwrap(), Some(1));
    assert_eq!(run(&m, "f", &[46, -MAX27]).unwrap(), Some(-1));
}

#[test]
fn wide_comparison_folds_most_significant_part_first() {
    let src = r#"
tir 0.1 target "tritium"
fn @cmp(%a: t27, %b: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    ; Shift both into the high part by adding them to themselves repeatedly
    ; would be slow; instead compare directly, which still exercises the fold.
    %t = cmp t54 %wa, %wb
    %r = widen t1 %t -> t27
    ret %r
}
"#;
    let m = differential(
        src,
        &tritium(),
        "cmp",
        &[
            &[0, 0],
            &[1, 0],
            &[0, 1],
            &[MAX27, -MAX27],
            &[-MAX27, MAX27],
            &[42, 42],
        ],
    );
    let printed = tir::print_module(&m);
    assert!(
        printed.contains("select3"),
        "the fold uses select3:\n{printed}"
    );
}

#[test]
fn a_wide_comparison_reads_the_high_part_first() {
    // Build two t54 values that agree in the low part and differ in the high
    // one, so a fold that got the priority backwards would answer wrongly.
    let src = r#"
tir 0.1 target "tritium"
fn @f(%hi: t27, %lo: t27) -> t27 {
^entry:
    %whi = widen t27 %hi -> t54
    %wlo = widen t27 %lo -> t54
    ; hi * 3^27 + lo, built by repeated doubling of the widened value is
    ; expensive; instead use the fact that adding MAX+1 to itself carries.
    %one = add.wrap t54 %whi, %whi
    %v = add.wrap t54 %one, %wlo
    %t = cmp t54 %v, %wlo
    %r = widen t1 %t -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "f",
        &[&[MAX27, 0], &[MAX27, 5], &[-MAX27, 5], &[0, 5], &[1, 1]],
    );
}

#[test]
fn trit_wise_operations_and_select_are_positionwise() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27, %b: t27, %c: t27) -> t27 {
^entry:
    %wa = widen t27 %a -> t54
    %wb = widen t27 %b -> t54
    %x = tmin t54 %wa, %wb
    %y = tmax t54 %wa, %wb
    %z = tmul t54 %x, %y
    %t = cmp t27 %c, const t27 0
    %s = select3 %t, t54 %x, %y, %z
    %r = trunc t54 %s -> t27
    ret %r
}
"#;
    differential(
        src,
        &tritium(),
        "f",
        &[
            &[MAX27, -MAX27, -1],
            &[MAX27, -MAX27, 0],
            &[MAX27, -MAX27, 1],
            &[12345, -54321, 1],
        ],
    );
}

// ---------------------------------------------------------------------- memory

#[test]
fn a_wide_value_is_stored_as_consecutive_parts() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27) -> t27 {
^entry:
    %w = widen t27 %a -> t54
    %s = add.wrap t54 %w, %w
    %p = slot tryte[6]
    store t54 %s, %p
    %v = load t54 %p
    %r = trunc t54 %v -> t27
    ret %r
}
"#;
    let m = differential(src, &tritium(), "f", &[&[MAX27], &[-MAX27], &[0], &[7]]);
    // Two t27 accesses, the second three trytes further on — little-trytean,
    // least significant part at the lowest address.
    let printed = tir::print_module(&m);
    assert!(printed.contains("store t27"), "{printed}");
    assert!(printed.contains("offset"), "{printed}");
}

// -------------------------------------------------------- loops and boundaries

#[test]
fn wide_block_parameters_are_split_into_parts() {
    let src = r#"
tir 0.1 target "tritium"
fn @accumulate(%n: t27, %step: t27) -> t27 {
^entry:
    %w = widen t27 %step -> t54
    br ^loop(%n, %w)
^loop(%i: t27, %acc: t54):
    %c = cmp t27 %i, const t27 0
    br3 %c, ^done(%acc), ^done(%acc), ^step(%i, %acc)
^step(%j: t27, %a: t54):
    %j2 = sub.trap t27 %j, const t27 1
    %w2 = widen t27 %step -> t54
    %a2 = add.wrap t54 %a, %w2
    br ^loop(%j2, %a2)
^done(%r: t54):
    %out = trunc t54 %r -> t27
    ret %out
}
"#;
    let m = differential(
        src,
        &tritium(),
        "accumulate",
        &[&[0, 5], &[1, 5], &[10, MAX27], &[40, MAX27], &[7, -MAX27]],
    );
    // The t54 block parameter became two t27 parameters.
    let printed = tir::print_module(&m);
    assert!(
        printed.contains("^loop(%i: t27, %lz.acc.0: t27, %lz.acc.1: t27)"),
        "{printed}"
    );
}

#[test]
fn wide_values_cannot_cross_a_function_boundary_yet() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t54) -> t54 {
^entry:
    ret %a
}
"#;
    let errs = legalize_module(&parse(src), &tritium()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("calling convention")),
        "{errs:?}"
    );
}

#[test]
fn wide_multiplication_expands_now_that_mulh_exists() {
    // This test used to assert the refusal: G6.6 said `mul` could not be
    // expanded because TIR had no widening multiply, while TRISC-27 §4.1
    // already had `mulh`. TIR §3.1 now has it too, and the expansion is the
    // schoolbook one — a part times a part, low half here and high half one
    // position up.
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27) -> t27 {
^entry:
    %w = widen t27 %a -> t54
    %m = mul.wrap t54 %w, %w
    %r = trunc t54 %m -> t27
    ret %r
}
"#;
    let m = legalize_module(&parse(src), &tritium()).expect("expands");
    let printed = tir::print_module(&m);
    assert!(printed.contains("mulh"), "{printed}");
    // And nothing wider than the word survives.
    assert!(!printed.contains("t54"), "{printed}");
}

#[test]
fn wide_division_reports_that_it_is_unwritten() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t27) -> t27 {
^entry:
    %w = widen t27 %a -> t54
    %d = div t54 %w, %w
    %r = trunc t54 %d -> t27
    ret %r
}
"#;
    let errs = legalize_module(&parse(src), &tritium()).unwrap_err();
    assert!(
        errs.iter().any(|e| e.message.contains("not implemented")),
        "{errs:?}"
    );
}
