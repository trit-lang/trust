//! Width-typed balanced ternary integers and the overflow flavors.
//!
//! [`Tint`] is a value of TIR type `tN` (TIR §2): an exact [`Bt`] together
//! with the width it is known to fit in. Every operation that can leave the
//! width comes in the three flavors of AM §3.1 / TIR §3, selected by
//! [`Flavor`].

use crate::bt::Bt;
use crate::fault::{Fault, FaultCode};
use crate::trit::Trit;
use core::fmt;

/// The maximum `tN` width a module may use (TIR §2, default cap).
pub const MAX_WIDTH: u32 = 243;

/// A balanced ternary integer of a fixed trit width — a TIR `tN` value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Tint {
    width: u32,
    v: Bt,
}

/// The overflow-behavior variant of an arithmetic operation (Naming §4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Flavor {
    /// `.wrap` — wrap into the symmetric range.
    #[default]
    Wrap,
    /// `.trap` — raise `F_OVERFLOW`.
    Trap,
    /// `.flag` — yield the wrapped value plus an overflow trit.
    Flag,
}

impl Flavor {
    /// The TIR suffix spelling.
    pub fn suffix(self) -> &'static str {
        match self {
            Flavor::Wrap => ".wrap",
            Flavor::Trap => ".trap",
            Flavor::Flag => ".flag",
        }
    }

    /// Parse a TIR suffix (without the dot).
    pub fn from_name(s: &str) -> Option<Flavor> {
        match s {
            "wrap" => Some(Flavor::Wrap),
            "trap" => Some(Flavor::Trap),
            "flag" => Some(Flavor::Flag),
            _ => None,
        }
    }
}

impl Tint {
    /// A value of the given width, wrapping if it does not fit.
    pub fn wrapping(width: u32, v: impl Into<Bt>) -> Tint {
        assert!(width >= 1, "tN width must be at least 1");
        let v: Bt = v.into();
        Tint {
            width,
            v: v.wrap_to(width),
        }
    }

    /// A value of the given width, or `None` if it does not fit — the
    /// constant-out-of-range check (Types Ch. 1 §3: "never a silent wrap").
    pub fn new(width: u32, v: impl Into<Bt>) -> Option<Tint> {
        let v: Bt = v.into();
        (width >= 1 && v.fits_width(width)).then_some(Tint { width, v })
    }

    /// Zero of the given width.
    pub fn zero(width: u32) -> Tint {
        Tint { width, v: Bt::ZERO }
    }

    /// The largest value of this width: (3^width − 1)/2.
    pub fn max(width: u32) -> Tint {
        Tint {
            width,
            v: Bt::max_of_width(width),
        }
    }

    /// The smallest value of this width. `MIN == -MAX`.
    pub fn min(width: u32) -> Tint {
        Tint {
            width,
            v: Bt::min_of_width(width),
        }
    }

    /// The trit width.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The exact value.
    pub fn value(&self) -> &Bt {
        &self.v
    }

    /// The value as `i128`, if it fits.
    pub fn to_i128(&self) -> Option<i128> {
        self.v.to_i128()
    }

    /// True iff zero.
    pub fn is_zero(&self) -> bool {
        self.v.is_zero()
    }

    /// The sign as a trit: `self <=> 0` (Types Ch. 1 §6, `sign`).
    ///
    /// This is the most significant nonzero trit — no subtraction and no
    /// comparison needed (AM §1.2, consequence 4).
    pub fn sign(&self) -> Trit {
        self.v.sign()
    }

    /// The same trit in every position of a `width`-trit value — the mask
    /// form of the trit-wise operations (see [`Bt::splat`]).
    pub fn splat(width: u32, t: Trit) -> Tint {
        Tint {
            width,
            v: Bt::splat(t, width),
        }
    }

    /// The trit at position `i`.
    pub fn trit(&self, i: u32) -> Trit {
        self.v.trit(i)
    }

    /// Storage size in trytes (Types Ch. 1 §7): ⌈width/9⌉.
    pub fn size_trytes(&self) -> u32 {
        self.width.div_ceil(9).max(1)
    }

    fn same_width(&self, other: &Tint, op: &str) {
        assert_eq!(
            self.width, other.width,
            "mixed-width {op}: t{} and t{} (Types Ch. 1, P2)",
            self.width, other.width
        );
    }

    /// Apply an overflow flavor to an exact result.
    ///
    /// The overflow trit is the direction of the overflow: +1 if the exact
    /// value exceeded MAX, −1 if it fell below MIN, 0 otherwise. For `add`
    /// and `sub` this is exactly the carry out of the top trit, which is what
    /// legalization's carry chaining consumes (TIR §6).
    fn apply(width: u32, exact: Bt, flavor: Flavor) -> Result<(Tint, Trit), Fault> {
        let fits = exact.fits_width(width);
        let over = if fits { Trit::Zero } else { exact.sign() };
        match flavor {
            Flavor::Trap if !fits => Err(Fault::new(FaultCode::Overflow)),
            _ => Ok((
                Tint {
                    width,
                    v: exact.wrap_to(width),
                },
                over,
            )),
        }
    }

    /// `add<fl>` (TIR §3.1).
    pub fn add(&self, other: &Tint, flavor: Flavor) -> Result<(Tint, Trit), Fault> {
        self.same_width(other, "add");
        Tint::apply(self.width, self.v.add(&other.v), flavor)
    }

    /// `sub<fl>` (TIR §3.1).
    pub fn sub(&self, other: &Tint, flavor: Flavor) -> Result<(Tint, Trit), Fault> {
        self.same_width(other, "sub");
        Tint::apply(self.width, self.v.sub(&other.v), flavor)
    }

    /// `mul<fl>` (TIR §3.1).
    pub fn mul(&self, other: &Tint, flavor: Flavor) -> Result<(Tint, Trit), Fault> {
        self.same_width(other, "mul");
        Tint::apply(self.width, self.v.mul(&other.v), flavor)
    }

    /// `neg` (TIR §3.1). Total — no flavor, and no `checked_neg` to lie with
    /// (Types Ch. 1 §4).
    pub fn neg(&self) -> Tint {
        Tint {
            width: self.width,
            v: self.v.neg(),
        }
    }

    /// `abs`. Total (Types Ch. 1 §1, P1).
    pub fn abs(&self) -> Tint {
        Tint {
            width: self.width,
            v: self.v.abs(),
        }
    }

    /// `div` (TIR §3.1): round to nearest, ties away from zero. Total except
    /// division by zero, which faults `F_DIVZERO`.
    ///
    /// Cannot overflow: `|q| ≤ |a|` for every `|b| ≥ 1`, and `MIN/−1 = MAX`
    /// because the range is symmetric.
    pub fn div(&self, other: &Tint) -> Result<Tint, Fault> {
        self.same_width(other, "div");
        let (q, _) = self
            .v
            .divrem(&other.v)
            .ok_or_else(|| Fault::new(FaultCode::DivZero))?;
        debug_assert!(q.fits_width(self.width));
        Ok(Tint {
            width: self.width,
            v: q,
        })
    }

    /// `rem` (TIR §3.1): `|r| ≤ |b|/2`, the remainder matching [`Tint::div`].
    pub fn rem(&self, other: &Tint) -> Result<Tint, Fault> {
        self.same_width(other, "rem");
        let (_, r) = self
            .v
            .divrem(&other.v)
            .ok_or_else(|| Fault::new(FaultCode::DivZero))?;
        Ok(Tint {
            width: self.width,
            v: r,
        })
    }

    /// `shl<fl>` (TIR §3.1): `self * 3^k`. `k` outside `0..width` faults
    /// `F_SHIFT`.
    pub fn shl(&self, k: u32, flavor: Flavor) -> Result<(Tint, Trit), Fault> {
        self.check_shift(k)?;
        Tint::apply(self.width, self.v.shl(k), flavor)
    }

    /// `shr` (TIR §3.1): `self / 3^k`, exact round-to-nearest, total in value.
    /// `k` outside `0..width` faults `F_SHIFT`.
    pub fn shr(&self, k: u32) -> Result<Tint, Fault> {
        self.check_shift(k)?;
        Ok(Tint {
            width: self.width,
            v: self.v.shr(k),
        })
    }

    fn check_shift(&self, k: u32) -> Result<(), Fault> {
        if k < self.width {
            Ok(())
        } else {
            Err(Fault::new(FaultCode::Shift))
        }
    }

    /// `cmp` (TIR §3.3): the three-way comparison, the only comparison.
    pub fn cmp3(&self, other: &Tint) -> Trit {
        self.same_width(other, "cmp");
        self.v.cmp3(&other.v)
    }

    /// `widen` (TIR §3.5): value-preserving. There is one extension, because
    /// there is no unsigned type to zero-extend for.
    pub fn widen(&self, to: u32) -> Tint {
        assert!(to >= self.width, "widen must not narrow");
        Tint {
            width: to,
            v: self.v.clone(),
        }
    }

    /// `trunc` (TIR §3.5): wraps into the narrow symmetric range.
    pub fn trunc(&self, to: u32) -> Tint {
        assert!(to <= self.width && to >= 1, "trunc must narrow");
        Tint {
            width: to,
            v: self.v.wrap_to(to),
        }
    }

    /// Checked narrowing: `None` when the value does not fit (the library
    /// `checked` narrowing of Types Ch. 1 §6).
    pub fn checked_trunc(&self, to: u32) -> Option<Tint> {
        self.v.fits_width(to).then(|| self.trunc(to))
    }

    /// `tmin` (TIR §3.2): trit-wise minimum.
    pub fn tmin(&self, other: &Tint) -> Tint {
        self.tritwise(other, Trit::tmin, "tmin")
    }

    /// `tmax` (TIR §3.2): trit-wise maximum.
    pub fn tmax(&self, other: &Tint) -> Tint {
        self.tritwise(other, Trit::tmax, "tmax")
    }

    /// `tmul` (TIR §3.2): trit-wise multiplication.
    pub fn tmul(&self, other: &Tint) -> Tint {
        self.tritwise(other, Trit::tmul, "tmul")
    }

    /// `tneg` (TIR §3.2): an alias of `neg`, trit-wise negation.
    pub fn tneg(&self) -> Tint {
        self.neg()
    }

    fn tritwise(&self, other: &Tint, f: fn(Trit, Trit) -> Trit, op: &str) -> Tint {
        self.same_width(other, op);
        let v =
            Bt::from_trits_lsb((0..self.width).map(|i| f(self.v.trit(i), other.v.trit(i)).to_i8()));
        Tint {
            width: self.width,
            v,
        }
    }

    /// `select3` (TIR §3.3): pick by trit value.
    pub fn select3(t: Trit, neg: &Tint, zero: &Tint, pos: &Tint) -> Tint {
        match t {
            Trit::Neg => neg.clone(),
            Trit::Zero => zero.clone(),
            Trit::Pos => pos.clone(),
        }
    }

    /// Trit string of exactly `width` trits, MST first.
    pub fn to_trit_string(&self) -> String {
        self.v.to_trit_string(self.width)
    }

    /// Heptavintimal string covering the full width.
    pub fn to_hept_string(&self) -> String {
        self.v.to_hept_string(self.width)
    }
}

impl fmt::Display for Tint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.v.to_decimal())
    }
}

impl fmt::Debug for Tint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{} {}", self.width, self.v.to_decimal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t27(v: i128) -> Tint {
        Tint::new(27, v).expect("in range")
    }

    #[test]
    fn division_rounds_to_nearest_ties_away() {
        // Types Ch. 1 §4, consequence 1 — the numbers the spec spells out.
        assert_eq!(t27(7).div(&t27(2)).unwrap().to_i128(), Some(4));
        assert_eq!(t27(8).div(&t27(3)).unwrap().to_i128(), Some(3));
        assert_eq!(t27(-8).div(&t27(3)).unwrap().to_i128(), Some(-3));
        assert_eq!(t27(8).rem(&t27(3)).unwrap().to_i128(), Some(-1));
    }

    #[test]
    fn min_div_neg_one_is_max() {
        let min = Tint::min(9);
        let neg1 = Tint::new(9, -1).unwrap();
        assert_eq!(min.div(&neg1).unwrap(), Tint::max(9));
        assert_eq!(min.neg(), Tint::max(9));
    }

    #[test]
    fn shift_right_is_exact_rounding() {
        for a in -400i128..=400 {
            let x = t27(a);
            for k in 0..4u32 {
                let want = t27(round_nearest(a, 3i128.pow(k)));
                assert_eq!(x.shr(k).unwrap(), want, "{a} >> {k}");
            }
        }
    }

    fn round_nearest(a: i128, b: i128) -> i128 {
        let q = (a as f64) / (b as f64);
        let r = if q >= 0.0 {
            (q + 0.5).floor()
        } else {
            (q - 0.5).ceil()
        };
        r as i128
    }

    #[test]
    fn wrapping_is_negation_symmetric() {
        // Types Ch. 1 §6: (-x as t9) == -(x as t9).
        for a in [-1_000_000i128, -12345, -7, 0, 7, 12345, 1_000_000] {
            let x = t27(a);
            assert_eq!(x.neg().trunc(9), x.trunc(9).neg());
        }
    }

    #[test]
    fn flavors() {
        let max = Tint::max(9);
        let one = Tint::new(9, 1).unwrap();
        assert_eq!(
            max.add(&one, Flavor::Trap).unwrap_err().code,
            FaultCode::Overflow
        );
        let (w, o) = max.add(&one, Flavor::Wrap).unwrap();
        assert_eq!(w, Tint::min(9));
        assert_eq!(o, Trit::Pos);
        let (w, o) = max.add(&one, Flavor::Flag).unwrap();
        assert_eq!(w, Tint::min(9));
        assert_eq!(o, Trit::Pos);
    }

    #[test]
    fn carry_out_chains() {
        // TIR §6: the .flag overflow trit is the carry into the next part.
        let width = 9;
        let base = Bt::max_of_width(width)
            .mul(&Bt::from_i128(2))
            .add(&Bt::from_i128(1));
        for (a, b) in [(9000i128, 9000i128), (-9000, -9000), (9000, -50)] {
            let (lo, carry) = Tint::new(width, a)
                .unwrap()
                .add(&Tint::new(width, b).unwrap(), Flavor::Flag)
                .unwrap();
            let recombined = lo
                .value()
                .add(&Bt::from_i128(carry.to_i8() as i128).mul(&base));
            assert_eq!(recombined.to_i128(), Some(a + b));
        }
    }
}
