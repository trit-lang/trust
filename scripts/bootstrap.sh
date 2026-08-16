#!/bin/bash
# Hold the lexer and parser written in Trust to the ones written in Rust.
#
# `bootstrap/` is this compiler written in the language it compiles — the
# lexer and the expression parser so far. The point of writing it is not elegance: it is to be
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

# The parsers are compared on the *tree*, printed with every operator a
# prefix and every child parenthesized — a form neither of them writes for
# any other purpose, so agreeing on it is agreeing on the shape and not on
# the printing.
e=0
for f in bootstrap/exprs/*.txt; do
    rust=$("$trust" ast "$f")
    mine=$("$trust" run bootstrap/tree.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two parsers disagree on $(cat "$f")" >&2
        echo "  rust : $rust" >&2
        echo "  trust: $mine" >&2
        exit 1
    fi
    e=$((e + 1))
done

i=0
for f in bootstrap/fns/*.tr; do
    rust=$("$trust" item "$f")
    mine=$("$trust" run bootstrap/items.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two parsers disagree on $f" >&2
        echo "  rust : $rust" >&2
        echo "  trust: $mine" >&2
        exit 1
    fi
    i=$((i + 1))
done

w=0
for f in bootstrap/files/*.tr; do
    rust=$("$trust" file "$f")
    mine=$("$trust" run bootstrap/file.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two parsers disagree on the items of $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    w=$((w + $(printf '%s\n' "$rust" | wc -l)))
done

# A whole *program*, not a file. The machine has a character port and no
# filesystem (ISA §2.2), so the driver walks the module tree and hands the
# program over on stdin — which is where finding files belongs anyway, since
# which files are compiled is a fact about a build (Ch. 6 §1.2).
m=0
for root in examples/trust/modules/main.tr bootstrap/whole.tr bootstrap/items.tr; do
    rust=$("$trust" modules "$root")
    mine=$("$trust" bundle "$root" | "$trust" run bootstrap/whole.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about the program rooted at $root" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    m=$((m + $(printf '%s\n' "$rust" | wc -l)))
done

printf 'bootstrap: %d tokens, %d refusals, %d expression trees, %d function trees, %d items, %d modules of whole programs — all agreed\n' \
    "$n" "$r" "$e" "$i" "$w" "$m"
