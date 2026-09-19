#!/usr/bin/env bash
# Run the double compile (docs/ddc.md §4 step 4).
#
# stage1 is the compiler the parent built: `trustc`'s TIR for the Trust
# compiler's source, taken the whole way to a machine image that stands on
# its own. stage2 is what that image says when it is handed the same
# source. sA is the front end whole — `bootstrap/program.tr` and the eight
# modules it reaches: a compiler written in the language it compiles,
# which is the only kind of thing DDC has ever been about.
#
#     stage1.tir = trust tir <root>                        cP(sA), as TIR
#     stage2.tir = tritium run stage1.timg < sA.bundle     stage1(sA), as TIR
#
# and stage2.tir must equal stage1.tir byte for byte. The equality is on
# the text, because the text is the canonical form (TIR §8): two images
# could differ and mean the same thing, two texts cannot — and because the
# machine has no filesystem, the source stage2 compiles is the bundle it is
# handed, so what is compared is named exactly.
#
# The hash of each artifact prints whether or not the comparison holds,
# because a number that is only printed when it is right is a number nobody
# checks.
set -eu

cd "$(dirname "$0")/.."

# What compiles itself. The front end is the whole compiler stage1 is made
# of: it reads a bundle, runs Ch. 6 §4's three passes, lowers, and prints
# the module it cut down to `main` — which is every pass the fixpoint in
# §3.3 is defined on. There is no smaller rehearsal for the ceremony:
# `bootstrap/main.tr` is a lexer, and only a compiler can compile itself.
: "${DDC_ROOT:=bootstrap/program.tr}"
: "${DDC_OUT:=target/ddc}"

trust=target/release/trust
tritium=target/release/tritium
if command -v cargo >/dev/null 2>&1; then
    cargo build --quiet --release -p trust -p tritium
elif [ ! -x "$trust" ] || [ ! -x "$tritium" ]; then
    echo "ddc: no cargo and no pre-built binaries to fall back on" >&2
    exit 1
fi

mkdir -p "$DDC_OUT"
say() { printf 'ddc: %s\n' "$*" >&2; }

# The source, handed over once: the module tree as length-prefixed sections
# with the prelude under `#prelude`, since no path can hold a `#` (Ch. 6
# §3.3). stage1 and stage2 both compile *this*, and its hash is what pins
# what they compiled.
say "bundling $DDC_ROOT"
"$trust" bundle "$DDC_ROOT" --prelude > "$DDC_OUT/sA.bundle"
say "sA.bundle $(wc -c < "$DDC_OUT/sA.bundle") bytes"
sha256sum "$DDC_OUT/sA.bundle" | awk '{printf "ddc:   sha256 %s  sA.bundle\n", $1}' >&2

# stage1 as text: the parent's TIR for the compiler. This is `cP(sA)` —
# one of the two things compared, and also what stage1's image is built
# from, which is what makes the comparison a fixpoint and not a pair of
# coincidences.
say "compiling stage1.tir with the parent"
"$trust" tir "$DDC_ROOT" > "$DDC_OUT/stage1.tir"
say "stage1.tir $(wc -l < "$DDC_OUT/stage1.tir") lines"

# stage1 as an artifact: assembled into an image that runs on the machine
# and nothing else. Its hash is the provenance of everything below.
say "assembling stage1.timg"
"$trust" asm "$DDC_ROOT" > "$DDC_OUT/stage1.t27"
"$tritium" asm "$DDC_OUT/stage1.t27" -o "$DDC_OUT/stage1.timg"
sha256sum "$DDC_OUT/stage1.timg" | awk '{printf "ddc:   sha256 %s  stage1.timg\n", $1}' >&2

# stage2: the image, handed the bundle, on the reference machine — stdin to
# stdout, like every other program, since a compiler that can open files
# would be an operating system (ISA §2.2). This is the long step: a
# compiler written in Trust runs at the machine's speed, not the host's.
# The CLI's step budget defaults to 10^8, which a compiler outlives in the
# first minute; the driver's own `run` passes u64::MAX, and 10^13 is that
# in a coat the argument parser is sure to read — a run that reaches it is
# hung by anyone's measure. Memory is the same story, and the tighter one:
# the default 3^15 words held every program the language was ever handed,
# but a compiler compiling itself keeps the source, the tree, the module
# and the text of the module in play at once — 3^17 words is the next
# ruled size up, and the bump allocator's arena is the whole of free
# memory either way.
say "running stage1 on its own source (this is the long step)"
"$tritium" run "$DDC_OUT/stage1.timg" --mem 129140163 --steps 10000000000000 < "$DDC_OUT/sA.bundle" > "$DDC_OUT/stage2.tir"
say "stage2.tir $(wc -l < "$DDC_OUT/stage2.tir") lines"

# The answer, both numbers, either way. §4: a number that is only printed
# when it is right is a number nobody checks.
s1=$(sha256sum "$DDC_OUT/stage1.tir" | cut -d' ' -f1)
s2=$(sha256sum "$DDC_OUT/stage2.tir" | cut -d' ' -f1)
printf 'ddc: stage1.tir sha256 %s\n' "$s1"
printf 'ddc: stage2.tir sha256 %s\n' "$s2"
if cmp -s "$DDC_OUT/stage1.tir" "$DDC_OUT/stage2.tir"; then
    printf 'ddc: stage2 == stage1, byte for byte — the fixpoint holds\n'
else
    {
        printf 'ddc: stage2 and stage1 DISAGREE — first difference:\n'
        cmp "$DDC_OUT/stage1.tir" "$DDC_OUT/stage2.tir" || true
        diff "$DDC_OUT/stage1.tir" "$DDC_OUT/stage2.tir" | head -20 || true
    } >&2
    exit 1
fi
