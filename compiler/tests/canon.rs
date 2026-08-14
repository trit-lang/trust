//! Canonicalization: what it promotes, what it refuses, and that the answer
//! never changes.
//!
//! The interpreter is the oracle. A TIR → TIR pass is correct iff it
//! preserves observable AM behavior, so every test here runs the module both
//! as written and after the pass and demands the same result — the same
//! criterion `legalize_semantics.rs` applies to legalization.

use trit_core::{FaultCode, Tint};
use trustc::tir::{self, Halt, Interp, Val};

type Outcome = Result<i128, FaultCode>;

fn parse(src: &str) -> tir::Module {
    let m = tir::parse_module(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
    let errs = tir::verify(&m);
    assert!(errs.is_empty(), "input does not verify: {errs:?}");
    m
}

fn interpret(m: &tir::Module, entry: &str, args: &[i128]) -> Outcome {
    let f = m.function(entry).expect("entry exists");
    let vals: Vec<Val> = f
        .sig
        .params
        .iter()
        .zip(args)
        .map(|((_, t), &a)| {
            Val::Int(Tint::new(t.width().expect("integer parameter"), a).expect("fits"))
        })
        .collect();
    match Interp::new(m).call(entry, &vals) {
        Ok(Some(Val::Int(i))) => Ok(i.to_i128().expect("fits")),
        Ok(other) => panic!("expected an integer result, got {other:?}"),
        Err(Halt::Fault(f)) => Err(f.code),
        Err(other) => panic!("interpreter stopped: {other}"),
    }
}

/// Canonicalize, check the result is still well-formed TIR, and demand the
/// same answers as the module as written.
fn canonical(src: &str, entry: &str, cases: &[&[i128]]) -> tir::Module {
    let m = parse(src);
    let out = tir::canonicalize_module(&m);
    let errs = tir::verify(&out);
    assert!(
        errs.is_empty(),
        "canonicalized module does not verify: {errs:?}\n{}",
        tir::print_module(&out)
    );
    for args in cases {
        assert_eq!(
            interpret(&m, entry, args),
            interpret(&out, entry, args),
            "@{entry}{args:?} changed under canonicalization\n{}",
            tir::print_module(&out)
        );
    }
    out
}

fn slots(m: &tir::Module, f: &str) -> usize {
    m.function(f)
        .expect("the function")
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i.kind, tir::InstKind::Slot { .. }))
        .count()
}

#[test]
fn a_parameter_written_to_its_slot_and_read_back_becomes_the_parameter() {
    // The shape the frontend emits for every function: `lang/lower.rs` gives
    // every local a slot, and a parameter arrives as an SSA value that is
    // immediately stored into one. Nothing else touches the slot, so it is
    // the value.
    let src = r#"tir 0.1 target "tritium"

fn @f(%a: t27, %b: t27) -> t27 {
^entry:
    %pa = slot tryte[3]
    %pb = slot tryte[3]
    store t27 %a, %pa
    store t27 %b, %pb
    %x = load t27 %pa
    %y = load t27 %pb
    %p = mul.wrap t27 %x, %y
    %q = load t27 %pa
    %r = add.wrap t27 %p, %q
    ret %r
}
"#;
    let out = canonical(src, "f", &[&[0, 0], &[3, 5], &[-7, 11], &[9841, -3]]);
    assert_eq!(slots(&out, "f"), 0, "{}", tir::print_module(&out));
    // And with the slots gone, so are the accesses.
    let insts: usize = out.function("f").unwrap().blocks[0].insts.len();
    assert_eq!(insts, 2, "{}", tir::print_module(&out));
}

#[test]
fn a_later_store_is_what_a_later_load_sees() {
    let src = r#"tir 0.1 target "tritium"

fn @f(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %a, %p
    %x = load t27 %p
    store t27 const t27 5, %p
    %y = load t27 %p
    %r = add.wrap t27 %x, %y
    ret %r
}
"#;
    let out = canonical(src, "f", &[&[0], &[1], &[-100]]);
    assert_eq!(slots(&out, "f"), 0);
}

#[test]
fn a_slot_whose_address_gets_out_is_left_alone() {
    // Three ways for the address to escape, and each has to stop the pass.
    // `@stored_as_a_value` has to hand `%q` to something, or promoting `%q`
    // would remove the store that let `%p` out and both would be promotable
    // after all — which is correct, and not what this test is about.
    // TIR §5's provenance rules mean an address that got out may be read
    // through later, so "nothing else names it" is the whole condition.
    let src = r#"tir 0.1 target "tritium"

fn @sink(%p: ptr) -> t27 {
^entry:
    %v = load t27 %p
    ret %v
}

fn @passed(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %a, %p
    %r = call @sink(%p) -> t27
    ret %r
}

fn @offset_into(%a: t27) -> t27 {
^entry:
    %p = slot tryte[9]
    store t27 %a, %p
    %q = offset %p, const t27 3
    store t27 const t27 4, %q
    %v = load t27 %p
    %w = load t27 %q
    %r = add.wrap t27 %v, %w
    ret %r
}

fn @via(%pp: ptr) -> t27 {
^entry:
    %inner = load ptr %pp
    %v = load t27 %inner
    ret %v
}

fn @stored_as_a_value(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    %q = slot tryte[3]
    store ptr %p, %q
    store t27 %a, %p
    %r = call @via(%q) -> t27
    ret %r
}
"#;
    let cases: &[&[i128]] = &[&[0], &[7], &[-7]];
    for entry in ["passed", "offset_into", "stored_as_a_value"] {
        let out = canonical(src, entry, cases);
        assert!(
            slots(&out, entry) > 0,
            "@{entry}'s slot escaped and must survive:\n{}",
            tir::print_module(&out)
        );
    }
}

#[test]
fn a_slot_read_before_it_is_written_is_left_alone() {
    // Reading uninitialized `slot` storage is UB and yields poison (TIR §4
    // item 4). A pass that promoted this would be choosing what poison is.
    let src = r#"tir 0.1 target "tritium"

fn @f(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    %v = load t27 %p
    store t27 %a, %p
    %w = load t27 %p
    %r = add.wrap t27 %v, %w
    ret %r
}
"#;
    let m = parse(src);
    let out = tir::canonicalize_module(&m);
    assert!(tir::verify(&out).is_empty());
    assert_eq!(slots(&out, "f"), 1, "{}", tir::print_module(&out));
}

#[test]
fn a_slot_read_at_two_widths_is_left_alone() {
    // Two access types means the storage is being reinterpreted, and "the
    // value in the slot" is then not one value.
    let src = r#"tir 0.1 target "tritium"

fn @f(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %a, %p
    %n = load t9 %p
    %w = widen t9 %n -> t27
    ret %w
}
"#;
    let m = parse(src);
    let out = tir::canonicalize_module(&m);
    assert!(tir::verify(&out).is_empty());
    assert_eq!(slots(&out, "f"), 1, "{}", tir::print_module(&out));
}

#[test]
fn a_forwarded_value_is_renamed_everywhere_it_was_used() {
    // The slot never leaves the block, but the *load's result* does. Deleting
    // the load without renaming its uses in the blocks below would leave the
    // function referring to a value that no longer exists — which the
    // verifier catches, and did.
    let src = r#"tir 0.1 target "tritium"

fn @f(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %a, %p
    %v = load t27 %p
    %c = cmp t27 %v, const t27 0
    br3 %c, ^neg, ^zero, ^pos
^neg:
    %n = neg t27 %v
    ret %n
^zero:
    ret const t27 0
^pos:
    %d = mul.wrap t27 %v, const t27 2
    ret %d
}
"#;
    let out = canonical(src, "f", &[&[-5], &[0], &[5]]);
    assert_eq!(slots(&out, "f"), 0, "{}", tir::print_module(&out));
}

#[test]
fn a_branch_on_a_select_of_constants_branches_on_the_selector() {
    // The frontend has no `bool`-producing comparison: `i < n` is a `cmp`
    // and then a `select3` turning its trit into a `bool`, and `br2` then
    // asks for the sign of that `bool` — which is the trit again. The branch
    // reads the comparison directly, with each arm sent where the constant it
    // would have produced points.
    let src = r#"tir 0.1 target "tritium"

fn @less(%i: t27, %n: t27) -> t27 {
^entry:
    %c = cmp t27 %i, %n
    %b = select3 %c, t1 const t1 1, const t1 0, const t1 0
    br2 %b, ^yes, ^no
^yes:
    ret const t27 1
^no:
    ret const t27 0
}
"#;
    let out = canonical(
        src,
        "less",
        &[&[0, 0], &[1, 2], &[2, 1], &[-5, 5], &[9841, -9841]],
    );
    let f = out.function("less").unwrap();
    // The `select3` is dead and gone, and the block holds only the compare.
    assert_eq!(f.blocks[0].insts.len(), 1, "{}", tir::print_module(&out));
    match &f.blocks[0].term {
        tir::Terminator::Br3 { t, neg, zero, pos } => {
            assert_eq!(t, &tir::Operand::Value("c".into()));
            // `%c` negative chose 1, which is positive, which went to ^yes.
            assert_eq!(neg.label, "yes");
            assert_eq!(zero.label, "no");
            assert_eq!(pos.label, "no");
        }
        other => panic!("expected a branch, got {other:?}"),
    }
}

#[test]
fn a_select_something_else_still_reads_is_left_where_it_is() {
    // The branch is rewritten either way; the `select3` survives because
    // `^yes` names it, and only then is nothing gained but nothing lost.
    let src = r#"tir 0.1 target "tritium"

fn @f(%i: t27, %n: t27) -> t27 {
^entry:
    %c = cmp t27 %i, %n
    %b = select3 %c, t1 const t1 1, const t1 0, const t1 0
    br2 %b, ^yes, ^no
^yes:
    %w = widen t1 %b -> t27
    ret %w
^no:
    ret const t27 0
}
"#;
    let out = canonical(src, "f", &[&[1, 2], &[2, 1], &[3, 3]]);
    assert_eq!(
        out.function("f").unwrap().blocks[0].insts.len(),
        2,
        "{}",
        tir::print_module(&out)
    );
}

#[test]
fn what_cannot_fault_and_nothing_reads_is_not_emitted() {
    // And what can fault stays, whether anything reads it or not: `@f`'s
    // dead `div` still has to raise F_DIVZERO.
    let src = r#"tir 0.1 target "tritium"

fn @pure(%x: t27) -> t27 {
^entry:
    %a = mul.wrap t27 %x, const t27 3
    %b = cmp t27 %x, const t27 0
    %c = tmin t27 %x, const t27 7
    %d = add.wrap t27 %x, const t27 1
    ret %d
}

fn @faulting(%x: t27) -> t27 {
^entry:
    %q = div t27 const t27 1, %x
    %s = shl.wrap t27 %x, %x
    %o = add.trap t27 %x, const t27 3812798742493
    ret %x
}
"#;
    let out = canonical(src, "pure", &[&[0], &[5], &[-5]]);
    assert_eq!(
        out.function("pure").unwrap().blocks[0].insts.len(),
        1,
        "{}",
        tir::print_module(&out)
    );

    // Three instructions that can raise, none of whose results is read.
    assert_eq!(out.function("faulting").unwrap().blocks[0].insts.len(), 3);
    // Dividing by zero still faults, which is the whole point of keeping it.
    let m = parse(src);
    let after = tir::canonicalize_module(&m);
    assert_eq!(
        interpret(&m, "faulting", &[0]),
        interpret(&after, "faulting", &[0])
    );
    assert!(interpret(&after, "faulting", &[0]).is_err());
}

#[test]
fn a_slot_read_in_another_block_becomes_a_block_parameter() {
    // What `promote_slots` cannot do and `mem2reg` can: the value at the
    // load depends on which path arrived, so the answer is a parameter on
    // the block — and where the predecessors agree, no parameter at all.
    let src = r#"tir 0.1 target "tritium"

fn @same(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    store t27 %a, %p
    %c = cmp t27 %a, const t27 0
    br3 %c, ^other, ^other, ^other
^other:
    %v = load t27 %p
    ret %v
}

fn @differs(%a: t27) -> t27 {
^entry:
    %p = slot tryte[3]
    %c = cmp t27 %a, const t27 0
    br3 %c, ^lo, ^zero, ^hi
^lo:
    store t27 const t27 -1, %p
    br ^join
^zero:
    store t27 const t27 0, %p
    br ^join
^hi:
    store t27 %a, %p
    br ^join
^join:
    %v = load t27 %p
    %d = mul.wrap t27 %v, const t27 2
    ret %d
}

fn @counts(%n: t27) -> t27 {
^entry:
    %i = slot tryte[3]
    %s = slot tryte[3]
    store t27 const t27 0, %i
    store t27 const t27 0, %s
    br ^head
^head:
    %iv = load t27 %i
    %c = cmp t27 %iv, %n
    br3 %c, ^body, ^done, ^done
^body:
    %sv = load t27 %s
    %s2 = add.wrap t27 %sv, %iv
    store t27 %s2, %s
    %i2 = add.wrap t27 %iv, const t27 1
    store t27 %i2, %i
    br ^head
^done:
    %r = load t27 %s
    ret %r
}
"#;
    // Every path stores the same thing, so the parameter that would carry it
    // is trivial and is removed again.
    let out = canonical(src, "same", &[&[-5], &[0], &[5]]);
    assert_eq!(slots(&out, "same"), 0, "{}", tir::print_module(&out));
    assert!(
        out.function("same").unwrap().blocks[1].params.is_empty(),
        "the predecessors agree:\n{}",
        tir::print_module(&out)
    );

    // Here they do not, and `^join` takes one.
    let out = canonical(src, "differs", &[&[-5], &[0], &[5], &[9841]]);
    assert_eq!(slots(&out, "differs"), 0, "{}", tir::print_module(&out));
    let join = out
        .function("differs")
        .unwrap()
        .blocks
        .iter()
        .find(|b| b.label == "join")
        .expect("^join");
    assert_eq!(join.params.len(), 1, "{}", tir::print_module(&out));

    // A loop: the counter and the accumulator both come round the back edge,
    // so `^head` takes two — and no more, because the parameters a loop does
    // not need are trivial and removed.
    let out = canonical(src, "counts", &[&[0], &[1], &[5], &[50]]);
    assert_eq!(slots(&out, "counts"), 0, "{}", tir::print_module(&out));
    let head = out
        .function("counts")
        .unwrap()
        .blocks
        .iter()
        .find(|b| b.label == "head")
        .expect("^head");
    assert_eq!(head.params.len(), 2, "{}", tir::print_module(&out));
}
