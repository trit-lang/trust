//! The Trust frontend: source text to TIR.
//!
//! Implements Language Ch. 0 (syntax), Ch. 1's type rules, Ch. 2's
//! composites and layout, Ch. 3's ownership and borrowing, and Ch. 4's
//! traits, generics, trait objects and closures. What it does not cover is
//! what the specification defers: strings and everything else that waits for
//! the library chapter.

pub mod ast;
pub mod lex;
pub mod lower;
pub mod parse;

pub use lex::SyntaxError;

/// The types Ch. 4 §5.8 makes the language's own rather than the library's,
/// because Ch. 2 §6 states niche guarantees about `Option` and Ch. 4 §5.7
/// needs it to describe `Iterator`.
///
/// They are ordinary enums, laid out by Ch. 2's rules with no special case —
/// which is the claim §5.8 makes, and prepending their source is the most
/// direct way to keep it honest.
pub const PRELUDE: &str = "\
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
";

/// Compile a source file to a TIR module.
pub fn compile(src: &str) -> Result<crate::tir::Module, Vec<SyntaxError>> {
    let prelude_lines = PRELUDE.lines().count() as u32;
    let file = parse::parse(&format!("{PRELUDE}{src}")).map_err(|mut e| {
        e.line = e.line.saturating_sub(prelude_lines);
        vec![e]
    })?;
    lower::lower(&file).map_err(|mut es| {
        for e in &mut es {
            e.line = e.line.saturating_sub(prelude_lines);
        }
        es
    })
}
