// cp3/test_am_appendix.cpp — the worked identities of AM Appendix A (each one a
// compiled, executed test), Lang Ch.1 §4 mulh, §6 conversions, and the ISA §4.3
// constant-split identity.

#include "testharness.hpp"

// A.1: "neg(neg(x)) = x; tneg is an involution trit-wise and word-wise."
TEST_CASE(am_appendix_A1_neg_involution, "AM Appendix A.1") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    for (long long v = -llmax(N); v <= llmax(N); ++v) {
      const T x = T::from_i64(v);
      CHECK(trt::neg(trt::neg(x)) == x);
      CHECK(trt::tneg(trt::tneg(x)) == x);
    }
  };
  sweep(width<1>{});
  sweep(width<2>{});
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  sweep(width<9>{});
  std::vector<trt::t27> xs;
  sample_values<27>(100, xs);
  for (const auto x : xs) {
    CHECK(trt::neg(trt::neg(x)) == x);
    CHECK(trt::tneg(trt::tneg(x)) == x);
  }
}

// A.2: "abs(x) = tmul(x, splat(sign(x))) where sign(x) = cmp(x, 0)" — absolute
// value with no branch and no overflow case.  (The erratum is why splat is
// present: tmul against a *widened* sign would keep only the lowest trit.)
TEST_CASE(am_appendix_A2_abs_identity, "AM Appendix A.2 + §3.4") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    for (long long v = -llmax(N); v <= llmax(N); ++v) {
      const T x = T::from_i64(v);
      const auto sign = trt::cmp(x, T::zero());
      CHECK(trt::abs(x) == trt::tmul(x, T::splat(sign)));
    }
  };
  sweep(width<1>{});
  sweep(width<2>{});
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  sweep(width<9>{});
  std::vector<trt::t27> xs;
  sample_values<27>(150, xs);
  for (const auto x : xs)
    CHECK(trt::abs(x) == trt::tmul(x, trt::t27::splat(trt::cmp(x, trt::t27::zero()))));
  const auto w = trt::Tint<81>::wrap_of(trt::trits_from_string("T1T0011T1T0011T"));
  CHECK(trt::abs(w) == trt::tmul(w, trt::Tint<81>::splat(trt::cmp(w, trt::Tint<81>::zero()))));
  CHECK(trt::abs(w) == trt::neg(w));
  // Explicitly at MIN: no overflow case exists.
  CHECK(trt::abs(trt::t27::min_value()) == trt::tmul(trt::t27::min_value(),
         trt::t27::splat(trt::cmp(trt::t27::min_value(), trt::t27::zero()))));
}

// A.3: "shr(shl(x, k), k) = x whenever the shl did not overflow."  The flag
// form is used both to detect the no-overflow condition and to supply the
// exact (== wrapped, then) shift value.
TEST_CASE(am_appendix_A3_shift_roundtrip, "AM Appendix A.3") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    const long long M = llmax(N);
    for (long long v = -M; v <= M; ++v)
      for (int k = 0; k < N; ++k) {
        const T x = T::from_i64(v);
        const auto sf = trt::shl_flag(x, k);
        if (sf.overflow.to_int() != 0) continue;  // only the no-overflow case is claimed
        CHECK(trt::shr(sf.value, k) == x);
        CHECK(trt::shr(trt::shl_trap(x, k), k) == x);
      }
  };
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  std::vector<trt::t9> x9;
  sample_values<9>(120, x9);
  for (const auto x : x9)
    for (int k = 0; k < 9; ++k) {
      const auto sf = trt::shl_flag(x, k);
      if (sf.overflow.to_int() != 0) continue;
      CHECK(trt::shr(sf.value, k) == x);
    }
  std::vector<trt::t27> x27;
  sample_values<27>(25, x27);
  for (const auto x : x27)
    for (int k = 0; k < 27; ++k) {
      const auto sf = trt::shl_flag(x, k);
      if (sf.overflow.to_int() != 0) continue;
      CHECK(trt::shr(sf.value, k) == x);
    }
}

// A.4: "wrap(x + y) = neg(wrap(neg(x) + neg(y))) — wrapping arithmetic commutes
// with negation, unlike two's complement where MIN breaks the symmetry."
TEST_CASE(am_appendix_A4_wrap_negation_commutes, "AM Appendix A.4") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    const long long M = llmax(N);
    for (long long ia = -M; ia <= M; ++ia)
      for (long long ib = -M; ib <= M; ++ib) {
        const T x = T::from_i64(ia), y = T::from_i64(ib);
        CHECK(trt::add_wrap(x, y) == trt::neg(trt::add_wrap(trt::neg(x), trt::neg(y))));
      }
  };
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  std::vector<trt::t9> xs;
  sample_values<9>(150, xs);
  for (const auto x : xs)
    for (const auto y : xs)
      CHECK(trt::add_wrap(x, y) == trt::neg(trt::add_wrap(trt::neg(x), trt::neg(y))));
  std::vector<trt::t27> ys;
  sample_values<27>(25, ys);
  for (const auto x : ys)
    for (const auto y : ys)
      CHECK(trt::add_wrap(x, y) == trt::neg(trt::add_wrap(trt::neg(x), trt::neg(y))));
}

// A.5: "rem(x, 3) = t₀(x) — the least significant trit *is* the remainder mod 3
// under round-to-nearest, taking values in {−1, 0, +1}."
TEST_CASE(am_appendix_A5_rem3_is_low_trit, "AM Appendix A.5") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    const T three = T::from_i64(3);
    for (long long v = -llmax(N); v <= llmax(N); ++v) {
      const T x = T::from_i64(v);
      const auto r = trt::rem(x, three);
      CHECK_LL(r.to_i64(), x.lst());
      CHECK(r.to_i64() >= -1 && r.to_i64() <= 1);
      CHECK(trt::cmp(r, T::zero()).to_int() == r.to_i64());
    }
  };
  sweep(width<2>{});  // t1 cannot hold the divisor 3, so the sweep starts at t2
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  sweep(width<6>{});
  std::vector<trt::t27> xs;
  sample_values<27>(150, xs);
  for (const auto x : xs) CHECK_LL(trt::rem(x, trt::t27::from_i64(3)).to_i64(), x.lst());
}

// Lang Ch.1 §4 and TIR §3.1: "a.mulh(b) · 3ᴺ + a.wrapping_mul(b) reconstructs
// the exact product", and mulh is total — the high half always fits.
TEST_CASE(lang_4_mulh, "Lang Ch.1 §4 + TIR §3.1 + ISA §4.1") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    const long long M = llmax(N);
    for (long long ia = -M; ia <= M; ++ia)
      for (long long ib = -M; ib <= M; ++ib) {
        const T a = T::from_i64(ia), b = T::from_i64(ib);
        const i128 exact = static_cast<i128>(ia) * ib;
        const auto hi = trt::mulh(a, b);
        const i128 lo = static_cast<i128>(trt::mul_wrap(a, b).to_i64());
        CHECK(static_cast<i128>(hi.to_i64()) * pow3i(N) + lo == exact);
        CHECK(hi.to_i64() >= -llmax(N) && hi.to_i64() <= llmax(N));  // total: always fits
      }
  };
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
  // t9/t27: boundary and fixed-seed samples (an exhaustive t9 pair sweep is
  // 3.9e8 pairs; targeted sampling plus full small-width sweeps instead).
  std::vector<trt::t9> xs;
  sample_values<9>(120, xs);
  for (const auto a : xs)
    for (const auto b : xs) {
      const i128 exact = static_cast<i128>(a.to_i64()) * b.to_i64();
      const auto hi = trt::mulh(a, b);
      CHECK(static_cast<i128>(hi.to_i64()) * pow3i(9) + trt::mul_wrap(a, b).to_i64() == exact);
      CHECK(hi.to_i64() >= -9841 && hi.to_i64() <= 9841);
    }
  std::vector<trt::t27> ys;
  sample_values<27>(30, ys);
  for (const auto a : ys)
    for (const auto b : ys) {
      const i128 exact = static_cast<i128>(a.to_i64()) * b.to_i64();
      const auto hi = trt::mulh(a, b);
      CHECK(static_cast<i128>(hi.to_i64()) * pow3i(27) + trt::mul_wrap(a, b).to_i64() == exact);
    }
  // "It is total: the high half of a product of two tN values always fits in
  // tN" — computed here at the extremes with no fault expected.
  CHECK(trt::mulh(trt::t9::min_value(), trt::t9::min_value()) == trt::mulh(trt::t9::max_value(), trt::t9::max_value()));
  CHECK(trt::mulh(trt::t27::max_value(), trt::t27::from_i64(1)) == trt::t27::zero());
  CHECK(trt::mulh(trt::t27::min_value(), trt::t27::from_i64(1)) == trt::t27::zero());
  CHECK(trt::mulh(trt::t27::max_value(), trt::t27::max_value()) > trt::t27::zero());
  CHECK(trt::mulh(trt::t27::max_value(), trt::t27::min_value()) < trt::t27::zero());
  // mulh is the *other* half of the product, not the low half:
  CHECK(trt::mulh(trt::t9::max_value(), trt::t9::max_value()) != trt::mul_wrap(trt::t9::max_value(), trt::t9::max_value()));
}

// Lang Ch.1 §6 (TIR §3.5): widening is value-preserving; narrowing wraps, with
// the negation symmetry "(-x as t9) == -(x as t9) holds".
TEST_CASE(lang_6_conversions, "Lang Ch.1 §6 + TIR §3.5") {
  // Every t9 value survives widening to t27 exactly.
  for (long long v = -9841; v <= 9841; ++v)
    CHECK_LL((trt::widen<9, 27>(trt::t9::from_i64(v)).to_i64()), v);
  // Narrowing is wrapping — full sweep t9 → t3, plus "checked" narrowing.
  for (long long v = -9841; v <= 9841; ++v) {
    const auto x = trt::t9::from_i64(v);
    CHECK_LL((trt::narrow<3, 9>(x).to_i64()), wrapi(v, 3));
    CHECK(trt::narrow<3, 9>(trt::neg(x)) == trt::neg(trt::narrow<3, 9>(x)));  // Ch.1 §6 note
    CHECK(trt::checked_narrow<3, 9>(x).has_value() == (v >= -13 && v <= 13));
    if (v >= -13 && v <= 13) CHECK_LL((trt::checked_narrow<3, 9>(x)->to_i64()), v);
    // narrow∘widen is not identity in general, but it is when the value fits:
    if (v >= -13 && v <= 13) {
      const auto small = trt::Tint<3>::from_i64(v);
      CHECK(trt::narrow<3, 9>(trt::widen<3, 9>(small)) == small);
    }
  }
  std::vector<trt::t27> xs;
  sample_values<27>(120, xs);
  for (const auto x : xs) {
    CHECK_LL((trt::narrow<9, 27>(x).to_i64()), wrapi(x.to_i64(), 9));
    CHECK(trt::narrow<9, 27>(trt::neg(x)) == trt::neg(trt::narrow<9, 27>(x)));
    CHECK(trt::checked_narrow<9, 27>(x).has_value() == (x.to_i64() >= -9841 && x.to_i64() <= 9841));
  }
  // *as* between same-width taddr and t27 is value-preserving on the AM.
  CHECK(trt::taddr::from_i64(123) == trt::t27::from_i64(123));
}

// ISA §4.3: splitting a constant c into hi = c >> 14 and lo = wrap(c, 14) needs
// no correction term, because balanced splitting borrows nothing:
//     c == hi·3^14 + lo,     hi fits 13 trits (−797161 … 797161), lo fits 14.
TEST_CASE(isa_4_3_constant_split, "ISA §4.3 + §3.2") {
  const long long P14 = 4782969;       // 3^14
  const long long IMM13MAX = 797161;   // (3^13−1)/2
  auto check = [](trt::t27 c) {
    const auto hi = trt::shr(c, 14);              // "the split of a constant c"
    const auto lo = trt::narrow<14, 27>(c);       // wrap(c,14)
    CHECK_LL(c.to_i64(), static_cast<i128>(hi.to_i64()) * P14 + lo.to_i64());
    CHECK(hi.to_i64() >= -IMM13MAX && hi.to_i64() <= IMM13MAX);
    CHECK(lo.to_i64() >= -2391484 && lo.to_i64() <= 2391484);  // 14-trit immediate range
  };
  for (const auto c : boundary_values<27>()) check(c);
  check(trt::t27::from_i64(9841));
  check(trt::t27::from_i64(-9841));
  check(trt::t27::from_i64(P14));
  check(trt::t27::from_i64(-P14));
  check(trt::t27::from_i64(P14 - 1));
  check(trt::t27::from_i64(P14 + 1));
  Lcg l(0x51CEull);
  const long long M = llmax(27);
  for (int i = 0; i < 500; ++i) check(trt::t27::from_i64(l.uniform(-M, M)));
  // The documented maximum reachable magnitude is exactly MAX of the word.
  CHECK_LL(trt::shr(trt::t27::max_value(), 14).to_i64(), IMM13MAX);
  CHECK_LL(trt::shr(trt::t27::min_value(), 14).to_i64(), -IMM13MAX);
  CHECK_LL((trt::narrow<14, 27>(trt::t27::max_value()).to_i64()), 2391484);
  CHECK_LL((trt::narrow<14, 27>(trt::t27::min_value()).to_i64()), -2391484);
}
