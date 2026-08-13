//! `trit-core` — balanced ternary arithmetic for the Trust programming
//! language.
//!
//! This crate is the shared numeric substrate of `trustc` and `tritium`: the
//! trit, unbounded balanced ternary integers, the width-typed `tN` values of
//! TIR, the overflow flavors, faults, and literal notation. Everything here
//! implements semantics fixed by the specification in `spec/`; nothing here
//! knows about IR, syntax, or targets.
//!
//! ```
//! use trit_core::{Bt, Flavor, Tint, Trit};
//!
//! // Division rounds to nearest, ties away from zero (Types Ch. 1 §4).
//! let a = Tint::new(27, 7).unwrap();
//! let b = Tint::new(27, 2).unwrap();
//! assert_eq!(a.div(&b).unwrap().to_i128(), Some(4));
//!
//! // Negation is total: MIN == -MAX.
//! assert_eq!(Tint::min(9).neg(), Tint::max(9));
//!
//! // Comparison is three-way and primitive.
//! assert_eq!(a.cmp3(&b), Trit::Pos);
//!
//! // `0t` literals are trit-exact.
//! assert_eq!(Bt::from_trit_str("1T0").unwrap().to_i128(), Some(6));
//! # let _ = Flavor::Wrap;
//! ```

pub mod bt;
pub mod fault;
pub mod int;
pub mod literal;
pub mod trit;

pub use bt::Bt;
pub use fault::{Fault, FaultCode};
pub use int::{Flavor, MAX_WIDTH, Tint};
pub use literal::{Literal, Radix, parse_literal};
pub use trit::{TRITS_PER_HEPT, TRITS_PER_TRYTE, Trit};
