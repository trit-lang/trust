// cp3/testharness.hpp — tiny zero-dependency test harness + independent oracles.
//
// The oracles below use host integer arithmetic only (__int128 and a
// round-to-nearest division built directly on the host's truncating divide),
// so the trit-array engine's results are cross-checked against something that
// shares no code with it.

#pragma once

#include <algorithm>
#include <cstdio>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>

#include "core.hpp"
#include "literals.hpp"

// ---------------------------------------------------------------------------
// Harness: CHECK macros count assertions; TEST_CASE registers a named case with
// the spec section(s) it transcribes.  test_main.cpp drives the registry.
// ---------------------------------------------------------------------------
inline long long g_asserts = 0, g_fails = 0, g_cases = 0;

// Variadic so that template-argument commas inside the condition survive the
// preprocessor (e.g. CHECK(trt::widen<1, 9>(x) == y)).
#define CHECK(...)                                                           \
  do {                                                                       \
    ++g_asserts;                                                             \
    if (!(__VA_ARGS__)) {                                                    \
      ++g_fails;                                                             \
      std::printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #__VA_ARGS__);     \
    }                                                                        \
  } while (0)

#define CHECK_LL(actual, expected)                                           \
  do {                                                                       \
    ++g_asserts;                                                             \
    const long long a_ = static_cast<long long>(actual);                     \
    const long long e_ = static_cast<long long>(expected);                   \
    if (a_ != e_) {                                                          \
      ++g_fails;                                                             \
      std::printf("FAIL %s:%d: %s == %s (got %lld, want %lld)\n",            \
                  __FILE__, __LINE__, #actual, #expected, a_, e_);           \
    }                                                                        \
  } while (0)

template <class F>
inline bool faults_with(trt::Fault code, F&& f) {
  try { f(); } catch (const trt::MachineFault& e) { return e.code() == code; }
  catch (...) { return false; }
  return false;
}
#define CHECK_FAULT(code, expr)                                              \
  do {                                                                       \
    ++g_asserts;                                                             \
    if (!faults_with(code, [&] { (void)(expr); })) {                         \
      ++g_fails;                                                             \
      std::printf("FAIL %s:%d: expected %s from %s\n", __FILE__, __LINE__,   \
                  trt::fault_name(code), #expr);                             \
    }                                                                        \
  } while (0)

#define CHECK_PARSE_ERR(expr)                                                \
  do {                                                                       \
    ++g_asserts;                                                             \
    bool ok_ = false;                                                        \
    try { (void)(expr); } catch (const trt::ParseError&) { ok_ = true; }     \
    catch (...) {}                                                           \
    if (!ok_) {                                                              \
      ++g_fails;                                                             \
      std::printf("FAIL %s:%d: expected ParseError from %s\n",               \
                  __FILE__, __LINE__, #expr);                                \
    }                                                                        \
  } while (0)

struct Case {
  const char* spec;
  const char* name;
  void (*fn)();
};
inline std::vector<Case>& registry() {
  static std::vector<Case> r;
  return r;
}
struct Registrar {
  Registrar(const char* spec, const char* name, void (*fn)()) {
    registry().push_back({spec, name, fn});
  }
};
#define TEST_CASE(NAME, SPEC)                                                \
  static void NAME();                                                        \
  static Registrar reg_##NAME(SPEC, #NAME, &NAME);                           \
  static void NAME()

// ---------------------------------------------------------------------------
// Independent host-side oracles.
// ---------------------------------------------------------------------------
using i128 = __int128;

inline i128 pow3i(int n) { i128 r = 1; for (int i = 0; i < n; ++i) r *= 3; return r; }
inline i128 maxi(int n) { return (pow3i(n) - 1) / 2; }   // AM §1.2 symmetrical range
inline i128 mini(int n) { return -maxi(n); }             // AM §1.2: MIN = −MAX
inline long long llmax(int n) { return static_cast<long long>(maxi(n)); }

// Symmetric residue mod 3^n (wrapping), host integers only.
inline i128 wrapi(i128 x, int n) {
  const i128 m = pow3i(n), hi = maxi(n);
  i128 r = x % m;
  if (r > hi) r -= m;
  if (r < -hi) r += m;
  return r;
}

// AM §3.2, re-derived from the host's truncating divide: nearest, with the
// exactly-half case stepped away from zero.  Independent of the engine.
inline i128 div_rn(i128 a, i128 b) {
  i128 q = a / b;                 // trunc toward zero (host)
  const i128 r = a - q * b;       // sign of a, |r| < |b|
  const i128 ar = r < 0 ? -r : r, ab = b < 0 ? -b : b;
  if (2 * ar >= ab) {             // strictly past half, or exactly at half
    const i128 s = static_cast<i128>((a < 0 ? -1 : 1) * (b < 0 ? -1 : 1));
    q += s;                       // toward the exact quotient; on ties, away from zero
  }
  return q;
}
inline i128 rem_rn(i128 a, i128 b) { return a - div_rn(a, b) * b; }

// Deterministic sampling only: fixed seeds, no clock, no environment input.
struct Lcg {
  unsigned long long s;
  explicit Lcg(unsigned long long seed = 0x243F6A8885A308D3ull) : s(seed) {}
  unsigned long long next() {
    s = s * 6364136223846793005ull + 1442695040888963407ull;
    return s >> 11;
  }
  long long uniform(long long lo, long long hi) {  // inclusive both ends
    return lo + static_cast<long long>(next() % static_cast<unsigned long long>(hi - lo + 1));
  }
};

// Exact integer value of an integer literal, via the spec's parser.
inline long long lit(std::string_view s) {
  return static_cast<long long>(trt::detail::t_to_i128(trt::parse_literal(s).exact));
}

template <int N>
inline std::vector<trt::Tint<N>> boundary_values() {
  std::vector<trt::Tint<N>> v;
  const long long M = llmax(N);
  const long long cases[] = {0, 1, -1, 2, -2, 3, -3, 4, -4, 5, -5,
                             M, -M, M - 1, -M + 1, M / 2, -(M / 2)};
  for (long long x : cases) v.push_back(trt::Tint<N>::from_i64(x));
  return v;
}

template <int N>
inline void sample_values(long long count, std::vector<trt::Tint<N>>& out) {
  Lcg l(0x9E3779B97F4A7C15ull + static_cast<unsigned long long>(N));
  const long long M = llmax(N);
  for (const auto& x : boundary_values<N>()) out.push_back(x);
  for (long long i = 0; i < count; ++i)
    out.push_back(trt::Tint<N>::from_i64(l.uniform(-M, M)));
}

// AM §3.1's carry out of the top trit, as a direct per-trit ripple-carry
// addition.  Independent of the engine's overflow computation.
template <int N>
inline int carry_out_top(trt::Tint<N> a, trt::Tint<N> b) {
  int c = 0;
  for (int i = 0; i < N; ++i) {
    const int v = a.trit(static_cast<std::size_t>(i)) + b.trit(static_cast<std::size_t>(i)) + c;
    int d = v % 3;
    if (d > 1) d -= 3;
    else if (d < -1) d += 3;
    c = (v - d) / 3;
  }
  return c;
}

// A cheap width tag for templated exhaustive sweeps.
template <int N>
using width = std::integral_constant<int, N>;
