//! Counting what actually ran.
//!
//! The machine has a cycle counter (TRISC-27 §2.3), and it answers "how
//! many" but never "which". This answers "which", by decoding each
//! instruction before executing it.
//!
//! It exists because the answer was surprising. The backend's next task had
//! been chosen by reading the code, and reading the code picked the hardest
//! of four candidates; a profile of `examples/trust/HPL.tr` put three cheap
//! ones ahead of it, worth 26% of the program between them
//! (`docs/spec-gaps.md` G8.6). An instrument that is not there does not
//! merely leave you uninformed — it leaves you confident.

use crate::inst::{Inst, Reg};
use crate::vm::{Stop, Vm};
use std::collections::BTreeMap;

/// What ran, and where.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Profile {
    /// Instructions counted, whether or not they completed.
    pub total: u64,
    /// How many of each kind (`classify`).
    pub by_kind: BTreeMap<String, u64>,
    /// How many times each address was executed.
    pub by_pc: BTreeMap<i128, u64>,
    /// Why the machine stopped, or `None` if the step cap was reached first.
    pub stop: Option<Stop>,
}

impl Profile {
    /// The share of everything counted, as a percentage.
    pub fn share(&self, n: u64) -> f64 {
        100.0 * n as f64 / self.total.max(1) as f64
    }

    /// Loads and stores addressed from `sp`: values the compiler could not
    /// keep in a register.
    pub fn frame_traffic(&self) -> u64 {
        self.access_traffic(" sp")
    }

    /// Loads and stores addressed from any other register: the program's own
    /// data.
    pub fn data_traffic(&self) -> u64 {
        self.access_traffic(" reg")
    }

    fn access_traffic(&self, from: &str) -> u64 {
        self.by_kind
            .iter()
            .filter(|(k, _)| (k.starts_with("ld.") || k.starts_with("st.")) && k.ends_with(from))
            .map(|(_, n)| n)
            .sum()
    }

    /// Execution counts, largest first — the concentration curve. A steep one
    /// says the work worth doing is small and findable.
    pub fn hottest(&self) -> Vec<(i128, u64)> {
        let mut v: Vec<_> = self.by_pc.iter().map(|(a, n)| (*a, *n)).collect();
        v.sort_by_key(|(a, n)| (std::cmp::Reverse(*n), *a));
        v
    }
}

/// What an instruction is counted as.
///
/// The mnemonic alone is not enough to act on. Three distinctions carry most
/// of what a profile is read for, and each is one field of the encoding:
///
/// - **What a load or store is addressed from.** Through `sp` it is frame
///   traffic; through any other register it is the program's own data. The
///   ratio between those two is the case for a better register allocator,
///   and no other measurement states it.
/// - **What an `alui` is computed from.** From `zero` it is not arithmetic:
///   it is a constant being materialized, which an immediate field may have
///   had room for.
/// - **Whether a `jal` links.** With `ra` it is a call the program asked
///   for; with `r0` it is an unconditional jump, which is a shape the code
///   generator chose and could choose differently.
pub fn classify(inst: &Inst) -> String {
    fn base(r: Reg) -> &'static str {
        match r.abi_name() {
            "sp" => "sp",
            "fp" => "fp",
            "zero" => "zero",
            _ => "reg",
        }
    }
    match inst {
        Inst::Alu { op, .. } => format!("alu.{}", op.name()),
        Inst::AluI { op, rs1, .. } => format!("alui.{} {}", op.name(), base(*rs1)),
        Inst::Load { width, rs1, .. } => format!("ld.{} {}", width.name(), base(*rs1)),
        Inst::Store { width, rs1, .. } => format!("st.{} {}", width.name(), base(*rs1)),
        Inst::Br3 { .. } => "br3".into(),
        Inst::Jal { rd, .. } if rd.0 == 0 => "jal (jump)".into(),
        Inst::Jal { .. } => "jal (call)".into(),
        Inst::Jalr { .. } => "jalr".into(),
        Inst::Lui { .. } => "lui".into(),
        Inst::Sel3 { .. } => "sel3".into(),
        Inst::Halt { .. } => "halt".into(),
        Inst::Trap { .. } => "trap".into(),
    }
}

/// Run a loaded machine one instruction at a time, counting what runs.
///
/// The machine is left where it stopped, so its memory and registers are
/// still there to look at.
pub fn profile(vm: &mut Vm, cap: u64) -> Profile {
    let mut p = Profile::default();
    loop {
        if p.total >= cap {
            return p;
        }
        let pc = vm.pc();
        // An unfetchable word is counted as what it is rather than guessed
        // at: the step below is what decides whether it faults.
        let kind = if vm.memory().contains(pc) {
            match Inst::decode(vm.memory().word(pc)) {
                Ok(inst) => classify(&inst),
                Err(_) => "malformed".to_string(),
            }
        } else {
            "outside memory".to_string()
        };
        *p.by_kind.entry(kind).or_default() += 1;
        *p.by_pc.entry(pc).or_default() += 1;
        p.total += 1;
        if let Err(s) = vm.step() {
            p.stop = Some(s);
            return p;
        }
    }
}
