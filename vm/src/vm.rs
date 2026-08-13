//! The machine: fetch, decode, execute (TRISC-27 §1–§6).

use crate::inst::{AluOp, Inst, Reg, Width};
use crate::mem::Memory;
use crate::word::{self, MAX_WORD, WORD_TRITS, WORD_TRYTES};
use trit_core::{FaultCode, Flavor};

/// The default address-space size of the reference machine — a platform
/// choice, not an architectural one (TRISC-27 §2.1).
pub const DEFAULT_MEM_SIZE: i128 = 14_348_907; // 3^15

/// Device addresses (TRISC-27 §2.2). Negative addresses are never memory, at
/// any address-space size, which is what lets `A` grow without moving them.
pub mod device {
    /// Reads one UTF-8 code unit, or −1 for "none yet", −2 for end of input.
    pub const IO_IN: i128 = -1;
    /// Writes one UTF-8 code unit.
    pub const IO_OUT: i128 = -2;
    /// Reads the address-space size A.
    pub const MEM_SIZE: i128 = -6;
}

/// Why the machine stopped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Stop {
    /// `halt` executed; the value is the exit status.
    Halted(i128),
    /// A fault (AM §4). Faults halt the machine and have no handler.
    Fault(FaultCode, String),
    /// Not a machine state: the interpreter's step budget ran out.
    OutOfSteps,
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Halted(s) => write!(f, "halted with status {s}"),
            Stop::Fault(c, why) => write!(f, "fault {c}: {why}"),
            Stop::OutOfSteps => f.write_str("step budget exhausted"),
        }
    }
}

/// The character ports' external ends.
#[derive(Default)]
pub struct Io {
    /// Input not yet consumed, in order.
    pending: std::collections::VecDeque<u8>,
    /// True once no further input will ever arrive.
    closed: bool,
    /// Everything written to `IO_OUT`.
    output: Vec<u8>,
}

impl Io {
    /// Supply input and close the stream — the whole program's input at once.
    pub fn with_input(bytes: &[u8]) -> Io {
        Io {
            pending: bytes.iter().copied().collect(),
            closed: true,
            output: Vec::new(),
        }
    }

    /// Everything the program has written.
    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

/// A TRISC-27 machine.
pub struct Vm {
    regs: [i128; 27],
    pc: i128,
    mem: Memory,
    /// The character ports.
    pub io: Io,
}

impl Vm {
    /// A machine with the given address-space size, at reset (§1.3).
    pub fn new(mem_size: i128) -> Vm {
        Vm {
            regs: [0; 27],
            pc: 0,
            mem: Memory::new(mem_size),
            io: Io::default(),
        }
    }

    /// A machine of the default size.
    pub fn with_default_memory() -> Vm {
        Vm::new(DEFAULT_MEM_SIZE)
    }

    /// Load a raw image at address 0 (TRISC-27 §8).
    pub fn load_image(&mut self, trytes: &[i16]) {
        self.mem.load_image(trytes);
    }

    /// Load a program given as instruction words.
    pub fn load_words(&mut self, words: &[i128]) {
        for (i, &w) in words.iter().enumerate() {
            self.mem.set_word(i as i128 * WORD_TRYTES, w);
        }
    }

    /// The value of a register. `r0` always reads 0.
    pub fn reg(&self, r: Reg) -> i128 {
        if r.0 == 0 { 0 } else { self.regs[r.index()] }
    }

    /// Write a register. Writes to `r0` are discarded.
    pub fn set_reg(&mut self, r: Reg, v: i128) {
        if r.0 != 0 {
            self.regs[r.index()] = v;
        }
    }

    /// The program counter.
    pub fn pc(&self) -> i128 {
        self.pc
    }

    /// Memory, for tests and for a debugger.
    pub fn memory(&self) -> &Memory {
        &self.mem
    }

    /// Run until the machine stops or the step budget runs out.
    pub fn run(&mut self, max_steps: u64) -> Stop {
        for _ in 0..max_steps {
            if let Err(stop) = self.step() {
                return stop;
            }
        }
        Stop::OutOfSteps
    }

    /// Execute one instruction.
    pub fn step(&mut self) -> Result<(), Stop> {
        let pc = self.pc;
        if !self.mem.contains(pc) || !self.mem.contains(pc + 2) {
            return Err(fault(
                FaultCode::Trap,
                format!("instruction fetch at {pc} is outside memory"),
            ));
        }
        let word = self.mem.word(pc);
        let inst =
            Inst::decode(word).map_err(|e| fault(FaultCode::Trap, format!("at {pc}: {e}")))?;

        // The default is to fall through to the next word; control transfers
        // overwrite this.
        self.pc = pc + WORD_TRYTES;
        self.exec(inst, pc)
    }

    fn exec(&mut self, inst: Inst, pc: i128) -> Result<(), Stop> {
        match inst {
            Inst::Alu {
                op,
                flavor,
                rd,
                rs1,
                rs2,
                rc,
            } => {
                let (a, b) = (self.reg(rs1), self.reg(rs2));
                let (value, over) = self.alu(op, flavor, a, b)?;
                self.set_reg(rd, value);
                if flavor == Flavor::Flag {
                    self.set_reg(rc, over);
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
                let a = self.reg(rs1);
                let (value, _) = self.alu(op, flavor, a, imm)?;
                self.set_reg(rd, value);
                Ok(())
            }

            Inst::Load {
                width,
                rd,
                rs1,
                imm,
            } => {
                let addr = self.reg(rs1) + imm;
                let v = self.load(addr, width)?;
                self.set_reg(rd, v);
                Ok(())
            }

            Inst::Store {
                width,
                rs2,
                rs1,
                imm,
            } => {
                let addr = self.reg(rs1) + imm;
                let v = self.reg(rs2);
                self.store(addr, width, v)
            }

            // The branch reads the *sign* of the register, so any value
            // works and a trit is simply the common case (§4.5).
            Inst::Br3 {
                rs1,
                neg,
                zero,
                pos,
            } => {
                let off = match word::sign(self.reg(rs1)) {
                    -1 => neg,
                    0 => zero,
                    _ => pos,
                };
                self.pc = pc + WORD_TRYTES * off;
                Ok(())
            }

            Inst::Jal { rd, off } => {
                self.set_reg(rd, pc + WORD_TRYTES);
                self.pc = pc + WORD_TRYTES * off;
                Ok(())
            }

            // The target is computed before the link register is written, so
            // `jalr ra, 0(ra)` is well defined (§4.5).
            Inst::Jalr { rd, rs1, imm } => {
                let target = self.reg(rs1) + imm;
                if target.rem_euclid(WORD_TRYTES) != 0 {
                    return Err(fault(
                        FaultCode::Align,
                        format!("jump target {target} is not word-aligned"),
                    ));
                }
                self.set_reg(rd, pc + WORD_TRYTES);
                self.pc = target;
                Ok(())
            }

            Inst::Lui { rd, imm } => {
                self.set_reg(rd, word::shl3(imm, 14));
                Ok(())
            }

            Inst::Sel3 { rd, rt, rn, rz, rp } => {
                let arm = match word::sign(self.reg(rt)) {
                    -1 => rn,
                    0 => rz,
                    _ => rp,
                };
                let v = self.reg(arm);
                self.set_reg(rd, v);
                Ok(())
            }

            Inst::Halt { rs1 } => Err(Stop::Halted(self.reg(rs1))),

            Inst::Trap { code } => Err(fault(code, "`trap` instruction".to_string())),
        }
    }

    /// The arithmetic unit. Returns the result and the overflow trit.
    fn alu(&self, op: AluOp, flavor: Flavor, a: i128, b: i128) -> Result<(i128, i128), Stop> {
        let exact = match op {
            AluOp::Add => a + b,
            AluOp::Sub => a - b,
            AluOp::Mul => a * b,

            AluOp::MulH => {
                // The high 27 trits of the 54-trit product: the half that a
                // same-width multiply throws away, and the reason multi-part
                // multiplication is possible at all (§4.1).
                let full = a * b;
                return Ok((word::shr3(full, WORD_TRITS), 0));
            }

            AluOp::Div | AluOp::Rem if b == 0 => {
                return Err(fault(FaultCode::DivZero, format!("{} by zero", op.name())));
            }
            AluOp::Div => return Ok((word::div_nearest(a, b), 0)),
            AluOp::Rem => return Ok((word::rem_nearest(a, b), 0)),

            AluOp::Shl | AluOp::Shr => {
                // AM §3.3: k is 0 … n−1 for an n-trit operand, and anything
                // else faults — not masked, not undefined.
                if !(0..WORD_TRITS as i128).contains(&b) {
                    return Err(fault(
                        FaultCode::Shift,
                        format!("shift amount {b} is outside 0..{}", WORD_TRITS - 1),
                    ));
                }
                match op {
                    AluOp::Shl => word::shl3(a, b as u32),
                    _ => return Ok((word::shr3(a, b as u32), 0)),
                }
            }

            AluOp::TMin => return Ok((word::tmin(a, b), 0)),
            AluOp::TMax => return Ok((word::tmax(a, b), 0)),
            AluOp::TMul => return Ok((word::tmul(a, b), 0)),
            AluOp::Cmp => return Ok((word::cmp3(a, b), 0)),

            AluOp::Wrap => {
                if !(1..=WORD_TRITS as i128).contains(&b) {
                    return Err(fault(
                        FaultCode::Trap,
                        format!("`wrap` width {b} is outside 1..27"),
                    ));
                }
                return Ok((word::wrap_to(a, b as u32), 0));
            }
        };

        let r = word::overflowing(exact);
        match flavor {
            Flavor::Trap if r.overflow != 0 => Err(fault(
                FaultCode::Overflow,
                format!("{} overflowed the word", op.name()),
            )),
            _ => Ok((r.wrapped, r.overflow)),
        }
    }

    // ------------------------------------------------------------- memory

    fn load(&mut self, addr: i128, width: Width) -> Result<i128, Stop> {
        if addr < 0 {
            return self.device_load(addr, width);
        }
        self.check_access(addr, width)?;
        Ok(match width {
            Width::Word => self.mem.word(addr),
            Width::Tryte => self.mem.tryte(addr),
        })
    }

    fn store(&mut self, addr: i128, width: Width, v: i128) -> Result<(), Stop> {
        if addr < 0 {
            return self.device_store(addr, width, v);
        }
        self.check_access(addr, width)?;
        match width {
            Width::Word => self.mem.set_word(addr, v),
            Width::Tryte => self.mem.set_tryte(addr, word::wrap_to(v, 9)),
        }
        Ok(())
    }

    fn check_access(&self, addr: i128, width: Width) -> Result<(), Stop> {
        if width == Width::Word && addr.rem_euclid(WORD_TRYTES) != 0 {
            return Err(fault(
                FaultCode::Align,
                format!("word access at {addr} is not 3-tryte aligned"),
            ));
        }
        if !self.mem.contains(addr) || !self.mem.contains(addr + width.trytes() - 1) {
            return Err(fault(
                FaultCode::Trap,
                format!(
                    "access at {addr} is outside memory (A = {})",
                    self.mem.size()
                ),
            ));
        }
        Ok(())
    }

    fn device_load(&mut self, addr: i128, width: Width) -> Result<i128, Stop> {
        match (addr, width) {
            (device::IO_IN, Width::Tryte) => Ok(match self.io.pending.pop_front() {
                Some(b) => b as i128,
                None if self.io.closed => -2,
                None => -1,
            }),
            (device::MEM_SIZE, Width::Word) => Ok(self.mem.size()),
            _ => Err(fault(
                FaultCode::Trap,
                format!("no readable {} device at {addr}", width.name()),
            )),
        }
    }

    fn device_store(&mut self, addr: i128, width: Width, v: i128) -> Result<(), Stop> {
        match (addr, width) {
            (device::IO_OUT, Width::Tryte) => {
                let unit = word::wrap_to(v, 9);
                if !(0..=255).contains(&unit) {
                    return Err(fault(
                        FaultCode::Trap,
                        format!("{unit} is not a UTF-8 code unit"),
                    ));
                }
                self.io.output.push(unit as u8);
                Ok(())
            }
            _ => Err(fault(
                FaultCode::Trap,
                format!("no writable {} device at {addr}", width.name()),
            )),
        }
    }
}

fn fault(code: FaultCode, why: impl Into<String>) -> Stop {
    Stop::Fault(code, why.into())
}

/// True iff `v` is a value this machine can hold in a register.
pub fn is_word(v: i128) -> bool {
    (-MAX_WORD..=MAX_WORD).contains(&v)
}
