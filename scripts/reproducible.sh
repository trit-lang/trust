#!/bin/bash
# Compiling the same source twice gives the same bytes.
#
# This is the property Diverse Double-Compiling rests on (docs/ddc.md): the
# claim it corroborates is "this binary is what this source says", and a
# compiler whose output varies between runs can corroborate nothing, because
# there is nothing to compare against.
#
# The runs are separate *processes*, and that is the point. Rust seeds every
# `HashMap` differently per process, so iteration order that reached the
# output would show up here as a difference between two runs of the same
# command — which is the cheapest test there is for the commonest way a
# compiler stops being a function of its input.
set -eu

cargo build --release -q
trust=target/release/trust
trustc=target/release/trustc

n=0
for f in examples/trust/*.tr; do
    for what in tir asm; do
        case "$what" in
            tir) a=$("$trustc" tir "$f") ; b=$("$trustc" tir "$f") ;;
            asm) a=$("$trust" asm "$f")  ; b=$("$trust" asm "$f")  ;;
        esac
        if [ "$a" != "$b" ]; then
            echo "reproducible: $what of $f differs between two runs" >&2
            diff <(printf '%s\n' "$a") <(printf '%s\n' "$b") | head -10 >&2
            exit 1
        fi
        n=$((n + 1))
    done
done

# The bootstrap's own programs, which are the largest Trust there is and the
# ones a self-hosted compiler will be built from.
for f in bootstrap/*.tr; do
    a=$("$trust" asm "$f" 2>/dev/null) || continue
    b=$("$trust" asm "$f" 2>/dev/null)
    if [ "$a" != "$b" ]; then
        echo "reproducible: the assembly of $f differs between two runs" >&2
        exit 1
    fi
    n=$((n + 1))
done

printf 'reproducible: %d compilations, each byte-for-byte the same twice\n' "$n"
