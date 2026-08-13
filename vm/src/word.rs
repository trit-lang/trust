//! Word arithmetic for the machine's data path.
//!
//! A TRISC-27 word is 27 trits, so every value the machine holds fits in an
//! `i128` with room to spare — the widest intermediate is a full 54-trit
//! product, about 1.5 × 10²⁵. Working in `i128` rather than in
//! [`trit_core::Tint`] keeps the interpreter fast enough to be useful, at the
//! cost of restating semantics that `trit-core` already implements.
//!
//! That restatement is the risk, and it is why `tests/agreement.rs` runs every
//! function here against `trit-core` over a large sample: if the two ever
//! disagree, the VM is wrong, because `trit-core` is the direct expression of
//! the AM.

/// Trits in a word (AM §1.3).
pub const WORD_TRITS: u32 = 27;

/// Trits in a tryte.
pub const TRYTE_TRITS: u32 = 9;

/// Trytes in a word — also the size of an instruction, and the alignment of
/// every word access.
pub const WORD_TRYTES: i128 = 3;

/// The largest word value: (3²⁷ − 1)/2.
pub const MAX_WORD: i128 = 3_812_798_742_493;

/// The smallest word value. `MIN_WORD == -MAX_WORD` — the range is symmetric
/// and negation is total (AM §1.2).
pub const MIN_WORD: i128 = -MAX_WORD;

/// The largest tryte value.
pub const MAX_TRYTE: i128 = 9841;

/// 3^`n`, for `n` up to 80.
pub fn pow3(n: u32) -> i128 {
    3i128.pow(n)
}

/// The balanced ternary digit of `v`: the unique trit congruent to `v` mod 3.
///
/// This is also `rem(v, 3)` under round-to-nearest (AM Appendix A.5).
pub fn low_trit(v: i128) -> i8 {
    match v.rem_euclid(3) {
        0 => 0,
        1 => 1,
        _ => -1,
    }
}

/// Drop the low `k` trits: `v / 3^k`, exactly, rounding to nearest.
///
/// Discarding low trits *is* round-to-nearest division by 3^k, and since 3^k
/// is odd no tie can arise (AM §3.3).
pub fn shr3(v: i128, k: u32) -> i128 {
    let mut v = v;
    for _ in 0..k {
        v = (v - low_trit(v) as i128) / 3;
    }
    v
}

/// `v · 3^k`.
pub fn shl3(v: i128, k: u32) -> i128 {
    v * pow3(k)
}

/// The symmetric residue of `v` modulo 3^`n` — the wrapping overflow flavor
/// and the `wrap` instruction (AM §3.1).
pub fn wrap_to(v: i128, n: u32) -> i128 {
    let m = pow3(n);
    let r = v.rem_euclid(m);
    if r > (m - 1) / 2 { r - m } else { r }
}

/// The trit at position `i`.
pub fn trit_at(v: i128, i: u32) -> i8 {
    low_trit(shr3(v, i))
}

/// The trits of a word, least significant first.
pub fn word_trits(v: i128) -> [i8; WORD_TRITS as usize] {
    let mut out = [0i8; WORD_TRITS as usize];
    let mut v = v;
    for t in out.iter_mut() {
        let d = low_trit(v);
        *t = d;
        v = (v - d as i128) / 3;
    }
    out
}

/// A word from its trits, least significant first.
pub fn from_trits(trits: &[i8]) -> i128 {
    trits
        .iter()
        .enumerate()
        .map(|(i, &t)| t as i128 * pow3(i as u32))
        .sum()
}

/// A word split into its three trytes, least significant first — the order it
/// occupies memory in (AM §2.2).
pub fn word_trytes(v: i128) -> [i16; 3] {
    let mut out = [0i16; 3];
    let mut v = v;
    for t in out.iter_mut() {
        *t = wrap_to(v, TRYTE_TRITS) as i16;
        v = shr3(v, TRYTE_TRITS);
    }
    out
}

/// True iff `v` fits in `n` trits.
pub fn fits(v: i128, n: u32) -> bool {
    let max = (pow3(n) - 1) / 2;
    -max <= v && v <= max
}

/// Division: round to nearest, ties away from zero (AM §3.2).
///
/// # Panics
/// If `b` is zero; the caller raises `F_DIVZERO` instead.
pub fn div_nearest(a: i128, b: i128) -> i128 {
    let q = a / b; // Rust truncates toward zero
    let r = a - q * b;
    if 2 * r.abs() >= b.abs() {
        // Step one further out; with truncating division the remainder has
        // the sign of the dividend, so the direction is the quotient's own.
        q + if (a < 0) == (b < 0) { 1 } else { -1 }
    } else {
        q
    }
}

/// The remainder matching [`div_nearest`]: `|r| ≤ |b|/2`.
pub fn rem_nearest(a: i128, b: i128) -> i128 {
    a - div_nearest(a, b) * b
}

/// The three-way comparison (AM §3.5).
pub fn cmp3(a: i128, b: i128) -> i128 {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// The sign of a value, as a trit.
pub fn sign(v: i128) -> i128 {
    cmp3(v, 0)
}

/// Apply a trit-wise operation positionwise across a word (AM §3.4).
pub fn tritwise(a: i128, b: i128, f: impl Fn(i8, i8) -> i8) -> i128 {
    let (at, bt) = (word_trits(a), word_trits(b));
    let mut out = [0i8; WORD_TRITS as usize];
    for i in 0..WORD_TRITS as usize {
        out[i] = f(at[i], bt[i]);
    }
    from_trits(&out)
}

/// `tmin` — the ternary AND analogue.
pub fn tmin(a: i128, b: i128) -> i128 {
    tritwise(a, b, |x, y| x.min(y))
}

/// `tmax` — the ternary OR analogue.
pub fn tmax(a: i128, b: i128) -> i128 {
    tritwise(a, b, |x, y| x.max(y))
}

/// `tmul` — nonzero iff both are, sign composing.
pub fn tmul(a: i128, b: i128) -> i128 {
    tritwise(a, b, |x, y| x * y)
}

/// The result of an operation that can leave the word range.
pub struct Overflowing {
    /// The result wrapped into the word range.
    pub wrapped: i128,
    /// The direction of the overflow: +1 above MAX, −1 below MIN, 0 if the
    /// exact result fit. This is the `.flag` overflow trit, and for `add` and
    /// `sub` it is exactly the carry out of the top trit (TIR §6).
    pub overflow: i128,
}

/// Wrap an exact result into the word range, reporting the direction.
pub fn overflowing(exact: i128) -> Overflowing {
    Overflowing {
        wrapped: wrap_to(exact, WORD_TRITS),
        overflow: if fits(exact, WORD_TRITS) {
            0
        } else {
            sign(exact)
        },
    }
}
