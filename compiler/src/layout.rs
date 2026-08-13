//! The layout engine — Language Ch. 2, *Composite Types and Layout*.
//!
//! Sizes, alignments, field offsets, the two layout regimes, discriminant
//! assignment, and niche optimization. This is the part of the frontend that
//! does not need the surface syntax: Ch. 2 is stated entirely in terms of
//! types, so it can be built and tested against types constructed directly
//! (`docs/spec-gaps.md` G0.3).
//!
//! Generics do not exist yet (Ch. 4), and the chapter says the rules "are
//! schematic and apply once generics exist; draft 0.1 tooling may implement
//! them monomorphically" — so `Option<T>` here is a monomorphic constructor
//! that expands to the two-variant enum it is, and goes through exactly the
//! same niche machinery as a user enum.
//!
//! Where the radix pays: a tryte holds 19 683 patterns, of which a `bool`
//! uses 2 and a `trit` 3. The niches left over are what make `Option<bool>`,
//! `Option<trit>` and even `Option<Option<trit>>` cost one tryte (§6).

use std::collections::{BTreeMap, BTreeSet};
use trit_core::{Bt, TRITS_PER_TRYTE};

// ------------------------------------------------------------------ the types

/// A balanced ternary integer type (Ch. 1 §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntTy {
    /// `t9`.
    T9,
    /// `t27`.
    T27,
    /// `taddr` — the address-width integer, signed like everything else.
    TAddr,
}

impl IntTy {
    /// Width in trits on the AM.
    pub fn trits(self) -> u32 {
        match self {
            IntTy::T9 => 9,
            IntTy::T27 | IntTy::TAddr => 27,
        }
    }

    /// The spelling.
    pub fn name(self) -> &'static str {
        match self {
            IntTy::T9 => "t9",
            IntTy::T27 => "t27",
            IntTy::TAddr => "taddr",
        }
    }
}

/// A type whose layout can be computed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// `()` — the canonical zero-sized type.
    Unit,
    /// `trit` — a first-class scalar, not sugar over an integer.
    Trit,
    /// `bool` — a distinct nominal type, `false` = 0 and `true` = 1.
    Bool,
    /// `t9`, `t27`, `taddr`.
    Int(IntTy),
    /// `&T` — excludes the null address, so it always has a niche (§6).
    Ref(Box<Ty>),
    /// `Box<T>` — same layout facts as a reference for this chapter's
    /// purposes: pointer-sized and never null.
    Box(Box<Ty>),
    /// `[T; N]`. N must be non-negative (§3).
    Array(Box<Ty>, i128),
    /// `(T₁, …, Tₙ)` — always `repr(lang)`; there is no `repr(linear)` tuple.
    Tuple(Vec<Ty>),
    /// `Option<T>`, monomorphized.
    Option(Box<Ty>),
    /// A named struct or enum, defined in the [`TypeDb`].
    Named(String),
}

impl Ty {
    /// `&T`.
    pub fn reference(t: Ty) -> Ty {
        Ty::Ref(Box::new(t))
    }
    /// `Box<T>`.
    pub fn boxed(t: Ty) -> Ty {
        Ty::Box(Box::new(t))
    }
    /// `[T; N]`.
    pub fn array(t: Ty, n: i128) -> Ty {
        Ty::Array(Box::new(t), n)
    }
    /// `Option<T>`.
    pub fn option(t: Ty) -> Ty {
        Ty::Option(Box::new(t))
    }
    /// A named type.
    pub fn named(name: &str) -> Ty {
        Ty::Named(name.to_string())
    }
}

/// Which layout regime a nominal type is in (§1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Repr {
    /// The compiler may order, pad and pack fields freely, and may exploit
    /// niches. Programs must not depend on it.
    #[default]
    Lang,
    /// Fields in declaration order, each at the next address satisfying its
    /// alignment, trailing padding to the type's alignment. The interop and
    /// on-disk-format regime — named `linear` because there is no C ABI to be
    /// compatible with.
    Linear,
}

/// A struct definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StructDef {
    /// Layout regime.
    pub repr: Repr,
    /// Fields in declaration order. Tuple structs use positional names.
    pub fields: Vec<(String, Ty)>,
}

/// One enum variant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Variant {
    /// Variant name.
    pub name: String,
    /// Payload fields; empty for a fieldless variant.
    pub fields: Vec<(String, Ty)>,
    /// An explicit discriminant, which may be negative (§5.1).
    pub discriminant: Option<i128>,
}

impl Variant {
    /// A fieldless variant with no explicit discriminant.
    pub fn unit(name: &str) -> Variant {
        Variant {
            name: name.to_string(),
            fields: Vec::new(),
            discriminant: None,
        }
    }

    /// A variant with one unnamed payload field.
    pub fn payload(name: &str, ty: Ty) -> Variant {
        Variant {
            name: name.to_string(),
            fields: vec![("0".to_string(), ty)],
            discriminant: None,
        }
    }

    /// Give this variant an explicit discriminant.
    pub fn at(mut self, d: i128) -> Variant {
        self.discriminant = Some(d);
        self
    }
}

/// An enum definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumDef {
    /// Layout regime.
    pub repr: Repr,
    /// Variants in declaration order.
    pub variants: Vec<Variant>,
}

/// A nominal type definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Def {
    /// A struct.
    Struct(StructDef),
    /// An enum.
    Enum(EnumDef),
}

/// The set of nominal types in scope.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TypeDb {
    defs: BTreeMap<String, Def>,
}

impl TypeDb {
    /// An empty database.
    pub fn new() -> TypeDb {
        TypeDb::default()
    }

    /// Define a struct.
    pub fn struct_(&mut self, name: &str, repr: Repr, fields: Vec<(&str, Ty)>) -> &mut TypeDb {
        self.defs.insert(
            name.to_string(),
            Def::Struct(StructDef {
                repr,
                fields: fields
                    .into_iter()
                    .map(|(n, t)| (n.to_string(), t))
                    .collect(),
            }),
        );
        self
    }

    /// Define an enum.
    pub fn enum_(&mut self, name: &str, repr: Repr, variants: Vec<Variant>) -> &mut TypeDb {
        self.defs
            .insert(name.to_string(), Def::Enum(EnumDef { repr, variants }));
        self
    }

    /// Look a definition up.
    pub fn get(&self, name: &str) -> Option<&Def> {
        self.defs.get(name)
    }
}

// ---------------------------------------------------------------- the results

/// Why a type has no layout.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// A `Named` type that the database does not define.
    Unknown(String),
    /// `[T; N]` with N negative (§3): not Python-style end-relative, simply
    /// ill-formed.
    NegativeArrayLength(i128),
    /// A type that contains itself without indirection (§8).
    Infinite(String),
    /// Two variants sharing a discriminant (§5.1).
    DuplicateDiscriminant {
        /// The enum.
        enum_name: String,
        /// The discriminant claimed twice.
        value: i128,
    },
    /// A discriminant too wide for any integer type this draft has.
    DiscriminantTooWide(i128),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::Unknown(n) => write!(f, "`{n}` is not a type in scope"),
            LayoutError::NegativeArrayLength(n) => {
                write!(
                    f,
                    "array length {n} is negative; lengths are non-negative (Ch. 2 §3)"
                )
            }
            LayoutError::Infinite(n) => write!(
                f,
                "`{n}` contains itself without indirection, so it has no finite size (Ch. 2 §8)"
            ),
            LayoutError::DuplicateDiscriminant { enum_name, value } => write!(
                f,
                "two variants of `{enum_name}` have discriminant {value} (Ch. 2 §5.1)"
            ),
            LayoutError::DiscriminantTooWide(v) => {
                write!(
                    f,
                    "discriminant {v} does not fit in any draft 0.1 integer type"
                )
            }
        }
    }
}

impl std::error::Error for LayoutError {}

/// How an enum's discriminant is stored.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tag {
    /// One variant, or none: nothing to distinguish.
    None,
    /// The enum is representation-identical to `trit`: three fieldless
    /// variants with discriminants −1, 0, +1, so `match` lowers to one `br3`
    /// (§5.2).
    TritShaped,
    /// A tag stored separately, at this offset and width.
    Direct {
        /// Tag type.
        ty: IntTy,
        /// Offset in trytes.
        offset: u32,
    },
    /// The discriminant lives in invalid representations of the payload (§6):
    /// no space of its own at all.
    Niche {
        /// The variant whose payload occupies the storage.
        untagged: usize,
        /// Offset of the scalar holding the niche.
        offset: u32,
        /// How many niche values this enum consumes.
        used: u128,
        /// The scalar whose invalid values encode the discriminant.
        spot: Niche,
    },
}

/// Where an enclosing enum may hide a discriminant, and in what values.
///
/// A count alone is not enough to generate code against: a lowering needs to
/// know *which* patterns are invalid. The scalars that offer niches — `bool`
/// and `trit` — have a contiguous valid range, so the invalid values are
/// everything else in the scalar's storage, taken from above the range first
/// and then from below it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Niche {
    /// Offset in trytes of the scalar holding the niche.
    pub offset: u32,
    /// Storage width of that scalar, in trits.
    pub trits: u32,
    /// The lowest valid value.
    pub lo: i128,
    /// The highest valid value.
    pub hi: i128,
}

impl Niche {
    /// How many invalid values this scalar has.
    pub fn count(&self) -> u128 {
        capacity(self.trits.div_ceil(TRITS_PER_TRYTE))
            .saturating_sub((self.hi - self.lo + 1) as u128)
    }

    /// The `i`th invalid value: above the valid range first, then below it.
    pub fn nth(&self, i: u128) -> Option<i128> {
        let max = (3i128.pow(self.trits) - 1) / 2;
        let above = (max - self.hi) as u128;
        if i < above {
            return Some(self.hi + 1 + i as i128);
        }
        let below = i - above;
        let v = self.lo - 1 - below as i128;
        (v >= -max).then_some(v)
    }

    /// Shrink the valid range to record that `used` niches have been taken,
    /// so that an enclosing type sees only what is left.
    fn consume(&self, used: u128) -> Niche {
        let max = (3i128.pow(self.trits) - 1) / 2;
        let above = (max - self.hi).max(0) as u128;
        let take_above = used.min(above) as i128;
        let take_below = used.saturating_sub(above) as i128;
        Niche {
            offset: self.offset,
            trits: self.trits,
            lo: self.lo - take_below,
            hi: self.hi + take_above,
        }
    }
}

/// The computed layout of a type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Size in trytes. Always a multiple of `align` (§1).
    pub size: u32,
    /// Alignment in trytes: a power of three, at least 1 (§1).
    pub align: u32,
    /// Field offsets in **declaration order**, whatever order the fields were
    /// actually placed in.
    pub offsets: Vec<u32>,
    /// For enums: how the discriminant is stored, and each variant's field
    /// offsets.
    pub enum_layout: Option<EnumLayout>,
    /// How many of this type's representations are invalid and therefore
    /// available to an enclosing enum (§6). Saturates rather than overflowing.
    pub niches: u128,
    /// Where those invalid representations are, when a lowering needs to put
    /// a discriminant in one.
    pub niche: Option<Niche>,
}

/// The enum-specific part of a layout.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumLayout {
    /// How the discriminant is stored.
    pub tag: Tag,
    /// Discriminant of each variant, in declaration order.
    pub discriminants: Vec<i128>,
    /// Field offsets of each variant's payload.
    pub variant_offsets: Vec<Vec<u32>>,
}

impl Layout {
    /// True for a zero-sized type: size 0, alignment 1 (§1).
    pub fn is_zst(&self) -> bool {
        self.size == 0
    }
}

// ------------------------------------------------------------- the computation

/// Patterns representable in `trytes` trytes, saturating.
fn capacity(trytes: u32) -> u128 {
    3u128
        .checked_pow(trytes * TRITS_PER_TRYTE)
        .unwrap_or(u128::MAX)
}

/// Round `offset` up to the next multiple of `align`.
fn align_to(offset: u32, align: u32) -> u32 {
    offset.div_ceil(align) * align
}

/// Compute the layout of a type.
pub fn layout_of(db: &TypeDb, ty: &Ty) -> Result<Layout, LayoutError> {
    let mut visiting = BTreeSet::new();
    compute(db, ty, &mut visiting)
}

/// Check that every name a type mentions is defined, without computing any
/// layout. `Ty` is a finite tree and `Named` is a leaf here, so this always
/// terminates — which is what makes it safe to use behind an indirection.
fn check_names(db: &TypeDb, ty: &Ty) -> Result<(), LayoutError> {
    match ty {
        Ty::Named(n) => db
            .get(n)
            .map(|_| ())
            .ok_or_else(|| LayoutError::Unknown(n.clone())),
        Ty::Ref(t) | Ty::Box(t) | Ty::Array(t, _) | Ty::Option(t) => check_names(db, t),
        Ty::Tuple(ts) => ts.iter().try_for_each(|t| check_names(db, t)),
        Ty::Unit | Ty::Trit | Ty::Bool | Ty::Int(_) => Ok(()),
    }
}

fn scalar(size: u32, align: u32, valid: u128) -> Layout {
    Layout {
        size,
        align,
        offsets: Vec::new(),
        enum_layout: None,
        niches: capacity(size).saturating_sub(valid),
        niche: None,
    }
}

/// A scalar whose valid values are the contiguous range `lo..=hi`.
fn ranged(size: u32, lo: i128, hi: i128) -> Layout {
    let mut l = scalar(size, size, (hi - lo + 1) as u128);
    l.niche = Some(Niche {
        offset: 0,
        trits: size * TRITS_PER_TRYTE,
        lo,
        hi,
    });
    l
}

fn compute(db: &TypeDb, ty: &Ty, visiting: &mut BTreeSet<String>) -> Result<Layout, LayoutError> {
    Ok(match ty {
        // ZSTs: size 0, alignment 1. Reads and writes compile to nothing.
        Ty::Unit => scalar(0, 1, 1),

        // A trit and a bool each occupy a full tryte, and their unused
        // capacity is not observable in safe code (Ch. 1 §7) — but it is
        // exactly what an enclosing enum may use (§6).
        Ty::Trit => ranged(1, -1, 1),
        Ty::Bool => ranged(1, 0, 1),

        Ty::Int(i) => {
            let trytes = i.trits().div_ceil(TRITS_PER_TRYTE);
            // Every pattern is a valid integer, so no niches.
            scalar(trytes, trytes, capacity(trytes))
        }

        // References exclude the null address → at least one niche (§6).
        Ty::Ref(inner) | Ty::Box(inner) => {
            // The pointee's *layout* is deliberately not computed: a pointer
            // is pointer-sized whatever it points at, and computing it here
            // would not terminate for precisely the recursive types §8 calls
            // well-formed. The pointee's names are still checked, so a typo
            // behind an indirection is not silently accepted.
            check_names(db, inner)?;
            let trytes = IntTy::TAddr.trits().div_ceil(TRITS_PER_TRYTE);
            // A reference holds an address, which is non-negative and never
            // null (§6). Treating 1…MAX as the valid range is a safe
            // under-count of the niches and puts the *null* address first
            // among them, which is the one every `Option<&T>` uses.
            let max = (3i128.pow(IntTy::TAddr.trits()) - 1) / 2;
            ranged(trytes, 1, max)
        }

        Ty::Array(elem, n) => {
            if *n < 0 {
                return Err(LayoutError::NegativeArrayLength(*n));
            }
            let e = compute(db, elem, visiting)?;
            let n = *n as u32;
            // Size is N·size_of::<T>(), elements at i·size_of::<T>() in index
            // order — array layout is fully defined even under repr(lang),
            // because indexing arithmetic depends on it (§3).
            let size = e.size.saturating_mul(n);
            let valid = capacity(e.size)
                .saturating_sub(e.niches)
                .checked_pow(n)
                .unwrap_or(u128::MAX);
            Layout {
                size,
                align: e.align,
                offsets: (0..n).map(|i| i * e.size).collect(),
                enum_layout: None,
                niches: capacity(size).saturating_sub(valid),
                // An element's niche is the array's, at that element's offset.
                niche: (n > 0).then(|| e.niche.clone()).flatten(),
            }
        }

        // Tuples are always repr(lang): their layout is unspecified and there
        // is no repr(linear) tuple. Code that needs defined layout uses a
        // struct.
        Ty::Tuple(fields) => {
            let layouts = fields
                .iter()
                .map(|f| compute(db, f, visiting))
                .collect::<Result<Vec<_>, _>>()?;
            aggregate(&layouts, Repr::Lang)
        }

        // Option<T> is the two-variant enum it is; nothing about it is
        // special-cased, which is what makes the §6 guarantees fall out of
        // the general niche rule.
        Ty::Option(inner) => {
            let def = EnumDef {
                repr: Repr::Lang,
                variants: vec![
                    Variant::unit("None"),
                    Variant::payload("Some", (**inner).clone()),
                ],
            };
            enum_layout(db, "Option", &def, visiting)?
        }

        Ty::Named(name) => {
            let Some(def) = db.get(name) else {
                return Err(LayoutError::Unknown(name.clone()));
            };
            // A type may reach itself only through indirection; `Ref`/`Box`
            // reset the visiting set, so anything still on it is an infinite
            // type (§8).
            if !visiting.insert(name.clone()) {
                return Err(LayoutError::Infinite(name.clone()));
            }
            let result = match def {
                Def::Struct(s) => {
                    let layouts = s
                        .fields
                        .iter()
                        .map(|(_, t)| compute(db, t, visiting))
                        .collect::<Result<Vec<_>, _>>()?;
                    aggregate(&layouts, s.repr)
                }
                Def::Enum(e) => enum_layout(db, name, e, visiting)?,
            };
            visiting.remove(name);
            result
        }
    })
}

/// Lay out a product type.
///
/// Under `repr(linear)` fields go in declaration order. Under `repr(lang)`
/// the compiler may permute them — and with trytes of 9 trits and only two
/// scalar alignments, padding arises exactly when a 3-aligned field follows a
/// prefix whose size is not a multiple of 3, so placing the most-aligned
/// fields first is enough (§4).
fn aggregate(fields: &[Layout], repr: Repr) -> Layout {
    let align = fields.iter().map(|f| f.align).max().unwrap_or(1);

    let mut order: Vec<usize> = (0..fields.len()).collect();
    if repr == Repr::Lang {
        order.sort_by_key(|&i| std::cmp::Reverse(fields[i].align));
    }

    let mut offsets = vec![0u32; fields.len()];
    let mut cursor = 0u32;
    for &i in &order {
        cursor = align_to(cursor, fields[i].align);
        offsets[i] = cursor;
        cursor += fields[i].size;
    }
    // Size is always a multiple of alignment, so arrays need no inter-element
    // padding beyond what the element already contains (§1).
    let size = align_to(cursor, align);

    // Padding trytes have unspecified contents, so every padding pattern is
    // "valid" and contributes nothing to the niche count; the niches of a
    // product are those of its fields.
    let payload: u128 = fields
        .iter()
        .map(|f| capacity(f.size).saturating_sub(f.niches))
        .try_fold(1u128, |acc, v| acc.checked_mul(v))
        .unwrap_or(u128::MAX);
    let padding = size - fields.iter().map(|f| f.size).sum::<u32>().min(size);
    let valid = payload.saturating_mul(capacity(padding));

    // A product's niche is the widest one among its fields, moved to that
    // field's offset.
    let niche = order
        .iter()
        .filter_map(|&i| {
            fields[i].niche.as_ref().map(|n| Niche {
                offset: offsets[i] + n.offset,
                ..n.clone()
            })
        })
        .max_by_key(Niche::count);

    Layout {
        size,
        align,
        offsets,
        enum_layout: None,
        niches: capacity(size).saturating_sub(valid),
        niche,
    }
}

/// Assign discriminants (§5.1).
///
/// Unassigned discriminants continue from the previous variant, which is what
/// the chapter's own appendix requires: it lists `enum { A=-1, B, C=1 }` as
/// trit-shaped, and that only holds if `B` is 0.
fn discriminants(name: &str, def: &EnumDef) -> Result<Vec<i128>, LayoutError> {
    let mut out = Vec::with_capacity(def.variants.len());
    let mut next = 0i128;
    for v in &def.variants {
        let d = v.discriminant.unwrap_or(next);
        out.push(d);
        next = d + 1;
    }
    let mut seen = BTreeSet::new();
    for &d in &out {
        if !seen.insert(d) {
            return Err(LayoutError::DuplicateDiscriminant {
                enum_name: name.to_string(),
                value: d,
            });
        }
    }
    Ok(out)
}

/// The narrowest integer type that holds every discriminant.
fn tag_type(discriminants: &[i128]) -> Result<IntTy, LayoutError> {
    let widest = discriminants
        .iter()
        .copied()
        .max_by_key(|d| Bt::from_i128(*d).trit_len())
        .unwrap_or(0);
    let trits = Bt::from_i128(widest).trit_len();
    if trits <= 9 {
        Ok(IntTy::T9)
    } else if trits <= 27 {
        Ok(IntTy::T27)
    } else {
        Err(LayoutError::DiscriminantTooWide(widest))
    }
}

fn enum_layout(
    db: &TypeDb,
    name: &str,
    def: &EnumDef,
    visiting: &mut BTreeSet<String>,
) -> Result<Layout, LayoutError> {
    let discs = discriminants(name, def)?;

    // Lay out each variant's payload as a product.
    let mut payloads = Vec::new();
    for v in &def.variants {
        let fields = v
            .fields
            .iter()
            .map(|(_, t)| compute(db, t, visiting))
            .collect::<Result<Vec<_>, _>>()?;
        payloads.push((aggregate(&fields, def.repr), fields));
    }

    // A three-variant fieldless enum whose discriminants are −1, 0, +1 is
    // representation-identical to `trit`, and `match` over it lowers to a
    // single `br3` (§5.2).
    let all_fieldless = def.variants.iter().all(|v| v.fields.is_empty());
    if def.repr == Repr::Lang
        && all_fieldless
        && discs.len() == 3
        && discs.iter().copied().collect::<BTreeSet<_>>() == BTreeSet::from([-1, 0, 1])
    {
        return Ok(Layout {
            size: 1,
            align: 1,
            offsets: Vec::new(),
            enum_layout: Some(EnumLayout {
                tag: Tag::TritShaped,
                discriminants: discs,
                variant_offsets: vec![Vec::new(); def.variants.len()],
            }),
            niches: capacity(1) - 3,
            // Representation-identical to `trit`, niches included.
            niche: Some(Niche {
                offset: 0,
                trits: TRITS_PER_TRYTE,
                lo: -1,
                hi: 1,
            }),
        });
    }

    // A single variant has nothing to distinguish, so it is simply its
    // payload — checked before the niche rule, which would otherwise "encode"
    // a discriminant that does not exist.
    if def.variants.len() == 1 {
        let (payload, _) = &payloads[0];
        return Ok(Layout {
            size: payload.size,
            align: payload.align,
            offsets: Vec::new(),
            enum_layout: Some(EnumLayout {
                tag: Tag::None,
                discriminants: discs,
                variant_offsets: vec![payload.offsets.clone()],
            }),
            niches: payload.niches,
            niche: payload.niche.clone(),
        });
    }

    // Niche encoding: exactly one variant carries a payload, the rest are
    // fieldless, and the payload has invalid representations to spare. The
    // discriminant then costs no space at all (§6).
    let carriers: Vec<usize> = (0..def.variants.len())
        .filter(|&i| !def.variants[i].fields.is_empty())
        .collect();
    if def.repr == Repr::Lang && carriers.len() == 1 {
        let untagged = carriers[0];
        let (payload, fields) = &payloads[untagged];
        let needed = (def.variants.len() - 1) as u128;
        if payload.niches >= needed
            && payload.size > 0
            && let Some(spot) = payload.niche.clone()
            && spot.count() >= needed
        {
            let mut variant_offsets = vec![Vec::new(); def.variants.len()];
            variant_offsets[untagged] = payload.offsets.clone();
            let _ = fields;
            return Ok(Layout {
                size: payload.size,
                align: payload.align,
                offsets: Vec::new(),
                enum_layout: Some(EnumLayout {
                    tag: Tag::Niche {
                        untagged,
                        offset: spot.offset,
                        used: needed,
                        spot: spot.clone(),
                    },
                    discriminants: discs,
                    variant_offsets,
                }),
                // The niches this enum did not consume remain available to
                // whatever encloses it — which is why nesting up to the
                // budget adds no size (§6, guarantee 3).
                niches: payload.niches - needed,
                niche: Some(spot.consume(needed)),
            });
        }
    }

    // Otherwise: a leading tag followed by the payloads, union-style, at the
    // payload's natural alignment (§5.1). `repr(linear)` requires exactly
    // this; `repr(lang)` falls back to it when no niche is available.
    let tag = tag_type(&discs)?;
    let tag_size = tag.trits().div_ceil(TRITS_PER_TRYTE);
    let payload_align = payloads.iter().map(|(p, _)| p.align).max().unwrap_or(1);
    let align = tag_size.max(payload_align);
    let payload_at = align_to(tag_size, payload_align);
    let payload_size = payloads.iter().map(|(p, _)| p.size).max().unwrap_or(0);
    let size = align_to(payload_at + payload_size, align);

    let variant_offsets = payloads
        .iter()
        .map(|(p, _)| p.offsets.iter().map(|o| o + payload_at).collect())
        .collect();

    // The tag's unused values are themselves niches: a t9 tag with three
    // variants leaves 19 680 patterns invalid.
    let tag_niches = capacity(tag_size).saturating_sub(discs.len() as u128);

    Ok(Layout {
        size,
        align,
        offsets: Vec::new(),
        enum_layout: Some(EnumLayout {
            tag: Tag::Direct { ty: tag, offset: 0 },
            discriminants: discs,
            variant_offsets,
        }),
        niches: tag_niches,
        // The tag's unused values are themselves niches, but they are
        // contiguous only if the discriminants are — not worth assuming, so
        // an enclosing type is offered none.
        niche: None,
    })
}
