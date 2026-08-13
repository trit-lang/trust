//! Assembler tests (`spec/isa/assembly-0.1.md`).

use tritium::{Inst, Io, Stop, Vm, assemble};

/// Assemble, or panic with the diagnostics.
fn asm(src: &str) -> Vec<i16> {
    assemble(src).unwrap_or_else(|e| panic!("assembly failed: {e:?}\n{src}"))
}

/// The message of the first error, for checking diagnostics.
fn error(src: &str) -> String {
    match assemble(src) {
        Err(errs) => errs[0].message.clone(),
        Ok(_) => panic!("expected an error from:\n{src}"),
    }
}

/// The words of an assembled image.
fn words(image: &[i16]) -> Vec<i128> {
    image
        .chunks(3)
        .map(|c| {
            c.iter()
                .enumerate()
                .map(|(i, &t)| t as i128 * 3i128.pow(9 * i as u32))
                .sum()
        })
        .collect()
}

/// Assemble and run, returning how it stopped and what it wrote.
fn run(src: &str, input: &[u8]) -> (Stop, Vec<u8>) {
    let mut vm = Vm::with_default_memory();
    vm.io = Io::with_input(input);
    vm.load_image(&asm(src));
    let stop = vm.run(1_000_000);
    (stop, vm.io.output().to_vec())
}

/// Assemble a program that computes something into `a0` and halts.
fn value_of(body: &str) -> i128 {
    match run(&format!("{body}\n    halt a0\n"), b"").0 {
        Stop::Halted(v) => v,
        other => panic!("expected a halt, got {other}"),
    }
}

// -------------------------------------------------------------- round trips

#[test]
fn every_instruction_form_assembles_and_disassembles() {
    let src = "
    add.wrap  a0, a1, a2
    sub.trap  a0, a1, a2
    mul.flag  a0, a1, a2, a3
    mulh      a0, a1, a2
    div       a0, a1, a2
    rem       a0, a1, a2
    shl.wrap  a0, a1, a2
    shr       a0, a1, a2
    tmin      a0, a1, a2
    tmax      a0, a1, a2
    tmul      a0, a1, a2
    cmp       a0, a1, a2
    addi.trap a0, a1, 100
    wrap      a0, a1, 9
    ld.word   a0, 12(sp)
    ld.tryte  a0, -1(zero)
    st.word   a0, 12(sp)
    st.tryte  a0, -2(zero)
    lui       a0, 797161
    sel3      a0, t0, a1, a2, a3
    jalr      ra, 0(a0)
    halt      a0
    trap      F_DIVZERO
";
    let image = asm(src);
    let disassembled: Vec<String> = words(&image)
        .into_iter()
        .map(|w| Inst::decode(w).expect("decodes").to_string())
        .collect();
    // Every line decodes back to something naming the same operation.
    assert_eq!(disassembled.len(), 23);
    assert!(disassembled[0].starts_with("add.wrap a0, a1, a2"));
    assert!(disassembled[2].starts_with("mul.flag a0, a1, a2, a3"));
    assert!(disassembled[13].starts_with("wrapi a0, a1, 9"));
    assert!(disassembled[15].starts_with("ld.tryte a0, -1(zero)"));
    assert!(disassembled[22].starts_with("trap F_DIVZERO"));
}

#[test]
fn pseudo_instructions_expand() {
    let image = asm("
    nop
    mv   a0, a1
    neg  a0, a1
    ret
    j    0
    call 0
    br2  t0, 0, 0
");
    let text: Vec<String> = words(&image)
        .into_iter()
        .map(|w| Inst::decode(w).unwrap().to_string())
        .collect();
    assert_eq!(text[0], "add.wrap zero, zero, zero"); // nop
    assert_eq!(text[1], "add.wrap a0, a1, zero"); // mv
    assert_eq!(text[2], "sub.wrap a0, zero, a1"); // neg
    assert_eq!(text[3], "jalr zero, 0(ra)"); // ret
    assert_eq!(text[4], "jal zero, -4"); // j 0, from address 12
    assert_eq!(text[5], "jal ra, -5"); // call 0
    // br2 puts `then` on the +1 arm and `else` on both others.
    assert_eq!(text[6], "br3 t0, -6, -6, -6");
}

#[test]
fn li_uses_one_word_when_it_fits_and_two_when_it_does_not() {
    assert_eq!(asm("    li a0, 100").len(), 3);
    assert_eq!(asm("    li a0, 2391484").len(), 3);
    assert_eq!(asm("    li a0, 2391485").len(), 6);
    // `la` of a label is always two words, so a statement's size never
    // depends on something pass one cannot see (§7.1).
    assert_eq!(asm("    la a0, later\nlater:").len(), 6);
}

#[test]
fn li_reaches_every_word_value() {
    for v in [
        0i128,
        1,
        -1,
        2_391_485,
        -2_391_485,
        3_812_798_742_493,
        -3_812_798_742_493,
        1_234_567_890_123,
    ] {
        assert_eq!(value_of(&format!("    li a0, {v}")), v, "li {v}");
    }
}

// -------------------------------------------------------------- expressions

#[test]
fn expressions_are_exact_and_use_the_machines_division() {
    // §4.3: `/` is round to nearest, ties away from zero — an assembler that
    // used a host language's division would disagree with its own target.
    assert_eq!(value_of("    li a0, 7 / 2"), 4);
    assert_eq!(value_of("    li a0, 8 / 3"), 3);
    assert_eq!(value_of("    li a0, -8 / 3"), -3);
    assert_eq!(value_of("    li a0, 8 % 3"), -1);
    // `>>` is division by a power of three.
    assert_eq!(value_of("    li a0, 100 >> 2"), 11);
    assert_eq!(value_of("    li a0, 4 << 3"), 108);
    // Precedence: unary, then * / %, then + -, then shifts.
    assert_eq!(value_of("    li a0, 1 + 2 * 3"), 7);
    assert_eq!(value_of("    li a0, (1 + 2) * 3"), 9);
    assert_eq!(value_of("    li a0, -2 * -3"), 6);
    assert_eq!(value_of("    li a0, 1 + 1 << 2"), 18);
}

#[test]
fn the_named_trit_operations_are_available() {
    assert_eq!(value_of("    li a0, tneg(5)"), -5);
    assert_eq!(value_of("    li a0, sign(-7)"), -1);
    assert_eq!(value_of("    li a0, cmp(3, 9)"), -1);
    assert_eq!(value_of("    li a0, wrap(9842, 9)"), -9841);
    // tmin(0t111, 0t1T0) = 0t1T0 = 6; positionwise, per AM §3.4.
    assert_eq!(value_of("    li a0, tmin(0t111, 0t1T0)"), 6);
    // tmax(0t1T1, 0tTT1) = 0t1T1 = 7
    assert_eq!(value_of("    li a0, tmax(0t1T1, 0tTT1)"), 7);
    // tmul(0t11T, 0t1T1) = 0t1TT = 5
    assert_eq!(value_of("    li a0, tmul(0t11T, 0t1T1)"), 5);
}

#[test]
fn all_three_radices_and_separators_work() {
    assert_eq!(value_of("    li a0, 0t1T0"), 6);
    assert_eq!(value_of("    li a0, 0hDDE"), 1);
    assert_eq!(value_of("    li a0, 3_812_798"), 3_812_798);
    assert_eq!(value_of("    li a0, 0t1T0_T01"), 154);
}

#[test]
fn the_location_counter_is_the_statements_own_address() {
    // `$` does not advance within a statement (§4.5).
    let image = asm("
    .word 0, 0
here:
    .word $, $, $
");
    let ws = words(&image);
    assert_eq!(ws[2..5], [6, 6, 6]);
}

// --------------------------------------------------------------- directives

#[test]
fn data_directives_emit_what_they_say() {
    let image = asm("
    .tryte 1, -1, 9841
    .word  6
    .trits \"1T0\"
    .zero  2
    .fill  3, -4
");
    assert_eq!(image[0..3], [1, -1, 9841]);
    assert_eq!(image[3..6], [6, 0, 0]); // little-trytean
    assert_eq!(image[6], 6); // 0t1T0 packed into one tryte
    assert_eq!(image[7..9], [0, 0]);
    assert_eq!(image[9..12], [-4, -4, -4]);
}

#[test]
fn a_trit_string_packs_nine_trits_per_tryte() {
    // 12 trits become two trytes, the top one zero-padded (§5.1).
    let image = asm("    .trits \"111_111111111\"");
    assert_eq!(image.len(), 2);
    assert_eq!(image[0], 9841); // the low nine 1 trits
    assert_eq!(image[1], 13); // 0t111
}

#[test]
fn align_and_org_pad_with_zeros() {
    let image = asm("
    .tryte 1
    .align 3
    .tryte 2
    .org   10
    .tryte 3
");
    assert_eq!(image[0], 1);
    assert_eq!(image[1..3], [0, 0]); // aligned to 3
    assert_eq!(image[3], 2);
    assert_eq!(image[4..10], [0, 0, 0, 0, 0, 0]);
    assert_eq!(image[10], 3);
}

#[test]
fn equ_defines_a_constant() {
    assert_eq!(value_of(".equ N, 40\n    li a0, N + 2"), 42);
}

#[test]
fn labels_may_be_referred_to_forward_and_are_addresses() {
    let image = asm("
    la a0, target
    j  target
    .word 0
target:
    .word 99
");
    // `la` is two words, `j` one, `.word` one — target is at tryte 12.
    assert_eq!(words(&image)[4], 99);
    assert_eq!(value_of("    la a0, here\nhere:"), 6);
}

#[test]
fn local_labels_work_and_are_not_directives() {
    // A label is always followed by `:` and a directive never is, so a name
    // beginning with `.` is unambiguous (§2.1).
    let (stop, _) = run(
        "
    j    .skip
    halt a0
.skip:
    li   a0, 5
    halt a0
",
        b"",
    );
    assert_eq!(stop, Stop::Halted(5));
}

// ------------------------------------------------------------------ errors

#[test]
fn the_binary_worlds_radices_are_rejected_by_name() {
    assert!(error("    li a0, 0xFF").contains("hexadecimal"));
    assert!(error("    li a0, 0b1").contains("binary"));
}

#[test]
fn reserved_operators_are_rejected_with_the_named_forms() {
    let e = error("    li a0, 1 & 2");
    assert!(e.contains("reserved"), "{e}");
    assert!(e.contains("tmin"), "{e}");
}

#[test]
fn binary_world_directives_name_their_replacement() {
    let e = error("    .byte 1");
    assert!(e.contains(".tryte"), "{e}");
}

#[test]
fn reserved_directives_say_so() {
    assert!(error("    .macro foo").contains("reserved"));
    assert!(error("    .string \"hi\"").contains("reserved"));
}

#[test]
fn out_of_range_values_are_errors_not_wraps() {
    // §4.4: never a silent wrap. `wrap` is the way to ask for one.
    assert!(error("    .tryte 9842").contains("does not fit"));
    assert!(error("    addi a0, a1, 2391485").contains("does not fit"));
    assert_eq!(value_of("    li a0, wrap(9842, 9)"), -9841);
}

#[test]
fn a_branch_that_cannot_reach_is_an_error() {
    // §4.5: out of range is an assembly-time error, never an automatic
    // expansion — expansion would make an instruction's size depend on its
    // operands.
    let mut src = String::from("    br3 t0, far, far, far\n");
    for _ in 0..1100 {
        src.push_str("    nop\n");
    }
    src.push_str("far:\n");
    let e = error(&src);
    assert!(e.contains("out of range"), "{e}");
}

#[test]
fn structural_mistakes_are_diagnosed() {
    assert!(error("    .align 4").contains("power of three"));
    assert!(error("    .org 0\n    .tryte 1\n    .org 0").contains("backwards"));
    assert!(error("x:\nx:").contains("more than once"));
    assert!(error("    li a0, nowhere").contains("not defined"));
    assert!(error("    li a0, 1 / 0").contains("division by zero"));
    assert!(error("    li a0, 1 >> -1").contains("negative"));
    assert!(error("    .trits \"12\"").contains("not a trit"));
    assert!(error("    add a0, a1, a2, a3").contains("operand"));
    assert!(error("    frobnicate a0").contains("not an instruction"));
    assert!(error("sp:").contains("register name"));
    assert!(error("    tmin.trap a0, a1, a2").contains("takes no flavor"));
    assert!(error("    addi.flag a0, a1, 1").contains("reserved"));
}

// ------------------------------------------------------------ whole programs

#[test]
fn the_appendix_echo_program_assembles_and_runs() {
    let src = include_str!("../../examples/trisc/echo.t27");
    let (stop, out) = run(src, "三進位 — trits\n".as_bytes());
    assert_eq!(stop, Stop::Halted(0));
    assert_eq!(String::from_utf8(out).unwrap(), "三進位 — trits\n");
}

#[test]
fn the_checked_in_image_matches_its_source() {
    // `examples/trisc/echo.timg` is generated from `echo.t27`; if they drift,
    // this catches it.
    let src = include_str!("../../examples/trisc/echo.t27");
    let checked_in = tritium::image::parse(include_str!("../../examples/trisc/echo.timg"))
        .expect("the image parses");
    assert_eq!(asm(src), checked_in);
}

#[test]
fn a_loop_with_a_three_way_branch_runs() {
    // Sum 1..n with a trapping add, dispatching on the sign of the counter.
    let src = "
    li   a0, 10          ; i
    li   a1, 0           ; acc
loop:
    cmp  t0, a0, zero
    br3  t0, done, done, body
body:
    add.trap a1, a1, a0
    addi.trap a0, a0, -1
    j    loop
done:
    mv   a0, a1
    halt a0
";
    assert_eq!(run(src, b"").0, Stop::Halted(55));
}
