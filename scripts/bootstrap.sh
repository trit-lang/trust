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
cargo build --quiet --release -p trust -p trustc
trust=target/release/trust
trustc=target/release/trustc

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

# The parser over its own source. Every file this directory holds goes
# through both parsers, and the two trees must be the same characters — which
# is the whole claim of a bootstrap, made against the largest Trust program
# that exists. It is the slow part of this script and it is the point of it.
b=0
for f in bootstrap/*.tr; do
    rust=$("$trust" file "$f")
    mine=$("$trust" run bootstrap/file.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two parsers disagree on the parser's own $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    b=$((b + $(printf '%s\n' "$rust" | wc -l)))
done

# The **library**, which is the largest Trust program neither implementation
# wrote and the one they both have to agree about before either can compile a
# program that uses it. It is not a file in this repository — it is carried
# inside the compiler and handed over by `--prelude` (Ch. 6 §3.3) — so it is
# taken out of a bundle and given to both.
prelude=$(mktemp)
trap 'rm -f "$prelude"' EXIT
bundle=$(mktemp)
trap 'rm -f "$prelude" "$bundle"' EXIT
"$trust" bundle bootstrap/programs/whole/main.tr --prelude > "$bundle"
# Up to the next section header. Not the character count the header gives:
# that is in *characters* and the library is not all ASCII (Ch. 5 §1.4).
tail -n +2 "$bundle" | awk '/^mod [^ ]+ [0-9]+$/ { exit } { print }' > "$prelude"
v=0
ask() {
    rust=$1
    mine=$2
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about $3 in the library" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    v=$((v + $(printf '%s\n' "$rust" | wc -l)))
}
ask "$("$trust" file "$prelude")" \
    "$("$trust" run bootstrap/file.tr < "$prelude")" "the items"
ask "$("$trust" symbols "$prelude")" \
    "$("$trust" bundle "$prelude" | "$trust" run bootstrap/symbols.tr)" \
    "the names defined"
ask "$("$trust" uses "$prelude")" \
    "$("$trust" bundle "$prelude" | "$trust" run bootstrap/uses.tr)" \
    "what every use reaches"
ask "$("$trust" layout "$prelude")" \
    "$("$trust" bundle "$prelude" | "$trust" run bootstrap/sizes.tr)" \
    "the layouts"
ask "$("$trust" agree "$prelude")" \
    "$({ printf '// agree\n'; cat "$prelude"; } | "$trust" run bootstrap/types.tr)" \
    "which functions type-check"

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

# Pass one of Ch. 6 §4: what the program defines, and what each name is
# called. Two implementations of the chapter, one answer.
y=0
for root in examples/trust/modules/main.tr bootstrap/symbols.tr bootstrap/file.tr; do
    rust=$("$trust" symbols "$root")
    mine=$("$trust" bundle "$root" | "$trust" run bootstrap/symbols.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about what $root defines" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    y=$((y + $(printf '%s\n' "$rust" | wc -l)))
done

# Pass two: what every `use` reaches, and by which rule it was refused when
# it reached nothing. `bootstrap/programs/refused` is every way it can fail.
u=0
for root in examples/trust/modules/main.tr bootstrap/programs/refused/main.tr bootstrap/uses.tr; do
    rust=$("$trust" uses "$root")
    mine=$("$trust" bundle "$root" | "$trust" run bootstrap/uses.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about what the \`use\`s of $root reach" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    u=$((u + $(printf '%s\n' "$rust" | wc -l)))
done

# Pass three: the whole program as one list of items, every name resolved.
# It is what the rest of the compiler has always been handed, and the last
# question about Ch. 6 that can be asked without asking about types.
z=0
for root in examples/trust/modules/main.tr bootstrap/symbols.tr bootstrap/flat.tr; do
    rust=$("$trust" flat "$root")
    mine=$("$trust" bundle "$root" | "$trust" run bootstrap/flat.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about the resolved items of $root" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -4 | cut -c1-200 >&2
        exit 1
    fi
    z=$((z + $(printf '%s\n' "$rust" | wc -l)))
done

# Ch. 2: what every type the program defines is laid out as. Sizes and
# offsets are facts about types and need no inference, which is what makes
# this the first part of the middle two implementations can both answer.
l=0
for f in bootstrap/layouts/*.tr; do
    rust=$("$trust" layout "$f")
    mine=$("$trust" bundle "$f" | "$trust" run bootstrap/sizes.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about the layouts in $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6 >&2
        exit 1
    fi
    l=$((l + $(printf '%s\n' "$rust" | wc -l)))
done

# Ch. 4: what type every binding turned out to have. A `let` is where
# inference is visible — it is the thing `let n = 1;` does not say.
t=0
# The corpus, and then the modules of `bootstrap/` that are whole files on
# their own: a type is a fact about a file, so the ones that declare no
# module can be asked directly, and they are the largest Trust programs
# there are to ask about.
for f in bootstrap/typed/*.tr bootstrap/ast.tr bootstrap/input.tr \
         bootstrap/bundle.tr bootstrap/lex.tr; do
    rust=$("$trust" types "$f")
    mine=$("$trust" run bootstrap/types.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about the types in $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6 >&2
        exit 1
    fi
    t=$((t + $(printf '%s\n' "$rust" | wc -l)))
done

# Ch. 4, the other question: which functions are *refused*, and by which
# rule. Reported per function rather than by position — two implementations
# should agree about the language, and the second one's tree carries no
# spans to point with. `bootstrap/mismatch` is one wrong thing per function
# and `bootstrap/typed` is the same programs the two agreed about above, so
# a checker that refused what compiles would be caught here.
c=0
for f in bootstrap/mismatch/*.tr bootstrap/typed/*.tr bootstrap/ast.tr \
         bootstrap/lex.tr bootstrap/bundle.tr bootstrap/input.tr; do
    rust=$("$trust" agree "$f")
    mine=$({ printf '// agree\n'; cat "$f"; } | "$trust" run bootstrap/types.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two disagree about which functions type-check in $f" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6 >&2
        exit 1
    fi
    c=$((c + $(printf '%s\n' "$rust" | wc -l)))
done

# TIR: the module a program lowers to, character for character — names
# included, since equality is on the text (docs/ddc.md §4). This is the pass
# that makes `stage1` exist, and it is the first slice of it: scalars, the
# arithmetic of Ch. 1, calls, `return` and a tail.
g=0
for f in bootstrap/lowered/*.tr; do
    rust=$("$trustc" tir "$f")
    mine=$("$trust" run bootstrap/build.tr < "$f")
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two lower $f differently" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    g=$((g + $(printf '%s\n' "$rust" | wc -l)))
done

# A whole *program* as TIR: Ch. 6's three passes and then the lowering, cut
# down to what `main` reaches — a function nothing calls is a function the
# program does not contain, and the two have to agree about which.
#
# The last of these is **`bootstrap/main.tr` itself**: this compiler's lexer,
# written in the language it compiles, lowered by both implementations to the
# same 7239 lines of TIR. That is what a bootstrap is for, and it is the
# first program in this list that neither implementation was written to
# handle — it was written to be *used*.
q=0
for root in bootstrap/programs/whole/main.tr bootstrap/programs/deeper/main.tr \
            bootstrap/programs/methods/main.tr \
            bootstrap/programs/generic/main.tr \
            bootstrap/programs/library/main.tr \
            bootstrap/programs/text/main.tr \
            bootstrap/programs/chars/main.tr \
            bootstrap/programs/heap/main.tr \
            bootstrap/programs/vector/main.tr \
            bootstrap/programs/scopes/main.tr \
            bootstrap/programs/loops/main.tr \
            bootstrap/programs/failing/main.tr \
            bootstrap/programs/oneof/main.tr \
            bootstrap/programs/named/main.tr \
            bootstrap/main.tr; do
    # The one that uses the library is handed the library, which is what
    # `--prelude` is for (Ch. 6 §3.3) — and asking for it where there is
    # none costs nothing, since a prelude nothing names is pruned away.
    rust=$("$trust" tir "$root")
    mine=$("$trust" bundle "$root" --prelude | "$trust" run bootstrap/program.tr)
    if [ "$rust" != "$mine" ]; then
        echo "bootstrap: the two lower the program at $root differently" >&2
        diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10 >&2
        exit 1
    fi
    q=$((q + $(printf '%s\n' "$rust" | wc -l)))
done

printf 'bootstrap: %d tokens, %d refusals, %d expression trees, %d function trees, %d items, %d items of the parser itself, %d lines about the library, %d modules of whole programs, %d names defined, %d names resolved, %d items rewritten, %d types laid out, %d bindings typed, %d functions checked, %d lines of TIR, %d of whole programs — all agreed\n' \
    "$n" "$r" "$e" "$i" "$w" "$b" "$v" "$m" "$y" "$u" "$z" "$l" "$t" "$c" "$g" "$q"
