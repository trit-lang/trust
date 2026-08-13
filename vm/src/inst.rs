//! Instruction encoding and decoding (TRISC-27 §3–§4).
//!
//! This module is the encoding, in both directions: an assembler builds
//! [`Inst`] values and calls [`Inst::encode`], and the machine calls
//! [`Inst::decode`] on the word it fetched. Keeping the two in one place is
//! what makes `tests/encoding.rs` able to prove they agree.

use crate::word::{self, WORD_TRITS};
use trit_core::{FaultCode, Flavor};

/// A register: its name *is* its 3-trit field value (TRISC-27 §1.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Reg(pub i8);

impl Reg {
    /// The zero register: reads 0, discards writes.
    pub const ZERO: Reg = Reg(0);

    /// The register with the given field value, if it is one.
    pub fn new(v: i8) -> Option<Reg> {
        (-13..=13).contains(&v).then_some(Reg(v))
    }

    /// Index into a register file laid out −13 … +13.
    pub fn index(self) -> usize {
        (self.0 + 13) as usize
    }

    /// The architectural name, `r0` / `r7` / `rT7`.
    pub fn name(self) -> String {
        match self.0 {
            0 => "r0".to_string(),
            v if v > 0 => format!("r{v}"),
            v => format!("rT{}", -v),
        }
    }

    /// The ABI name of TRISC-27 §6.1, if this register has one.
    pub fn abi_name(self) -> &'static str {
        match self.0 {
            0 => "zero",
            1 => "ra",
            2 => "sp",
            3 => "fp",
            4..=11 => ["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"][(self.0 - 4) as usize],
            12 => "t0",
            13 => "t1",
            -6..=-1 => ["t2", "t3", "t4", "t5", "t6", "t7"][(-self.0 - 1) as usize],
            -13..=-7 => ["s0", "s1", "s2", "s3", "s4", "s5", "s6"][(-self.0 - 7) as usize],
            _ => "?",
        }
    }

    /// Look a register up by architectural or ABI name.
    pub fn from_name(name: &str) -> Option<Reg> {
        for v in -13..=13i8 {
            let r = Reg(v);
            if r.name() == name || r.abi_name() == name {
                return Some(r);
            }
        }
        None
    }
}

/// Arithmetic operations, selected by the `funct` field (TRISC-27 §4.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i8)]
pub enum AluOp {
    /// `add` — flavored.
    Add = 0,
    /// `sub` — flavored.
    Sub = 1,
    /// `mul` — flavored; the low 27 trits of the product.
    Mul = 2,
    /// `mulh` — the high 27 trits of the 54-trit product.
    MulH = 3,
    /// `div` — round to nearest, ties away from zero.
    Div = 4,
    /// `rem`.
    Rem = 5,
    /// `shl` — flavored.
    Shl = 6,
    /// `shr`.
    Shr = 7,
    /// `tmin`.
    TMin = 8,
    /// `tmax`.
    TMax = 9,
    /// `tmul`.
    TMul = 10,
    /// `cmp` — three-way.
    Cmp = 11,
    /// `wrap` — immediate form only.
    Wrap = 12,
}

impl AluOp {
    /// The operation with this funct value.
    pub fn from_funct(v: i8) -> Option<AluOp> {
        use AluOp::*;
        Some(match v {
            0 => Add,
            1 => Sub,
            2 => Mul,
            3 => MulH,
            4 => Div,
            5 => Rem,
            6 => Shl,
            7 => Shr,
            8 => TMin,
            9 => TMax,
            10 => TMul,
            11 => Cmp,
            12 => Wrap,
            _ => return None,
        })
    }

    /// True for the four operations that take an overflow flavor.
    pub fn is_flavored(self) -> bool {
        matches!(self, AluOp::Add | AluOp::Sub | AluOp::Mul | AluOp::Shl)
    }

    /// The mnemonic stem.
    pub fn name(self) -> &'static str {
        use AluOp::*;
        match self {
            Add => "add",
            Sub => "sub",
            Mul => "mul",
            MulH => "mulh",
            Div => "div",
            Rem => "rem",
            Shl => "shl",
            Shr => "shr",
            TMin => "tmin",
            TMax => "tmax",
            TMul => "tmul",
            Cmp => "cmp",
            Wrap => "wrap",
        }
    }
}

/// The width of a memory access.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// One word, 3 trytes, 3-tryte aligned.
    Word,
    /// One tryte, any address.
    Tryte,
}

impl Width {
    /// Trytes moved.
    pub fn trytes(self) -> i128 {
        match self {
            Width::Word => 3,
            Width::Tryte => 1,
        }
    }

    /// The mnemonic suffix.
    pub fn name(self) -> &'static str {
        match self {
            Width::Word => "word",
            Width::Tryte => "tryte",
        }
    }
}

/// A decoded instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Inst {
    /// `alu` — register-register arithmetic.
    Alu {
        /// Operation.
        op: AluOp,
        /// Overflow flavor.
        flavor: Flavor,
        /// Destination.
        rd: Reg,
        /// Left operand.
        rs1: Reg,
        /// Right operand.
        rs2: Reg,
        /// Destination of the `.flag` overflow trit.
        rc: Reg,
    },
    /// `alui` — register-immediate arithmetic.
    AluI {
        /// Operation.
        op: AluOp,
        /// Overflow flavor; the flag flavor is reserved here.
        flavor: Flavor,
        /// Destination.
        rd: Reg,
        /// Left operand.
        rs1: Reg,
        /// Immediate.
        imm: i128,
    },
    /// `ld` — load.
    Load {
        /// Access width.
        width: Width,
        /// Destination.
        rd: Reg,
        /// Base address.
        rs1: Reg,
        /// Displacement in trytes.
        imm: i128,
    },
    /// `st` — store.
    Store {
        /// Access width.
        width: Width,
        /// Value.
        rs2: Reg,
        /// Base address.
        rs1: Reg,
        /// Displacement in trytes.
        imm: i128,
    },
    /// `br3` — the three-way branch.
    Br3 {
        /// The register whose sign selects.
        rs1: Reg,
        /// Displacement in words when the sign is −1.
        neg: i128,
        /// Displacement when the sign is 0.
        zero: i128,
        /// Displacement when the sign is +1.
        pos: i128,
    },
    /// `jal` — jump and link.
    Jal {
        /// Link register; `r0` for a plain jump.
        rd: Reg,
        /// Displacement in words.
        off: i128,
    },
    /// `jalr` — jump and link, register.
    Jalr {
        /// Link register.
        rd: Reg,
        /// Base address.
        rs1: Reg,
        /// Displacement in trytes.
        imm: i128,
    },
    /// `lui` — `rd ← imm · 3¹⁴`.
    Lui {
        /// Destination.
        rd: Reg,
        /// Upper immediate.
        imm: i128,
    },
    /// `sel3` — three-way select.
    Sel3 {
        /// Destination.
        rd: Reg,
        /// Selector.
        rt: Reg,
        /// Chosen when the selector is −1.
        rn: Reg,
        /// Chosen when the selector is 0.
        rz: Reg,
        /// Chosen when the selector is +1.
        rp: Reg,
    },
    /// `halt` — stop, reporting a status.
    Halt {
        /// The register holding the exit status.
        rs1: Reg,
    },
    /// `trap` — raise a fault deliberately.
    Trap {
        /// The fault to raise.
        code: FaultCode,
    },
}

// ------------------------------------------------------------------- opcodes

const OP_ALU: i8 = 0;
const OP_ALUI: i8 = 1;
const OP_LD: i8 = 2;
const OP_ST: i8 = 3;
const OP_BR3: i8 = 4;
const OP_JAL: i8 = 5;
const OP_JALR: i8 = 6;
const OP_LUI: i8 = 7;
const OP_SEL3: i8 = 8;
const OP_SYS: i8 = 9;

const SYS_HALT: i8 = 0;
const SYS_TRAP: i8 = 1;

/// Why an instruction word could not be decoded.
///
/// Every case is "malformed" in the sense of TRISC-27 §3.4, and the machine
/// raises `F_TRAP` for all of them — see that section on why the AM's code
/// list has nothing better.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Malformed(pub String);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed instruction: {}", self.0)
    }
}

/// Extract the field at trits `[lo .. lo+len)`.
fn field(w: i128, lo: u32, len: u32) -> i128 {
    word::wrap_to(word::shr3(w, lo), len)
}

/// Place a value into the field at trits `[lo .. lo+len)`.
fn place(v: i128, lo: u32, len: u32) -> i128 {
    debug_assert!(
        word::fits(v, len),
        "field value {v} does not fit {len} trits"
    );
    word::shl3(v, lo)
}

fn reg_field(w: i128, lo: u32) -> Reg {
    Reg(field(w, lo, 3) as i8)
}

fn flavor_trit(f: Flavor) -> i128 {
    match f {
        Flavor::Flag => -1,
        Flavor::Wrap => 0,
        Flavor::Trap => 1,
    }
}

fn flavor_of(t: i128) -> Flavor {
    match t {
        -1 => Flavor::Flag,
        1 => Flavor::Trap,
        _ => Flavor::Wrap,
    }
}

impl Inst {
    /// Encode this instruction into its 27-trit word.
    pub fn encode(self) -> i128 {
        match self {
            Inst::Alu {
                op,
                flavor,
                rd,
                rs1,
                rs2,
                rc,
            } => {
                place(OP_ALU as i128, 0, 3)
                    + place(rd.0 as i128, 3, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(rs2.0 as i128, 9, 3)
                    + place(op as i128, 12, 3)
                    + place(flavor_trit(flavor), 15, 1)
                    + place(rc.0 as i128, 16, 3)
            }
            Inst::AluI {
                op,
                flavor,
                rd,
                rs1,
                imm,
            } => {
                place(OP_ALUI as i128, 0, 3)
                    + place(rd.0 as i128, 3, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(op as i128, 9, 3)
                    + place(flavor_trit(flavor), 12, 1)
                    + place(imm, 13, 14)
            }
            Inst::Load {
                width,
                rd,
                rs1,
                imm,
            } => {
                place(OP_LD as i128, 0, 3)
                    + place(rd.0 as i128, 3, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(width_funct(width), 9, 3)
                    + place(imm, 13, 14)
            }
            Inst::Store {
                width,
                rs2,
                rs1,
                imm,
            } => {
                place(OP_ST as i128, 0, 3)
                    + place(rs2.0 as i128, 3, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(width_funct(width), 9, 3)
                    + place(imm, 13, 14)
            }
            Inst::Br3 {
                rs1,
                neg,
                zero,
                pos,
            } => {
                place(OP_BR3 as i128, 0, 3)
                    + place(rs1.0 as i128, 3, 3)
                    + place(neg, 6, 7)
                    + place(zero, 13, 7)
                    + place(pos, 20, 7)
            }
            Inst::Jal { rd, off } => {
                place(OP_JAL as i128, 0, 3) + place(rd.0 as i128, 3, 3) + place(off, 6, 21)
            }
            Inst::Jalr { rd, rs1, imm } => {
                place(OP_JALR as i128, 0, 3)
                    + place(rd.0 as i128, 3, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(imm, 13, 14)
            }
            Inst::Lui { rd, imm } => {
                place(OP_LUI as i128, 0, 3) + place(rd.0 as i128, 3, 3) + place(imm, 6, 13)
            }
            Inst::Sel3 { rd, rt, rn, rz, rp } => {
                place(OP_SEL3 as i128, 0, 3)
                    + place(rd.0 as i128, 3, 3)
                    + place(rt.0 as i128, 6, 3)
                    + place(rn.0 as i128, 9, 3)
                    + place(rz.0 as i128, 12, 3)
                    + place(rp.0 as i128, 15, 3)
            }
            Inst::Halt { rs1 } => {
                place(OP_SYS as i128, 0, 3)
                    + place(rs1.0 as i128, 6, 3)
                    + place(SYS_HALT as i128, 9, 3)
            }
            Inst::Trap { code } => {
                place(OP_SYS as i128, 0, 3)
                    + place(SYS_TRAP as i128, 9, 3)
                    + place(trap_imm(code), 13, 14)
            }
        }
    }

    /// Decode a 27-trit instruction word.
    pub fn decode(w: i128) -> Result<Inst, Malformed> {
        if !word::fits(w, WORD_TRITS) {
            return Err(Malformed("not a word value".into()));
        }
        let op = field(w, 0, 3) as i8;
        match op {
            OP_ALU => {
                let funct = field(w, 12, 3) as i8;
                let op = AluOp::from_funct(funct)
                    .ok_or_else(|| Malformed(format!("reserved alu funct {funct}")))?;
                if op == AluOp::Wrap {
                    return Err(Malformed("`wrap` has no register form".into()));
                }
                let flavor = flavor_of(field(w, 15, 1));
                if !op.is_flavored() && flavor != Flavor::Wrap {
                    return Err(Malformed(format!("`{}` takes no flavor", op.name())));
                }
                zero_field(w, 19, 8)?;
                Ok(Inst::Alu {
                    op,
                    flavor,
                    rd: reg_field(w, 3),
                    rs1: reg_field(w, 6),
                    rs2: reg_field(w, 9),
                    rc: reg_field(w, 16),
                })
            }
            OP_ALUI => {
                let funct = field(w, 9, 3) as i8;
                let op = AluOp::from_funct(funct)
                    .ok_or_else(|| Malformed(format!("reserved alui funct {funct}")))?;
                let flavor = flavor_of(field(w, 12, 1));
                if !op.is_flavored() && flavor != Flavor::Wrap {
                    return Err(Malformed(format!("`{}` takes no flavor", op.name())));
                }
                if flavor == Flavor::Flag {
                    // There is no `rc` field in the I format, so the flag
                    // flavor has nowhere to put the overflow trit (§4.1).
                    return Err(Malformed("the flag flavor is reserved in `alui`".into()));
                }
                Ok(Inst::AluI {
                    op,
                    flavor,
                    rd: reg_field(w, 3),
                    rs1: reg_field(w, 6),
                    imm: field(w, 13, 14),
                })
            }
            OP_LD => {
                zero_field(w, 12, 1)?;
                Ok(Inst::Load {
                    width: funct_width(field(w, 9, 3) as i8)?,
                    rd: reg_field(w, 3),
                    rs1: reg_field(w, 6),
                    imm: field(w, 13, 14),
                })
            }
            OP_ST => {
                zero_field(w, 12, 1)?;
                Ok(Inst::Store {
                    width: funct_width(field(w, 9, 3) as i8)?,
                    rs2: reg_field(w, 3),
                    rs1: reg_field(w, 6),
                    imm: field(w, 13, 14),
                })
            }
            OP_BR3 => Ok(Inst::Br3 {
                rs1: reg_field(w, 3),
                neg: field(w, 6, 7),
                zero: field(w, 13, 7),
                pos: field(w, 20, 7),
            }),
            OP_JAL => Ok(Inst::Jal {
                rd: reg_field(w, 3),
                off: field(w, 6, 21),
            }),
            OP_JALR => {
                zero_field(w, 9, 4)?;
                Ok(Inst::Jalr {
                    rd: reg_field(w, 3),
                    rs1: reg_field(w, 6),
                    imm: field(w, 13, 14),
                })
            }
            OP_LUI => {
                zero_field(w, 19, 8)?;
                Ok(Inst::Lui {
                    rd: reg_field(w, 3),
                    imm: field(w, 6, 13),
                })
            }
            OP_SEL3 => {
                zero_field(w, 18, 9)?;
                Ok(Inst::Sel3 {
                    rd: reg_field(w, 3),
                    rt: reg_field(w, 6),
                    rn: reg_field(w, 9),
                    rz: reg_field(w, 12),
                    rp: reg_field(w, 15),
                })
            }
            OP_SYS => match field(w, 9, 3) as i8 {
                SYS_HALT => {
                    zero_field(w, 3, 3)?;
                    zero_field(w, 12, 1)?;
                    zero_field(w, 13, 14)?;
                    Ok(Inst::Halt {
                        rs1: reg_field(w, 6),
                    })
                }
                SYS_TRAP => {
                    zero_field(w, 3, 3)?;
                    zero_field(w, 6, 3)?;
                    zero_field(w, 12, 1)?;
                    let imm = field(w, 13, 14);
                    let code = trap_code(imm)
                        .ok_or_else(|| Malformed(format!("reserved trap code {imm}")))?;
                    Ok(Inst::Trap { code })
                }
                f => Err(Malformed(format!("reserved sys funct {f}"))),
            },
            other => Err(Malformed(format!("reserved opcode {other}"))),
        }
    }
}

fn zero_field(w: i128, lo: u32, len: u32) -> Result<(), Malformed> {
    if field(w, lo, len) == 0 {
        Ok(())
    } else {
        Err(Malformed(format!(
            "trits [{lo}..{}] must be zero",
            lo + len - 1
        )))
    }
}

fn width_funct(width: Width) -> i128 {
    match width {
        Width::Word => 0,
        Width::Tryte => 1,
    }
}

fn funct_width(f: i8) -> Result<Width, Malformed> {
    match f {
        0 => Ok(Width::Word),
        1 => Ok(Width::Tryte),
        other => Err(Malformed(format!("reserved access width {other}"))),
    }
}

fn trap_imm(code: FaultCode) -> i128 {
    match code {
        FaultCode::Trap => 0,
        FaultCode::Overflow => 1,
        FaultCode::DivZero => 2,
        FaultCode::Shift => 3,
        FaultCode::Align => 4,
    }
}

fn trap_code(imm: i128) -> Option<FaultCode> {
    Some(match imm {
        0 => FaultCode::Trap,
        1 => FaultCode::Overflow,
        2 => FaultCode::DivZero,
        3 => FaultCode::Shift,
        4 => FaultCode::Align,
        _ => return None,
    })
}

impl std::fmt::Display for Inst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = |x: Reg| x.abi_name();
        match self {
            Inst::Alu {
                op,
                flavor,
                rd,
                rs1,
                rs2,
                rc,
            } => {
                let fl = if op.is_flavored() {
                    flavor.suffix()
                } else {
                    ""
                };
                write!(f, "{}{fl} {}, {}, {}", op.name(), r(*rd), r(*rs1), r(*rs2))?;
                if *flavor == Flavor::Flag {
                    write!(f, ", {}", r(*rc))?;
                }
                Ok(())
            }
            Inst::AluI {
                op,
                flavor,
                rd,
                rs1,
                imm,
            } => {
                let fl = if op.is_flavored() {
                    flavor.suffix()
                } else {
                    ""
                };
                write!(f, "{}i{fl} {}, {}, {imm}", op.name(), r(*rd), r(*rs1))
            }
            Inst::Load {
                width,
                rd,
                rs1,
                imm,
            } => write!(f, "ld.{} {}, {imm}({})", width.name(), r(*rd), r(*rs1)),
            Inst::Store {
                width,
                rs2,
                rs1,
                imm,
            } => write!(f, "st.{} {}, {imm}({})", width.name(), r(*rs2), r(*rs1)),
            Inst::Br3 {
                rs1,
                neg,
                zero,
                pos,
            } => write!(f, "br3 {}, {neg}, {zero}, {pos}", r(*rs1)),
            Inst::Jal { rd, off } => write!(f, "jal {}, {off}", r(*rd)),
            Inst::Jalr { rd, rs1, imm } => write!(f, "jalr {}, {imm}({})", r(*rd), r(*rs1)),
            Inst::Lui { rd, imm } => write!(f, "lui {}, {imm}", r(*rd)),
            Inst::Sel3 { rd, rt, rn, rz, rp } => write!(
                f,
                "sel3 {}, {}, {}, {}, {}",
                r(*rd),
                r(*rt),
                r(*rn),
                r(*rz),
                r(*rp)
            ),
            Inst::Halt { rs1 } => write!(f, "halt {}", r(*rs1)),
            Inst::Trap { code } => write!(f, "trap {code}"),
        }
    }
}
