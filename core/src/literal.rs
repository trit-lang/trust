//! Numeric literals (Types Ch. 1 §3).
//!
//! Three radices, all accepting `_` separators:
//!
//! | Form | Example | |
//! |---|---|---|
//! | decimal | `-9_841` | usual meaning |
//! | balanced ternary, `0t` | `0t1T0` = 6 | one character per trit, MST first |
//! | heptavintimal, `0h` | `0hD` = 0 | one character per 3 trits |
//!
//! Plus `trit` literals with the `t` suffix: `-1t`, `0t`, `1t`.
//!
//! There is no hexadecimal or octal: both are artifacts of the bit.

use crate::bt::Bt;
use crate::trit::Trit;
use core::fmt;

/// The radix a literal was written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Radix {
    /// Decimal.
    Dec,
    /// Balanced ternary, prefix `0t`.
    Ternary,
    /// Heptavintimal (base 27), prefix `0h`.
    Hept,
}

impl Radix {
    /// The literal prefix, if any.
    pub fn prefix(self) -> &'static str {
        match self {
            Radix::Dec => "",
            Radix::Ternary => "0t",
            Radix::Hept => "0h",
        }
    }
}

/// A parsed numeric literal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Literal {
    /// A `trit`-typed literal (`-1t`, `0t`, `1t`).
    Trit(Trit),
    /// An integer literal; its type is inferred, defaulting to `t27`.
    Int {
        /// The exact value.
        value: Bt,
        /// How it was written.
        radix: Radix,
    },
}

/// Why a literal failed to parse.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LitError {
    /// The token was empty or only a sign/prefix/separators.
    Empty,
    /// A character not valid in the chosen radix.
    BadDigit(char),
    /// A `0b`, `0o` or `0x` prefix — a radix from the binary world.
    ///
    /// Worth its own variant rather than a generic bad-digit report: it is the
    /// mistake a newcomer makes first, and the answer is not "that digit is
    /// wrong" but "that radix does not exist here". The assembly language
    /// specification requires exactly this diagnostic (ISA Assembly §2.2).
    BinaryWorldRadix(char),
}

impl fmt::Display for LitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LitError::Empty => f.write_str("empty numeric literal"),
            LitError::BadDigit(c) => write!(f, "invalid digit `{c}` in numeric literal"),
            LitError::BinaryWorldRadix(c) => {
                let name = match c {
                    'b' => "binary",
                    'o' => "octal",
                    _ => "hexadecimal",
                };
                write!(
                    f,
                    "there is no {name} literal (`0{c}`): {name} is an artifact of the bit. \
                     Use decimal, `0t` for balanced ternary, or `0h` for heptavintimal"
                )
            }
        }
    }
}

impl std::error::Error for LitError {}

/// Parse a complete literal token, including any leading `-`.
///
/// The `0t` prefix and the `trit` literal suffix collide by construction
/// (`0t` is both); this parser resolves it by maximal munch, so `0t` followed
/// by at least one of `0 1 T _` is a balanced ternary literal and a bare `0t`
/// is the trit zero. See `docs/spec-gaps.md`.
pub fn parse_literal(s: &str) -> Result<Literal, LitError> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if body.is_empty() {
        return Err(LitError::Empty);
    }

    // trit literals: `1t`, `0t` (bare), and their negations.
    if let Some(digits) = body.strip_suffix('t')
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_')
        && !digits.starts_with("0t")
    {
        let v = Bt::from_decimal(digits).ok_or(LitError::Empty)?;
        let v = if negative { v.neg() } else { v };
        if let Some(t) = v
            .to_i128()
            .and_then(|n| i8::try_from(n).ok())
            .and_then(Trit::from_i8)
        {
            return Ok(Literal::Trit(t));
        }
        return Err(LitError::BadDigit('t'));
    }

    // Reject the binary world's radices by name before anything else, so the
    // diagnostic names the real problem.
    if let Some(marker) = body
        .strip_prefix('0')
        .and_then(|rest| rest.chars().next())
        .filter(|c| matches!(c.to_ascii_lowercase(), 'b' | 'o' | 'x'))
    {
        return Err(LitError::BinaryWorldRadix(marker.to_ascii_lowercase()));
    }

    let (radix, digits) = if let Some(rest) = body.strip_prefix("0t") {
        (Radix::Ternary, rest)
    } else if let Some(rest) = body.strip_prefix("0h") {
        (Radix::Hept, rest)
    } else {
        (Radix::Dec, body)
    };

    if digits.chars().all(|c| c == '_') {
        return Err(LitError::Empty);
    }
    let value = match radix {
        Radix::Dec => {
            if let Some(c) = digits.chars().find(|c| !c.is_ascii_digit() && *c != '_') {
                return Err(LitError::BadDigit(c));
            }
            Bt::from_decimal(digits)
        }
        Radix::Ternary => {
            if let Some(c) = digits
                .chars()
                .find(|c| Trit::from_char(*c).is_none() && *c != '_')
            {
                return Err(LitError::BadDigit(c));
            }
            Bt::from_trit_str(digits)
        }
        Radix::Hept => {
            if let Some(c) = digits
                .chars()
                .find(|c| crate::bt::hept_value(*c).is_none() && *c != '_')
            {
                return Err(LitError::BadDigit(c));
            }
            Bt::from_hept_str(digits)
        }
    }
    .ok_or(LitError::Empty)?;

    Ok(Literal::Int {
        value: if negative { value.neg() } else { value },
        radix,
    })
}

/// Render a value in the given radix, with prefix, minimal width.
pub fn render(value: &Bt, radix: Radix) -> String {
    match radix {
        Radix::Dec => value.to_decimal(),
        Radix::Ternary => format!("0t{}", value.to_trit_string_min()),
        Radix::Hept => format!("0h{}", value.to_hept_string_min()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(s: &str) -> i128 {
        match parse_literal(s).unwrap() {
            Literal::Int { value, .. } => value.to_i128().unwrap(),
            other => panic!("expected integer, got {other:?}"),
        }
    }

    #[test]
    fn spec_examples() {
        assert_eq!(int("0t1T0"), 6); // Types Ch. 1 §3
        assert_eq!(int("6"), 6);
        assert_eq!(int("-9841"), -9841);
        assert_eq!(int("3_812_798"), 3_812_798);
        // 243 − 81 − 9 + 1: the separator is decoration, not a digit group.
        assert_eq!(int("0t1T0_T01"), 154);
    }

    #[test]
    fn negated_ternary_is_redundant() {
        // Types Ch. 1 §3: `-0t1T` == `0tT1`.
        assert_eq!(int("-0t1T"), int("0tT1"));
    }

    #[test]
    fn trit_literals() {
        assert_eq!(parse_literal("1t"), Ok(Literal::Trit(Trit::Pos)));
        assert_eq!(parse_literal("-1t"), Ok(Literal::Trit(Trit::Neg)));
        assert_eq!(parse_literal("0t"), Ok(Literal::Trit(Trit::Zero)));
    }

    #[test]
    fn hept_is_trit_exact() {
        assert_eq!(int("0hD"), 0);
        assert_eq!(int("0hE"), 1);
        // Ch. 1 §3.1: `D` is the zero digit, not `0`, so a leading zero is
        // not neutral. Every other notation here permits padding; this one
        // does not, and the table exists to say so.
        assert_eq!(int("0hJ"), 6);
        assert_eq!(int("0h0J"), -345);
        assert_eq!(int("0h00J"), -9822);
        assert_eq!(int("0h2C9"), -8050);
        // Whereas padding a `0t` literal or a decimal one changes nothing.
        assert_eq!(int("0t1T0"), int("0t01T0"));
        assert_eq!(int("6"), int("06"));
        assert_eq!(int("0hC"), -1);
        assert_eq!(int("0hQ"), 13);
        assert_eq!(int("0h0"), -13);
        assert_eq!(int("0hDQ"), 13);
        assert_eq!(int("0hED"), 27);
    }

    #[test]
    fn round_trips() {
        for v in [-9841i128, -40, -1, 0, 1, 40, 9841, 3_812_798_742_493] {
            let b = Bt::from_i128(v);
            for radix in [Radix::Dec, Radix::Ternary, Radix::Hept] {
                assert_eq!(int(&render(&b, radix)), v, "{v} in {radix:?}");
            }
        }
    }

    #[test]
    fn rejects_binary_world_radices_by_name() {
        // ISA Assembly §2.2 requires a diagnostic that says the radix does
        // not exist, not a generic bad-digit report.
        for (src, marker, word) in [
            ("0xFF", 'x', "hexadecimal"),
            ("0o17", 'o', "octal"),
            ("0b101", 'b', "binary"),
            ("-0xFF", 'x', "hexadecimal"),
        ] {
            let e = parse_literal(src).unwrap_err();
            assert_eq!(e, LitError::BinaryWorldRadix(marker), "{src}");
            assert!(e.to_string().contains(word), "{src}: {e}");
            assert!(e.to_string().contains("artifact of the bit"), "{src}");
        }
        // The radices that do exist are unaffected, and so is a bare `0`.
        assert!(parse_literal("0hQ").is_ok());
        assert!(parse_literal("0t1T").is_ok());
        assert!(parse_literal("0").is_ok());
    }
}
