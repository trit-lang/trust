//! Unbounded balanced ternary integers.
//!
//! [`Bt`] is the exact-arithmetic substrate under the width-typed [`Tint`]
//! (`crate::int`). It exists because TIR permits arbitrary `tN` widths up to
//! 243 trits (TIR §2) and "constant folding must be correct at every N" —
//! which rules out an `i128` shortcut.
//!
//! Representation: a `Vec<i8>` of trits, **least significant trit first**,
//! normalized so the most significant trit is nonzero (the empty vector is
//! zero). This is the internal order; textual trit strings are written most
//! significant trit first (Naming §3).

use crate::trit::{TRITS_PER_HEPT, TRITS_PER_TRYTE, Trit};
use core::cmp::Ordering;
use core::fmt;

/// An arbitrary-precision balanced ternary integer.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Bt {
    /// Trits, LSB first, each in −1..=1, normalized (no trailing zeros).
    d: Vec<i8>,
}

impl Bt {
    /// Zero.
    pub const ZERO: Bt = Bt { d: Vec::new() };

    /// Build from LSB-first trits, normalizing.
    ///
    /// # Panics
    /// If any element is outside −1..=1.
    pub fn from_trits_lsb(trits: impl IntoIterator<Item = i8>) -> Bt {
        let d: Vec<i8> = trits.into_iter().collect();
        assert!(d.iter().all(|&t| (-1..=1).contains(&t)), "not a trit");
        let mut b = Bt { d };
        b.normalize();
        b
    }

    /// Build from a most-significant-first trit string body (`T`, `0`, `1`,
    /// `_`), as written after the `0t` prefix.
    pub fn from_trit_str(s: &str) -> Option<Bt> {
        let mut d = Vec::new();
        for c in s.chars().rev() {
            if c == '_' {
                continue;
            }
            d.push(Trit::from_char(c)?.to_i8());
        }
        if d.is_empty() {
            return None;
        }
        Some(Bt::from_trits_lsb(d))
    }

    fn normalize(&mut self) {
        while self.d.last() == Some(&0) {
            self.d.pop();
        }
    }

    /// The trit at position `i` (weight 3^`i`), or zero beyond the top.
    pub fn trit(&self, i: u32) -> Trit {
        Trit::from_i8(self.d.get(i as usize).copied().unwrap_or(0)).unwrap()
    }

    /// The trits, LSB first, without the leading zeros.
    pub fn trits_lsb(&self) -> impl Iterator<Item = Trit> + '_ {
        self.d.iter().map(|&t| Trit::from_i8(t).unwrap())
    }

    /// Number of significant trits — the smallest width that can hold this
    /// value. Zero has length 0.
    pub fn trit_len(&self) -> u32 {
        self.d.len() as u32
    }

    /// True iff this is zero.
    pub fn is_zero(&self) -> bool {
        self.d.is_empty()
    }

    /// The sign, as a trit: the AM three-way `cmp` against zero.
    pub fn sign(&self) -> Trit {
        match self.d.last() {
            None => Trit::Zero,
            Some(&t) => Trit::from_i8(t).unwrap(),
        }
    }

    /// Negation. Total for every value — the property P1 buys (Types §1).
    pub fn neg(&self) -> Bt {
        Bt {
            d: self.d.iter().map(|t| -t).collect(),
        }
    }

    /// Absolute value. Total (Types §1, P1).
    pub fn abs(&self) -> Bt {
        if self.sign().is_neg() {
            self.neg()
        } else {
            self.clone()
        }
    }

    /// Addition. Exact — no width, so no overflow.
    pub fn add(&self, other: &Bt) -> Bt {
        let n = self.d.len().max(other.d.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0i8;
        for i in 0..n {
            let a = self.d.get(i).copied().unwrap_or(0);
            let b = other.d.get(i).copied().unwrap_or(0);
            let (digit, c) = carry_norm(a + b + carry);
            out.push(digit);
            carry = c;
        }
        if carry != 0 {
            out.push(carry);
        }
        let mut r = Bt { d: out };
        r.normalize();
        r
    }

    /// Subtraction, `self - other`.
    pub fn sub(&self, other: &Bt) -> Bt {
        self.add(&other.neg())
    }

    /// Multiplication. Exact.
    pub fn mul(&self, other: &Bt) -> Bt {
        if self.is_zero() || other.is_zero() {
            return Bt::ZERO;
        }
        let mut acc = vec![0i8; self.d.len() + other.d.len() + 1];
        for (i, &b) in other.d.iter().enumerate() {
            if b == 0 {
                continue;
            }
            let mut carry = 0i8;
            for (j, &a) in self.d.iter().enumerate() {
                let (digit, c) = carry_norm(acc[i + j] + a * b + carry);
                acc[i + j] = digit;
                carry = c;
            }
            let mut k = i + self.d.len();
            while carry != 0 {
                let (digit, c) = carry_norm(acc[k] + carry);
                acc[k] = digit;
                carry = c;
                k += 1;
            }
        }
        let mut r = Bt { d: acc };
        r.normalize();
        r
    }

    /// Shift left by `k` trits: `self * 3^k` (AM §3.3). Exact.
    pub fn shl(&self, k: u32) -> Bt {
        if self.is_zero() {
            return Bt::ZERO;
        }
        let mut d = vec![0i8; k as usize];
        d.extend_from_slice(&self.d);
        Bt { d }
    }

    /// Shift right by `k` trits: `self / 3^k` (AM §3.3).
    ///
    /// Dropping the low `k` trits *is* round-to-nearest division by 3^k: the
    /// dropped tail has magnitude at most (3^k−1)/2 < 3^k/2, so no tie can
    /// arise (Types Ch. 1 §4, consequence 2). Total.
    pub fn shr(&self, k: u32) -> Bt {
        let k = k as usize;
        if k >= self.d.len() {
            return Bt::ZERO;
        }
        Bt {
            d: self.d[k..].to_vec(),
        }
    }

    /// The low `width` trits, i.e. the symmetric residue mod 3^`width`.
    ///
    /// This is both the wrapping overflow flavor (AM §3.1) and the `trunc`
    /// conversion (TIR §3.5). Negation-symmetric by construction.
    pub fn wrap_to(&self, width: u32) -> Bt {
        let w = width as usize;
        if w >= self.d.len() {
            return self.clone();
        }
        let mut r = Bt {
            d: self.d[..w].to_vec(),
        };
        r.normalize();
        r
    }

    /// The trits at or above position `width`: the part `wrap_to` discards.
    pub fn high_part(&self, width: u32) -> Bt {
        self.shr(width)
    }

    /// Three-way comparison (AM §3.5) — the only comparison there is.
    ///
    /// Scans from the most significant trit and stops at the first position
    /// where the two disagree: everything below is bounded by 3^i − 1 and so
    /// cannot outweigh a difference of at least 3^i. "Comparison does not
    /// require subtraction" (AM §1.2, consequence 4) — this is that.
    pub fn cmp3(&self, other: &Bt) -> Trit {
        let n = self.d.len().max(other.d.len());
        for i in (0..n as u32).rev() {
            let (a, b) = (self.trit(i).to_i8(), other.trit(i).to_i8());
            if a != b {
                return if a > b { Trit::Pos } else { Trit::Neg };
            }
        }
        Trit::Zero
    }

    /// Division: round to nearest, ties away from zero (Types Ch. 1 §4).
    /// Returns `(quotient, remainder)` with `a = q*b + r` and `|r| ≤ |b|/2`.
    ///
    /// Returns `None` for `b == 0` (the caller raises `F_DIVZERO`).
    pub fn divrem(&self, other: &Bt) -> Option<(Bt, Bt)> {
        if other.is_zero() {
            return None;
        }
        if self.is_zero() {
            return Some((Bt::ZERO, Bt::ZERO));
        }

        // Long division, most significant trit first, Horner-style: each step
        // shifts the quotient up one trit and adds the digit that leaves the
        // smallest remainder, maintaining |r| ≤ |b|/2.
        //
        // The digit is drawn from −2..=2, not from {−1, 0, +1}. One trit is
        // *not* always enough: the invariant permits |3r + d| = 3|b|/2 + 1
        // (reachable whenever |b| is even), which no single trit can pull back
        // under |b|/2 — and the resulting error compounds down the rest of the
        // quotient. Accumulating the quotient as a value rather than as a trit
        // string makes an out-of-range digit a non-event: adding 2 simply
        // carries into the trits already emitted.
        let candidates: [(i8, Bt); 5] = [
            (-2, other.mul(&Bt::from_i128(-2))),
            (-1, other.neg()),
            (0, Bt::ZERO),
            (1, other.clone()),
            (2, other.mul(&Bt::from_i128(2))),
        ];
        let mut q = Bt::ZERO;
        let mut r = Bt::ZERO;
        for i in (0..self.d.len()).rev() {
            r = r.shl(1).add(&Bt::from_i128(self.d[i] as i128));
            let (digit, rem) = candidates
                .iter()
                // rem = r − digit*b, so subtract the precomputed digit*b.
                .map(|(k, kb)| (*k, r.sub(kb)))
                .min_by(|(_, x), (_, y)| x.abs().cmp(&y.abs()))
                .expect("candidate set is non-empty");
            q = q.shl(1).add(&Bt::from_i128(digit as i128));
            r = rem;
        }

        // Tie (|r| == |b|/2, possible only for even b): the rule is "away from
        // zero", so step one further out when the fractional part points the
        // same way as the true quotient's sign.
        let two_r = r.abs().mul(&Bt::from_i128(2));
        if two_r.cmp3(&other.abs()).is_zero() {
            let s = r.sign().tmul(other.sign());
            let v_sign = self.sign().tmul(other.sign());
            if s == v_sign && !s.is_zero() {
                let s = Bt::from_i128(s.to_i8() as i128);
                q = q.add(&s);
                r = r.sub(&other.mul(&s));
            }
        }
        Some((q, r))
    }

    /// Value from an `i128`.
    pub fn from_i128(mut v: i128) -> Bt {
        let mut d = Vec::new();
        while v != 0 {
            let mut rem = (v % 3) as i8;
            v /= 3;
            if rem == 2 {
                rem = -1;
                v += 1;
            } else if rem == -2 {
                rem = 1;
                v -= 1;
            }
            d.push(rem);
        }
        Bt { d }
    }

    /// Value as an `i128`, or `None` if it does not fit.
    pub fn to_i128(&self) -> Option<i128> {
        let mut acc: i128 = 0;
        for &t in self.d.iter().rev() {
            acc = acc.checked_mul(3)?.checked_add(t as i128)?;
        }
        Some(acc)
    }

    /// The same trit in every one of `width` positions.
    ///
    /// The trit-wise operations take this as a mask: `tmul` with a splat sign
    /// is branch-free `abs` (AM Appendix A.2), and `tmul` with a run of `1`s
    /// masks a value down to a narrower width without leaving the wide one —
    /// which is how legalization normalizes a promoted value.
    pub fn splat(t: Trit, width: u32) -> Bt {
        Bt::from_trits_lsb(std::iter::repeat_n(t.to_i8(), width as usize))
    }

    /// The largest value representable in `width` trits: (3^width − 1)/2.
    pub fn max_of_width(width: u32) -> Bt {
        Bt {
            d: vec![1; width as usize],
        }
    }

    /// The smallest value representable in `width` trits: −(3^width − 1)/2.
    ///
    /// `MIN == -MAX` — the symmetric range (Naming §4).
    pub fn min_of_width(width: u32) -> Bt {
        Bt {
            d: vec![-1; width as usize],
        }
    }

    /// True iff this value fits in `width` trits.
    pub fn fits_width(&self, width: u32) -> bool {
        self.trit_len() <= width
    }

    /// Trit string, most significant trit first, `width` trits wide
    /// (zero-padded). Used for `0t` literals and debug output.
    pub fn to_trit_string(&self, width: u32) -> String {
        let width = width.max(1);
        (0..width).rev().map(|i| self.trit(i).to_char()).collect()
    }

    /// Trit string with no leading zeros (`0` for zero).
    pub fn to_trit_string_min(&self) -> String {
        self.to_trit_string(self.trit_len())
    }

    /// Heptavintimal (base 27) string, most significant digit first, covering
    /// `width` trits rounded up to a multiple of 3.
    ///
    /// See `docs/spec-gaps.md`: each digit encodes one balanced 3-trit group
    /// with value −13..=13, so `0hD == 0`.
    pub fn to_hept_string(&self, width: u32) -> String {
        let digits = width.div_ceil(TRITS_PER_HEPT).max(1);
        (0..digits)
            .rev()
            .map(|i| {
                let lo = i * TRITS_PER_HEPT;
                let v = self.trit(lo).to_i8() as i32
                    + 3 * self.trit(lo + 1).to_i8() as i32
                    + 9 * self.trit(lo + 2).to_i8() as i32;
                hept_char(v).unwrap()
            })
            .collect()
    }

    /// Heptavintimal string with no leading `D`s (`D` for zero).
    pub fn to_hept_string_min(&self) -> String {
        self.to_hept_string(self.trit_len())
    }

    /// Number of trytes needed to store this value: ⌈trit_len / 9⌉, at least 1.
    pub fn tryte_len(&self) -> u32 {
        self.trit_len().div_ceil(TRITS_PER_TRYTE).max(1)
    }

    /// Decimal string.
    pub fn to_decimal(&self) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        let neg = self.sign().is_neg();
        let mut mag = self.abs();
        let ten = Bt::from_i128(10);
        let mut digits = Vec::new();
        while !mag.is_zero() {
            let (q, r) = mag.divrem(&ten).expect("ten is nonzero");
            // divrem rounds to nearest; step back to truncation so the
            // remainder is a real decimal digit in 0..=9.
            let (q, r) = if r.sign().is_neg() {
                (q.sub(&Bt::from_i128(1)), r.add(&ten))
            } else {
                (q, r)
            };
            digits.push(b'0' + r.to_i128().unwrap() as u8);
            mag = q;
        }
        let mut s = String::new();
        if neg {
            s.push('-');
        }
        s.extend(digits.iter().rev().map(|&b| b as char));
        s
    }

    /// Parse a decimal digit string (no sign, no prefix; `_` allowed).
    pub fn from_decimal(s: &str) -> Option<Bt> {
        let ten = Bt::from_i128(10);
        let mut acc = Bt::ZERO;
        let mut any = false;
        for c in s.chars() {
            if c == '_' {
                continue;
            }
            let v = c.to_digit(10)?;
            acc = acc.mul(&ten).add(&Bt::from_i128(v as i128));
            any = true;
        }
        any.then_some(acc)
    }

    /// Parse a heptavintimal digit string (no sign, no prefix; `_` allowed),
    /// most significant digit first, each digit a balanced 3-trit group.
    pub fn from_hept_str(s: &str) -> Option<Bt> {
        let mut d = Vec::new();
        for c in s.chars().rev() {
            if c == '_' {
                continue;
            }
            let mut v = hept_value(c)?;
            for _ in 0..TRITS_PER_HEPT {
                // Least significant trit of the balanced group first.
                let mut rem = v % 3;
                v /= 3;
                if rem == 2 {
                    rem = -1;
                    v += 1;
                } else if rem == -2 {
                    rem = 1;
                    v -= 1;
                }
                d.push(rem as i8);
            }
        }
        if d.is_empty() {
            return None;
        }
        Some(Bt::from_trits_lsb(d))
    }
}

/// Normalize a sum in −4..=4 into (trit, carry).
fn carry_norm(s: i8) -> (i8, i8) {
    debug_assert!((-4..=4).contains(&s));
    match s {
        -4 => (-1, -1),
        -3 => (0, -1),
        -2 => (1, -1),
        4 => (1, 1),
        3 => (0, 1),
        2 => (-1, 1),
        d => (d, 0),
    }
}

/// The heptavintimal character for a balanced digit value in −13..=13.
pub fn hept_char(v: i32) -> Option<char> {
    let idx = v + 13;
    match idx {
        0..=9 => char::from_u32(b'0' as u32 + idx as u32),
        10..=26 => char::from_u32(b'A' as u32 + (idx - 10) as u32),
        _ => None,
    }
}

/// The balanced digit value in −13..=13 for a heptavintimal character.
pub fn hept_value(c: char) -> Option<i32> {
    let idx = match c {
        '0'..='9' => c as i32 - '0' as i32,
        'A'..='Q' => c as i32 - 'A' as i32 + 10,
        'a'..='q' => c as i32 - 'a' as i32 + 10,
        _ => return None,
    };
    Some(idx - 13)
}

impl fmt::Display for Bt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_decimal())
    }
}

impl fmt::Debug for Bt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (0t{})", self.to_decimal(), self.to_trit_string_min())
    }
}

impl PartialOrd for Bt {
    fn partial_cmp(&self, other: &Bt) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bt {
    fn cmp(&self, other: &Bt) -> Ordering {
        match self.cmp3(other) {
            Trit::Neg => Ordering::Less,
            Trit::Zero => Ordering::Equal,
            Trit::Pos => Ordering::Greater,
        }
    }
}

impl From<i128> for Bt {
    fn from(v: i128) -> Bt {
        Bt::from_i128(v)
    }
}

impl From<i64> for Bt {
    fn from(v: i64) -> Bt {
        Bt::from_i128(v as i128)
    }
}

impl From<i32> for Bt {
    fn from(v: i32) -> Bt {
        Bt::from_i128(v as i128)
    }
}

impl From<Trit> for Bt {
    fn from(t: Trit) -> Bt {
        Bt::from_i128(t.to_i8() as i128)
    }
}
