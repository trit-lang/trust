//! Expansion — the other direction of legalization (TIR §6).
//!
//! A value wider than the target's widest legal width is represented as `k`
//! **parts**, each a legal-width value, least significant part first:
//!
//! > value = Σ partᵢ · 3^(L·i)
//!
//! That is not a new encoding, just a regrouping: part `i` holds trits
//! `[i·L, (i+1)·L)` of the same balanced ternary number, and because a
//! balanced representation is unique, so is the split. The most significant
//! part carries only `top = w − (k−1)·L` logical trits and is kept
//! normalized to that range, which is what keeps the whole value inside
//! `tw`.
//!
//! Two properties of the radix make this much less work than its binary
//! equivalent:
//!
//! - **There is no sign extension.** Widening a negative value into more
//!   parts fills the new parts with *zero* (AM §3.5 — "there is one
//!   extension"), so `widen` costs nothing and cannot be confused with a
//!   zero-extending sibling that does not exist.
//! - **`neg` is trit-wise**, so negating a multi-part value negates each part
//!   independently with no carry and no `MIN` special case (AM §1.2). `sub`
//!   is therefore `add` of the negation, exactly, at every width.

use super::*;

/// How a value too wide for the target is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Wide {
    /// Legal width of each part.
    pub part: u32,
    /// Number of parts.
    pub k: u32,
    /// Logical width of the most significant part.
    pub top: u32,
}

impl Wide {
    /// The split of a logical width `w` over parts of width `part`.
    pub(super) fn new(w: u32, part: u32) -> Wide {
        let k = w.div_ceil(part);
        Wide {
            part,
            k,
            top: w - (k - 1) * part,
        }
    }

    pub(super) fn ty(self) -> Type {
        Type::Int(self.part)
    }

    /// The logical width this layout represents.
    pub(super) fn logical(self) -> u32 {
        (self.k - 1) * self.part + self.top
    }
}

/// The parts of an operand, least significant first.
pub(super) fn parts_of(e: &mut Emit, o: &Operand, wide: Wide) -> Vec<Operand> {
    match e.resolve(o) {
        Operand::Const(_, v) => (0..wide.k)
            .map(|i| Operand::Const(wide.ty(), v.shr(i * wide.part).wrap_to(wide.part)))
            .collect(),
        Operand::Value(name) => match e.parts.get(&name) {
            Some(parts) => parts.clone(),
            None => {
                // A value that is not itself wide sits entirely in the low
                // part; the rest are zero, because balanced ternary has no
                // sign extension.
                let lo = e.coerce(&Operand::Value(name), wide.ty());
                let mut parts = vec![lo];
                parts.extend((1..wide.k).map(|_| konst(wide.part, 0)));
                parts
            }
        },
        other => {
            e.errs
                .push(format!("cannot expand {other:?} into t{} parts", wide.part));
            vec![konst(wide.part, 0); wide.k as usize]
        }
    }
}

/// Record the parts of an instruction's result.
pub(super) fn bind(e: &mut Emit, inst: &Inst, parts: Vec<Operand>) {
    if let Some(name) = inst.results.first() {
        e.parts.insert(name.clone(), parts);
    }
}

/// `add`/`sub` over parts, with the carry chained through `.flag`.
///
/// This is the construction TIR §6 points at when it says the `.flag` form
/// "exists chiefly for this — carry out of a part is the overflow trit of
/// that part's `add.flag`".
pub(super) fn add_sub(
    e: &mut Emit,
    inst: &Inst,
    op: FlavoredOp,
    flavor: Flavor,
    wide: Wide,
    a: &Operand,
    b: &Operand,
) {
    let a = parts_of(e, a, wide);
    let mut b = parts_of(e, b, wide);

    // `a − b` is `a + (−b)`, exactly: negation is trit-wise and total, so the
    // negated operand needs no carry and has no unrepresentable case.
    if op == FlavoredOp::Sub {
        b = b
            .into_iter()
            .map(|p| {
                e.emit(
                    "ng",
                    wide.ty(),
                    InstKind::Neg {
                        ty: wide.ty(),
                        a: p,
                    },
                )
            })
            .collect();
    }

    let mut sums = Vec::with_capacity(wide.k as usize);
    let mut carry: Option<Operand> = None;
    for i in 0..wide.k as usize {
        let (sum, out) = e.emit2(
            "p",
            wide.ty(),
            Type::Int(1),
            InstKind::Flavored {
                op: FlavoredOp::Add,
                flavor: Flavor::Flag,
                ty: wide.ty(),
                a: a[i].clone(),
                b: b[i].clone(),
            },
        );
        let (sum, out) = match carry.take() {
            None => (sum, out),
            Some(c) => {
                let cw = e.coerce(&c, wide.ty());
                let (sum2, out2) = e.emit2(
                    "p",
                    wide.ty(),
                    Type::Int(1),
                    InstKind::Flavored {
                        op: FlavoredOp::Add,
                        flavor: Flavor::Flag,
                        ty: wide.ty(),
                        a: sum,
                        b: cw,
                    },
                );
                // At most one of the two additions can carry: if `a+b` left
                // the range, the wrapped result has the opposite sign and
                // adding ±1 cannot leave it again. So the carry out is
                // whichever of the two is nonzero.
                let merged = e.emit(
                    "cy",
                    Type::Int(1),
                    InstKind::Select3 {
                        t: out,
                        ty: Type::Int(1),
                        neg: konst(1, -1),
                        zero: out2,
                        pos: konst(1, 1),
                    },
                );
                (sum2, merged)
            }
        };
        sums.push(sum);
        carry = Some(out);
    }

    let carry = carry.expect("k >= 1");
    let overflow = overflow_trit(e, &sums, carry, wide);

    match flavor {
        Flavor::Wrap => {
            let top = sums.len() - 1;
            sums[top] = e.renormalize(sums[top].clone(), wide.top, wide.part);
            bind(e, inst, sums);
        }
        Flavor::Trap => {
            e.trap_if(overflow, [true, false, true], FaultCode::Overflow);
            // Past the check the value fits, so the top part is already
            // normalized.
            bind(e, inst, sums);
        }
        Flavor::Flag => {
            let top = sums.len() - 1;
            sums[top] = e.renormalize(sums[top].clone(), wide.top, wide.part);
            bind(e, inst, sums);
            if let Some(name) = inst.results.get(1) {
                e.actual.insert(name.clone(), Type::Int(1));
                e.subst.insert(name.clone(), overflow);
            }
        }
    }
}

/// Multiply, part by part (TIR §6, and G6.6 for why this needed `mulh`).
///
/// Schoolbook: part *i* of `a` times part *j* of `b` lands at position
/// *i + j*, and its top half at *i + j + 1*. `mul.wrap` gives the low half
/// and `mulh` the high one, and TIR §3.1 defines them so that together they
/// are the exact product — which is the whole reason `mulh` exists.
///
/// The accumulator is **2k parts wide**, not k. The extra half costs a little
/// work and buys the overflow answer for nothing: the product fits exactly
/// when every part above the result's width is zero, and the direction of an
/// overflow is the sign of the most significant nonzero part, because that is
/// what the sign of a balanced positional number is.
pub(super) fn mul(e: &mut Emit, inst: &Inst, flavor: Flavor, wide: Wide, a: &Operand, b: &Operand) {
    let a = parts_of(e, a, wide);
    let b = parts_of(e, b, wide);
    let k = wide.k as usize;
    let ty = wide.ty();

    // The full product, least significant part first. Two k-part values
    // multiply into at most 2k parts.
    let mut acc: Vec<Operand> = (0..2 * k).map(|_| konst(wide.part, 0)).collect();

    // Add `v` into `acc[at]` and carry upwards for as long as it carries.
    fn accumulate(e: &mut Emit, acc: &mut [Operand], at: usize, v: Operand, ty: Type) {
        let mut i = at;
        let mut addend = v;
        while i < acc.len() {
            let (sum, carry) = e.emit2(
                "mp",
                ty,
                Type::Int(1),
                InstKind::Flavored {
                    op: FlavoredOp::Add,
                    flavor: Flavor::Flag,
                    ty,
                    a: acc[i].clone(),
                    b: addend,
                },
            );
            acc[i] = sum;
            i += 1;
            if i == acc.len() {
                break;
            }
            // The carry is a trit, and a trit added to the next part may
            // itself carry, so this continues rather than stopping.
            addend = e.coerce(&carry, ty);
        }
    }

    for (i, ai) in a.iter().enumerate() {
        for (j, bj) in b.iter().enumerate() {
            let (lo, _) = e.emit2(
                "ml",
                ty,
                Type::Int(1),
                InstKind::Flavored {
                    op: FlavoredOp::Mul,
                    flavor: Flavor::Flag,
                    ty,
                    a: ai.clone(),
                    b: bj.clone(),
                },
            );
            let hi = e.emit(
                "mh",
                ty,
                InstKind::Plain {
                    op: PlainOp::MulH,
                    ty,
                    a: ai.clone(),
                    b: bj.clone(),
                },
            );
            accumulate(e, &mut acc, i + j, lo, ty);
            if i + j + 1 < 2 * k {
                accumulate(e, &mut acc, i + j + 1, hi, ty);
            }
        }
    }

    // Everything at or above the result's logical width is overflow: the
    // parts past the last one, plus whatever spilled past the top part's own
    // width when that is narrower than a part.
    let mut spilled: Vec<Operand> = acc[k..].to_vec();
    if wide.top != wide.part {
        let hi = e.high_part(acc[k - 1].clone(), wide.top, wide.part);
        spilled.push(hi);
    }

    // The sign of a balanced positional number is the sign of its most
    // significant nonzero part, so folding the signs from the top down with
    // "the first that is nonzero" gives the direction of the overflow.
    let mut direction = konst(1, 0);
    for part in spilled.iter() {
        let s = e.sign_of(part.clone(), wide.part);
        direction = e.emit(
            "ov",
            Type::Int(1),
            InstKind::Select3 {
                t: s.clone(),
                ty: Type::Int(1),
                neg: s.clone(),
                zero: direction,
                pos: s,
            },
        );
    }

    let mut parts: Vec<Operand> = acc[..k].to_vec();
    match flavor {
        Flavor::Wrap => {
            parts[k - 1] = e.renormalize(parts[k - 1].clone(), wide.top, wide.part);
            bind(e, inst, parts);
        }
        Flavor::Trap => {
            e.trap_if(direction, [true, false, true], FaultCode::Overflow);
            parts[k - 1] = e.renormalize(parts[k - 1].clone(), wide.top, wide.part);
            bind(e, inst, parts);
        }
        Flavor::Flag => {
            parts[k - 1] = e.renormalize(parts[k - 1].clone(), wide.top, wide.part);
            bind(e, inst, parts);
            if let Some(name) = inst.results.get(1) {
                e.actual.insert(name.clone(), Type::Int(1));
                e.subst.insert(name.clone(), direction);
            }
        }
    }
}

/// A wide `div` or `rem`: a call to the helper of `divide.rs`.
///
/// Division is the one operation expansion cannot rewrite — it is an
/// algorithm, not a pattern over the parts — so it becomes a function, the
/// way `__divdi3` does. The call is built already reshaped (G6.5): the result
/// comes back through a slot this frame supplies, and each wide argument goes
/// as its parts.
pub(super) fn divide(e: &mut Emit, inst: &Inst, wide: Wide, a: &Operand, b: &Operand, rem: bool) {
    let Some(stride) = part_stride(e, wide) else {
        return;
    };
    let slot = e.emit(
        "dq",
        Type::Ptr,
        InstKind::Slot {
            trytes: wide.k * stride,
        },
    );
    let mut args = vec![slot.clone()];
    args.extend(parts_of(e, a, wide));
    args.extend(parts_of(e, b, wide));
    e.push(
        Vec::new(),
        InstKind::Call {
            callee: Callee::Direct(super::divide::helper_name(wide.logical(), rem)),
            args,
            ret: None,
        },
    );
    let parts = load_parts(e, wide, &slot);
    bind(e, inst, parts);
}

/// Right shift by a constant, part by part — and **without any carry**.
///
/// Write the amount as `k = q·L + r` over parts of width `L`. Dropping `q`
/// parts is a reindex; the remainder shifts each part down by `r` and pulls
/// the low `r` trits of the part above into the vacated top:
///
/// > `out[i] = shr(a[i+q], r) + wrap(a[i+q+1], r) · 3^(L−r)`
///
/// Both terms are bounded so that their sum still fits one part —
/// `|shr(a,r)| ≤ (3^(L−r)−1)/2` and the other `≤ (3^L − 3^(L−r))/2` — so no
/// carry can escape. That is a property of the balanced representation, not
/// an arrangement.
///
/// **Truncation is round-to-nearest here, exactly.** The discarded remainder
/// is `Σ_{i<q} a[i]·B^i + wrap(a[q], r)·B^q`, whose magnitude is at most
/// `(3^k − 1)/2` — strictly less than half of `3^k`. So no correction is
/// needed and no tie can arise, which is AM §3.3's claim about `shr` seen
/// from the multi-part side, with the bound tight.
pub(super) fn shr(e: &mut Emit, inst: &Inst, wide: Wide, a: &Operand, k: u32) {
    let parts = parts_of(e, a, wide);
    let ty = wide.ty();
    let l = wide.part;
    let (q, r) = ((k / l) as usize, k % l);

    let mut out = Vec::with_capacity(wide.k as usize);
    for i in 0..wide.k as usize {
        // Everything shifted in from above the top is zero: `shr` is exact
        // division, and there is nothing above.
        let Some(lower) = parts.get(i + q) else {
            out.push(konst(l, 0));
            continue;
        };
        if r == 0 {
            out.push(lower.clone());
            continue;
        }
        let down = e.emit(
            "sd",
            ty,
            InstKind::Plain {
                op: PlainOp::Shr,
                ty,
                a: lower.clone(),
                b: konst(l, r as i128),
            },
        );
        let Some(upper) = parts.get(i + q + 1) else {
            out.push(down);
            continue;
        };
        // The low `r` trits of the part above: `x − shr(x, r)·3^r`, which is
        // `wrap(x, r)` written with the operations a legal width has.
        let up_down = e.emit(
            "sd",
            ty,
            InstKind::Plain {
                op: PlainOp::Shr,
                ty,
                a: upper.clone(),
                b: konst(l, r as i128),
            },
        );
        let back = mul_by_pow3(e, ty, up_down, r);
        let (low, _) = e.emit2(
            "sv",
            ty,
            Type::Int(1),
            InstKind::Flavored {
                op: FlavoredOp::Sub,
                flavor: Flavor::Flag,
                ty,
                a: upper.clone(),
                b: back,
            },
        );
        let lifted = mul_by_pow3(e, ty, low, l - r);
        let (sum, _) = e.emit2(
            "ss",
            ty,
            Type::Int(1),
            InstKind::Flavored {
                op: FlavoredOp::Add,
                flavor: Flavor::Flag,
                ty,
                a: down,
                b: lifted,
            },
        );
        out.push(sum);
    }
    bind(e, inst, out);
}

/// `v · 3^n` at a legal width, wrapping — every use above is bounded so that
/// nothing is lost.
fn mul_by_pow3(e: &mut Emit, ty: Type, v: Operand, n: u32) -> Operand {
    let w = ty.width().expect("an integer width");
    let (r, _) = e.emit2(
        "sp",
        ty,
        Type::Int(1),
        InstKind::Flavored {
            op: FlavoredOp::Mul,
            flavor: Flavor::Flag,
            ty,
            a: v,
            b: Operand::Const(ty, Bt::from_i128(1).shl(n)),
        },
    );
    let _ = w;
    r
}

/// The direction of an overflow out of the whole wide value: the carry out of
/// the top part if there is one, otherwise whatever spilled past the top
/// part's logical width.
fn overflow_trit(e: &mut Emit, sums: &[Operand], carry: Operand, wide: Wide) -> Operand {
    if wide.top == wide.part {
        return carry;
    }
    let hi = e.high_part(sums[sums.len() - 1].clone(), wide.top, wide.part);
    let spill = e.sign_of(hi, wide.part);
    e.emit(
        "ov",
        Type::Int(1),
        InstKind::Select3 {
            t: carry,
            ty: Type::Int(1),
            neg: konst(1, -1),
            zero: spill,
            pos: konst(1, 1),
        },
    )
}

/// Trit-wise operations and `neg` are positionwise, so they are simply
/// applied part by part — no carries, nothing to chain.
pub(super) fn positionwise(
    e: &mut Emit,
    inst: &Inst,
    wide: Wide,
    a: &Operand,
    b: Option<&Operand>,
    make: impl Fn(Operand, Option<Operand>, Type) -> InstKind,
) {
    let a = parts_of(e, a, wide);
    let b = b.map(|b| parts_of(e, b, wide));
    let parts = (0..wide.k as usize)
        .map(|i| {
            let kind = make(a[i].clone(), b.as_ref().map(|b| b[i].clone()), wide.ty());
            e.emit("q", wide.ty(), kind)
        })
        .collect();
    bind(e, inst, parts);
}

/// `cmp`, folded most significant part first.
///
/// Built from the bottom up so it stays branch-free: each step lets a more
/// significant part override everything below it, which is exactly what the
/// TIR appendix describes ("legalizes `cmp t27` into three `cmp t9` parts
/// folded most-significant-first").
pub(super) fn cmp(e: &mut Emit, inst: &Inst, wide: Wide, a: &Operand, b: &Operand) {
    let a = parts_of(e, a, wide);
    let b = parts_of(e, b, wide);
    let mut acc = e.emit(
        "k",
        Type::Int(1),
        InstKind::Cmp {
            ty: wide.ty(),
            a: a[0].clone(),
            b: b[0].clone(),
        },
    );
    for i in 1..wide.k as usize {
        let here = e.emit(
            "k",
            Type::Int(1),
            InstKind::Cmp {
                ty: wide.ty(),
                a: a[i].clone(),
                b: b[i].clone(),
            },
        );
        acc = e.emit(
            "k",
            Type::Int(1),
            InstKind::Select3 {
                t: here,
                ty: Type::Int(1),
                neg: konst(1, -1),
                zero: acc,
                pos: konst(1, 1),
            },
        );
    }
    if let Some(name) = inst.results.first() {
        e.actual.insert(name.clone(), Type::Int(1));
        e.subst.insert(name.clone(), acc);
    }
}

/// `select3` picks each part with the same selector.
pub(super) fn select3(
    e: &mut Emit,
    inst: &Inst,
    wide: Wide,
    t: &Operand,
    neg: &Operand,
    zero: &Operand,
    pos: &Operand,
) {
    let t = e.coerce(t, Type::Int(1));
    let neg = parts_of(e, neg, wide);
    let zero = parts_of(e, zero, wide);
    let pos = parts_of(e, pos, wide);
    let parts = (0..wide.k as usize)
        .map(|i| {
            e.emit(
                "q",
                wide.ty(),
                InstKind::Select3 {
                    t: t.clone(),
                    ty: wide.ty(),
                    neg: neg[i].clone(),
                    zero: zero[i].clone(),
                    pos: pos[i].clone(),
                },
            )
        })
        .collect();
    bind(e, inst, parts);
}

/// Widening into a wide layout: the source becomes the low part and every
/// other part is zero. No sign extension exists to get wrong.
pub(super) fn widen_into(e: &mut Emit, inst: &Inst, a: &Operand, from: Class, to: Wide) {
    let mut parts = match from {
        Class::Wide(w) => parts_of(e, a, w),
        Class::Legal(_) => {
            let lo = e.coerce(a, to.ty());
            vec![lo]
        }
    };
    while parts.len() < to.k as usize {
        parts.push(konst(to.part, 0));
    }
    parts.truncate(to.k as usize);
    bind(e, inst, parts);
}

/// Truncating out of a wide layout keeps the low parts and wraps the new top.
pub(super) fn trunc_from(e: &mut Emit, inst: &Inst, a: &Operand, from: Wide, to: Class, w: u32) {
    let parts = parts_of(e, a, from);
    match to {
        Class::Wide(t) => {
            let mut kept: Vec<Operand> = parts.into_iter().take(t.k as usize).collect();
            let top = kept.len() - 1;
            kept[top] = e.renormalize(kept[top].clone(), t.top, t.part);
            bind(e, inst, kept);
        }
        Class::Legal(l) => {
            let lo = parts[0].clone();
            let narrowed = if l < from.part {
                e.emit(
                    "t",
                    Type::Int(l),
                    InstKind::Trunc {
                        from: from.ty(),
                        a: lo,
                        to: Type::Int(l),
                    },
                )
            } else {
                lo
            };
            let v = e.renormalize(narrowed, w, l);
            if let Some(name) = inst.results.first() {
                if let Some(t) = e.type_of(&v) {
                    e.actual.insert(name.clone(), t);
                }
                e.subst.insert(name.clone(), v);
            }
        }
    }
}

/// A wide load is one load per part, at ascending addresses — the parts are
/// stored little-trytean, like everything else (AM §2.2).
pub(super) fn load(e: &mut Emit, inst: &Inst, wide: Wide, p: &Operand) {
    let parts = load_parts(e, wide, p);
    bind(e, inst, parts);
}

/// Read a wide value's parts from memory, least significant first.
pub(super) fn load_parts(e: &mut Emit, wide: Wide, p: &Operand) -> Vec<Operand> {
    let Some(stride) = part_stride(e, wide) else {
        return vec![konst(wide.part, 0); wide.k as usize];
    };
    let base = e.coerce(p, Type::Ptr);
    (0..wide.k)
        .map(|i| {
            let addr = offset_by(e, &base, i * stride);
            e.emit(
                "ld",
                wide.ty(),
                InstKind::Load {
                    ty: wide.ty(),
                    p: addr,
                },
            )
        })
        .collect()
}

/// A wide store is one store per part.
pub(super) fn store(e: &mut Emit, wide: Wide, v: &Operand, p: &Operand) {
    let Some(stride) = part_stride(e, wide) else {
        return;
    };
    let base = e.coerce(p, Type::Ptr);
    let parts = parts_of(e, v, wide);
    for (i, part) in parts.into_iter().enumerate() {
        let addr = offset_by(e, &base, i as u32 * stride);
        e.push(
            Vec::new(),
            InstKind::Store {
                ty: wide.ty(),
                v: part,
                p: addr,
            },
        );
    }
}

/// Addressable units between consecutive parts, or `None` if the parts do not
/// land on addressable boundaries.
fn part_stride(e: &mut Emit, wide: Wide) -> Option<u32> {
    let unit = e.widths.target.addr_unit;
    if unit == 0 || !wide.part.is_multiple_of(unit) {
        e.errs.push(format!(
            "cannot split a wide access into t{} parts: a part is not a whole \
             number of {unit}-trit addressable units",
            wide.part
        ));
        return None;
    }
    Some(wide.part / unit)
}

fn offset_by(e: &mut Emit, base: &Operand, units: u32) -> Operand {
    if units == 0 {
        return base.clone();
    }
    let Ok(aw) = super::width(e, e.widths.target.ptr_width) else {
        return base.clone();
    };
    let d = konst(aw, units as i128);
    e.emit("ad", Type::Ptr, InstKind::Offset { p: base.clone(), d })
}
