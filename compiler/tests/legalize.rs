//! Legalization tests (TIR §6).
//!
//! The correctness criterion is the one the TIR spec states in its preamble:
//! a transformation is correct iff it preserves observable AM behavior for
//! all defined executions. So every test here runs the *same* function before
//! and after legalization, on the same inputs, and demands the same answer —
//! including the same faults. The interpreter is the oracle.

use std::collections::BTreeSet;
use trit_core::Tint;
use trustc::tir::{self, Halt, Interp, TargetDesc, Type, Val, legalize_module};

/// A target with no `t1` in its legal set, so every comparison result and
/// every trit-width operation has to be promoted.
fn no_t1() -> TargetDesc {
    TargetDesc {
        name: "no-t1".into(),
        addr_unit: 9,
        ptr_width: 27,
        legal: vec![9, 27],
        word: 27,
        call_conv: "test0".into(),
    }
}

/// A target with a single legal width: everything narrower than the word is
/// promoted to it.
fn word_only() -> TargetDesc {
    TargetDesc {
        name: "word-only".into(),
        addr_unit: 9,
        ptr_width: 27,
        legal: vec![27],
        word: 27,
        call_conv: "test0".into(),
    }
}

fn parse(src: &str) -> tir::Module {
    tir::parse_module(src).unwrap_or_else(|e| panic!("parse failed: {e}"))
}

fn verified(m: &tir::Module, what: &str) {
    let errs = tir::verify(m);
    assert!(errs.is_empty(), "{what} does not verify: {errs:?}");
}

/// Every arithmetic width in the module must be in the legal set. `t1`
/// survives only as a condition — a `cmp` result, a `.flag` trit, or a
/// selector — which is the invariant the pass documents.
fn assert_legal(m: &tir::Module, target: &TargetDesc) {
    let legal: BTreeSet<u32> = target.legal.iter().copied().collect();
    let ok = |t: Type, what: &str| {
        if let Type::Int(w) = t {
            assert!(
                legal.contains(&w) || w == 1,
                "{what}: t{w} is not legal for \"{}\"",
                target.name
            );
        }
    };
    for f in &m.funcs {
        for (_, t) in &f.sig.params {
            ok(*t, "parameter");
        }
        if let Some(t) = f.sig.ret {
            ok(t, "return");
        }
        for b in &f.blocks {
            for (_, t) in &b.params {
                ok(*t, "block parameter");
            }
            for i in &b.insts {
                use tir::InstKind::*;
                match &i.kind {
                    Flavored { ty, .. } | Plain { ty, .. } | Neg { ty, .. } | Cmp { ty, .. } => {
                        // A `t1` *operation* would be arithmetic at an illegal
                        // width, which is exactly what the pass must remove.
                        if let Type::Int(w) = ty {
                            assert!(
                                legal.contains(w),
                                "arithmetic at t{w} survived legalization"
                            );
                        }
                    }
                    Select3 { ty, .. } => ok(*ty, "select3"),
                    Widen { from, to, .. } | Trunc { from, to, .. } => {
                        ok(*from, "conversion source");
                        ok(*to, "conversion target");
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Run one function in one module, reporting the numeric result so that
/// versions with different widths stay comparable.
fn run(m: &tir::Module, entry: &str, args: &[i128]) -> Result<Option<i128>, Halt> {
    let f = m.function(entry).expect("entry exists");
    let vals: Vec<Val> = f
        .sig
        .params
        .iter()
        .zip(args)
        .map(|((_, t), &a)| {
            let w = t.width().expect("integer parameter");
            Val::Int(Tint::new(w, a).expect("argument fits"))
        })
        .collect();
    match Interp::new(m).call(entry, &vals) {
        Ok(None) => Ok(None),
        Ok(Some(Val::Int(i))) => Ok(Some(i.to_i128().expect("result fits"))),
        Ok(Some(other)) => panic!("unexpected result {other}"),
        Err(e) => Err(e),
    }
}

/// The whole point: legalize, check the output is well-formed and legal, and
/// confirm it behaves identically on every given input.
fn differential(src: &str, target: &TargetDesc, entry: &str, cases: &[&[i128]]) -> tir::Module {
    let original = parse(src);
    verified(&original, "input");
    let legalized =
        legalize_module(&original, target).unwrap_or_else(|e| panic!("legalization failed: {e:?}"));
    verified(&legalized, "legalized output");
    assert_legal(&legalized, target);

    for args in cases {
        let before = run(&original, entry, args);
        let after = run(&legalized, entry, args);
        assert_eq!(
            before,
            after,
            "@{entry}{args:?} behaves differently after legalization for \"{}\"\n{}",
            target.name,
            tir::print_module(&legalized)
        );
    }
    legalized
}

// ------------------------------------------------------------------- promotion

#[test]
fn a_module_already_legal_is_left_alone() {
    let src = include_str!("../../examples/tir/steps_toward.tir");
    let original = parse(src);
    let out = legalize_module(&original, &TargetDesc::tritium()).unwrap();
    assert_eq!(out.funcs, original.funcs, "nothing to do, nothing done");
}

#[test]
fn comparison_results_stay_trits_and_selectors_still_work() {
    // `cmp` yields t1 by definition even when 1 is not in the legal set.
    let src = include_str!("../../examples/tir/steps_toward.tir");
    let m = differential(
        src,
        &no_t1(),
        "steps_toward",
        &[&[0, -5], &[0, 0], &[0, 7], &[100, 100], &[-9841, 9841]],
    );
    let printed = tir::print_module(&m);
    assert!(printed.contains("cmp t27"), "{printed}");
}

#[test]
fn trit_arithmetic_is_promoted_and_renormalized() {
    // Wrapping at t1 means wrapping mod 3: 1 + 1 == -1.
    let src = r#"
tir 0.1 target "tritium"
fn @wrap_add(%a: t1, %b: t1) -> t1 {
^entry:
    %r = add.wrap t1 %a, %b
    ret %r
}
"#;
    for target in [no_t1(), word_only()] {
        let m = differential(
            src,
            &target,
            "wrap_add",
            &[&[1, 1], &[1, 0], &[-1, -1], &[0, 0], &[1, -1], &[-1, 1]],
        );
        // Renormalization is a mask, not a trunc: a trunc would reintroduce
        // the illegal width the pass just removed.
        let printed = tir::print_module(&m);
        assert!(printed.contains("tmul"), "expected a mask:\n{printed}");
        assert!(!printed.contains("-> t1"), "no trunc to t1:\n{printed}");
    }
    assert_eq!(run(&parse(src), "wrap_add", &[1, 1]).unwrap(), Some(-1));
}

#[test]
fn promoted_trapping_arithmetic_still_faults_at_the_narrow_boundary() {
    // t9 promoted to t27 must trap at t9's boundary, not t27's.
    let src = r#"
tir 0.1 target "tritium"
fn @add(%a: t9, %b: t9) -> t9 {
^entry:
    %r = add.trap t9 %a, %b
    ret %r
}
"#;
    differential(
        src,
        &word_only(),
        "add",
        &[
            &[9841, 0],
            &[9840, 1],
            &[9841, 1],
            &[-9841, -1],
            &[5000, 5000],
            &[-5000, -5000],
            &[9841, -9841],
        ],
    );
}

#[test]
fn promoted_wrapping_arithmetic_wraps_at_the_narrow_width() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9, %b: t9) -> t9 {
^entry:
    %r = add.wrap t9 %a, %b
    %s = mul.wrap t9 %r, %b
    %t = sub.wrap t9 %s, %a
    ret %t
}
"#;
    let cases: Vec<Vec<i128>> = [
        (9841i128, 1i128),
        (9841, 9841),
        (-9841, -9841),
        (1234, -4321),
        (0, 0),
        (99, 99),
    ]
    .iter()
    .map(|(a, b)| vec![*a, *b])
    .collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, &word_only(), "f", &refs);
}

#[test]
fn the_flag_flavor_survives_promotion() {
    let src = r#"
tir 0.1 target "tritium"
fn @carry(%a: t9, %b: t9) -> t9 {
^entry:
    %r, %c = add.flag t9 %a, %b
    %w = widen t1 %c -> t9
    ret %w
}
"#;
    differential(
        src,
        &word_only(),
        "carry",
        &[
            &[9841, 1],
            &[-9841, -1],
            &[1, 1],
            &[9841, -9841],
            &[5000, 5000],
        ],
    );
}

#[test]
fn division_and_the_trit_wise_set_need_no_fixup() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9, %b: t9) -> t9 {
^entry:
    %q = div t9 %a, %b
    %r = rem t9 %a, %b
    %m = tmin t9 %q, %r
    %x = tmax t9 %m, %a
    %y = tmul t9 %x, %b
    %z = neg t9 %y
    ret %z
}
"#;
    let cases: Vec<Vec<i128>> = [
        (7i128, 2i128),
        (8, 3),
        (-8, 3),
        (9841, -1),
        (0, 5),
        (100, 7),
    ]
    .iter()
    .map(|(a, b)| vec![*a, *b])
    .collect();
    let refs: Vec<&[i128]> = cases.iter().map(|v| v.as_slice()).collect();
    differential(src, &word_only(), "f", &refs);

    // Division by zero must still fault, not be optimized into the promotion.
    let m = legalize_module(&parse(src), &word_only()).unwrap();
    assert!(matches!(run(&m, "f", &[1, 0]), Err(Halt::Fault(_))));
}

#[test]
fn shifts_keep_the_narrow_range_check() {
    // AM §3.3: k outside 0…w−1 faults. After promotion to t27 the machine
    // would happily accept k = 9, so the pass has to re-impose t9's limit.
    let src = r#"
tir 0.1 target "tritium"
fn @shift(%a: t9, %k: t9) -> t9 {
^entry:
    %r = shl.wrap t9 %a, %k
    %s = shr t9 %r, %k
    ret %s
}
"#;
    differential(
        src,
        &word_only(),
        "shift",
        &[
            &[1, 0],
            &[1, 8],
            &[1, 9],
            &[1, -1],
            &[40, 2],
            &[9841, 4],
            &[1, 100],
        ],
    );
}

#[test]
fn a_constant_shift_out_of_range_becomes_a_static_fault() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9) -> t9 {
^entry:
    %r = shl.wrap t9 %a, const t9 9
    ret %r
}
"#;
    let m = differential(src, &word_only(), "f", &[&[1], &[0]]);
    let printed = tir::print_module(&m);
    assert!(printed.contains("trap F_SHIFT"), "{printed}");
}

#[test]
fn loops_block_parameters_and_calls_survive() {
    let src = r#"
tir 0.1 target "tritium"
fn @count_down(%n: t9) -> t9 {
^entry:
    br ^loop(%n, const t9 0)
^loop(%i: t9, %acc: t9):
    %c = cmp t9 %i, const t9 0
    br3 %c, ^done(%acc), ^done(%acc), ^step(%i, %acc)
^step(%j: t9, %a: t9):
    %j2 = sub.trap t9 %j, const t9 1
    %a2 = add.trap t9 %a, %j
    br ^loop(%j2, %a2)
^done(%r: t9):
    %d = call @double(%r) -> t9
    ret %d
}

fn @double(%x: t9) -> t9 {
^entry:
    %y = add.trap t9 %x, %x
    ret %y
}
"#;
    differential(
        src,
        &word_only(),
        "count_down",
        &[&[0], &[1], &[5], &[20], &[-3], &[140]],
    );
    // 140·141/2 = 9870 > t9's MAX, so the accumulator must still fault.
    let m = legalize_module(&parse(src), &word_only()).unwrap();
    assert!(matches!(run(&m, "count_down", &[141]), Err(Halt::Fault(_))));
}

#[test]
fn conversions_between_promoted_widths_collapse_correctly() {
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9) -> t9 {
^entry:
    %w = widen t9 %a -> t27
    %t = trunc t27 %w -> t3
    %u = widen t3 %t -> t9
    ret %u
}
"#;
    differential(
        src,
        &word_only(),
        "f",
        &[&[0], &[1], &[13], &[14], &[-13], &[-14], &[9841], &[-9841]],
    );
}

// ------------------------------------------------------------------- expansion

#[test]
fn promotion_and_expansion_coexist_in_one_function() {
    // t3 is below the only legal width and t54 is above it, so both
    // directions run over the same values. Expansion itself is covered in
    // `tests/expand.rs`; what matters here is that the two paths do not
    // interfere.
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t9) -> t9 {
^entry:
    %narrow = trunc t9 %a -> t3
    %n2 = add.wrap t3 %narrow, %narrow
    %wide = widen t9 %a -> t54
    %w2 = add.wrap t54 %wide, %wide
    %back = trunc t54 %w2 -> t9
    %n9 = widen t3 %n2 -> t9
    %r = add.wrap t9 %back, %n9
    ret %r
}
"#;
    differential(
        src,
        &word_only(),
        "f",
        &[&[0], &[1], &[13], &[-13], &[9841], &[-9841], &[40]],
    );
}

#[test]
fn a_target_that_cannot_detect_multiplication_overflow_says_so() {
    // t9 `mul.trap` promoted to t27 needs 18 trits for the exact product, so
    // it works; t18 would need 36 and cannot.
    let src = r#"
tir 0.1 target "tritium"
fn @f(%a: t18, %b: t18) -> t18 {
^entry:
    %r = mul.trap t18 %a, %b
    ret %r
}
"#;
    let errs = legalize_module(&parse(src), &word_only()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("cannot detect overflow")),
        "{errs:?}"
    );
}

#[test]
fn the_post_condition_catches_an_unlegalized_module() {
    // TIR §6: "backends may assume legalized input and are not required to
    // handle any other." Without a check, an unlegalized module is a licence
    // for the backend to emit anything — and legalization is incomplete
    // today (G6.6 blocks `mul`; `div`, `rem` and the shifts are unwritten),
    // so that path is reachable rather than hypothetical.
    let m = tir::parse_module(
        r#"tir 0.1 target "tritium"

fn @f(%x: t9, %y: t9) -> t9 {
^entry:
    %r = add.wrap t9 %x, %y
    ret %r
}
"#,
    )
    .expect("parses");
    let target = tir::TargetDesc::tritium();

    // Well-formed, so plain verification is happy.
    assert!(tir::verify(&m).is_empty());
    // But `tritium` has no native t9 add, so it is not legalized.
    let errs = tir::verify_legalized(&m, &target);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].message.contains("t9"), "{:?}", errs[0]);

    // Legalizing it makes the post-condition hold.
    let legalized = tir::legalize_module(&m, &target).expect("legalizes");
    assert!(tir::verify_legalized(&legalized, &target).is_empty());
}
