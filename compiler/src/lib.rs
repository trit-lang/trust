//! `trustc` — the Trust compiler.
//!
//! Draft 0.1 implements the layers the specification actually pins down:
//! balanced ternary arithmetic (in `trit-core`), TIR — its data structures,
//! textual format, verifier, reference interpreter and legalization pass —
//! and the layout engine of Language Ch. 2.
//!
//! The Trust surface syntax is explicitly provisional in the spec ("syntax
//! provisional", TIR appendix; "the rest of surface syntax", Types Ch. 1 §4),
//! so there is no `.tr` parser yet. The layout engine does not need one: Ch. 2
//! is stated in terms of types, not syntax. See `docs/spec-gaps.md`.

pub mod codegen;
pub mod lang;
pub mod layout;
pub mod tir;
