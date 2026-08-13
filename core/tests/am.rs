//! The abstract machine's own claims, tested.
//!
//! Every assertion here is a sentence from `spec/00-abstract-machine.md`:
//! the consequences it draws in §1.2, the symmetry identities it justifies
//! the division tie-break with (§3.2), the shift properties of §3.3, and the
//! worked identities of Appendix A that "optimizer rule tables may rely on".
//! If a future rewrite of the arithmetic core breaks one of these, it breaks
//! the machine, not just this crate.

use trit_core::{Bt, Flavor, Tint, Trit};

const W: u32 = 27;

fn t(v: i128) -> Tint {
    Tint::new(W, v).expect("in range")
}

/// A spread of values including both range ends, at t9 so the ends are cheap
/// to reach.
fn sample9() -> Vec<Tint> {
    [-9841i128, -9840, -1000, -13, -1, 0, 1, 13, 1000, 9840, 9841]
        .into_iter()
        .map(|v| Tint::new(9, v).unwrap())
        .collect()
}

// ------------------------------------------------------ §1.1–1.2 representation

#[test]
fn trit_strings_are_most_significant_first() {
    // AM §1.1: `1T0` denotes (+1)·9 + (−1)·3 + 0 = 6.
    assert_eq!(Bt::from_trit_str("1T0").unwrap().to_i128(), Some(6));
}

#[test]
fn every_sequence_denotes_a_unique_integer_in_the_symmetric_range() {
    // AM §1.2: the n-trit sequences are a bijection onto the symmetric range.
    for w in 1..=7u32 {
        let max = (3i128.pow(w) - 1) / 2;
        let mut seen = std::collections::BTreeSet::new();
        for v in -max..=max {
            let x = Tint::new(w, v).expect("in range");
            assert!(seen.insert(x.to_trit_string()), "two values, one encoding");
            assert_eq!(x.to_i128(), Some(v));
        }
        assert_eq!(seen.len() as i128, 2 * max + 1);
        assert!(Tint::new(w, max + 1).is_none(), "range must end at MAX");
    }
}

#[test]
fn there_is_no_negative_zero() {
    for w in 1..=9u32 {
        assert_eq!(Tint::zero(w).neg(), Tint::zero(w));
        assert_eq!(Tint::zero(w).to_trit_string(), "0".repeat(w as usize));
    }
}

#[test]
fn negation_is_trit_wise_and_total() {
    // AM §1.2, consequence 3: −x replaces every trit with its negation.
    for x in sample9() {
        let n = x.neg();
        for i in 0..9 {
            assert_eq!(n.trit(i), x.trit(i).tneg());
        }
        assert_eq!(n.to_i128(), Some(-x.to_i128().unwrap()));
    }
}

#[test]
fn the_sign_is_the_most_significant_nonzero_trit() {
    // AM §1.2, consequence 4.
    for x in sample9() {
        let expected = (0..9)
            .rev()
            .map(|i| x.trit(i))
            .find(|t| !t.is_zero())
            .unwrap_or(Trit::Zero);
        assert_eq!(x.sign(), expected);
        assert_eq!(x.cmp3(&Tint::zero(9)), expected);
    }
}

#[test]
fn storage_unit_ranges_match_the_table() {
    // AM §1.3: tryte −9841…+9841, word ±3_812_798_742_493, word = 3 trytes.
    assert_eq!(Tint::max(9).to_i128(), Some(9841));
    assert_eq!(Tint::min(9).to_i128(), Some(-9841));
    assert_eq!(Tint::max(27).to_i128(), Some(3_812_798_742_493));
    assert_eq!(Tint::min(27).to_i128(), Some(-3_812_798_742_493));
    assert_eq!(Tint::zero(27).size_trytes(), 3);
}

// ------------------------------------------------------------- §3.1 arithmetic

#[test]
fn wrapping_is_congruence_modulo_three_to_the_n() {
    // AM §3.1: the unique n-trit value congruent to the exact result mod 3^n.
    for w in 1..=6u32 {
        let m = 3i128.pow(w);
        for a in -100i128..=100 {
            for b in -100i128..=100 {
                let (r, _) = Tint::wrapping(w, a)
                    .add(&Tint::wrapping(w, b), Flavor::Wrap)
                    .unwrap();
                let r = r.to_i128().unwrap();
                assert_eq!((a + b - r).rem_euclid(m), 0, "({a} + {b}) mod 3^{w}");
            }
        }
    }
}

#[test]
fn neg_and_abs_never_overflow_at_any_width() {
    // AM §3.1: total, hence flavorless.
    for w in [1u32, 2, 9, 27, 81, 243] {
        for x in [Tint::min(w), Tint::max(w), Tint::zero(w)] {
            assert_eq!(x.neg().neg(), x);
            assert_eq!(
                x.abs(),
                if x.sign().is_neg() {
                    x.neg()
                } else {
                    x.clone()
                }
            );
            assert_eq!(x.abs().width(), w);
        }
    }
}

// ---------------------------------------------------------- §3.2 division

#[test]
fn the_division_identity_always_holds() {
    // AM §3.2: a == div(a,b)*b + rem(a,b), with |r| <= |b|/2.
    for a in -500i128..=500 {
        for b in [-64i128, -10, -7, -2, -1, 1, 2, 7, 10, 64] {
            let (x, y) = (t(a), t(b));
            let (q, r) = (x.div(&y).unwrap(), x.rem(&y).unwrap());
            assert_eq!(
                q.mul(&y, Flavor::Trap)
                    .unwrap()
                    .0
                    .add(&r, Flavor::Trap)
                    .unwrap()
                    .0,
                x,
                "{a} = q*{b} + r"
            );
            assert!(2 * r.to_i128().unwrap().abs() <= b.abs());
        }
    }
}

#[test]
fn the_tie_break_preserves_the_symmetry_identities() {
    // AM §3.2 states these as the *reason* ties round away from zero:
    //   div(−a, b) = div(a, −b) = −div(a, b)
    //   rem(−a, b) = −rem(a, b)
    for a in -400i128..=400 {
        for b in [-8i128, -6, -4, -3, -2, -1, 1, 2, 3, 4, 6, 8] {
            let (x, y) = (t(a), t(b));
            let q = x.div(&y).unwrap();
            assert_eq!(x.neg().div(&y).unwrap(), q.neg(), "div(−{a}, {b})");
            assert_eq!(x.div(&y.neg()).unwrap(), q.neg(), "div({a}, −{b})");
            let r = x.rem(&y).unwrap();
            assert_eq!(x.neg().rem(&y).unwrap(), r.neg(), "rem(−{a}, {b})");
        }
    }
}

#[test]
fn ties_round_away_from_zero_and_only_even_divisors_have_them() {
    assert_eq!(t(7).div(&t(2)).unwrap().to_i128(), Some(4)); // 3.5 → 4
    assert_eq!(t(-7).div(&t(2)).unwrap().to_i128(), Some(-4)); // −3.5 → −4
    assert_eq!(t(1).div(&t(2)).unwrap().to_i128(), Some(1)); // 0.5 → 1
    for a in -300i128..=300 {
        for b in [-9i128, -3, -1, 1, 3, 9] {
            // Odd divisor: |r| = |b|/2 is unreachable, so no tie ever arises.
            let r = t(a).rem(&t(b)).unwrap().to_i128().unwrap();
            assert!(2 * r.abs() < b.abs() || b.abs() == 1);
        }
    }
}

#[test]
fn div_min_by_minus_one_is_max() {
    // AM §3.2: "another entire class of binary-world faults that does not
    // exist here".
    for w in [1u32, 9, 27, 81] {
        let neg1 = Tint::new(w, -1).unwrap();
        assert_eq!(Tint::min(w).div(&neg1).unwrap(), Tint::max(w));
        assert_eq!(Tint::max(w).div(&neg1).unwrap(), Tint::min(w));
    }
}

// ------------------------------------------------------------- §3.3 shifts

#[test]
fn shr_is_round_to_nearest_division_and_never_ties() {
    // AM §3.3: 3^k is odd, so the tie-break is never invoked.
    for a in -2000i128..=2000 {
        for k in 0..7u32 {
            let p = 3i128.pow(k);
            assert_eq!(t(a).shr(k).unwrap(), t(a).div(&t(p)).unwrap(), "{a} >> {k}");
            let r = t(a).rem(&t(p)).unwrap().to_i128().unwrap();
            assert!(2 * r.abs() < p || p == 1, "{a} rem 3^{k} hit a tie");
        }
    }
}

#[test]
fn shift_amounts_outside_the_width_fault_rather_than_mask() {
    // AM §3.3: "not masked, not undefined".
    for w in [1u32, 9, 27] {
        let x = Tint::new(w, 1).unwrap();
        assert!(x.shl(w - 1, Flavor::Wrap).is_ok());
        assert!(x.shl(w, Flavor::Wrap).is_err());
        assert!(x.shl(w + 1, Flavor::Wrap).is_err());
        assert!(x.shr(w).is_err());
    }
}

// --------------------------------------------------------- §3.4 trit-wise

#[test]
fn tneg_on_a_word_coincides_with_arithmetic_neg() {
    // AM §3.4: "they are the same operation".
    for x in sample9() {
        assert_eq!(x.tneg(), x.neg());
    }
}

#[test]
fn the_tritwise_set_is_positionwise() {
    for a in sample9() {
        for b in sample9() {
            for i in 0..9 {
                assert_eq!(a.tmin(&b).trit(i), a.trit(i).tmin(b.trit(i)));
                assert_eq!(a.tmax(&b).trit(i), a.trit(i).tmax(b.trit(i)));
                assert_eq!(a.tmul(&b).trit(i), a.trit(i).tmul(b.trit(i)));
            }
        }
    }
}

// ------------------------------------------------------- Appendix A identities

#[test]
fn a1_negation_is_an_involution() {
    for x in sample9() {
        assert_eq!(x.neg().neg(), x);
        assert_eq!(x.tneg().tneg(), x);
    }
}

#[test]
fn a2_abs_is_tmul_with_the_sign() {
    // abs(x) = tmul(x, sign(x)) — branch-free, no overflow case.
    for x in sample9() {
        let sign = Tint::splat(9, x.sign());
        assert_eq!(x.tmul(&sign), x.abs(), "abs({x})");
    }
}

#[test]
fn a3_shr_undoes_a_non_overflowing_shl() {
    for a in -300i128..=300 {
        for k in 0..5u32 {
            let x = t(a);
            let (shifted, over) = x.shl(k, Flavor::Wrap).unwrap();
            if over.is_zero() {
                assert_eq!(shifted.shr(k).unwrap(), x, "shr(shl({a}, {k}), {k})");
            }
        }
    }
}

#[test]
fn a4_wrapping_arithmetic_commutes_with_negation() {
    // wrap(x + y) = neg(wrap(neg(x) + neg(y))) — the identity two's
    // complement cannot offer, because MIN breaks its symmetry.
    // Exhaustive at the narrow widths, where "exhaustive" is cheap; a t9 pass
    // would be 19683² pairs, so it walks a stride instead.
    for w in [1u32, 2, 3, 9] {
        let max = (3i128.pow(w) - 1) / 2;
        let step = if w >= 9 { 97 } else { 1 };
        for a in (-max..=max).step_by(step) {
            for b in (-max..=max).step_by(step) {
                let (x, y) = (Tint::new(w, a).unwrap(), Tint::new(w, b).unwrap());
                let lhs = x.add(&y, Flavor::Wrap).unwrap().0;
                let rhs = x.neg().add(&y.neg(), Flavor::Wrap).unwrap().0.neg();
                assert_eq!(lhs, rhs, "{a} + {b} at t{w}");
            }
        }
    }
}

#[test]
fn a5_the_least_significant_trit_is_the_remainder_mod_three() {
    for a in -2000i128..=2000 {
        let x = t(a);
        assert_eq!(
            x.rem(&t(3)).unwrap().to_i128(),
            Some(x.trit(0).to_i8() as i128),
            "rem({a}, 3)"
        );
    }
}
