//! Legalization as a *checked transform* (TIR §6).
//!
//! `compiler/tests/pipeline.rs` runs a program on the interpreter and through
//! the whole pipeline and demands the same answer — but both sides take the
//! same TIR, so it covers `TIR → machine` and says nothing about whether any
//! pass in between preserved meaning. Legalization is a pass, it is mandatory,
//! and until this file existed nothing checked that it computed the same thing
//! afterwards as before.
//!
//! The check is the obvious one: interpret the module, legalize it, interpret
//! it again, demand the same result — faults included. The interpreter is
//! width-generic, so it can execute a module legalized for a machine that does
//! not exist.
//!
//! TIR §6 says legalization is in the core design because it "lets one
//! frontend serve a t27 reference machine and a t9 SBTCVM-class target from
//! identical mid-level IR". That claim is what this file tests, and the second
//! half of it does not hold yet. What follows records exactly how far it gets,
//! so that progress fails a test rather than passing unnoticed.

use trit_core::Tint;
use trustc::tir::{self, Halt, Interp, TargetDesc, Val};

/// The t9 machine TIR §6 names as the reason legalization is in the core
/// design.
///
/// Its addressable unit is the AM's tryte, so its alignment rule and the
/// interpreter's agree — which the 6-trit-unit `sbtcvm` target's do not
/// (G7.6). Its pointers are t9, because §7 requires the legal set to contain
/// a width at least `ptr_width`: a machine whose widest register is nine
/// trits cannot hold a 27-trit address, and so has at most 3⁹ addressable
/// units. That is SBTCVM-class, which is the point.
///
/// `check()` is asserted here because building a `TargetDesc` by hand skips
/// it — the CLI validates what it parses, and a test does not.
fn t9_target() -> TargetDesc {
    let t = TargetDesc {
        name: "t9".into(),
        addr_unit: 9,
        ptr_width: 9,
        legal: vec![1, 9],
        word: 9,
        call_conv: "t9".into(),
    };
    assert!(t.check().is_empty(), "{:?}", t.check());
    t
}

/// What running a function produced: a value, or a fault.
type Outcome = Result<i128, trit_core::FaultCode>;

fn interpret(m: &tir::Module, entry: &str, args: &[i128]) -> Outcome {
    let f = m.function(entry).expect("the entry exists");
    let vals: Vec<Val> = f
        .sig
        .params
        .iter()
        .zip(args)
        .map(|((_, t), &a)| {
            Val::Int(Tint::new(t.width().expect("an integer parameter"), a).expect("fits"))
        })
        .collect();
    match Interp::new(m).call(entry, &vals) {
        Ok(Some(Val::Int(i))) => Ok(i.to_i128().expect("fits")),
        Ok(other) => panic!("expected an integer result, got {other:?}"),
        Err(Halt::Fault(f)) => Err(f.code),
        Err(other) => panic!("interpreter stopped: {other}"),
    }
}

/// Legalize for a target and check that meaning survived.
fn preserves_meaning(src: &str, target: &TargetDesc, cases: &[&[i128]]) {
    let m = tir::parse_module(src).expect("parses");
    assert!(
        tir::verify(&m).is_empty(),
        "the source module is ill-formed"
    );
    let legalized = tir::legalize_module(&m, target)
        .unwrap_or_else(|e| panic!("legalization failed for {}: {e:?}", target.name));

    let errs = tir::verify_legalized(&legalized, target);
    assert!(errs.is_empty(), "post-condition: {errs:?}");

    for args in cases {
        assert_eq!(
            interpret(&m, "f", args),
            interpret(&legalized, "f", args),
            "legalizing for {} changed the answer at {args:?}",
            target.name
        );
    }
}

/// What legalizing refuses to do, as the message it gives.
fn refusal(src: &str, target: &TargetDesc) -> String {
    let m = tir::parse_module(src).expect("parses");
    match tir::legalize_module(&m, target) {
        Ok(_) => panic!("expected legalization to refuse:\n{src}"),
        Err(errs) => errs[0].to_string(),
    }
}

const CASES: &[&[i128]] = &[&[7, 3], &[-7, 3], &[9841, -9841], &[0, 5], &[-1, -1]];

fn module(body: &str) -> String {
    format!("tir 0.1 target \"tritium\"\n\nfn @f(%a: t9, %b: t9) -> t9 {{\n^entry:\n{body}\n}}\n")
}

// ------------------------------------------- promotion, which does work

#[test]
fn promotion_preserves_meaning() {
    // A width below the smallest legal one is promoted, operated on, and
    // renormalized. Every operation the reference target needs takes this
    // path, since `tritium`'s legal set is the word alone.
    let target = TargetDesc::tritium();
    for body in [
        "    %r = add.wrap t9 %a, %b\n    ret %r",
        "    %r = sub.trap t9 %a, %b\n    ret %r",
        "    %r = mul.wrap t9 %a, %b\n    ret %r",
        "    %r = mulh t9 %a, %b\n    ret %r",
        "    %r = div t9 %a, %b\n    ret %r",
        "    %r = rem t9 %a, %b\n    ret %r",
        "    %r = neg t9 %a\n    ret %r",
        "    %r = tmin t9 %a, %b\n    ret %r",
        "    %c = cmp t9 %a, %b\n    %r = widen t1 %c -> t9\n    ret %r",
        "    %c = cmp t9 %a, %b\n    %r = select3 %c, t9 %a, %b, %a\n    ret %r",
    ] {
        preserves_meaning(&module(body), &target, CASES);
    }
}

#[test]
fn promotion_preserves_faults() {
    // A fault is part of the answer: a trapping overflow must survive at the
    // *narrow* boundary, not at the width it was promoted to.
    let target = TargetDesc::tritium();
    preserves_meaning(
        &module("    %r = add.trap t9 %a, %b\n    ret %r"),
        &target,
        &[&[9841, 1], &[-9841, -1], &[9841, -9841]],
    );
    preserves_meaning(
        &module("    %r = div t9 %a, %b\n    ret %r"),
        &target,
        &[&[7, 0], &[0, 0]],
    );
}

// ----------------------------------------------------------- expansion

#[test]
fn expansion_preserves_meaning() {
    // A width above the widest legal one is expanded into parts with carry
    // chaining, and the parts are stored and loaded one at a time. All of
    // this is checked by running it twice, not by reading it.
    let target = t9_target();
    let wide = |body: &str| {
        format!(
            "tir 0.1 target \"tritium\"\n\nfn @f(%p: t9, %q: t9) -> t9 {{\n^entry:\n\
             \x20   %a = widen t9 %p -> t27\n    %b = widen t9 %q -> t27\n{body}\n}}\n"
        )
    };
    let narrow = "    %n = trunc t27 %r -> t9\n    ret %n";
    for body in [
        format!("    %r = add.wrap t27 %a, %b\n{narrow}"),
        format!("    %r = sub.wrap t27 %a, %b\n{narrow}"),
        format!("    %r = add.trap t27 %a, %b\n{narrow}"),
        format!("    %r = mul.wrap t27 %a, %b\n{narrow}"),
        format!("    %r = mul.trap t27 %a, %b\n{narrow}"),
        format!("    %r = neg t27 %a\n{narrow}"),
        format!("    %r = tmin t27 %a, %b\n{narrow}"),
        format!("    %r = tmax t27 %a, %b\n{narrow}"),
        format!("    %r = tmul t27 %a, %b\n{narrow}"),
        "    %c = cmp t27 %a, %b\n    %n = widen t1 %c -> t9\n    ret %n".to_string(),
        format!("    %c = cmp t27 %a, %b\n    %r = select3 %c, t27 %a, %b, %a\n{narrow}"),
        // Wide memory: the value is split across parts at addressable
        // boundaries, stored, and read back.
        format!("    %s = slot tryte[3]\n    store t27 %a, %s\n    %r = load t27 %s\n{narrow}"),
    ] {
        preserves_meaning(&wide(&body), &target, CASES);
    }
}

#[test]
fn the_expansion_frontier_is_exactly_this() {
    // What expansion still refuses, with the reason each gives. When one of
    // these starts working this test fails, which is the point: the frontier
    // moves in the document at the same time as in the code.
    let target = t9_target();
    let wide = |op: &str| {
        format!(
            "tir 0.1 target \"tritium\"\n\nfn @f(%p: t9, %q: t9) -> t9 {{\n^entry:\n\
             \x20   %a = widen t9 %p -> t27\n    %b = widen t9 %q -> t27\n\
             \x20   {op}\n    %n = trunc t27 %r -> t9\n    ret %n\n}}\n"
        )
    };

    // G6.6 is closed: `mulh` exists and `mul` expands. `shl` is next, and
    // still says what it needs.
    let msg = refusal(&wide("%r = shl.wrap t27 %a, %b"), &target);
    assert!(msg.contains("widening multiply"), "{msg}");
    // Unwritten: multi-part division and shifts.
    for op in [
        "%r = div t27 %a, %b",
        "%r = rem t27 %a, %b",
        "%r = shr t27 %a, %b",
    ] {
        let msg = refusal(&wide(op), &target);
        assert!(msg.contains("not implemented"), "{msg}");
    }
    // G6.5: a wide value cannot cross a function boundary, because TIR has
    // neither multiple return values nor an `sret` convention.
    let msg = refusal(
        "tir 0.1 target \"tritium\"\n\nfn @f(%a: t27) -> t27 {\n^entry:\n    ret %a\n}\n",
        &target,
    );
    assert!(msg.contains("crosses a function boundary"), "{msg}");
}

#[test]
fn a_trust_program_legalizes_for_a_nine_trit_machine() {
    // TIR §6's own justification: legalization is in the core design because
    // it "lets one frontend serve a t27 reference machine and a t9
    // SBTCVM-class target from identical mid-level IR". This is that claim,
    // executed — the same source, legalized two ways, agreeing.
    let target = t9_target();
    for src in [
        "fn f(p: t9, q: t9) -> t9 { let a = p as t27; let b = q as t27; (a + b) as t9 }",
        "fn f(p: t9, q: t9) -> t9 { let a = p as t27; if a > 100 { 1 } else { 0 } }",
        "fn f(p: t9, q: t9) -> t9 { \
             let mut s: t27 = 0; let mut i: t27 = 0; \
             while i < 3 { s = s + (p as t27); i = i + 1; } \
             s as t9 \
         }",
    ] {
        let m = trustc::lang::compile(src).expect("compiles");
        let legalized = tir::legalize_module(&m, &target).expect("legalizes for t9");
        assert!(tir::verify(&legalized).is_empty());
        assert!(tir::verify_legalized(&legalized, &target).is_empty());
        for args in [&[7i128, 3][..], &[-7, 3], &[100, 1], &[0, 0]] {
            assert_eq!(
                interpret(&m, "f", args),
                interpret(&legalized, "f", args),
                "t9 legalization changed the answer at {args:?} for:\n{src}"
            );
        }
        // And the same source still works for the reference machine.
        let reference = tir::legalize_module(&m, &TargetDesc::tritium()).expect("legalizes");
        for args in [&[7i128, 3][..], &[-7, 3]] {
            assert_eq!(interpret(&m, "f", args), interpret(&reference, "f", args));
        }
    }
}

#[test]
fn a_multi_part_multiply_carries_and_overflows_correctly() {
    // The probe that mattered: operands widened from `t9` never leave one
    // part's worth of magnitude, so the carry chain is barely touched and an
    // overflow never happens. These are whole-range constants — including
    // MAX and MIN — against all three flavors.
    let target = t9_target();
    const VALS: [i128; 8] = [
        1,
        -1,
        9841,
        -9841,
        1_000_003,
        -1_000_003,
        3_812_798_742_493,
        -3_812_798_742_493,
    ];
    for flavor in ["wrap", "trap", "flag"] {
        for x in VALS {
            for y in VALS {
                let src = if flavor == "flag" {
                    // The flag is the answer: the direction of the overflow.
                    format!(
                        "tir 0.1 target \"tritium\"\n\nfn @f(%z: t9) -> t9 {{\n^entry:\n\
                         \x20   %r, %o = mul.flag t27 const t27 {x}, const t27 {y}\n    ret %o\n}}\n"
                    )
                } else {
                    format!(
                        "tir 0.1 target \"tritium\"\n\nfn @f(%z: t9) -> t9 {{\n^entry:\n\
                         \x20   %r = mul.{flavor} t27 const t27 {x}, const t27 {y}\n\
                         \x20   %n = trunc t27 %r -> t9\n    ret %n\n}}\n"
                    )
                };
                let m = tir::parse_module(&src).expect("parses");
                let legalized = tir::legalize_module(&m, &target)
                    .unwrap_or_else(|e| panic!("{flavor} {x} * {y}: {e:?}"));
                assert_eq!(
                    interpret(&m, "f", &[0]),
                    interpret(&legalized, "f", &[0]),
                    "mul.{flavor} of {x} and {y}"
                );
            }
        }
    }
}
