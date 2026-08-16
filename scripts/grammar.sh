#!/bin/sh
# Check the tree-sitter grammar against the parser it can drift from.
#
# `editors/tree-sitter-trust` is a second grammar for this language. The first
# is `compiler/src/lang/parse.rs`, and it is the one that decides what a
# program means; the second exists because Zed and its neighbours have no
# other way to colour a file. Nothing keeps two grammars together on its own,
# so this is what does: every file in `examples/trust/` must parse with no
# ERROR and no MISSING node, and the corpus in `test/corpus/` must pass.
#
# It catches what the grammar *rejects*. A grammar that accepts too much is
# not caught here and is not meant to be — the compiler refuses those, and a
# highlighter that gives up on a file is worse than one that is wrong about a
# line of it. `grammar.js` lists the places it is deliberately looser.
#
# The generated parser (`src/parser.c`) is committed, because that is what
# Zed compiles; this regenerates it, so a diff after running means `src` was
# stale.
set -eu

cd "$(dirname "$0")/.."
grammar=editors/tree-sitter-trust

if ! command -v tree-sitter >/dev/null 2>&1; then
    echo "grammar: tree-sitter is not on PATH; skipping (install tree-sitter-cli)" >&2
    exit 0
fi

(cd "$grammar" && tree-sitter generate && tree-sitter test)

status=0
for f in examples/trust/*.tr examples/trust/*/*.tr examples/trust/*/*/*.tr; do
    [ -e "$f" ] || continue
    out=$(cd "$grammar" && tree-sitter parse "../../$f" 2>&1) || true
    # `tree-sitter parse` reports a bad node inline and in its summary line.
    if printf '%s' "$out" | grep -qE '\(ERROR|\(MISSING|MISSING "'; then
        echo "grammar: $f does not parse:" >&2
        printf '%s\n' "$out" | grep -E 'ERROR|MISSING' | head -5 >&2
        status=1
    fi
done

# The highlight queries have one home. Zed reads them from the extension it
# loaded, so they have to be there too, and a copy that drifts is worse than
# no copy: this is the check that keeps them one file.
if ! cmp -s "$grammar/queries/highlights.scm" editors/zed/languages/trust/highlights.scm; then
    echo "grammar: editors/zed/languages/trust/highlights.scm is stale" >&2
    echo "         cp $grammar/queries/highlights.scm editors/zed/languages/trust/" >&2
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "grammar: $(ls examples/trust/*.tr examples/trust/*/*.tr examples/trust/*/*/*.tr 2>/dev/null | wc -l | tr -d ' ') files parse with no error node"
fi
exit "$status"
