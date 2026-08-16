#!/bin/bash
# Hold the lexer written in Trust to the one written in Rust.
#
# `bootstrap/lex.tr` is the first piece of this compiler written in the
# language it compiles. The point of writing it is not elegance: it is to be
# *used*, so that what the language cannot yet express is found by trying
# rather than by thinking. Two bugs and one wart came out of the first
# hundred lines (G9.35, G9.36).
#
# A second implementation of anything is worth having only if something holds
# it to the first, so this runs both over every file in `bootstrap/corpus/`
# and compares them character for character.
#
# **What the corpus covers is the contract.** It now holds every literal form
# §1.4 defines — all three radices, trit literals, character literals with
# their escapes, lifetimes — and the places two lexers stop agreeing:
# maximal munch, a string holding what looks like a comment, a comment
# holding what looks like a comment, and text above the basic plane.
#
# `bootstrap/refuse/` is the other half: each file is one the lexers must
# **refuse**, and they are compared on *where* rather than on why. Two
# implementations agreeing on the wording of a diagnostic would be one
# copying the other; agreeing on which character is wrong is the claim worth
# checking.
set -eu

cd "$(dirname "$0")/.."
cargo build --quiet --release -p trust
trust=target/release/trust

n=0
for f in bootstrap/corpus/*.tr; do
    rust=$("$trust" lex "$f")
    mine=$("$trust" run bootstrap/main.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two lexers disagree on $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -20 >&2
        exit 1
    fi
    n=$((n + $(printf '%s\n' "$rust" | wc -l)))
done

r=0
for f in bootstrap/refuse/*.tr; do
    rust=$("$trust" lex "$f" | tail -1)
    mine=$("$trust" run bootstrap/main.tr < "$f" | tail -1)
    case "$rust" in
        error\ *) ;;
        *) echo "bootstrap: $f is in refuse/ and the Rust lexer accepted it" >&2; exit 1 ;;
    esac
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two lexers refuse $f differently" >&2
        echo "  rust: $rust" >&2
        echo "  trust: $mine" >&2
        exit 1
    fi
    r=$((r + 1))
done

printf 'bootstrap: two lexers agree on %d tokens across %d files, and refuse %d more at the same character\n' \
    "$n" "$(ls bootstrap/corpus/*.tr | wc -l | tr -d ' ')" "$r"
