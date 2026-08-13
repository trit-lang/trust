//! Differential tests: `Bt`/`Tint` against `i128` over exhaustive small
//! ranges, plus the identities the specification states in prose.

use trit_core::{Bt, Flavor, Tint, Trit};

/// Round to nearest, ties away from zero — the reference semantics of
/// Types Ch. 1 §4 written out in `i128`.
fn ref_div(a: i128, b: i128) -> i128 {
    let (q, r) = (a / b, a % b); // Rust truncates toward zero
    if 2 * r.abs() >= b.abs() {
        q + if (a < 0) == (b < 0) { 1 } else { -1 }
    } else {
        q
    }
}

#[test]
fn add_sub_mul_match_i128() {
    for a in -300i128..=300 {
        for b in -300i128..=300 {
            let (x, y) = (Bt::from_i128(a), Bt::from_i128(b));
            assert_eq!(x.add(&y).to_i128(), Some(a + b), "{a} + {b}");
            assert_eq!(x.sub(&y).to_i128(), Some(a - b), "{a} - {b}");
            assert_eq!(x.mul(&y).to_i128(), Some(a * b), "{a} * {b}");
            assert_eq!(x.cmp3(&y).to_i8() as i128, (a - b).signum(), "{a} <=> {b}");
        }
    }
}

#[test]
fn divrem_matches_reference_and_invariants() {
    for a in -2000i128..=2000 {
        for b in [-100i128, -10, -7, -3, -2, -1, 1, 2, 3, 7, 10, 100] {
            let (q, r) = Bt::from_i128(a).divrem(&Bt::from_i128(b)).unwrap();
            let (q, r) = (q.to_i128().unwrap(), r.to_i128().unwrap());
            assert_eq!(q * b + r, a, "a = q*b + r for {a} / {b}");
            assert!(
                2 * r.abs() <= b.abs(),
                "|r| <= |b|/2 for {a} / {b}: r = {r}"
            );
            assert_eq!(q, ref_div(a, b), "{a} / {b}");
        }
    }
}

#[test]
fn divrem_on_wide_values() {
    // Beyond i128 — 200 trits, where the whole point of Bt is that it still
    // has to be exact (TIR §2).
    let big = Bt::max_of_width(200);
    let d = Bt::from_i128(1_000_000_007);
    let (q, r) = big.divrem(&d).unwrap();
    assert_eq!(q.mul(&d).add(&r), big);
    assert!(r.abs().mul(&Bt::from_i128(2)).cmp3(&d.abs()) != Trit::Pos);
}

#[test]
fn division_by_zero_is_none() {
    assert!(Bt::from_i128(5).divrem(&Bt::ZERO).is_none());
}

#[test]
fn shifts_are_multiplication_and_division_by_powers_of_three() {
    for a in -500i128..=500 {
        for k in 0..5u32 {
            let p = 3i128.pow(k);
            assert_eq!(Bt::from_i128(a).shl(k).to_i128(), Some(a * p), "{a} << {k}");
            assert_eq!(
                Bt::from_i128(a).shr(k).to_i128(),
                Some(ref_div(a, p)),
                "{a} >> {k}"
            );
        }
    }
}

#[test]
fn wrapping_is_the_symmetric_residue() {
    for w in 1..=6u32 {
        let m = 3i128.pow(w);
        let max = (m - 1) / 2;
        for a in -2000i128..=2000 {
            let w_val = Bt::from_i128(a).wrap_to(w).to_i128().unwrap();
            assert!(w_val.abs() <= max, "wrap to t{w} of {a} out of range");
            assert_eq!((a - w_val) % m, 0, "wrap to t{w} of {a} changed residue");
        }
    }
}

#[test]
fn min_is_minus_max_at_every_width() {
    for w in 1..=30u32 {
        assert_eq!(Tint::min(w), Tint::max(w).neg());
        assert_eq!(Tint::max(w).to_i128(), Some((3i128.pow(w) - 1) / 2));
    }
}

#[test]
fn spec_table_widths() {
    // Types Ch. 1 §2: t9 spans -9841..=9841, t27 spans ±3_812_798_742_493.
    assert_eq!(Tint::max(9).to_i128(), Some(9841));
    assert_eq!(Tint::max(27).to_i128(), Some(3_812_798_742_493));
    assert!(Tint::new(9, 9842).is_none());
    assert!(Tint::new(9, -9842).is_none());
}

#[test]
fn tritwise_ops_are_positionwise() {
    for a in -100i128..=100 {
        for b in -100i128..=100 {
            let (x, y) = (Tint::wrapping(9, a), Tint::wrapping(9, b));
            for i in 0..9 {
                assert_eq!(x.tmin(&y).trit(i), x.trit(i).tmin(y.trit(i)));
                assert_eq!(x.tmax(&y).trit(i), x.trit(i).tmax(y.trit(i)));
                assert_eq!(x.tmul(&y).trit(i), x.trit(i).tmul(y.trit(i)));
                assert_eq!(x.tneg().trit(i), x.trit(i).tneg());
            }
        }
    }
}

#[test]
fn widen_then_trunc_is_identity() {
    for a in -9841i128..=9841 {
        let x = Tint::new(9, a).unwrap();
        assert_eq!(x.widen(27).trunc(9), x);
        assert_eq!(x.widen(27).to_i128(), Some(a));
    }
}

#[test]
fn flag_flavor_carry_reconstructs_the_exact_sum() {
    // TIR §6: expansion into multi-part arithmetic chains the .flag trit.
    let w = 9;
    let base = Bt::from_i128(3i128.pow(w));
    for a in [-9841i128, -5000, -1, 0, 1, 5000, 9841] {
        for b in [-9841i128, -5000, -1, 0, 1, 5000, 9841] {
            let (lo, carry) = Tint::new(w, a)
                .unwrap()
                .add(&Tint::new(w, b).unwrap(), Flavor::Flag)
                .unwrap();
            let exact = lo.value().add(&Bt::from(carry).mul(&base));
            assert_eq!(exact.to_i128(), Some(a + b), "{a} + {b}");
        }
    }
}

#[test]
fn trap_flavor_faults_exactly_on_overflow() {
    for a in -9841i128..=9841 {
        let x = Tint::new(9, a).unwrap();
        let one = Tint::new(9, 1).unwrap();
        let trapped = x.add(&one, Flavor::Trap).is_err();
        assert_eq!(trapped, a + 1 > 9841, "{a} + 1");
    }
}

#[test]
fn division_never_overflows() {
    // Including MIN / -1, which is F_OVERFLOW in the binary world.
    for a in -9841i128..=9841 {
        for b in [-9841i128, -1, 1, 9841] {
            let q = Tint::new(9, a).unwrap().div(&Tint::new(9, b).unwrap());
            assert!(q.is_ok(), "{a} / {b}");
        }
    }
}

#[test]
fn shift_out_of_range_faults() {
    let x = Tint::new(9, 1).unwrap();
    assert!(x.shl(8, Flavor::Wrap).is_ok());
    assert!(x.shl(9, Flavor::Wrap).is_err());
    assert!(x.shr(9).is_err());
}
