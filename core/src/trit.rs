//! The trit: the three-valued unit of information (Naming §4).

use core::fmt;

/// A trit — −1, 0, or +1.
///
/// Written `T`, `0`, `1` in trit strings (Naming §3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(i8)]
pub enum Trit {
    /// −1, written `T`.
    Neg = -1,
    /// 0, written `0`.
    #[default]
    Zero = 0,
    /// +1, written `1`.
    Pos = 1,
}

impl Trit {
    /// Every trit value, in ascending order.
    pub const ALL: [Trit; 3] = [Trit::Neg, Trit::Zero, Trit::Pos];

    /// The trit with the given value, or `None` if `v` is not in −1..=1.
    pub const fn from_i8(v: i8) -> Option<Trit> {
        match v {
            -1 => Some(Trit::Neg),
            0 => Some(Trit::Zero),
            1 => Some(Trit::Pos),
            _ => None,
        }
    }

    /// The numeric value, −1, 0 or +1.
    pub const fn to_i8(self) -> i8 {
        self as i8
    }

    /// The character used in trit strings: `T`, `0`, `1`.
    pub const fn to_char(self) -> char {
        match self {
            Trit::Neg => 'T',
            Trit::Zero => '0',
            Trit::Pos => '1',
        }
    }

    /// Parse one trit-string character. `t` is accepted as a lowercase `T`.
    pub const fn from_char(c: char) -> Option<Trit> {
        match c {
            'T' | 't' => Some(Trit::Neg),
            '0' => Some(Trit::Zero),
            '1' => Some(Trit::Pos),
            _ => None,
        }
    }

    /// Trit-wise negation (AM §3.4). Total, like every negation in Trust.
    pub const fn tneg(self) -> Trit {
        match self {
            Trit::Neg => Trit::Pos,
            Trit::Zero => Trit::Zero,
            Trit::Pos => Trit::Neg,
        }
    }

    /// Trit-wise minimum (AM §3.4) — the ternary AND analogue.
    pub const fn tmin(self, other: Trit) -> Trit {
        if (self as i8) <= (other as i8) {
            self
        } else {
            other
        }
    }

    /// Trit-wise maximum (AM §3.4) — the ternary OR analogue.
    pub const fn tmax(self, other: Trit) -> Trit {
        if (self as i8) >= (other as i8) {
            self
        } else {
            other
        }
    }

    /// Trit-wise multiplication (AM §3.4) — the ternary XOR analogue.
    pub const fn tmul(self, other: Trit) -> Trit {
        match (self, other) {
            (Trit::Zero, _) | (_, Trit::Zero) => Trit::Zero,
            (a, b) if (a as i8) == (b as i8) => Trit::Pos,
            _ => Trit::Neg,
        }
    }

    /// `true` iff this trit is +1 (Types Ch. 1 §6, `is_pos`).
    pub const fn is_pos(self) -> bool {
        matches!(self, Trit::Pos)
    }

    /// `true` iff this trit is 0 (Types Ch. 1 §6, `is_zero`).
    pub const fn is_zero(self) -> bool {
        matches!(self, Trit::Zero)
    }

    /// `true` iff this trit is −1 (Types Ch. 1 §6, `is_neg`).
    pub const fn is_neg(self) -> bool {
        matches!(self, Trit::Neg)
    }
}

impl fmt::Display for Trit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Trit::Neg => "T",
            Trit::Zero => "0",
            Trit::Pos => "1",
        })
    }
}

impl fmt::Debug for Trit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}t", self.to_i8())
    }
}

impl From<Trit> for i8 {
    fn from(t: Trit) -> i8 {
        t.to_i8()
    }
}

impl From<Trit> for i128 {
    fn from(t: Trit) -> i128 {
        t.to_i8() as i128
    }
}

/// Trits per tryte (Naming §4). The tryte is the smallest addressable unit.
pub const TRITS_PER_TRYTE: u32 = 9;

/// Trits per heptavintimal digit (Types Ch. 1 §3).
pub const TRITS_PER_HEPT: u32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_arithmetic() {
        for a in Trit::ALL {
            for b in Trit::ALL {
                assert_eq!(a.tmin(b).to_i8(), a.to_i8().min(b.to_i8()));
                assert_eq!(a.tmax(b).to_i8(), a.to_i8().max(b.to_i8()));
                assert_eq!(a.tmul(b).to_i8(), a.to_i8() * b.to_i8());
            }
            assert_eq!(a.tneg().to_i8(), -a.to_i8());
        }
    }

    #[test]
    fn char_roundtrip() {
        for a in Trit::ALL {
            assert_eq!(Trit::from_char(a.to_char()), Some(a));
        }
    }
}
