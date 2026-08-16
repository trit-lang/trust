#!/bin/sh
# Every `Ch. N §M` / `AM §M` / `TIR §M` / `ISA §M` citation in the source *and
# in the specification itself*, checked against the document it names.
#
# The specification cites itself far more than the code does, and for a long
# time nothing checked those: this script read `core/src compiler/src vm/src`
# only. A chapter that cites a section of another chapter is exactly where a
# renumbering goes wrong.
#
# It cannot tell whether a citation is *apt* — only whether the section it
# points at exists. That is a low bar, and it is still the bar the drop-glue
# double-free failed: `drop_at` cited Ch. 3 §1.4, whose item 3 says a field
# the destructor moved out of is not dropped again, while the code dropped
# every field twice. A citation that names a real section it contradicts is
# the shape this catches next, once §-level anchors are extracted.
set -eu
cd "$(dirname "$0")/.."

doc_for() {
  case "$1" in
    AM)    echo spec/00-abstract-machine.md ;;
    TIR)   echo spec/tir-0.1.md ;;
    ISA)   echo spec/isa/trisc-27-0.1.md ;;
    "Ch. 0") echo spec/language/00-syntax.md ;;
    "Ch. 1") echo spec/01-types.md ;;
    "Ch. 2") echo spec/02-composites.md ;;
    "Ch. 3") echo spec/language/03-references.md ;;
    "Ch. 4") echo spec/language/04-generics.md ;;
    "Ch. 5") echo spec/language/05-library.md ;;
    *)     echo "" ;;
  esac
}

out=$(mktemp)
grep -rhoE '(AM|TIR|ISA|Ch\. [0-9]) §[0-9]+(\.[0-9]+)*' \
  core/src compiler/src vm/src spec docs README.md 2>/dev/null | sort -u |
while IFS= read -r cite; do
  doc=$(doc_for "${cite% §*}")
  sec="${cite##* §}"
  [ -n "$doc" ] || continue
  # A section exists if some heading in the document begins with its number.
  if ! grep -qE "^#+ ${sec}[. ]|^#+ ${sec}\$" "$doc"; then
    printf 'no such section: %s  (%s)\n' "$cite" "$doc" >>"$out"
  fi
done

if [ -s "$out" ]; then
  cat "$out"
  n=$(wc -l <"$out")
  rm -f "$out"
  printf '%d citation(s) name a section that does not exist\n' "$n"
  exit 1
fi
rm -f "$out"
n=$(grep -rhoE '(AM|TIR|ISA|Ch\. [0-9]) §[0-9]+(\.[0-9]+)*' \
  core/src compiler/src vm/src spec docs README.md 2>/dev/null | sort -u | wc -l)
printf 'citations: %d distinct, every section named exists\n' "$n"

# The gap registry, checked the same way and for the same reason.
#
# `docs/spec-gaps.md` is a registry: other documents and the source cite an
# entry by number, so a number that names two entries names neither. Three
# collisions accumulated before anything looked — G0.4, G0.5 and G0.6 — each
# added by someone reaching for "the next number" without reading to the end
# of a file that is thousands of lines long. Reading is what a script is for.
gaps=docs/spec-gaps.md
bad=$(grep -oE '^\*\*G[0-9]+\.[0-9]+[a-z]*' "$gaps" | sed 's/^\*\*//' | sort | uniq -d)
if [ -n "$bad" ]; then
  echo "$bad" | sed 's/^/gap number names more than one entry: /'
  exit 1
fi

# And a citation of a gap that does not exist, which is the other way the
# registry stops being one.
missing=$(grep -rhoE '\bG[0-9]+\.[0-9]+[a-z]*' \
  core/src compiler/src vm/src lsp/src driver/src spec docs editors README.md 2>/dev/null |
  sort -u |
  while IFS= read -r g; do
    grep -qE "^\*\*$(echo "$g" | sed 's/\./\\./g')[^0-9]" "$gaps" || echo "$g"
  done)
if [ -n "$missing" ]; then
  echo "$missing" | sed 's/^/no such gap: /'
  exit 1
fi
printf 'gaps: %d entries, every number unique and every citation resolves\n' \
  "$(grep -cE '^\*\*G[0-9]+\.[0-9]+[a-z]*' "$gaps")"
