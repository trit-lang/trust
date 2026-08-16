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

The grammar is fetched by Zed from the `repository` in
`editors/zed/extension.toml`, not from your checkout. That URL is where this
repository will live; until it is pushed there, edit the file to point at your
own copy:

```toml
[grammars.trust]
repository = "file:///absolute/path/to/trust-lang"
rev = "<any commit you have>"
path = "editors/tree-sitter-trust"
```

`rev` has to be bumped whenever the grammar changes. That is the whole cost of
keeping the grammar in this repository instead of a separate one, and it buys
`scripts/grammar.sh` being able to check the grammar against the examples in
the same commit that changes either.

## What is checked here, and what is not

Checked: the grammar parses all ten example programs with no error node, its
nine corpus cases pass, the highlight queries compile against it, and the Zed
extension compiles to WebAssembly.

Not checked: Zed loading any of it. That needs Zed, and the discipline this
repository holds to is that an untested claim is not made — so this is the
claim: **the pieces are each verified, and their assembly inside the editor is
not.** If Zed rejects the manifest, the manifest is where to look; the grammar
and the server are known to work on their own.

## Other editors

Anything that speaks the Language Server Protocol can use `trust-lsp` directly
— it reads stdio and needs no arguments. Anything that loads a tree-sitter
grammar from a directory can use `tree-sitter-trust` directly, including
Neovim (`nvim-treesitter`) and Helix. Neither needs the Zed extension, which is
only the glue Zed happens to require.
