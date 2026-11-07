#!/bin/sh
# cp3/run.sh — build and run the spec-transcribed test suite with the
# prescribed compiler (GCC 15.2.0 at ~/.nix-profile/bin/g++), no make/cmake.
# POSIX sh, so it works whatever shells the machine happens to install.
set -eu
cd "$(dirname "$0")"

GXX="${GXX:-$HOME/.nix-profile/bin/g++}"

# Note: no -Wpedantic solely because __int128 is a GNU extension; it is used
# only in the host-side test oracles and the i64 convenience bridge, never in
# the AM semantics themselves.
"$GXX" -std=c++20 -O2 -Wall -Wextra -Werror \
  test_main.cpp \
  test_am_repr.cpp \
  test_am_flavors.cpp \
  test_am_div.cpp \
  test_am_shift_tritwise_cmp.cpp \
  test_am_cmp_faults.cpp \
  test_am_appendix.cpp \
  test_literals.cpp \
  -o tests

./tests
