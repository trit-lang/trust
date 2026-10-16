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
    /// Instructions retired since reset, as a word (ISA §2.3).
    pub const CYCLES: i128 = -9;
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
    /// A source read one code unit at a time, when the program asks for one.
    ///
    /// Lazily, on purpose: a program that never touches `IO_IN` must not
    /// block on a stream that may never close.
    source: Option<Box<dyn std::io::Read>>,
    /// Everything written to `IO_OUT`.
    output: Vec<u8>,
    /// An optional pass-through for output, so a long-running program's
    /// writing is seen as it happens rather than at the end.
    sink: Option<Box<dyn std::io::Write>>,
}

impl Clone for Io {
    /// Snapshots need the machine's exact io state; the stream ends do not
    /// clone, and do not need to: a snapshot is taken with the input held in
    /// `pending` and is restored with no sink.
    fn clone(&self) -> Io {
        Io {
            pending: self.pending.clone(),
            closed: self.closed,
            source: None,
            output: self.output.clone(),
            sink: None,
        }
    }
}

impl Io {
    /// Input not yet consumed, as one slice — for snapshots.
    pub fn pending_bytes(&self) -> Vec<u8> {
        self.pending.iter().copied().collect()
    }

    /// True once the program has been told there is no more input.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Rebuild an io state from snapshot parts: the input that was left and
    /// whether the stream had closed. Output starts empty — it is not part
    /// of anything the machine can observe.
    pub fn from_pending(pending: Vec<u8>, closed: bool) -> Io {
        Io {
            pending: pending.into(),
            closed,
            ..Io::default()
        }
    }
}

impl Io {
    /// Supply input and close the stream — the whole program's input at once.
    pub fn with_input(bytes: &[u8]) -> Io {
        Io {
            pending: bytes.iter().copied().collect(),
            closed: true,
            ..Io::default()
        }
    }

    /// Draw input from a stream, one code unit at a time as the program asks.
    pub fn with_source(source: Box<dyn std::io::Read>) -> Io {
        Io {
            source: Some(source),
            ..Io::default()
        }
    }

    /// Also write everything the program writes to this sink.
    pub fn with_sink(mut self, sink: Box<dyn std::io::Write>) -> Io {
        self.sink = Some(sink);
        self
    }

    /// Everything the program has written.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// The next code unit, or `None` if none is available yet.
    fn next_unit(&mut self) -> Option<u8> {
        if let Some(b) = self.pending.pop_front() {
            return Some(b);
        }
        let source = self.source.as_mut()?;
        let mut byte = [0u8; 1];
        match source.read(&mut byte) {
            Ok(1) => Some(byte[0]),
            _ => {
                // End of stream, or an error we cannot report to a machine
                // that has no error port: either way, no more input.
                self.closed = true;
                self.source = None;
                None
            }
        }
    }
}

/// A TRISC-27 machine.
pub struct Vm {
    regs: [i128; 27],
    pc: i128,
    mem: Memory,
    /// The character ports.
    pub io: Io,
    /// Instructions retired since reset, which `CYCLES` reports (ISA §2.3).
    steps: u64,
    /// Decoded instructions, by address — see `decode_at`.
    icache: Vec<Option<(i128, Inst)>>,
    /// Opt-in diagnostics: everything here is off with zero fields set, and
    /// costs one branch per step while off. See `Diag`.
    diag: Diag,
}

/// Entries in the decode cache. Direct-mapped, so a lookup is one index and
/// one comparison; 2^16 covers any image this machine runs.
const ICACHE: usize = 1 << 16;

/// The diagnostic gear a long run (read: DDC) carries. All opt-in:
///
/// - `ring` — the last N PCs, one store per retired instruction. On a fault
///   the tail of the ring is the recent control flow, and with an assembly
///   listing that is the functions the fault sits in.
/// - `watch` — tryte ranges a store is reported on hitting. The word a heap
///   bug poisons is found once in a core dump; watching it then names the
///   instruction that did the poisoning — determinism makes the poisoned
///   address stable across runs of an unchanged image.
/// - `snap_every`/`snap`/`snap_path` — a full machine state saved every N
///   instructions. Execution is deterministic given the image and input, so
///   replaying the last snapshot with a watch armed catches in seconds what
///   a from-scratch rerun catches in hours.
#[derive(Default)]
struct Diag {
    /// Power-of-two ring of PCs; empty means off.
    ring: Vec<i128>,
    /// Next slot in `ring` to write.
    at: usize,
    /// The address of the instruction currently executing.
    cur_pc: i128,
    /// Inclusive tryte ranges to report stores into.
    watch: Vec<(i128, i128)>,
    /// Snapshot cadence, in retired instructions.
    snap_every: Option<u64>,
    /// The most recent snapshot.
    snap: Option<Box<crate::snap::Snap>>,
    /// Where every snapshot is also written as it is taken.
    snap_path: Option<String>,
}

impl Vm {
    /// A machine with the given address-space size, at reset (§1.3).
    pub fn new(mem_size: i128) -> Vm {
        Vm {
            regs: [0; 27],
            pc: 0,
            mem: Memory::new(mem_size),
            io: Io::default(),
            steps: 0,
            icache: vec![None; ICACHE],
            diag: Diag::default(),
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
            self.forget(i as i128 * WORD_TRYTES);
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

    /// Instructions retired since reset — what `CYCLES` reads, and what the
    /// CLI reports when a program stops.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Execute one instruction.
    pub fn step(&mut self) -> Result<(), Stop> {
        let pc = self.pc;
        if !self.mem.contains(pc) || !self.mem.contains(pc + 2) {
            self.diag.cur_pc = pc;
            return Err(fault(
                FaultCode::Trap,
                format!("instruction fetch at {pc} is outside memory"),
            ));
        }
        let inst = self.decode_at(pc)?;

        // The default is to fall through to the next word; control transfers
        // overwrite this.
        self.pc = pc + WORD_TRYTES;
        self.diag.cur_pc = pc;
        if !self.diag.ring.is_empty() {
            let at = self.diag.at & (self.diag.ring.len() - 1);
            self.diag.ring[at] = pc;
            self.diag.at = at + 1;
        }
        let outcome = self.exec(inst, pc);
        // Retired, not merely started: an instruction that faults did not
        // complete and is not counted (ISA §2.3).
        if outcome.is_ok() {
            self.steps = self.steps.wrapping_add(1);
            if let Some(n) = self.diag.snap_every {
                if self.steps % n == 0 {
                    self.take_snap();
                }
            }
        }
        outcome
    }

    /// The instruction at `pc`, decoded once per address.
    ///
    /// An image is ordinary memory and a program may write to it, so the
    /// cache is invalidated by every store — which is one array write, and
    /// cheaper than decoding.
    fn decode_at(&mut self, pc: i128) -> Result<Inst, Stop> {
        let slot = ((pc / WORD_TRYTES) as usize) & (ICACHE - 1);
        if let Some((at, inst)) = &self.icache[slot]
            && *at == pc
        {
            return Ok(*inst);
        }
        let word = self.mem.word(pc);
        let inst =
            Inst::decode(word).map_err(|e| fault(FaultCode::Trap, format!("at {pc}: {e}")))?;
        self.icache[slot] = Some((pc, inst));
        Ok(inst)
    }

    /// Forget any decoding of the word covering `addr`, and of the words a
    /// store overlapping it could have changed.
    fn forget(&mut self, addr: i128) {
        for a in [addr - 2, addr - 1, addr] {
            if a < 0 {
                continue;
            }
            let slot = ((a / WORD_TRYTES) as usize) & (ICACHE - 1);
            if let Some((at, _)) = &self.icache[slot]
                && *at == (a / WORD_TRYTES) * WORD_TRYTES
            {
                self.icache[slot] = None;
            }
        }
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
        if !self.diag.watch.is_empty()
            && self
                .diag
                .watch
                .iter()
                .any(|&(lo, hi)| addr <= hi && lo < addr + width.trytes())
        {
            eprintln!(
                "[watch] steps={} pc={} store {} tryte(s) at {} value {}",
                self.steps,
                self.diag.cur_pc,
                width.trytes(),
                addr,
                v
            );
            if std::env::var_os("TRITIUM_WATCH_REGS").is_some() {
                for r in -13..=13i8 {
                    let reg = Reg(r);
                    eprintln!("  {}({}) = {}", reg.name(), reg.abi_name(), self.reg(reg));
                }
            }
        }
        match width {
            Width::Word => self.mem.set_word(addr, v),
            Width::Tryte => self.mem.set_tryte(addr, word::wrap_to(v, 9)),
        }
        // An image is ordinary memory: nothing stops a program from writing
        // over its own code, so a decoding is only good until something does.
        self.forget(addr);
        Ok(())
    }

    fn check_access(&self, addr: i128, width: Width) -> Result<(), Stop> {
        // `addr` is non-negative here — a negative one is a device and was
        // handled before this — so `%` is the cheaper remainder.
        if width == Width::Word && addr % WORD_TRYTES != 0 {
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
            (device::IO_IN, Width::Tryte) => Ok(match self.io.next_unit() {
                Some(b) => b as i128,
                None if self.io.closed => -2,
                None => -1,
            }),
            (device::MEM_SIZE, Width::Word) => Ok(self.mem.size()),
            // ISA §2.3: the load has not retired yet, so two readings differ
            // by exactly what ran between them.
            (device::CYCLES, Width::Word) => Ok(word::wrap_to(self.steps as i128, 27)),
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
                if let Some(sink) = self.io.sink.as_mut() {
                    let _ = sink.write_all(&[unit as u8]);
                    let _ = sink.flush();
                }
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

// ---------------------------------------------------------- diagnostics
//
// Everything here exists for one hunt: a heap corruption that only shows
// hours into a DDC run (target/HANDOFF.md). Off means off — no field is
// touched on the hot path unless the matching enable call was made.

/// How many ring entries a report prints, newest last.
const REPORT_RING: usize = 512;

impl Vm {
    /// Record the last `cap` PCs. `cap` must be a power of two.
    pub fn enable_ring(&mut self, cap: usize) {
        assert!(cap.is_power_of_two(), "ring capacity must be a power of two");
        self.diag.ring = vec![0; cap];
        self.diag.at = 0;
    }

    /// Report every store that touches one of these inclusive tryte ranges.
    pub fn set_watch(&mut self, ranges: Vec<(i128, i128)>) {
        self.diag.watch = ranges;
    }

    /// Save a snapshot every `every` retired instructions — kept in memory,
    /// and also written to `path` when given, replacing the previous one.
    pub fn enable_snapshots(&mut self, every: u64, path: Option<String>) {
        self.diag.snap_every = Some(every);
        self.diag.snap_path = path;
    }

    /// The most recent periodic snapshot.
    pub fn last_snap(&self) -> Option<&crate::snap::Snap> {
        self.diag.snap.as_deref()
    }

    /// The whole machine, as a snapshot.
    pub fn capture_snap(&self) -> crate::snap::Snap {
        crate::snap::Snap {
            mem_size: self.mem.size(),
            pc: self.pc,
            steps: self.steps,
            regs: self.regs,
            io: self.io.clone(),
            pages: self
                .mem
                .pages()
                .map(|(start, contents)| (start, contents.into()))
                .collect(),
        }
    }

    /// A machine standing exactly where the snapshot was taken. Execution
    /// from here is identical to the run the snapshot came from.
    pub fn from_snap(snap: &crate::snap::Snap) -> Vm {
        let mut vm = Vm::new(snap.mem_size);
        vm.regs = snap.regs;
        vm.pc = snap.pc;
        vm.steps = snap.steps;
        vm.io = snap.io.clone();
        vm.mem.restore_pages(
            snap.pages
                .iter()
                .map(|(start, contents)| (*start, contents.clone()))
                .collect(),
        );
        vm
    }

    /// Take the periodic snapshot: keep the newest, write it through.
    fn take_snap(&mut self) {
        let snap = Box::new(self.capture_snap());
        if let Some(path) = &self.diag.snap_path {
            let _ = snap.write_file(path);
        }
        eprintln!(
            "[snap] steps={} pages={}",
            self.steps,
            snap.pages.len()
        );
        self.diag.snap = Some(snap);
    }

    /// The instruction at `cur_pc`, for the report — tolerant of the same
    /// faults the machine reports on (an unaligned or outside address).
    fn cur_inst(&self) -> String {
        let pc = self.diag.cur_pc;
        if pc % WORD_TRYTES != 0 || !self.mem.contains(pc) {
            return format!("<not an aligned instruction address: {pc}>");
        }
        match Inst::decode(self.mem.word(pc)) {
            Ok(inst) => format!("{inst}"),
            Err(e) => format!("<does not decode: {e}>"),
        }
    }

    /// Everything there is to say when a run stops on a fault: the stop
    /// itself, how far in it happened, the faulting instruction, every
    /// register, and the recent control flow. Two runs of an unchanged
    /// image are bit-identical, so every number here reproduces.
    pub fn report(&self, stop: &Stop) -> String {
        let mut out = format!("tritium: {stop}\n");
        out.push_str(&format!("steps: {}\n", self.steps));
        out.push_str(&format!("at {}: {}\n", self.diag.cur_pc, self.cur_inst()));
        out.push_str("registers:\n");
        for v in -13..=13i8 {
            let r = Reg(v);
            out.push_str(&format!(
                "  {:<4} {:<4} {}\n",
                r.name(),
                r.abi_name(),
                self.regs[r.index()]
            ));
        }
        if self.diag.ring.is_empty() {
            return out;
        }
        out.push_str("recent pcs, oldest first:\n");
        let cap = self.diag.ring.len();
        let kept = self.diag.at.min(cap);
        let show = kept.min(REPORT_RING);
        let first = self.diag.at - show;
        for i in 0..show {
            if i % 8 == 0 {
                out.push_str("  ");
            }
            out.push_str(&format!("{} ", self.diag.ring[(first + i) & (cap - 1)]));
            if i % 8 == 7 || i + 1 == show {
                out.push('\n');
            }
        }
        out
    }
}

