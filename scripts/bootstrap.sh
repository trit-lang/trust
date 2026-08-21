#!/usr/bin/env bash
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

# Some loops here run one comparison per file and their iterations do not
# depend on one another; the machine's cores stayed idle for most of the
# 25-minute run in the sequential form. `throttle` caps how many are in
# flight, so the two-way `trust` invocations of each iteration have room
# without swamping memory. Overridable so a CI machine can pick its own.
: "${BOOTSTRAP_JOBS:=$(nproc 2>/dev/null || echo 4)}"
throttle() {
    while [ "$(jobs -pr 2>/dev/null | wc -l)" -ge "$BOOTSTRAP_JOBS" ]; do
        wait -n 2>/dev/null || wait
    done
}
# After a parallel batch: sort by index so a diagnostic reads in the same
# order the loop was written in, and exit if any iteration wrote one.
report_errors() {
    local dir=$1 had=0
    if compgen -G "$dir/*.err" > /dev/null; then
        for f in $(ls "$dir"/*.err | sort -V); do
            cat "$f" >&2
            had=1
        done
    fi
    [ "$had" = 1 ] && exit 1 || true
}
# Sum the integer written to each `*.n` file in a batch directory.
sum_ns() {
    local dir=$1 tot=0
    if compgen -G "$dir/*.n" > /dev/null; then
        for f in "$dir"/*.n; do
            tot=$((tot + $(cat "$f")))
        done
    fi
    echo "$tot"
}

n=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/corpus/*.tr; do
    (
        rust=$("$trust" lex "$f")
        mine=$("$trust" run bootstrap/main.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two lexers disagree on $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -20
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
n=$((n + $(sum_ns "$tmp")))
rm -rf "$tmp"

r=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/refuse/*.tr; do
    (
        rust=$("$trust" lex "$f" | tail -1)
        mine=$("$trust" run bootstrap/main.tr < "$f" | tail -1)
        case "$rust" in
            error\ *) ;;
            *)
                echo "bootstrap: $f is in refuse/ and the Rust lexer accepted it" \
                    > "$tmp/$idx.err"
                exit 1
                ;;
        esac
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two lexers refuse $f differently"
                echo "  rust: $rust"
                echo "  trust: $mine"
            } > "$tmp/$idx.err"
            exit 1
        fi
        echo 1 > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
r=$((r + $(sum_ns "$tmp")))
rm -rf "$tmp"

# The parsers are compared on the *tree*, printed with every operator a
# prefix and every child parenthesized — a form neither of them writes for
# any other purpose, so agreeing on it is agreeing on the shape and not on
# the printing.
e=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/exprs/*.txt; do
    (
        rust=$("$trust" ast "$f")
        mine=$("$trust" run bootstrap/tree.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two parsers disagree on $(cat "$f")"
                echo "  rust : $rust"
                echo "  trust: $mine"
            } > "$tmp/$idx.err"
            exit 1
        fi
        echo 1 > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
e=$((e + $(sum_ns "$tmp")))
rm -rf "$tmp"

i=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/fns/*.tr; do
    (
        rust=$("$trust" item "$f")
        mine=$("$trust" run bootstrap/items.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two parsers disagree on $f"
                echo "  rust : $rust"
                echo "  trust: $mine"
            } > "$tmp/$idx.err"
            exit 1
        fi
        echo 1 > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
i=$((i + $(sum_ns "$tmp")))
rm -rf "$tmp"

w=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/files/*.tr; do
    (
        rust=$("$trust" file "$f")
        mine=$("$trust" run bootstrap/file.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two parsers disagree on the items of $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
w=$((w + $(sum_ns "$tmp")))
rm -rf "$tmp"

# The parser over its own source. Every file this directory holds goes
# through both parsers, and the two trees must be the same characters — which
# is the whole claim of a bootstrap, made against the largest Trust program
# that exists. It is the slow part of this script and it is the point of it.
b=0
tmp=$(mktemp -d)
i=0
for f in bootstrap/*.tr; do
    (
        rust=$("$trust" file "$f")
        mine=$("$trust" run bootstrap/file.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two parsers disagree on the parser's own $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$i.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$i.n"
    ) &
    i=$((i + 1))
    throttle
done
wait
report_errors "$tmp"
b=$((b + $(sum_ns "$tmp")))
rm -rf "$tmp"

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
tmp=$(mktemp -d)
# Five asks, each reading the prelude twice, and all five share the same
# input — so they run at once and the aggregate is the same.
(
    rust=$("$trust" file "$prelude")
    mine=$("$trust" run bootstrap/file.tr < "$prelude")
    if [ "$rust" != "$mine" ]; then
        {
            echo "bootstrap: the two disagree about the items in the library"
            diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
        } > "$tmp/0.err"
        exit 1
    fi
    printf '%s\n' "$rust" | wc -l > "$tmp/0.n"
) &
throttle
(
    rust=$("$trust" symbols "$prelude")
    mine=$("$trust" bundle "$prelude" | "$trust" run bootstrap/symbols.tr)
    if [ "$rust" != "$mine" ]; then
        {
            echo "bootstrap: the two disagree about the names defined in the library"
            diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
        } > "$tmp/1.err"
        exit 1
    fi
    printf '%s\n' "$rust" | wc -l > "$tmp/1.n"
) &
throttle
(
    rust=$("$trust" uses "$prelude")
    mine=$("$trust" bundle "$prelude" | "$trust" run bootstrap/uses.tr)
    if [ "$rust" != "$mine" ]; then
        {
            echo "bootstrap: the two disagree about what every use reaches in the library"
            diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
        } > "$tmp/2.err"
        exit 1
    fi
    printf '%s\n' "$rust" | wc -l > "$tmp/2.n"
) &
throttle
(
    rust=$("$trust" layout "$prelude")
    mine=$("$trust" bundle "$prelude" | "$trust" run bootstrap/sizes.tr)
    if [ "$rust" != "$mine" ]; then
        {
            echo "bootstrap: the two disagree about the layouts in the library"
            diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
        } > "$tmp/3.err"
        exit 1
    fi
    printf '%s\n' "$rust" | wc -l > "$tmp/3.n"
) &
throttle
(
    rust=$("$trust" agree "$prelude")
    mine=$({ printf '// agree\n'; cat "$prelude"; } | "$trust" run bootstrap/types.tr)
    if [ "$rust" != "$mine" ]; then
        {
            echo "bootstrap: the two disagree about which functions type-check in the library"
            diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
        } > "$tmp/4.err"
        exit 1
    fi
    printf '%s\n' "$rust" | wc -l > "$tmp/4.n"
) &
throttle
wait
report_errors "$tmp"
v=$((v + $(sum_ns "$tmp")))
rm -rf "$tmp"

# A whole *program*, not a file. The machine has a character port and no
# filesystem (ISA §2.2), so the driver walks the module tree and hands the
# program over on stdin — which is where finding files belongs anyway, since
# which files are compiled is a fact about a build (Ch. 6 §1.2).
m=0
tmp=$(mktemp -d)
idx=0
for root in examples/trust/modules/main.tr bootstrap/whole.tr bootstrap/items.tr; do
    (
        rust=$("$trust" modules "$root")
        mine=$("$trust" bundle "$root" | "$trust" run bootstrap/whole.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about the program rooted at $root"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
m=$((m + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Pass one of Ch. 6 §4: what the program defines, and what each name is
# called. Two implementations of the chapter, one answer.
y=0
tmp=$(mktemp -d)
idx=0
for root in examples/trust/modules/main.tr bootstrap/symbols.tr bootstrap/file.tr; do
    (
        rust=$("$trust" symbols "$root")
        mine=$("$trust" bundle "$root" | "$trust" run bootstrap/symbols.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about what $root defines"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
y=$((y + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Pass two: what every `use` reaches, and by which rule it was refused when
# it reached nothing. `bootstrap/programs/refused` is every way it can fail.
u=0
tmp=$(mktemp -d)
idx=0
for root in examples/trust/modules/main.tr bootstrap/programs/refused/main.tr bootstrap/uses.tr; do
    (
        rust=$("$trust" uses "$root")
        mine=$("$trust" bundle "$root" | "$trust" run bootstrap/uses.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about what the \`use\`s of $root reach"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
u=$((u + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Pass three: the whole program as one list of items, every name resolved.
# It is what the rest of the compiler has always been handed, and the last
# question about Ch. 6 that can be asked without asking about types.
z=0
tmp=$(mktemp -d)
idx=0
for root in examples/trust/modules/main.tr bootstrap/symbols.tr bootstrap/flat.tr; do
    (
        rust=$("$trust" flat "$root")
        mine=$("$trust" bundle "$root" | "$trust" run bootstrap/flat.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about the resolved items of $root"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -4 | cut -c1-200
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
z=$((z + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Ch. 2: what every type the program defines is laid out as. Sizes and
# offsets are facts about types and need no inference, which is what makes
# this the first part of the middle two implementations can both answer.
l=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/layouts/*.tr; do
    (
        rust=$("$trust" layout "$f")
        mine=$("$trust" bundle "$f" | "$trust" run bootstrap/sizes.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about the layouts in $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
l=$((l + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Ch. 4: what type every binding turned out to have. A `let` is where
# inference is visible — it is the thing `let n = 1;` does not say.
t=0
# The corpus, and then the modules of `bootstrap/` that are whole files on
# their own: a type is a fact about a file, so the ones that declare no
# module can be asked directly, and they are the largest Trust programs
# there are to ask about.
tmp=$(mktemp -d)
idx=0
for f in bootstrap/typed/*.tr bootstrap/ast.tr bootstrap/input.tr \
         bootstrap/bundle.tr bootstrap/lex.tr; do
    (
        rust=$("$trust" types "$f")
        mine=$("$trust" run bootstrap/types.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about the types in $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
t=$((t + $(sum_ns "$tmp")))
rm -rf "$tmp"

# Ch. 4, the other question: which functions are *refused*, and by which
# rule. Reported per function rather than by position — two implementations
# should agree about the language, and the second one's tree carries no
# spans to point with. `bootstrap/mismatch` is one wrong thing per function
# and `bootstrap/typed` is the same programs the two agreed about above, so
# a checker that refused what compiles would be caught here.
c=0
tmp=$(mktemp -d)
idx=0
for f in bootstrap/mismatch/*.tr bootstrap/typed/*.tr bootstrap/ast.tr \
         bootstrap/lex.tr bootstrap/bundle.tr bootstrap/input.tr; do
    (
        rust=$("$trust" agree "$f")
        mine=$({ printf '// agree\n'; cat "$f"; } | "$trust" run bootstrap/types.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two disagree about which functions type-check in $f"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -6
            } > "$tmp/$idx.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$idx.n"
    ) &
    idx=$((idx + 1))
    throttle
done
wait
report_errors "$tmp"
c=$((c + $(sum_ns "$tmp")))
rm -rf "$tmp"

# TIR: the module a program lowers to, character for character — names
# included, since equality is on the text (docs/ddc.md §4). This is the pass
# that makes `stage1` exist, and it is the first slice of it: scalars, the
# arithmetic of Ch. 1, calls, `return` and a tail.
g=0
tmp=$(mktemp -d)
i=0
for f in bootstrap/lowered/*.tr; do
    (
        rust=$("$trustc" tir "$f")
        mine=$("$trust" run bootstrap/build.tr < "$f")
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two lower $f differently"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$i.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$i.n"
    ) &
    i=$((i + 1))
    throttle
done
wait
report_errors "$tmp"
g=$((g + $(sum_ns "$tmp")))
rm -rf "$tmp"

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
tmp=$(mktemp -d)
i=0
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
            bootstrap/programs/boxed/main.tr \
            bootstrap/programs/reassign/main.tr \
            bootstrap/main.tr; do
    # The one that uses the library is handed the library, which is what
    # `--prelude` is for (Ch. 6 §3.3) — and asking for it where there is
    # none costs nothing, since a prelude nothing names is pruned away.
    (
        rust=$("$trust" tir "$root")
        mine=$("$trust" bundle "$root" --prelude | "$trust" run bootstrap/program.tr)
        if [ "$rust" != "$mine" ]; then
            {
                echo "bootstrap: the two lower the program at $root differently"
                diff <(printf '%s\n' "$rust") <(printf '%s\n' "$mine") | head -10
            } > "$tmp/$i.err"
            exit 1
        fi
        printf '%s\n' "$rust" | wc -l > "$tmp/$i.n"
    ) &
    i=$((i + 1))
    throttle
done
wait
report_errors "$tmp"
q=$((q + $(sum_ns "$tmp")))
rm -rf "$tmp"

printf 'bootstrap: %d tokens, %d refusals, %d expression trees, %d function trees, %d items, %d items of the parser itself, %d lines about the library, %d modules of whole programs, %d names defined, %d names resolved, %d items rewritten, %d types laid out, %d bindings typed, %d functions checked, %d lines of TIR, %d of whole programs — all agreed\n' \
    "$n" "$r" "$e" "$i" "$w" "$b" "$v" "$m" "$y" "$u" "$z" "$l" "$t" "$c" "$g" "$q"
