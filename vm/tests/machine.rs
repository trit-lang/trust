//! Machine tests: encoding, arithmetic agreement with `trit-core`, faults,
//! the device region, and whole programs.

use trit_core::{Bt, FaultCode, Flavor, Tint};
use tritium::vm::device;
use tritium::word::{self, MAX_WORD, WORD_TRITS};
use tritium::{AluOp, Inst, Io, Reg, Stop, Vm, Width};

fn r(name: &str) -> Reg {
    Reg::from_name(name).unwrap_or_else(|| panic!("no register `{name}`"))
}

/// Assemble and run, returning how it stopped.
fn run(program: &[Inst]) -> Stop {
    run_with(program, &[]).0
}

/// Assemble and run with input, returning how it stopped and what it wrote.
fn run_with(program: &[Inst], input: &[u8]) -> (Stop, Vec<u8>) {
    let words: Vec<i128> = program.iter().map(|i| i.encode()).collect();
    let mut vm = Vm::with_default_memory();
    vm.io = Io::with_input(input);
    vm.load_words(&words);
    let stop = vm.run(1_000_000);
    (stop, vm.io.output().to_vec())
}

fn addi(rd: Reg, rs1: Reg, imm: i128) -> Inst {
    Inst::AluI {
        op: AluOp::Add,
        flavor: Flavor::Wrap,
        rd,
        rs1,
        imm,
    }
}

fn alu(op: AluOp, rd: Reg, rs1: Reg, rs2: Reg) -> Inst {
    Inst::Alu {
        op,
        flavor: Flavor::Wrap,
        rd,
        rs1,
        rs2,
        rc: Reg::ZERO,
    }
}

// ------------------------------------------------------------------ encoding

#[test]
fn every_instruction_form_round_trips() {
    let a = r("a0");
    let b = r("a1");
    let c = r("s3");
    let d = r("t7");
    let forms = [
        Inst::Alu {
            op: AluOp::Add,
            flavor: Flavor::Flag,
            rd: a,
            rs1: b,
            rs2: c,
            rc: d,
        },
        Inst::Alu {
            op: AluOp::TMin,
            flavor: Flavor::Wrap,
            rd: a,
            rs1: b,
            rs2: c,
            rc: Reg::ZERO,
        },
        Inst::AluI {
            op: AluOp::Shl,
            flavor: Flavor::Trap,
            rd: a,
            rs1: b,
            imm: 9,
        },
        Inst::AluI {
            op: AluOp::Wrap,
            flavor: Flavor::Wrap,
            rd: a,
            rs1: b,
            imm: 9,
        },
        Inst::Load {
            width: Width::Word,
            rd: a,
            rs1: b,
            imm: -2_391_484,
        },
        Inst::Store {
            width: Width::Tryte,
            rs2: a,
            rs1: b,
            imm: 2_391_484,
        },
        Inst::Br3 {
            rs1: a,
            neg: -1093,
            zero: 0,
            pos: 1093,
        },
        Inst::Jal { rd: a, off: -5 },
        Inst::Jalr {
            rd: a,
            rs1: b,
            imm: 12,
        },
        Inst::Lui {
            rd: a,
            imm: 797_161,
        },
        Inst::Sel3 {
            rd: a,
            rt: b,
            rn: c,
            rz: d,
            rp: Reg::ZERO,
        },
        Inst::Halt { rs1: a },
        Inst::Trap {
            code: FaultCode::DivZero,
        },
    ];
    for inst in forms {
        let w = inst.encode();
        assert!(word::fits(w, WORD_TRITS), "{inst} does not fit a word");
        assert_eq!(Inst::decode(w), Ok(inst), "{inst} did not survive encoding");
    }
}

#[test]
fn the_all_zeros_word_is_a_nop() {
    // TRISC-27 §3.4: zeroed memory is a field of no-ops, because `r0` is the
    // register whose field value is zero.
    let inst = Inst::decode(0).expect("decodes");
    assert_eq!(
        inst,
        Inst::Alu {
            op: AluOp::Add,
            flavor: Flavor::Wrap,
            rd: Reg::ZERO,
            rs1: Reg::ZERO,
            rs2: Reg::ZERO,
            rc: Reg::ZERO,
        }
    );
    // And it really does nothing: a program of nops then a halt.
    assert_eq!(
        run(&[
            Inst::decode(0).unwrap(),
            Inst::decode(0).unwrap(),
            addi(r("a0"), Reg::ZERO, 7),
            Inst::Halt { rs1: r("a0") },
        ]),
        Stop::Halted(7)
    );
}

#[test]
fn malformed_words_are_rejected() {
    // A reserved opcode, a reserved funct, a flavor on an unflavored
    // operation, and a nonzero reserved field.
    for w in [
        word::shl3(-13, 0),                                       // reserved opcode
        word::shl3(0, 0) + word::shl3(13, 12),                    // reserved alu funct
        word::shl3(0, 0) + word::shl3(8, 12) + word::shl3(1, 15), // tmin.trap
        word::shl3(0, 0) + word::shl3(1, 19),                     // reserved field nonzero
    ] {
        assert!(Inst::decode(w).is_err(), "expected {w} to be malformed");
    }
    // Executing one faults rather than doing something arbitrary.
    let mut vm = Vm::with_default_memory();
    vm.load_words(&[word::shl3(-13, 0)]);
    assert!(matches!(vm.run(10), Stop::Fault(FaultCode::Trap, _)));
}

// ------------------------------------------------- agreement with trit-core

/// The machine's arithmetic must be the AM's arithmetic. `trit-core` is the
/// direct expression of the AM, so every operation is checked against it —
/// the VM works in `i128` for speed and this is what keeps that honest.
#[test]
fn the_alu_agrees_with_trit_core() {
    let sample = [
        0i128,
        1,
        -1,
        2,
        -2,
        40,
        -40,
        9841,
        -9841,
        1_000_000,
        -1_000_000,
        3_812_798_742_493,
        -3_812_798_742_493,
        123_456_789,
        -987_654_321,
        19_683,
        -19_683,
    ];
    let t = |v: i128| Tint::new(WORD_TRITS, v).expect("word value");

    for &a in &sample {
        for &b in &sample {
            let (ta, tb) = (t(a), t(b));

            let (want, over) = ta.add(&tb, Flavor::Wrap).unwrap();
            let got = word::overflowing(a + b);
            assert_eq!(got.wrapped, want.to_i128().unwrap(), "{a} + {b}");
            assert_eq!(got.overflow, over.to_i8() as i128, "carry of {a} + {b}");

            let (want, _) = ta.sub(&tb, Flavor::Wrap).unwrap();
            assert_eq!(word::wrap_to(a - b, WORD_TRITS), want.to_i128().unwrap());

            let (want, _) = ta.mul(&tb, Flavor::Wrap).unwrap();
            assert_eq!(word::wrap_to(a * b, WORD_TRITS), want.to_i128().unwrap());

            assert_eq!(
                word::cmp3(a, b),
                ta.cmp3(&tb).to_i8() as i128,
                "{a} <=> {b}"
            );
            assert_eq!(word::tmin(a, b), ta.tmin(&tb).to_i128().unwrap(), "tmin");
            assert_eq!(word::tmax(a, b), ta.tmax(&tb).to_i128().unwrap(), "tmax");
            assert_eq!(word::tmul(a, b), ta.tmul(&tb).to_i128().unwrap(), "tmul");

            if b != 0 {
                assert_eq!(
                    word::div_nearest(a, b),
                    ta.div(&tb).unwrap().to_i128().unwrap()
                );
                assert_eq!(
                    word::rem_nearest(a, b),
                    ta.rem(&tb).unwrap().to_i128().unwrap()
                );
            }
        }

        for k in 0..WORD_TRITS {
            assert_eq!(word::shr3(a, k), t(a).shr(k).unwrap().to_i128().unwrap());
            let (want, _) = t(a).shl(k, Flavor::Wrap).unwrap();
            assert_eq!(
                word::wrap_to(word::shl3(a, k), WORD_TRITS),
                want.to_i128().unwrap()
            );
        }
        for n in 1..=WORD_TRITS {
            let want = Bt::from_i128(a).wrap_to(n).to_i128().unwrap();
            assert_eq!(word::wrap_to(a, n), want, "wrap({a}, {n})");
        }
    }
}

/// `li rd, value` — the two-instruction expansion of TRISC-27 §7.1, which
/// needs no correction term because a balanced constant splits exactly.
fn li(rd: Reg, value: i128) -> [Inst; 2] {
    [
        Inst::Lui {
            rd,
            imm: word::shr3(value, 14),
        },
        addi(rd, rd, word::wrap_to(value, 14)),
    ]
}

#[test]
fn mulh_gives_the_half_a_same_width_multiply_throws_away() {
    // TRISC-27 §4.1: the primitive multi-part multiplication needs, executed
    // on the machine and checked against the exact product.
    for (a, b) in [
        (MAX_WORD, MAX_WORD),
        (MAX_WORD, -MAX_WORD),
        (-MAX_WORD, -MAX_WORD),
        (1_000_000_007, 999_999_937),
        (3, 5),
        (0, MAX_WORD),
    ] {
        let [la0, la1] = li(r("a0"), a);
        let [lb0, lb1] = li(r("a1"), b);
        let hi = run(&[
            la0,
            la1,
            lb0,
            lb1,
            alu(AluOp::MulH, r("a2"), r("a0"), r("a1")),
            Inst::Halt { rs1: r("a2") },
        ]);
        let lo = run(&[
            la0,
            la1,
            lb0,
            lb1,
            alu(AluOp::Mul, r("a2"), r("a0"), r("a1")),
            Inst::Halt { rs1: r("a2") },
        ]);
        let (Stop::Halted(hi), Stop::Halted(lo)) = (hi, lo) else {
            panic!("{a} × {b} did not halt");
        };
        // The two halves reconstruct the exact 54-trit product — which is
        // exactly what expanding a wide multiply needs.
        assert_eq!(lo + hi * 3i128.pow(WORD_TRITS), a * b, "{a} × {b}");
    }
}

// ---------------------------------------------------------------- execution

#[test]
fn lui_and_addi_reach_every_word_with_no_correction_term() {
    // TRISC-27 §4.3, and the reason it is worth stating: the equivalent
    // binary sequence needs a fixup that this one does not.
    for target in [
        MAX_WORD,
        -MAX_WORD,
        0,
        1,
        -1,
        2_391_485,
        -2_391_485,
        1_234_567_890_123,
    ] {
        let hi = word::shr3(target, 14);
        let lo = word::wrap_to(target, 14);
        let program = [
            Inst::Lui {
                rd: r("a0"),
                imm: hi,
            },
            addi(r("a0"), r("a0"), lo),
            Inst::Halt { rs1: r("a0") },
        ];
        assert_eq!(run(&program), Stop::Halted(target), "building {target}");
    }
}

#[test]
fn br3_dispatches_three_ways_in_one_instruction() {
    for (input, expected) in [(-5i128, -1i128), (0, 0), (5, 1)] {
        let program = [
            addi(r("a0"), Reg::ZERO, input),
            alu(AluOp::Cmp, r("t0"), r("a0"), Reg::ZERO),
            // +1 → skip two, 0 → skip one, −1 → next
            Inst::Br3 {
                rs1: r("t0"),
                neg: 1,
                zero: 3,
                pos: 5,
            },
            addi(r("a1"), Reg::ZERO, -1),
            Inst::Halt { rs1: r("a1") },
            addi(r("a1"), Reg::ZERO, 0),
            Inst::Halt { rs1: r("a1") },
            addi(r("a1"), Reg::ZERO, 1),
            Inst::Halt { rs1: r("a1") },
        ];
        assert_eq!(run(&program), Stop::Halted(expected), "input {input}");
    }
}

#[test]
fn sel3_picks_by_sign_without_branching() {
    for (sel, expected) in [(-9i128, 10i128), (0, 20), (9, 30)] {
        let program = [
            addi(r("t0"), Reg::ZERO, sel),
            addi(r("a1"), Reg::ZERO, 10),
            addi(r("a2"), Reg::ZERO, 20),
            addi(r("a3"), Reg::ZERO, 30),
            Inst::Sel3 {
                rd: r("a0"),
                rt: r("t0"),
                rn: r("a1"),
                rz: r("a2"),
                rp: r("a3"),
            },
            Inst::Halt { rs1: r("a0") },
        ];
        assert_eq!(run(&program), Stop::Halted(expected), "selector {sel}");
    }
}

#[test]
fn the_flag_flavor_writes_the_carry_to_its_second_destination() {
    let program = [
        Inst::Lui {
            rd: r("a0"),
            imm: 797_161,
        },
        addi(r("a0"), r("a0"), 2_391_484), // a0 = MAX
        Inst::Alu {
            op: AluOp::Add,
            flavor: Flavor::Flag,
            rd: r("a1"),
            rs1: r("a0"),
            rs2: r("a0"),
            rc: r("a2"),
        },
        Inst::Halt { rs1: r("a2") },
    ];
    // MAX + MAX overflows upward, so the trit is +1.
    assert_eq!(run(&program), Stop::Halted(1));
}

#[test]
fn calls_and_returns_work() {
    let program = [
        addi(r("a0"), Reg::ZERO, 20),
        Inst::Jal {
            rd: r("ra"),
            off: 2,
        }, // call double
        Inst::Halt { rs1: r("a0") },
        alu(AluOp::Add, r("a0"), r("a0"), r("a0")), // double:
        Inst::Jalr {
            rd: Reg::ZERO,
            rs1: r("ra"),
            imm: 0,
        }, // ret
    ];
    assert_eq!(run(&program), Stop::Halted(40));
}

#[test]
fn memory_round_trips_at_both_widths() {
    for value in [0i128, 1, -1, MAX_WORD, -MAX_WORD, 9841, -9841] {
        let hi = word::shr3(value, 14);
        let lo = word::wrap_to(value, 14);
        let program = [
            Inst::Lui {
                rd: r("a0"),
                imm: hi,
            },
            addi(r("a0"), r("a0"), lo),
            addi(r("t0"), Reg::ZERO, 300), // a word-aligned scratch address
            Inst::Store {
                width: Width::Word,
                rs2: r("a0"),
                rs1: r("t0"),
                imm: 0,
            },
            Inst::Load {
                width: Width::Word,
                rd: r("a1"),
                rs1: r("t0"),
                imm: 0,
            },
            Inst::Halt { rs1: r("a1") },
        ];
        assert_eq!(run(&program), Stop::Halted(value), "storing {value}");
    }
}

// -------------------------------------------------------------------- faults

fn fault_of(program: &[Inst]) -> FaultCode {
    match run(program) {
        Stop::Fault(c, _) => c,
        other => panic!("expected a fault, got {other}"),
    }
}

#[test]
fn the_five_am_fault_codes_all_arise() {
    // F_OVERFLOW — trapping flavor.
    assert_eq!(
        fault_of(&[
            Inst::Lui {
                rd: r("a0"),
                imm: 797_161
            },
            addi(r("a0"), r("a0"), 2_391_484),
            Inst::Alu {
                op: AluOp::Add,
                flavor: Flavor::Trap,
                rd: r("a1"),
                rs1: r("a0"),
                rs2: r("a0"),
                rc: Reg::ZERO,
            },
        ]),
        FaultCode::Overflow
    );

    // F_DIVZERO.
    assert_eq!(
        fault_of(&[
            addi(r("a0"), Reg::ZERO, 1),
            alu(AluOp::Div, r("a1"), r("a0"), Reg::ZERO),
        ]),
        FaultCode::DivZero
    );

    // F_SHIFT — 27 is outside 0…26, and is not masked.
    assert_eq!(
        fault_of(&[
            addi(r("a0"), Reg::ZERO, 1),
            addi(r("t0"), Reg::ZERO, 27),
            alu(AluOp::Shl, r("a1"), r("a0"), r("t0")),
        ]),
        FaultCode::Shift
    );

    // F_ALIGN — a word access one tryte off.
    assert_eq!(
        fault_of(&[
            addi(r("t0"), Reg::ZERO, 301),
            Inst::Load {
                width: Width::Word,
                rd: r("a0"),
                rs1: r("t0"),
                imm: 0,
            },
        ]),
        FaultCode::Align
    );

    // F_TRAP — asked for.
    assert_eq!(
        fault_of(&[Inst::Trap {
            code: FaultCode::Trap
        }]),
        FaultCode::Trap
    );
}

#[test]
fn a_tryte_access_needs_no_alignment() {
    let program = [
        addi(r("t0"), Reg::ZERO, 301),
        addi(r("a0"), Reg::ZERO, 7),
        Inst::Store {
            width: Width::Tryte,
            rs2: r("a0"),
            rs1: r("t0"),
            imm: 0,
        },
        Inst::Load {
            width: Width::Tryte,
            rd: r("a1"),
            rs1: r("t0"),
            imm: 0,
        },
        Inst::Halt { rs1: r("a1") },
    ];
    assert_eq!(run(&program), Stop::Halted(7));
}

// ------------------------------------------------------------ device region

#[test]
fn a_program_can_discover_its_own_memory_size() {
    // The point of the negative device region: this works unchanged on a
    // machine with a different A (TRISC-27 §2.1).
    for size in [14_348_907i128, 531_441, 3i128.pow(18)] {
        let program = [
            Inst::Load {
                width: Width::Word,
                rd: r("sp"),
                rs1: Reg::ZERO,
                imm: device::MEM_SIZE,
            },
            Inst::Halt { rs1: r("sp") },
        ];
        let words: Vec<i128> = program.iter().map(|i| i.encode()).collect();
        let mut vm = Vm::new(size);
        vm.load_words(&words);
        assert_eq!(vm.run(100), Stop::Halted(size));
    }
}

#[test]
fn the_device_region_is_out_of_memory_reach() {
    // Reserved negative addresses fault, and so does the wrong width.
    assert_eq!(
        fault_of(&[Inst::Load {
            width: Width::Word,
            rd: r("a0"),
            rs1: Reg::ZERO,
            imm: device::IO_IN, // a tryte port, read as a word
        }]),
        FaultCode::Trap
    );
    assert_eq!(
        fault_of(&[Inst::Load {
            width: Width::Tryte,
            rd: r("a0"),
            rs1: Reg::ZERO,
            imm: -99,
        }]),
        FaultCode::Trap
    );
    // Storing to the input port is reserved.
    assert_eq!(
        fault_of(&[Inst::Store {
            width: Width::Tryte,
            rs2: Reg::ZERO,
            rs1: Reg::ZERO,
            imm: device::IO_IN,
        }]),
        FaultCode::Trap
    );
}

/// The echo program from TRISC-27's Appendix A, assembled by hand.
///
/// Its two branches are both three-way: "code unit / waiting / closed" and
/// "still waiting / closed" are genuinely three-valued and two-valued
/// questions asked of the same port.
fn echo_program() -> Vec<Inst> {
    vec![
        // start: sp ← A
        Inst::Load {
            width: Width::Word,
            rd: r("sp"),
            rs1: Reg::ZERO,
            imm: device::MEM_SIZE,
        },
        // loop: a0 ← IO_IN
        Inst::Load {
            width: Width::Tryte,
            rd: r("a0"),
            rs1: Reg::ZERO,
            imm: device::IO_IN,
        },
        alu(AluOp::Cmp, r("t0"), r("a0"), Reg::ZERO),
        // −1 → check_eof (+1), 0 or +1 → echo (+3)
        Inst::Br3 {
            rs1: r("t0"),
            neg: 1,
            zero: 3,
            pos: 3,
        },
        // check_eof: t1 ← a0 + 1   (−1 → 0 waiting, −2 → −1 closed)
        addi(r("t1"), r("a0"), 1),
        // −1 → done (+3), 0 → loop (−4), +1 → done (+2)
        Inst::Br3 {
            rs1: r("t1"),
            neg: 3,
            zero: -4,
            pos: 2,
        },
        // echo: IO_OUT ← a0
        Inst::Store {
            width: Width::Tryte,
            rs2: r("a0"),
            rs1: Reg::ZERO,
            imm: device::IO_OUT,
        },
        // j loop
        Inst::Jal {
            rd: Reg::ZERO,
            off: -6,
        },
        // done:
        Inst::Halt { rs1: Reg::ZERO },
    ]
}

#[test]
fn the_appendix_echo_program_runs() {
    let (stop, out) = run_with(&echo_program(), b"Hello, trits!\n");
    assert_eq!(stop, Stop::Halted(0));
    assert_eq!(out, b"Hello, trits!\n");
}

#[test]
fn echo_handles_empty_input() {
    let (stop, out) = run_with(&echo_program(), b"");
    assert_eq!(stop, Stop::Halted(0));
    assert!(out.is_empty());
}

#[test]
fn echo_is_byte_exact_on_utf8() {
    let text = "三進位 — trits\n";
    let (stop, out) = run_with(&echo_program(), text.as_bytes());
    assert_eq!(stop, Stop::Halted(0));
    assert_eq!(String::from_utf8(out).unwrap(), text);
}

// ------------------------------------------------------------ whole programs

#[test]
fn a_factorial_loop_traps_when_it_overflows() {
    // n! with a trapping multiply: the same program the TIR examples run,
    // now on the machine.
    let factorial = |n: i128| -> Stop {
        let program = [
            addi(r("a0"), Reg::ZERO, n), // i
            addi(r("a1"), Reg::ZERO, 1), // acc
            // loop:
            alu(AluOp::Cmp, r("t0"), r("a0"), Reg::ZERO),
            Inst::Br3 {
                rs1: r("t0"),
                neg: 4,
                zero: 4,
                pos: 1,
            },
            Inst::Alu {
                op: AluOp::Mul,
                flavor: Flavor::Trap,
                rd: r("a1"),
                rs1: r("a1"),
                rs2: r("a0"),
                rc: Reg::ZERO,
            },
            Inst::AluI {
                op: AluOp::Sub,
                flavor: Flavor::Trap,
                rd: r("a0"),
                rs1: r("a0"),
                imm: 1,
            },
            Inst::Jal {
                rd: Reg::ZERO,
                off: -4,
            },
            // done:
            Inst::Halt { rs1: r("a1") },
        ];
        run(&program)
    };

    assert_eq!(factorial(0), Stop::Halted(1));
    assert_eq!(factorial(5), Stop::Halted(120));
    assert_eq!(factorial(9), Stop::Halted(362_880));
    assert_eq!(factorial(15), Stop::Halted(1_307_674_368_000));
    // 16! exceeds the word, and `.trap` makes that a fault rather than a lie.
    assert!(matches!(factorial(16), Stop::Fault(FaultCode::Overflow, _)));
}

#[test]
fn division_on_the_machine_rounds_to_nearest() {
    // The whole point of AM §3.2, executed rather than asserted.
    for (a, b, q) in [(7i128, 2i128, 4i128), (8, 3, 3), (-8, 3, -3), (1, 2, 1)] {
        let program = [
            addi(r("a0"), Reg::ZERO, a),
            addi(r("a1"), Reg::ZERO, b),
            alu(AluOp::Div, r("a2"), r("a0"), r("a1")),
            Inst::Halt { rs1: r("a2") },
        ];
        assert_eq!(run(&program), Stop::Halted(q), "{a} / {b}");
    }
}

#[test]
fn wrap_narrows_in_one_instruction() {
    // The instruction legalization wanted: renormalizing a promoted value.
    for (value, n, expected) in [(9842i128, 9, -9841i128), (13, 3, 13), (14, 3, -13)] {
        let program = [
            addi(r("a0"), Reg::ZERO, value),
            Inst::AluI {
                op: AluOp::Wrap,
                flavor: Flavor::Wrap,
                rd: r("a1"),
                rs1: r("a0"),
                imm: n,
            },
            Inst::Halt { rs1: r("a1") },
        ];
        assert_eq!(run(&program), Stop::Halted(expected), "wrap({value}, {n})");
    }
}

#[test]
fn every_fault_the_isa_promises_is_raised() {
    // TRISC-27 states thirteen conditions that fault, scattered over §§2, 4
    // and 6. Each is exercised here rather than read, because a fault the
    // machine does not raise is a fault a program cannot rely on.
    let prelude = ".equ IO_IN, -1\n.equ IO_OUT, -2\n.equ MEM_SIZE, -6\n.equ CYCLES, -9\n";
    for (what, body, code) in [
        (
            "a load at or above A (§2.1)",
            "ld.word a0, MEM_SIZE(zero)\n    ld.word t0, 0(a0)",
            FaultCode::Trap,
        ),
        (
            "a word load from a tryte device (§2.2)",
            "ld.word t0, IO_IN(zero)",
            FaultCode::Trap,
        ),
        (
            "a tryte load from a word device (§2.2)",
            "ld.tryte t0, MEM_SIZE(zero)",
            FaultCode::Trap,
        ),
        (
            "a load from IO_OUT (§2.2)",
            "ld.tryte t0, IO_OUT(zero)",
            FaultCode::Trap,
        ),
        (
            "a store to IO_IN (§2.2)",
            "st.tryte zero, IO_IN(zero)",
            FaultCode::Trap,
        ),
        (
            "a store to MEM_SIZE (§2.2)",
            "st.word zero, MEM_SIZE(zero)",
            FaultCode::Trap,
        ),
        (
            "a store to CYCLES (§2.3)",
            "st.word zero, CYCLES(zero)",
            FaultCode::Trap,
        ),
        (
            "a tryte load from CYCLES (§2.3)",
            "ld.tryte t0, CYCLES(zero)",
            FaultCode::Trap,
        ),
        (
            "a reserved device address (§2.2)",
            "ld.tryte t0, -3(zero)",
            FaultCode::Trap,
        ),
        (
            "a shift amount above 26 (§4.1)",
            "addi.wrap t1, zero, 27\n    shl.wrap t2, t1, t1",
            FaultCode::Shift,
        ),
        (
            "a negative shift amount (§4.1)",
            "addi.wrap t1, zero, -1\n    shr t2, t1, t1",
            FaultCode::Shift,
        ),
        (
            "an unaligned word access (§4.4)",
            "addi.wrap t0, zero, 1\n    ld.word t1, 0(t0)",
            FaultCode::Align,
        ),
        (
            "an unaligned jump target (§4.5)",
            "addi.wrap t0, zero, 1\n    jalr ra, 0(t0)",
            FaultCode::Align,
        ),
    ] {
        let src = format!("{prelude}start:\n    {body}\n    halt zero\n");
        let image = tritium::assemble(&src).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let mut vm = Vm::with_default_memory();
        vm.load_image(&image);
        match vm.run(1_000) {
            Stop::Fault(c, _) => assert_eq!(c, code, "{what}"),
            other => panic!("{what}: expected a fault, got {other}"),
        }
    }
}

#[test]
fn the_cycle_counter_counts_what_ran_between_two_readings() {
    // TRISC-27 §2.3: the load has not retired when its value is produced, so
    // the difference is the code between the readings with nothing to
    // subtract for the measurement itself.
    let src = ".equ CYCLES, -9\n\
               start:\n\
               \x20   ld.word t0, CYCLES(zero)\n\
               \x20   addi.wrap t1, zero, 0\n\
               \x20   addi.wrap t1, zero, 0\n\
               \x20   addi.wrap t1, zero, 0\n\
               \x20   ld.word t2, CYCLES(zero)\n\
               \x20   sub.wrap a0, t2, t0\n\
               \x20   halt a0\n";
    let image = tritium::assemble(src).expect("assembles");
    let mut vm = Vm::with_default_memory();
    vm.load_image(&image);
    // Three `addi`, plus the first `ld` retiring after its value was read.
    assert_eq!(vm.run(1_000), Stop::Halted(4));
    // And the machine reports what it did.
    assert_eq!(vm.steps(), 7);
}

#[test]
fn a_profile_counts_what_ran_and_says_what_kind_it_was() {
    // The cycle counter answers "how many"; this answers "which". The
    // distinctions it draws are the ones that are actionable: what an access
    // is addressed from, what an `alui` is computed from, and whether a
    // `jal` links.
    let program = &[
        // Ten iterations of: one frame store, one frame load, one add.
        addi(r("sp"), r("zero"), 300), // a stack to store into
        addi(r("t0"), r("zero"), 10),  // the counter
        addi(r("t1"), r("zero"), 0),   // the sum
        // ^loop
        Inst::Store {
            width: Width::Word,
            rs2: r("t0"),
            rs1: r("sp"),
            imm: 0,
        },
        Inst::Load {
            width: Width::Word,
            rd: r("t2"),
            rs1: r("sp"),
            imm: 0,
        },
        alu(AluOp::Add, r("t1"), r("t1"), r("t2")),
        addi(r("t0"), r("t0"), -1),
        Inst::Br3 {
            rs1: r("t0"),
            neg: 1,
            zero: 1,
            pos: -4,
        },
        Inst::Halt { rs1: r("t1") },
    ];
    let words: Vec<i128> = program.iter().map(|i| i.encode()).collect();
    let mut vm = Vm::with_default_memory();
    vm.load_words(&words);
    let p = tritium::profile(&mut vm, 1_000_000);

    // 10 + 9 + … + 1 = 55, and the machine stopped of its own accord.
    assert_eq!(p.stop, Some(Stop::Halted(55)));
    // Three setup instructions, five per iteration, and the halt.
    assert_eq!(p.total, 3 + 5 * 10 + 1);

    // One store and one load per iteration, both through `sp`, and nothing
    // through any other register.
    assert_eq!(p.by_kind.get("st.word sp"), Some(&10));
    assert_eq!(p.by_kind.get("ld.word sp"), Some(&10));
    assert_eq!(p.frame_traffic(), 20);
    assert_eq!(p.data_traffic(), 0);

    // The three constants and the ten decrements are `alui`, and the profile
    // separates them: three are computed from `zero` — a constant being
    // materialized — and ten from a register.
    assert_eq!(p.by_kind.get("alui.add zero"), Some(&3));
    assert_eq!(p.by_kind.get("alui.add reg"), Some(&10));

    // Nine branches taken back, one falling out.
    assert_eq!(p.by_kind.get("br3"), Some(&10));

    // The loop body is five words, executed ten times each, and they are the
    // hottest thing here.
    let hot = p.hottest();
    assert_eq!(
        &hot[..5].iter().map(|(_, n)| *n).collect::<Vec<_>>(),
        &[10; 5]
    );
    assert!((p.share(10) - 1000.0 / 54.0).abs() < 0.01);
}

#[test]
fn a_profile_of_a_program_that_faults_reports_the_fault() {
    let program = &[
        addi(r("t0"), r("zero"), 1),
        alu(AluOp::Div, r("t1"), r("t0"), r("zero")),
        Inst::Halt { rs1: r("t1") },
    ];
    let words: Vec<i128> = program.iter().map(|i| i.encode()).collect();
    let mut vm = Vm::with_default_memory();
    vm.load_words(&words);
    let p = tritium::profile(&mut vm, 1_000_000);

    // The faulting instruction is counted — it ran, and it is the one worth
    // knowing about — and the halt after it is not.
    assert_eq!(p.total, 2);
    assert!(matches!(p.stop, Some(Stop::Fault(FaultCode::DivZero, _))));
    assert_eq!(p.by_kind.get("alu.div"), Some(&1));
}
