//! Layout tests (Language Ch. 2).
//!
//! The chapter ends with a table of worked layout examples "assuming the AM
//! (tryte-addressable, alignments 1 and 3)". That table is the core of this
//! file, followed by the §6 niche guarantees, which the chapter explicitly
//! elevates "from implementation detail to spec" — so they are promises to
//! user code, not just observations.

use trustc::layout::*;

fn db() -> TypeDb {
    TypeDb::new()
}

fn layout(db: &TypeDb, ty: &Ty) -> Layout {
    layout_of(db, ty).unwrap_or_else(|e| panic!("no layout for {ty:?}: {e}"))
}

fn size_align(db: &TypeDb, ty: &Ty) -> (u32, u32) {
    let l = layout(db, ty);
    (l.size, l.align)
}

const T9: Ty = Ty::Int(IntTy::T9);
const T27: Ty = Ty::Int(IntTy::T27);

// ------------------------------------------------------------------- scalars

#[test]
fn scalar_sizes_and_alignments_match_chapter_one() {
    // Ch. 1 §2's table, which Ch. 2 §1 builds on.
    let db = db();
    assert_eq!(size_align(&db, &Ty::Trit), (1, 1));
    assert_eq!(size_align(&db, &Ty::Bool), (1, 1));
    assert_eq!(size_align(&db, &T9), (1, 1));
    assert_eq!(size_align(&db, &T27), (3, 3));
    assert_eq!(size_align(&db, &Ty::Int(IntTy::TAddr)), (3, 3));
    assert_eq!(size_align(&db, &Ty::Unit), (0, 1));
    assert!(layout(&db, &Ty::Unit).is_zst());
}

#[test]
fn size_is_always_a_multiple_of_alignment() {
    // Ch. 2 §1 — which is why arrays need no inter-element padding.
    let mut db = db();
    db.struct_("A", Repr::Linear, vec![("a", T9), ("b", T27)])
        .struct_(
            "B",
            Repr::Lang,
            vec![("a", Ty::Trit), ("b", T27), ("c", T9)],
        )
        .struct_("C", Repr::Linear, vec![("a", Ty::Bool)]);
    for name in ["A", "B", "C"] {
        let l = layout(&db, &Ty::named(name));
        assert_eq!(l.size % l.align, 0, "{name}");
        assert!(l.align.is_power_of_two() || [1, 3, 9, 27].contains(&l.align));
    }
}

// ------------------------------------------------- the appendix's worked table

#[test]
fn appendix_tuple_of_two_t9() {
    // (t9, t9) | 2 | 1 | no padding possible
    let db = db();
    let t = Ty::Tuple(vec![T9, T9]);
    assert_eq!(size_align(&db, &t), (2, 1));
    assert_eq!(layout(&db, &t).offsets, vec![0, 1]);
}

#[test]
fn appendix_struct_t9_then_t27_linear() {
    // struct { a: t9, b: t27 } repr(linear) | 6 | 3 | a at 0, 2 trytes
    // padding, b at 3
    let mut db = db();
    db.struct_("S", Repr::Linear, vec![("a", T9), ("b", T27)]);
    let l = layout(&db, &Ty::named("S"));
    assert_eq!((l.size, l.align), (6, 3));
    assert_eq!(l.offsets, vec![0, 3]);
}

#[test]
fn appendix_the_same_struct_under_repr_lang_still_pads_to_six() {
    // "reordering (b at 0, a at 3) still pads to 6: size must be a multiple
    // of align — an example of padding that reordering cannot remove"
    let mut db = db();
    db.struct_("S", Repr::Lang, vec![("a", T9), ("b", T27)]);
    let l = layout(&db, &Ty::named("S"));
    assert_eq!((l.size, l.align), (6, 3));
    // Offsets are reported in declaration order, and the compiler did reorder.
    assert_eq!(l.offsets, vec![3, 0]);
}

#[test]
fn appendix_declaration_order_already_optimal() {
    // struct { a: t9, b: t9, c: t27 } repr(linear) | 6 | 3 | a at 0, b at 1,
    // 1 tryte padding, c at 3
    let mut db = db();
    db.struct_("S", Repr::Linear, vec![("a", T9), ("b", T9), ("c", T27)]);
    let l = layout(&db, &Ty::named("S"));
    assert_eq!((l.size, l.align), (6, 3));
    assert_eq!(l.offsets, vec![0, 1, 3]);

    // Under repr(lang) the same fields cost the same: the reordering
    // obligation is mild, exactly as §4 says.
    db.struct_("T", Repr::Lang, vec![("a", T9), ("b", T9), ("c", T27)]);
    let t = layout(&db, &Ty::named("T"));
    assert_eq!((t.size, t.align), (6, 3));
}

#[test]
fn appendix_trailing_padding() {
    // struct { a: t27, b: t9 } repr(linear) | 6 | 3 | b at 3, 2 trytes
    // trailing padding
    let mut db = db();
    db.struct_("S", Repr::Linear, vec![("a", T27), ("b", T9)]);
    let l = layout(&db, &Ty::named("S"));
    assert_eq!((l.size, l.align), (6, 3));
    assert_eq!(l.offsets, vec![0, 3]);
}

#[test]
fn appendix_trit_array_is_unpacked() {
    // [trit; 9] | 9 | 1 | unpacked, one trit per tryte
    //
    // Ch. 2 §3 claims this loudly to stop `[trit; N]` from being "fixed" to
    // pack; the packed form is the library type TritSlab.
    let db = db();
    let t = Ty::array(Ty::Trit, 9);
    assert_eq!(size_align(&db, &t), (9, 1));
    assert_eq!(layout(&db, &t).offsets, (0..9).collect::<Vec<_>>());
}

#[test]
fn appendix_option_trit_is_one_tryte() {
    // Option<trit> | 1 | 1 | niche
    let db = db();
    assert_eq!(size_align(&db, &Ty::option(Ty::Trit)), (1, 1));
}

#[test]
fn appendix_a_trit_shaped_enum() {
    // enum { A=-1, B, C=1 } | 1 | 1 | trit-shaped; match → br3
    //
    // Note what this entry settles: `B` has no explicit discriminant, and the
    // entry only holds if it is 0 — so an unassigned discriminant continues
    // from the previous variant rather than taking its positional index.
    let mut db = db();
    db.enum_(
        "Sign",
        Repr::Lang,
        vec![
            Variant::unit("A").at(-1),
            Variant::unit("B"),
            Variant::unit("C").at(1),
        ],
    );
    let l = layout(&db, &Ty::named("Sign"));
    assert_eq!((l.size, l.align), (1, 1));
    let e = l.enum_layout.expect("an enum");
    assert_eq!(e.discriminants, vec![-1, 0, 1]);
    assert_eq!(e.tag, Tag::TritShaped);
}

// ------------------------------------------------------------- discriminants

#[test]
fn discriminants_default_in_declaration_order_and_may_be_negative() {
    let mut db = db();
    db.enum_(
        "E",
        Repr::Lang,
        vec![
            Variant::unit("A"),
            Variant::unit("B"),
            Variant::unit("C"),
            Variant::unit("D"),
        ],
    );
    let l = layout(&db, &Ty::named("E"));
    assert_eq!(l.enum_layout.unwrap().discriminants, vec![0, 1, 2, 3]);

    // "may be negative — the balanced range is symmetric and the language
    // does not pretend otherwise" (§5.1).
    db.enum_(
        "F",
        Repr::Lang,
        vec![
            Variant::unit("A").at(-100),
            Variant::unit("B"),
            Variant::unit("C").at(-1),
        ],
    );
    let l = layout(&db, &Ty::named("F"));
    assert_eq!(l.enum_layout.unwrap().discriminants, vec![-100, -99, -1]);
}

#[test]
fn two_variants_with_the_same_discriminant_are_ill_formed() {
    let mut db = db();
    db.enum_(
        "E",
        Repr::Lang,
        vec![
            Variant::unit("A").at(1),
            Variant::unit("B").at(2),
            Variant::unit("C").at(1),
        ],
    );
    assert_eq!(
        layout_of(&db, &Ty::named("E")),
        Err(LayoutError::DuplicateDiscriminant {
            enum_name: "E".into(),
            value: 1
        })
    );
}

#[test]
fn a_linear_enum_leads_with_its_tag() {
    // §5.1: repr(linear) enums store the discriminant as a leading t9 (or t27
    // if any explicit discriminant requires it) followed by the payload,
    // union-style, at the payload's natural alignment.
    let mut db = db();
    db.enum_(
        "Shape",
        Repr::Linear,
        vec![
            Variant::unit("Dot"),
            Variant::payload("Line", T27),
            Variant::unit("Rect"),
        ],
    );
    let l = layout(&db, &Ty::named("Shape"));
    let e = l.enum_layout.clone().expect("an enum");
    assert_eq!(
        e.tag,
        Tag::Direct {
            ty: IntTy::T9,
            offset: 0
        }
    );
    // Tag in tryte 0, payload aligned to 3, so it starts at tryte 3.
    assert_eq!(e.variant_offsets[1], vec![3]);
    assert_eq!((l.size, l.align), (6, 3));

    // A discriminant too wide for t9 widens the tag to t27.
    db.enum_(
        "Wide",
        Repr::Linear,
        vec![Variant::unit("A").at(100_000), Variant::unit("B")],
    );
    let l = layout(&db, &Ty::named("Wide"));
    assert!(matches!(
        l.enum_layout.unwrap().tag,
        Tag::Direct { ty: IntTy::T27, .. }
    ));
}

// --------------------------------------------------- §6 niche guarantees

#[test]
fn ternary_scalars_are_unusually_niche_rich() {
    // §6's own arithmetic: a bool uses 2 of 19 683 patterns → 19 681 niches;
    // a trit uses 3 → 19 680.
    let db = db();
    assert_eq!(layout(&db, &Ty::Bool).niches, 19_681);
    assert_eq!(layout(&db, &Ty::Trit).niches, 19_680);
    // Integers use every pattern, so they offer none.
    assert_eq!(layout(&db, &T9).niches, 0);
    assert_eq!(layout(&db, &T27).niches, 0);
    // A reference excludes the null address → at least one niche.
    assert!(layout(&db, &Ty::reference(T27)).niches >= 1);
}

#[test]
fn guarantee_one_option_of_a_reference_is_pointer_sized() {
    let db = db();
    let ptr = size_align(&db, &Ty::reference(T27));
    assert_eq!(size_align(&db, &Ty::option(Ty::reference(T27))), ptr);
    assert_eq!(size_align(&db, &Ty::option(Ty::boxed(T9))), ptr);
    assert_eq!(ptr, (3, 3));
}

#[test]
fn guarantee_two_option_of_bool_or_trit_is_one_tryte() {
    let db = db();
    assert_eq!(size_align(&db, &Ty::option(Ty::Bool)), (1, 1));
    assert_eq!(size_align(&db, &Ty::option(Ty::Trit)), (1, 1));

    // "any enum with ≤ 19 680 fieldless variants wrapped around a bool/trit
    // payload occupy one tryte".
    let mut db = db;
    let many = |n: usize| {
        let mut variants = vec![Variant::payload("Some", Ty::Trit)];
        variants.extend((0..n).map(|i| Variant::unit(&format!("N{i}"))));
        variants
    };
    db.enum_("Big", Repr::Lang, many(19_680));
    assert_eq!(size_align(&db, &Ty::named("Big")), (1, 1));

    // One more fieldless variant than there are niches, and it no longer
    // fits: the guarantee is exact, not approximate.
    db.enum_("TooBig", Repr::Lang, many(19_681));
    let l = layout(&db, &Ty::named("TooBig"));
    assert!(l.size > 1, "expected a tag, got {l:?}");
}

#[test]
fn guarantee_three_nesting_within_the_budget_adds_no_size() {
    let db = db();
    // Option<Option<trit>> is still one tryte.
    let nested = Ty::option(Ty::option(Ty::Trit));
    assert_eq!(size_align(&db, &nested), (1, 1));
    // And it keeps nesting, because each layer consumes exactly one niche.
    let deep = Ty::option(Ty::option(Ty::option(Ty::option(Ty::Trit))));
    assert_eq!(size_align(&db, &deep), (1, 1));
    assert_eq!(layout(&db, &nested).niches, 19_680 - 2);
}

#[test]
fn option_of_bool_is_kleene_logics_value_space() {
    // §6: the "unknown / false / true" of three-valued logic is a one-tryte
    // type here with full type safety.
    let db = db();
    let l = layout(&db, &Ty::option(Ty::Bool));
    assert_eq!((l.size, l.align), (1, 1));
    let e = l.enum_layout.expect("an enum");
    assert!(matches!(e.tag, Tag::Niche { .. }));
    assert_eq!(e.discriminants, vec![0, 1]);
}

#[test]
fn a_niche_enum_costs_nothing_over_its_payload() {
    let mut db = db();
    db.struct_("Pair", Repr::Lang, vec![("a", Ty::Bool), ("b", T27)]);
    let bare = size_align(&db, &Ty::named("Pair"));
    // The bool inside the struct still has its niches, so wrapping the whole
    // struct in an Option is free.
    assert_eq!(size_align(&db, &Ty::option(Ty::named("Pair"))), bare);
}

#[test]
fn an_option_of_an_integer_has_to_pay_for_its_tag() {
    // t9 has no invalid representations, so there is no niche to hide in.
    let db = db();
    let l = layout(&db, &Ty::option(T9));
    assert!(l.size > 1, "{l:?}");
    assert!(matches!(l.enum_layout.unwrap().tag, Tag::Direct { .. }));
}

// -------------------------------------------------- §8 recursion and errors

#[test]
fn a_directly_recursive_type_is_ill_formed() {
    // §8: `struct Node { next: Node }` is ill-formed.
    let mut db = db();
    db.struct_("Node", Repr::Lang, vec![("next", Ty::named("Node"))]);
    assert_eq!(
        layout_of(&db, &Ty::named("Node")),
        Err(LayoutError::Infinite("Node".into()))
    );
}

#[test]
fn recursion_through_indirection_is_the_pattern() {
    // §8: `struct Node { next: Option<Box<Node>> }` — "and by §6.1 the Option
    // is free".
    let mut db = db();
    db.struct_(
        "Node",
        Repr::Lang,
        vec![
            ("value", T27),
            ("next", Ty::option(Ty::boxed(Ty::named("Node")))),
        ],
    );
    let l = layout(&db, &Ty::named("Node"));
    assert_eq!((l.size, l.align), (6, 3));
    // The Option really is free: the same struct with a bare Box is the same
    // size.
    db.struct_(
        "Bare",
        Repr::Lang,
        vec![("value", T27), ("next", Ty::boxed(Ty::named("Node")))],
    );
    assert_eq!(size_align(&db, &Ty::named("Bare")), (l.size, l.align));
}

#[test]
fn mutual_recursion_without_indirection_is_also_caught() {
    let mut db = db();
    db.struct_("A", Repr::Lang, vec![("b", Ty::named("B"))]);
    db.struct_("B", Repr::Lang, vec![("a", Ty::named("A"))]);
    assert!(matches!(
        layout_of(&db, &Ty::named("A")),
        Err(LayoutError::Infinite(_))
    ));
}

#[test]
fn a_negative_array_length_is_ill_formed() {
    // §3: negative indices are not Python-style end-relative aliases, and a
    // negative length is simply ill-formed.
    let db = db();
    assert_eq!(
        layout_of(&db, &Ty::array(T9, -1)),
        Err(LayoutError::NegativeArrayLength(-1))
    );
}

#[test]
fn an_unknown_type_is_reported() {
    let db = db();
    assert_eq!(
        layout_of(&db, &Ty::named("Nope")),
        Err(LayoutError::Unknown("Nope".into()))
    );
}

// ------------------------------------------------------------------- ZSTs

#[test]
fn zero_sized_types_compose() {
    // §1: unit structs and empty tuples of ZSTs are ZSTs; an array of ZSTs is
    // itself a ZST regardless of length.
    let mut db = db();
    db.struct_("Marker", Repr::Lang, vec![]);
    assert_eq!(size_align(&db, &Ty::named("Marker")), (0, 1));
    assert_eq!(size_align(&db, &Ty::Tuple(vec![])), (0, 1));
    assert_eq!(
        size_align(&db, &Ty::Tuple(vec![Ty::Unit, Ty::named("Marker")])),
        (0, 1)
    );
    assert_eq!(size_align(&db, &Ty::array(Ty::Unit, 1_000)), (0, 1));
}

#[test]
fn a_one_variant_enum_is_its_payload() {
    let mut db = db();
    db.enum_("Only", Repr::Lang, vec![Variant::payload("It", T27)]);
    let l = layout(&db, &Ty::named("Only"));
    assert_eq!((l.size, l.align), (3, 3));
    assert_eq!(l.enum_layout.unwrap().tag, Tag::None);
}

#[test]
fn arrays_have_defined_layout_even_under_repr_lang() {
    // §3: "array layout is fully defined even under repr(lang), because
    // iteration and indexing arithmetic depend on it".
    let mut db = db();
    db.struct_("S", Repr::Lang, vec![("a", T9), ("b", T27)]);
    let elem = layout(&db, &Ty::named("S"));
    let arr = layout(&db, &Ty::array(Ty::named("S"), 4));
    assert_eq!(arr.size, 4 * elem.size);
    assert_eq!(arr.align, elem.align);
    assert_eq!(
        arr.offsets,
        (0..4).map(|i| i * elem.size).collect::<Vec<_>>()
    );
}

#[test]
fn a_niche_is_used_however_wide_the_payload_is() {
    // §6's rule is about the payload having invalid representations to
    // spare, and a payload wider than eight trytes has as many as a narrow
    // one. Draft 0.1 asked `payload.niches >= needed` first, and that count
    // is 3^n over the payload's whole width — it saturated at `u128::MAX`
    // and then read as zero once the subtraction was done, so §6 was
    // quietly off for every large payload (G9.49).
    let mut db = TypeDb::new();
    db.struct_(
        "Wide",
        Repr::Lang,
        vec![
            ("r", Ty::reference(Ty::Int(IntTy::T27))),
            ("a", Ty::Int(IntTy::TAddr)),
        ],
    );
    db.enum_(
        "Maybe",
        Repr::Lang,
        vec![
            Variant::unit("None"),
            Variant::payload("Some", Ty::named("Wide")),
        ],
    );
    let wide = layout_of(&db, &Ty::named("Wide")).expect("a layout");
    let maybe = layout_of(&db, &Ty::named("Maybe")).expect("a layout");
    assert_eq!(
        maybe.size, wide.size,
        "the discriminant goes in the reference's niche and costs nothing"
    );
    assert!(matches!(
        maybe.enum_layout.expect("an enum").tag,
        Tag::Niche { .. }
    ));
}
