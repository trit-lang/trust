//! TIR — the Ternary Intermediate Representation (`spec/tir-0.1.md`).
//!
//! TIR is not a stable interface: it is the internal contract between this
//! repository's frontend and its backends, and may change without deprecation
//! in any release.

pub mod canon;
pub mod inline;
pub mod interp;
pub mod ir;
pub mod legalize;
pub mod mem2reg;
pub mod target;
pub mod text;
pub mod verify;

pub use canon::{branch_through_select, canonicalize_module, promote_slots, remove_dead};
pub use inline::{drop_uncalled, inline_module};
pub use interp::{Halt, Interp, Val};
pub use ir::*;
pub use legalize::{LegalizeError, legalize_module};
pub use target::TargetDesc;
pub use text::{ParseError, parse_module, print_module};
pub use verify::{VerifyError, verify, verify_legalized};
