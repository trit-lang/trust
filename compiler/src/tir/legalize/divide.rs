//! Multi-part division, as a helper function written in TIR.
//!
//! `div` and `rem` are the only operations expansion cannot rewrite: every
//! other one is a fixed pattern over the parts, and division is an
//! *algorithm*. So legalization emits it as an ordinary function and turns a
//! wide `div` into a call — which is what a C compiler does with `__divdi3`,
//! and for the same reason.
//!
//! The helper is written as **TIR source text** rather than assembled from
//! `Inst` values. It is a page of long division either way, and one of the two
//! can be read.
//!
//! # Why the digits run −2…2
//!
//! Schoolbook long division picks, at each step, the digit that leaves the
//! smallest remainder. In balanced ternary the obvious digit set is
//! {−1, 0, +1} — one trit — and **it is not enough.** Carrying the invariant
//! `|r| ≤ |b|/2` through one step gives `|3r + t| ≤ 3|b|/2 + 1`, and when
//! `|b|` is even that bound is reachable and no single trit pulls it back
//! under `|b|/2`. The error then compounds down the rest of the quotient.
//!
//! Widening the digit set to −2…2 fixes it, and costs nothing structural,
//! because the quotient is accumulated as a *value* (`q ← 3q + d`) rather than
//! as a string of trits: a digit outside −1…1 simply carries into what is
//! already there. `trit_core::Bt::divrem` reaches the same conclusion the same
//! way, and this is that routine transcribed.
//!
//! # The tie
//!
//! `|r| = |b|/2` exactly — again only possible for even `|b|` — is the tie AM
//! §3.2 resolves *away from zero*. The loop's digit rule leaves the remainder
//! there rather than stepping past it, so the last block steps one further out
//! when the leftover points the same way as the quotient's sign.

#[allow(unused_imports)]
use super::*;

/// The name of the division helper for a given width.
pub fn helper_name(w: u32, rem: bool) -> String {
    let what = if rem { "rem" } else { "div" };
    format!("lz.{what}.t{w}")
}

/// The helper's source, at width `w`.
///
/// Returns the quotient. `rem` is `a − q·b`, which the caller computes with a
/// multiply it already knows how to expand — so one helper serves both.
pub fn helper_source(w: u32, target: &str, rem: bool) -> String {
    let t = format!("t{w}");
    let top = w - 1;
    let name = helper_name(w, rem);
    // Both answers come out of the same loop; only the last block differs.
    // The remainder's fixup subtracts the step it took rather than adding it,
    // which keeps `a = q·b + r` true through the tie.
    let finish = if rem {
        format!(
            "    %bstep = mul.wrap {t} %b, %stepw\n\
             \x20   %rf = sub.wrap {t} %fr, %bstep\n\
             \x20   ret %rf"
        )
    } else {
        format!(
            "    %qf = add.wrap {t} %fq, %stepw\n\
             \x20   ret %qf"
        )
    };

    // The five candidates, in the order −2 … 2. Each is `r − d·b` and the
    // magnitude of that, since the digit chosen is the one leaving the
    // smallest remainder.
    let digits = [
        ("n2", "-2"),
        ("n1", "-1"),
        ("z", "0"),
        ("p1", "1"),
        ("p2", "2"),
    ];
    let candidate = |tag: &str, expr: String, digit: &str| {
        format!(
            "    %c.{tag} = {expr}\n\
             \x20   %s.{tag} = cmp {t} %c.{tag}, const {t} 0\n\
             \x20   %g.{tag} = neg {t} %c.{tag}\n\
             \x20   %m.{tag} = select3 %s.{tag}, {t} %g.{tag}, %c.{tag}, %c.{tag}\n\
             \x20   %d.{tag} = add.wrap {t} const {t} {digit}, const {t} 0\n"
        )
    };
    let cand = [
        candidate("n2", format!("add.wrap {t} %r1, %b2"), "-2"),
        candidate("n1", format!("add.wrap {t} %r1, %b"), "-1"),
        candidate("z", format!("add.wrap {t} %r1, const {t} 0"), "0"),
        candidate("p1", format!("sub.wrap {t} %r1, %b"), "1"),
        candidate("p2", format!("sub.wrap {t} %r1, %b2"), "2"),
    ]
    .join("");

    // Fold the candidates left to right, keeping the smaller magnitude and
    // carrying its digit and remainder. A tie keeps the earlier, which is the
    // more negative digit — the same choice `Bt::divrem`'s `min_by` makes.
    let mut fold = String::new();
    let mut best = digits[0].0.to_string();
    for (tag, _) in &digits[1..] {
        let out = format!("{best}{tag}");
        fold.push_str(&format!(
            "    %k.{out} = cmp {t} %m.{tag}, %m.{best}\n\
             \x20   %m.{out} = select3 %k.{out}, {t} %m.{tag}, %m.{best}, %m.{best}\n\
             \x20   %c.{out} = select3 %k.{out}, {t} %c.{tag}, %c.{best}, %c.{best}\n\
             \x20   %d.{out} = select3 %k.{out}, {t} %d.{tag}, %d.{best}, %d.{best}\n"
        ));
        best = out;
    }

    format!(
        r#"tir 0.1 target "{target}"

fn @{name}(%a: {t}, %b: {t}) -> {t} {{
^entry:
    %bz = cmp {t} %b, const {t} 0
    br3 %bz, ^go(const {t} {w}, const {t} 0, const {t} 0, %a), ^divzero, ^go(const {t} {w}, const {t} 0, const {t} 0, %a)

^divzero:
    trap F_DIVZERO

^go(%i: {t}, %q: {t}, %r: {t}, %x: {t}):
    %more = cmp {t} %i, const {t} 0
    br3 %more, ^fix(%q, %r), ^fix(%q, %r), ^step(%i, %q, %r, %x)

^step(%si: {t}, %sq: {t}, %sr: {t}, %sx: {t}):
    ; The most significant trit of what is left of the dividend, and the
    ; dividend shifted up so that the next step sees the next one.
    %t = shr {t} %sx, const {t} {top}
    %x2 = shl.wrap {t} %sx, const {t} 1
    %r3 = mul.wrap {t} %sr, const {t} 3
    %r1 = add.wrap {t} %r3, %t
    %b2 = mul.wrap {t} %b, const {t} 2
{cand}{fold}
    %q3 = mul.wrap {t} %sq, const {t} 3
    %q2 = add.wrap {t} %q3, %d.{best}
    %i2 = sub.wrap {t} %si, const {t} 1
    br ^go(%i2, %q2, %c.{best}, %x2)

^fix(%fq: {t}, %fr: {t}):
    ; `|2r| = |b|` is the tie AM §3.2 sends away from zero. The quotient's
    ; sign is the product of the operands' signs — one `tmul` of two trits,
    ; which is the radix doing the work a binary machine does with a branch.
    %rs = cmp {t} %fr, const {t} 0
    %rn = neg {t} %fr
    %ra = select3 %rs, {t} %rn, %fr, %fr
    %r2 = mul.wrap {t} %ra, const {t} 2
    %bs = cmp {t} %b, const {t} 0
    %bn = neg {t} %b
    %ba = select3 %bs, {t} %bn, %b, %b
    %tie = cmp {t} %r2, %ba

    %as = cmp {t} %a, const {t} 0
    %want = tmul t1 %as, %bs
    %have = tmul t1 %rs, %bs
    %agree = cmp t1 %have, %want

    ; Step out only on a tie, and only when the leftover points the way the
    ; quotient already does. A zero remainder agrees with nothing, so it is
    ; excluded without a test of its own.
    %stepd = select3 %agree, t1 const t1 0, %have, const t1 0
    %step0 = select3 %tie, t1 const t1 0, %stepd, const t1 0
    %stepw = widen t1 %step0 -> {t}
{finish}
}}
"#
    )
}
