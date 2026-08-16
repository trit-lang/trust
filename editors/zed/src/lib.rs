//! The Zed half of the Trust editor support.
//!
//! Zed can highlight from a tree-sitter grammar with no code at all — that is
//! `extension.toml` and `languages/trust/` — but registering a language
//! server needs a compiled extension, which is all this is.
//!
//! It does not download anything. `trust-lsp` is built from this repository
//! by `cargo build --release`, and this looks for it on `PATH`: an extension
//! that fetched a binary would have to know which build matched the compiler
//! the user is actually using, and it does not.

use zed_extension_api::{self as zed, Result};

struct TrustExtension;

impl zed::Extension for TrustExtension {
    fn new() -> Self {
        TrustExtension
    }

    fn language_server_command(
        &mut self,
        _id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("trust-lsp").ok_or_else(|| {
            "`trust-lsp` is not on PATH. Build it from the Trust repository with \
             `cargo build --release` and put `target/release/trust-lsp` somewhere \
             on PATH."
                .to_string()
        })?;
        Ok(zed::Command {
            command,
            args: Vec::new(),
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(TrustExtension);
