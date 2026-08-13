//! The Trust frontend: source text to TIR.
//!
//! Implements Language Ch. 0 (syntax), the type rules of Ch. 1 that do not
//! need traits, and the parts of Ch. 2 that scalars and arrays reach. What it
//! does not cover is what the specification has not written: strings,
//! references, generics, structs and enums.

pub mod ast;
pub mod lex;
pub mod lower;
pub mod parse;

pub use lex::SyntaxError;

/// Compile a source file to a TIR module.
pub fn compile(src: &str) -> Result<crate::tir::Module, Vec<SyntaxError>> {
    let file = parse::parse(src).map_err(|e| vec![e])?;
    lower::lower(&file)
}
