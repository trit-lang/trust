# Editor support

Two things, which are separate because editors treat them separately.

| | what it is | verified by |
|---|---|---|
| `tree-sitter-trust/` | the grammar an editor colours from | `scripts/grammar.sh` |
| `zed/` | the Zed extension: language config, and how to start `trust-lsp` | `cargo build --target wasm32-wasip2` |

Diagnostics come from `trust-lsp`, which is in this repository and is built
from the same compiler that builds programs. Highlighting comes from the
grammar, which is a **second** parser and can drift from the real one — see
`tree-sitter-trust/grammar.js` for what it is deliberately looser about, and
`scripts/grammar.sh` for what stops it drifting further.

## Installing in Zed

```sh
cargo build --release                      # builds trust-lsp
cp target/release/trust-lsp ~/.local/bin/  # anywhere on PATH
```

Then Zed → command palette → **zed: install dev extension** → pick
`editors/zed`.

Nothing is downloaded for you: `trust-lsp` is found on `PATH`, because an
extension that fetched a binary would have to know which build matched the
compiler you are using, and it does not. Rebuild and copy again after changing
it — Zed runs the copy on `PATH`, not the one in `target/`.

The **grammar** is not read from your checkout either. Zed fetches it from the
`repository` in `editors/zed/extension.toml` — `git remote add`, `git fetch`,
`git checkout <rev>` — and then compiles `<path>/src/parser.c` and
`scanner.c`, which is why both are committed. So `rev` has to be bumped
whenever the grammar changes, and a grammar change is not visible to Zed until
it is *pushed*.

That is the whole cost of keeping the grammar in this repository instead of a
separate one, and it buys `scripts/grammar.sh` being able to check the grammar
against the examples in the same commit that changes either.

Zed compiles the grammar with its own `clang`, and downloads a wasi-sdk the
first time it needs one. Extensions installed from Zed's registry arrive
pre-compiled and never do this; a dev extension always does.

If you are working on the grammar and do not want to push to try it, point
`repository` at your checkout — git takes a plain absolute path as a remote
URL, and Zed passes it straight to git:

```toml
[grammars.trust]
repository = "/absolute/path/to/trust-lang"
rev = "<any commit you have>"
path = "editors/tree-sitter-trust"
```

Do not commit that.

## What is checked here, and what is not

Checked:

- the grammar parses all ten example programs with no error node and its nine
  corpus cases pass (`scripts/grammar.sh`);
- the highlight queries compile against that grammar;
- the extension compiles to `wasm32-wasip2`;
- `trust-lsp` answers `initialize`, `documentSymbol`, `definition` and `hover`
  over real JSON-RPC framing;
- every field of `extension.toml` against Zed 1.14.2's own
  `extension_manifest.rs` and `extension_builder.rs`. `[lib]` and `languages`
  are deliberately absent: `populate_defaults` fills them from `Cargo.toml`
  and `languages/*/config.toml`, and the versions you see written out in an
  installed extension's manifest are Zed's, not its author's.
- the grammar fetch, by doing what `checkout_repo` does — `git remote add`,
  `git fetch <rev>`, `git checkout` from the public URL with no credentials —
  and confirming `src/parser.c` and `src/scanner.c` land where `path` says.

Not checked: **Zed loading it**, which needs the GUI (`zed: install dev
extension`; there is no CLI flag for it), and the wasi-sdk download and wasm
compile of the grammar that the first dev-extension build does. The discipline
this repository holds to is that an untested claim is not made, so: everything
up to the editor's front door is verified, and going through it is not.

## Other editors

Anything that speaks the Language Server Protocol can use `trust-lsp` directly
— it reads stdio and needs no arguments. Anything that loads a tree-sitter
grammar from a directory can use `tree-sitter-trust` directly, including
Neovim (`nvim-treesitter`) and Helix. Neither needs the Zed extension, which is
only the glue Zed happens to require.
