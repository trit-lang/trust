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
# **What the corpus covers is the contract.** The Trust lexer does not read a
# character literal, a trit literal, a lifetime, or a non-decimal radix
# (Ch. 0 §1.4), so the corpus does not hold one — and that list is the list
# of what is left to write.
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
printf 'bootstrap: two lexers agree on %d tokens across %d files\n' \
    "$n" "$(ls bootstrap/corpus/*.tr | wc -l | tr -d ' ')"
