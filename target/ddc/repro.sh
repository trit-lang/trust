#!/usr/bin/env bash
# Correct repro harness: trustc tir <file>  vs  bundle+prelude through
# bootstrap/program.tr (what the suite's q loop does).
set -u
cd /home/nixos/code/Young-TW/trust-lang
f=$1
tag=$(basename "$f" .tr)
target/release/trustc tir "$f" > /tmp/$tag.rust 2>/tmp/$tag.rust.err
rc=$?
target/release/trust bundle "$f" --prelude > /tmp/$tag.bundle 2>/dev/null
target/release/trust run bootstrap/program.tr < /tmp/$tag.bundle > /tmp/$tag.mine 2>/tmp/$tag.mine.err
mc=$?
if [ $rc -ne 0 ] && [ $mc -ne 0 ]; then echo "$tag AGREE-REFUSE"; exit 0; fi
if [ $rc -ne 0 ]; then echo "$tag trustc-refuses-only"; exit 0; fi
if cmp -s /tmp/$tag.rust /tmp/$tag.mine; then echo "$tag SAME"; else echo "$tag DIFF"; fi
