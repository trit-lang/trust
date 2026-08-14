#!/bin/sh
# Every number in docs/status.md §2 and §4. Generated, not recalled — the
# document opens by claiming its numbers were produced by running something,
# and a number nobody can reproduce quietly withdraws that claim.
set -eu
cd "$(dirname "$0")/.."

printf 'specification   %6d lines, %d documents\n' \
  "$(cat spec/*.md spec/language/*.md spec/isa/*.md | wc -l)" \
  "$(ls spec/*.md spec/language/*.md spec/isa/*.md | wc -l)"
printf 'docs + README   %6d lines\n' "$(cat docs/*.md README.md | wc -l)"
for c in core compiler vm; do
  printf '%-15s %6d lines\n' "$c" "$(find $c -name '*.rs' -exec cat {} + | wc -l)"
done
printf 'gap entries     %6d\n' "$(grep -cE '^\*\*G[0-9]+\.[0-9a-z]+ ' docs/spec-gaps.md)"
printf 'commits         %6d\n' "$(git rev-list --count HEAD)"
printf 'tests           %6d passing\n' "$(cargo test 2>&1 | grep -cE '^test .* ok$')"
