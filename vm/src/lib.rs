//! `tritium` — the reference virtual machine implementing TRISC-27.
//!
//! It is the concrete realization of the abstract machine that
//! `spec/isa/trisc-27-0.1.md` specifies: 27 registers, one-word instructions,
//! a tryte-addressed memory, and the two memory-mapped character ports of
//! AM §5 sitting at negative addresses where memory can never reach them.
//!
//! ```
//! use tritium::{Inst, Reg, Stop, Vm};
//! use trit_core::Flavor;
//!
//! // A program that adds two constants and halts with the result.
//! let a0 = Reg::from_name("a0").unwrap();
//! let program = [
//!     Inst::AluI { op: tritium::AluOp::Add, flavor: Flavor::Wrap, rd: a0, rs1: Reg::ZERO, imm: 40 },
//!     Inst::AluI { op: tritium::AluOp::Add, flavor: Flavor::Wrap, rd: a0, rs1: a0, imm: 2 },
//!     Inst::Halt { rs1: a0 },
//! ];
//!
//! let mut vm = Vm::with_default_memory();
//! vm.load_words(&program.map(Inst::encode));
//! assert_eq!(vm.run(100), Stop::Halted(42));
//! ```

pub mod asm;
pub mod image;
pub mod inst;
pub mod mem;
pub mod profile;
pub mod vm;
pub mod word;

pub use asm::{AsmError, assemble};
pub use inst::{AluOp, Inst, Malformed, Reg, Width};
pub use mem::Memory;
pub use profile::{Profile, classify, profile};
pub use vm::{DEFAULT_MEM_SIZE, Io, Stop, Vm, device};
