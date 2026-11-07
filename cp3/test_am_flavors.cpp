// cp3/test_am_flavors.cpp — AM §3.1: add/sub/mul exact-when-representable,
// the wrapping / trapping / flag flavors, and Lang Ch.1 §4/P4's checked_* and
// saturating_* library compositions.

#include "testharness.hpp"

// Exhaustive small-width sweep: wrap = symmetric residue, congruent modulo 3^N;
// flag yields the same wrapped value plus the overflow *direction* trit; for
// add/sub that trit is exactly the carry out of the top trit (AM §3.1).
template <int N>
static void wrap_and_flag_exhaustive() {
  const long long M = llmax(N);
  for (long long ia = -M; ia <= M; ++ia) {
    for (long long ib = -M; ib <= M; ++ib) {
      const auto a = trt::Tint<N>::from_i64(ia), b = trt::Tint<N>::from_i64(ib);
      const i128 s = static_cast<i128>(ia) + ib, d = static_cast<i128>(ia) - ib,
                 p = static_cast<i128>(ia) * ib;
      const auto aw = trt::add_wrap(a, b), sw = trt::sub_wrap(a, b), mw = trt::mul_wrap(a, b);
      CHECK_LL(aw.to_i64(), wrapi(s, N));
      CHECK_LL(sw.to_i64(), wrapi(d, N));
      CHECK_LL(mw.to_i64(), wrapi(p, N));
      CHECK((s - static_cast<i128>(aw.to_i64())) % pow3i(N) == 0);
      CHECK((d - static_cast<i128>(sw.to_i64())) % pow3i(N) == 0);
      CHECK((p - static_cast<i128>(mw.to_i64())) % pow3i(N) == 0);
      // Exact when representable: trapping flavor agrees with wrapping then.
      if (s >= mini(N) && s <= maxi(N)) CHECK_LL(trt::add_trap(a, b).to_i64(), s);
      if (d >= mini(N) && d <= maxi(N)) CHECK_LL(trt::sub_trap(a, b).to_i64(), d);
      if (p >= mini(N) && p <= maxi(N)) CHECK_LL(trt::mul_trap(a, b).to_i64(), p);
      const auto af = trt::add_flag(a, b), sf = trt::sub_flag(a, b), mf = trt::mul_flag(a, b);
      CHECK(af.value == aw && sf.value == sw && mf.value == mw);
      CHECK_LL(af.overflow.to_int(), s > maxi(N) ? 1 : s < mini(N) ? -1 : 0);
      CHECK_LL(sf.overflow.to_int(), d > maxi(N) ? 1 : d < mini(N) ? -1 : 0);
      CHECK_LL(mf.overflow.to_int(), p > maxi(N) ? 1 : p < mini(N) ? -1 : 0);
      CHECK_LL(af.overflow.to_int(), carry_out_top(a, b));
      CHECK_LL(sf.overflow.to_int(), carry_out_top(a, trt::neg(b)));
      // AM §3.1: "wrapping is negation-compatible: wrap(−x) = −wrap(x)" (A.4 below).
      CHECK(aw == trt::neg(trt::add_wrap(trt::neg(a), trt::neg(b))));
      // Same for multiplication: (−a)·(−b) has the *same* exact product as a·b,
      // while one negated factor negates the wrapped product (wrap(−x) = −wrap(x)).
      CHECK(mw == trt::mul_wrap(trt::neg(a), trt::neg(b)));
      CHECK(trt::mul_wrap(trt::neg(a), b) == trt::neg(mw));
      CHECK(trt::mul_wrap(a, trt::neg(b)) == trt::neg(mw));
      CHECK(sw == trt::add_wrap(a, trt::neg(b)));
    }
  }
}

TEST_CASE(am_3_1_wrap_flag_exhaustive_small, "AM §3.1") {
  wrap_and_flag_exhaustive<3>();
  wrap_and_flag_exhaustive<4>();
}

TEST_CASE(am_3_1_wrap_flag_t9_samples, "AM §3.1") {
  std::vector<trt::t9> xs;
  sample_values<9>(250, xs);
  for (const auto a : xs)
    for (const auto b : xs) {
      const i128 ia = a.to_i64(), ib = b.to_i64();
      CHECK_LL(trt::add_wrap(a, b).to_i64(), wrapi(ia + ib, 9));
      CHECK_LL(trt::sub_wrap(a, b).to_i64(), wrapi(ia - ib, 9));
      CHECK_LL(trt::mul_wrap(a, b).to_i64(), wrapi(ia * ib, 9));
      CHECK_LL(trt::add_flag(a, b).overflow.to_int(), ia + ib > maxi(9) ? 1 : ia + ib < mini(9) ? -1 : 0);
      CHECK_LL(trt::add_flag(a, b).overflow.to_int(), carry_out_top(a, b));
      CHECK_LL(trt::sub_flag(a, b).overflow.to_int(), carry_out_top(a, trt::neg(b)));
    }
}

TEST_CASE(am_3_1_wrap_flag_t27_samples, "AM §3.1") {
  std::vector<trt::t27> xs;
  sample_values<27>(45, xs);
  for (const auto a : xs)
    for (const auto b : xs) {
      const i128 ia = a.to_i64(), ib = b.to_i64();
      CHECK_LL(trt::add_wrap(a, b).to_i64(), wrapi(ia + ib, 27));
      CHECK_LL(trt::sub_wrap(a, b).to_i64(), wrapi(ia - ib, 27));
      CHECK_LL(trt::mul_wrap(a, b).to_i64(), wrapi(ia * ib, 27));
      const auto af = trt::add_flag(a, b), sf = trt::sub_flag(a, b), mf = trt::mul_flag(a, b);
      CHECK(af.value == trt::add_wrap(a, b));
      CHECK(sf.value == trt::sub_wrap(a, b));
      CHECK(mf.value == trt::mul_wrap(a, b));
      CHECK_LL(af.overflow.to_int(), ia + ib > maxi(27) ? 1 : ia + ib < mini(27) ? -1 : 0);
      CHECK_LL(sf.overflow.to_int(), ia - ib > maxi(27) ? 1 : ia - ib < mini(27) ? -1 : 0);
      CHECK_LL(mf.overflow.to_int(), ia * ib > maxi(27) ? 1 : ia * ib < mini(27) ? -1 : 0);
      CHECK_LL(af.overflow.to_int(), carry_out_top(a, b));
      CHECK_LL(sf.overflow.to_int(), carry_out_top(a, trt::neg(b)));
    }
}

// AM §3.1: the flag "extends unchanged to mul and shl, whose exact result can
// exceed the width by more than one trit" (G2.1) — it carries the direction,
// not a magnitude of one trit.
TEST_CASE(am_3_1_flag_beyond_one_trit, "AM §3.1 (G2.1)") {
  using t3 = trt::Tint<3>;
  const auto p1 = trt::mul_flag(t3::from_i64(13), t3::from_i64(13));   // exact +169
  CHECK_LL(p1.value.to_i64(), 7);
  CHECK_LL(p1.overflow.to_int(), 1);
  const auto p2 = trt::mul_flag(t3::from_i64(13), t3::from_i64(-13));  // exact −169
  CHECK_LL(p2.value.to_i64(), -7);
  CHECK_LL(p2.overflow.to_int(), -1);
  const auto sh = trt::shl_flag(t3::from_i64(5), 2);                   // exact +45
  CHECK_LL(sh.value.to_i64(), -9);
  CHECK_LL(sh.overflow.to_int(), 1);
  const auto shn = trt::shl_flag(t3::from_i64(-5), 2);                 // exact −45
  CHECK_LL(shn.value.to_i64(), 9);
  CHECK_LL(shn.overflow.to_int(), -1);
  // No overflow: the trit is 0 and the wrapped value is exact.
  const auto ok = trt::add_flag(t3::from_i64(2), t3::from_i64(3));
  CHECK_LL(ok.value.to_i64(), 5);
  CHECK_LL(ok.overflow.to_int(), 0);
  // Boundary: exact MAX and exact MIN do not overflow.
  const auto atmax = trt::add_flag(t3::from_i64(12), t3::from_i64(1));
  CHECK_LL(atmax.value.to_i64(), 13);
  CHECK_LL(atmax.overflow.to_int(), 0);
  const auto atmin = trt::add_flag(t3::from_i64(-12), t3::from_i64(-1));
  CHECK_LL(atmin.value.to_i64(), -13);
  CHECK_LL(atmin.overflow.to_int(), 0);
}

TEST_CASE(am_3_1_trapping, "AM §3.1 + §4 (F_OVERFLOW)") {
  using trt::t9;
  const auto MAX = t9::max_value(), MIN = t9::min_value();
  const auto one = t9::from_i64(1), two = t9::from_i64(2);
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::add_trap(MAX, one));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::sub_trap(MIN, one));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::add_trap(MIN, trt::neg(one)));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::sub_trap(MAX, trt::neg(one)));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::mul_trap(MAX, two));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::mul_trap(MIN, two));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::mul_trap(MAX, MAX));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::mul_trap(MIN, MIN));
  CHECK_LL(trt::add_trap(MAX, t9::zero()).to_i64(), 9841);
  CHECK_LL(trt::add_trap(MIN, t9::zero()).to_i64(), -9841);
  // Boundary products that fit: MAX·1, MIN·1, MAX·(−1), MIN·(−1) — no overflow.
  CHECK_LL(trt::mul_trap(MAX, one).to_i64(), 9841);
  CHECK_LL(trt::mul_trap(MIN, one).to_i64(), -9841);
  CHECK_LL(trt::mul_trap(MAX, trt::neg(one)).to_i64(), -9841);
  CHECK_LL(trt::mul_trap(MIN, trt::neg(one)).to_i64(), 9841);
  CHECK_LL(trt::add_trap(t9::from_i64(2), t9::from_i64(3)).to_i64(), 5);
  CHECK_LL(trt::mul_trap(t9::from_i64(-4), t9::from_i64(3)).to_i64(), -12);
  CHECK_LL(trt::sub_trap(t9::from_i64(2), t9::from_i64(3)).to_i64(), -1);
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::add_trap(trt::t27::max_value(), trt::t27::from_i64(1)));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::mul_trap(trt::t27::min_value(), trt::t27::from_i64(2)));
  // Overflow faults have their own code, not anyone else's (AM §4).
  CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_trap(t9::max_value(), 9));  // k out of range too
}

// Lang Ch.1 §4/P4: checked_* / saturating_* families, composed from the AM
// primitives plus comparison (AM §3.1 third bullet).  Interpretation recorded
// as reader question Q4 (saturating clamps to MIN/MAX; checked yields none).
TEST_CASE(lang_4_checked_and_saturating, "Lang Ch.1 §4/P4 + AM §3.1") {
  using trt::t9;
  const auto MAX = t9::max_value(), MIN = t9::min_value(), one = t9::from_i64(1);
  CHECK(!trt::add_checked(MAX, one).has_value());
  CHECK(trt::add_checked(t9::from_i64(2), t9::from_i64(3)).has_value());
  CHECK_LL(trt::add_checked(t9::from_i64(2), t9::from_i64(3))->to_i64(), 5);
  CHECK(trt::add_checked(MAX, t9::zero()).has_value());
  CHECK(!trt::mul_checked(MAX, t9::from_i64(2)).has_value());
  CHECK_LL(trt::mul_checked(t9::from_i64(3), t9::from_i64(3))->to_i64(), 9);
  CHECK(!trt::sub_checked(MIN, one).has_value());
  CHECK(!trt::shl_checked(t9::from_i64(2), 8).has_value());
  CHECK_LL(trt::shl_checked(t9::from_i64(2), 7)->to_i64(), 4374);

  CHECK(trt::add_sat(MAX, one) == MAX);
  CHECK(trt::add_sat(MIN, trt::neg(one)) == MIN);
  CHECK(trt::sub_sat(MIN, one) == MIN);
  CHECK(trt::sub_sat(MAX, trt::neg(one)) == MAX);
  CHECK(trt::add_sat(MAX, t9::zero()) == MAX);
  CHECK(trt::mul_sat(MAX, MAX) == MAX);
  CHECK(trt::mul_sat(MIN, MIN) == MAX);
  CHECK(trt::mul_sat(MIN, MAX) == MIN);
  CHECK(trt::shl_sat(t9::from_i64(2), 8) == MAX);
  CHECK(trt::shl_sat(t9::from_i64(-2), 8) == MIN);
  CHECK_LL(trt::add_sat(t9::from_i64(2), t9::from_i64(3)).to_i64(), 5);
  CHECK_LL(trt::shl_sat(t9::from_i64(1), 8).to_i64(), 6561);
  // saturating agrees with wrapping whenever nothing overflows.
  std::vector<trt::t9> xs;
  sample_values<9>(60, xs);
  for (const auto a : xs)
    for (const auto b : xs) {
      CHECK((trt::add_sat(a, b) == trt::add_wrap(a, b)) == (trt::add_flag(a, b).overflow.to_int() == 0));
      CHECK((trt::mul_sat(a, b) == trt::mul_wrap(a, b)) == (trt::mul_flag(a, b).overflow.to_int() == 0));
      CHECK(trt::add_sat(a, b) == trt::neg(trt::add_sat(trt::neg(a), trt::neg(b))));
      CHECK(trt::add_checked(a, b).has_value() == (trt::add_flag(a, b).overflow.to_int() == 0));
    }
}

// AM §3.1 closing paragraph: neg and abs are total and have no flavors.
TEST_CASE(am_3_1_neg_abs_total_no_flavors, "AM §3.1") {
  CHECK(trt::abs(trt::t9::min_value()) == trt::t9::max_value());
  CHECK(trt::neg(trt::t9::min_value()) == trt::t9::max_value());
  CHECK(trt::abs(trt::t27::min_value()) == trt::t27::max_value());
  CHECK(trt::neg(trt::t27::min_value()) == trt::t27::max_value());
  CHECK(trt::abs(trt::t27::zero()) == trt::t27::zero());
  // t1 (the arithmetic trit width) mixes total neg with wrapping add:
  using t1 = trt::Tint<1>;
  CHECK_LL(trt::add_wrap(t1::from_i64(1), t1::from_i64(1)).to_i64(), -1);  // 2 wraps to −1
  CHECK_LL(trt::mul_wrap(t1::from_i64(1), t1::from_i64(1)).to_i64(), 1);
  const auto f = trt::add_flag(t1::from_i64(1), t1::from_i64(1));
  CHECK_LL(f.value.to_i64(), -1);
  CHECK_LL(f.overflow.to_int(), 1);
  CHECK(trt::add_sat(t1::from_i64(1), t1::from_i64(1)) == t1::max_value());
  CHECK(!trt::add_checked(t1::from_i64(1), t1::from_i64(1)).has_value());
}
