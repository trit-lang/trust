// cp3/test_am_div.cpp — AM §3.2: the single round-to-nearest division and
// remainder, its tie-break and symmetry identities, faults, and worked examples
// (the same examples appear in Lang Ch.1 §4 and ISA Assembly §4.3).

#include "testharness.hpp"

TEST_CASE(am_3_2_worked_examples, "AM §3.2, Lang Ch.1 §4, Assembly §4.3") {
  using trt::t27;
  auto x = [](long long v) { return t27::from_i64(v); };
  // "7 / 2 == 4 (nearest, tie away from zero), 8 / 3 == 3, -8 / 3 == -3.
  //  The remainder … may differ in sign from the dividend: 8 % 3 == -1."
  CHECK_LL(trt::div(x(7), x(2)).to_i64(), 4);
  CHECK_LL(trt::div(x(8), x(3)).to_i64(), 3);
  CHECK_LL(trt::div(x(-8), x(3)).to_i64(), -3);
  CHECK_LL(trt::rem(x(8), x(3)).to_i64(), -1);
  CHECK_LL(trt::rem(x(7), x(2)).to_i64(), -1);
  CHECK_LL(trt::div(x(-7), x(2)).to_i64(), -4);
  CHECK_LL(trt::rem(x(-7), x(2)).to_i64(), 1);
  CHECK_LL(trt::div(x(8), x(-3)).to_i64(), -3);
  CHECK_LL(trt::rem(x(8), x(-3)).to_i64(), -1);   // rem(a,−b) = rem(a,b)
  CHECK_LL(trt::div(x(0), x(5)).to_i64(), 0);
  CHECK_LL(trt::rem(x(0), x(5)).to_i64(), 0);
  CHECK_LL(trt::rem(x(2), x(2)).to_i64(), 0);
  // "x >> k equals x / 3ᵏ under round-to-nearest with no tie case ever arising."
  CHECK_LL(trt::shr(x(8), 1).to_i64(), 3);
  CHECK_LL(trt::shr(x(-8), 1).to_i64(), -3);
  CHECK_LL(trt::shr(x(7), 1).to_i64(), 2);
  CHECK_LL(trt::shr(x(2), 1).to_i64(), 1);    // 2/3 rounds up (not truncation!)
  CHECK_LL(trt::shr(x(-2), 1).to_i64(), -1);
  CHECK_LL(trt::shr(x(1), 1).to_i64(), 0);
  CHECK_LL(trt::shr(x(-1), 1).to_i64(), 0);
}

TEST_CASE(am_3_2_ties_away_from_zero, "AM §3.2") {
  using trt::t9;
  auto x = [](long long v) { return t9::from_i64(v); };
  // b = 2: every odd a gives a tie at ±0.5; the quotient steps away from zero.
  for (long long a = -99; a <= 99; a += 2) {
    const long long sign = a > 0 ? 1 : -1;
    CHECK_LL(trt::div(x(a), x(2)).to_i64(), (a + sign) / 2);
    CHECK_LL(trt::rem(x(a), x(2)).to_i64(), -sign);   // |r| = |b|/2 = 1
  }
  CHECK_LL(trt::div(x(1), x(2)).to_i64(), 1);
  CHECK_LL(trt::rem(x(1), x(2)).to_i64(), -1);
  CHECK_LL(trt::div(x(-1), x(2)).to_i64(), -1);
  CHECK_LL(trt::rem(x(-1), x(2)).to_i64(), 1);
  CHECK_LL(trt::div(x(5), x(2)).to_i64(), 3);
  CHECK_LL(trt::div(x(-5), x(2)).to_i64(), -3);
  // b = 4 ties: 2/4 = 0.5 → 1 away from zero, remainder −2 with |r| = |b|/2.
  CHECK_LL(trt::div(x(2), x(4)).to_i64(), 1);
  CHECK_LL(trt::rem(x(2), x(4)).to_i64(), -2);
  CHECK_LL(trt::div(x(-2), x(4)).to_i64(), -1);
  CHECK_LL(trt::rem(x(-2), x(4)).to_i64(), 2);
  CHECK_LL(trt::div(x(6), x(4)).to_i64(), 2);    // 1.5 → 2
  CHECK_LL(trt::rem(x(6), x(4)).to_i64(), -2);
  // Even-b non-tie cases round to nearest (these are not at the half point).
  CHECK_LL(trt::div(x(7), x(4)).to_i64(), 2);    // 1.75 → 2
  CHECK_LL(trt::rem(x(7), x(4)).to_i64(), -1);
  CHECK_LL(trt::div(x(5), x(4)).to_i64(), 1);    // 1.25 → 1
  CHECK_LL(trt::rem(x(5), x(4)).to_i64(), 1);
  // Odd-b divisions can never tie (3^k odd, see §3.3); at exact halves there
  // is nothing to decide, and nearest still applies: 4/3 = 1.33 → 1, 5/3 → 2.
  CHECK_LL(trt::div(x(4), x(3)).to_i64(), 1);
  CHECK_LL(trt::div(x(5), x(3)).to_i64(), 2);
  CHECK_LL(trt::rem(x(5), x(3)).to_i64(), -1);
}

// The identities of AM §3.2, each for one (a,b) pair:
//   div(−a, b) = div(a, −b) = −div(a, b)
//   rem(−a, b) = −rem(a, b)
//   a == div(a,b)·b + rem(a,b)
//   |r| ≤ |b|/2
// Plus the nearest-quotient property against the independent host oracle.
template <int N>
static void div_identities_for(trt::Tint<N> a, trt::Tint<N> b) {
  const auto q = trt::div(a, b);
  const auto r = trt::rem(a, b);
  CHECK(trt::div(trt::neg(a), b) == trt::neg(q));
  CHECK(trt::div(a, trt::neg(b)) == trt::neg(q));
  CHECK(trt::rem(trt::neg(a), b) == trt::neg(r));
  CHECK(trt::rem(trt::neg(a), trt::neg(b)) == trt::neg(r));
  const i128 ia = a.to_i64(), ib = b.to_i64();
  CHECK(static_cast<i128>(q.to_i64()) * ib + r.to_i64() == ia);
  const i128 ir = r.to_i64() < 0 ? -static_cast<i128>(r.to_i64()) : static_cast<i128>(r.to_i64());
  const i128 iab = ib < 0 ? -ib : ib;
  CHECK(2 * ir <= iab);                          // |r| ≤ |b|/2
  CHECK(static_cast<i128>(q.to_i64()) == div_rn(ia, ib));
  CHECK(static_cast<i128>(r.to_i64()) == rem_rn(ia, ib));
}

template <int N>
static void div_identities_exhaustive() {
  const long long M = llmax(N);
  for (long long ia = -M; ia <= M; ++ia)
    for (long long ib = -M; ib <= M; ++ib) {
      if (ib == 0) continue;
      div_identities_for<N>(trt::Tint<N>::from_i64(ia), trt::Tint<N>::from_i64(ib));
    }
}

TEST_CASE(am_3_2_identities_exhaustive_small, "AM §3.2") {
  div_identities_exhaustive<3>();
  div_identities_exhaustive<4>();
  div_identities_exhaustive<5>();
}

TEST_CASE(am_3_2_identities_t9_all_dividends, "AM §3.2") {
  // Every t9 dividend against divisors chosen to hit both signs, even/odd
  // (tie/no-tie), 1, and the boundary magnitudes.
  const long long bs[] = {-9841, -101, -6, -4, -3, -2, -1, 1, 2, 3, 4, 6, 101, 9841};
  for (long long ia = -9841; ia <= 9841; ++ia)
    for (long long ib : bs)
      div_identities_for<9>(trt::t9::from_i64(ia), trt::t9::from_i64(ib));
}

TEST_CASE(am_3_2_identities_t27_samples, "AM §3.2") {
  const long long bs[] = {-3812798742493LL, -9841, -6, -4, -3, -2, -1,
                          1, 2, 3, 4, 6, 9841, 3812798742493LL};
  for (const auto a : boundary_values<27>())
    for (long long ib : bs)
      div_identities_for<27>(a, trt::t27::from_i64(ib));
  Lcg l(0xC0FFEEull);
  const long long M = llmax(27);
  for (int i = 0; i < 3000; ++i) {
    const long long ia = l.uniform(-M, M);
    long long ib = l.uniform(-M, M);
    if (ib == 0) ib = 1;
    div_identities_for<27>(trt::t27::from_i64(ia), trt::t27::from_i64(ib));
  }
}

TEST_CASE(am_3_2_divzero_faults, "AM §3.2 + §4 (F_DIVZERO)") {
  using trt::t9, trt::t27;
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::div(t9::from_i64(5), t9::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::rem(t9::from_i64(5), t9::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::div(t9::zero(), t9::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::rem(t9::zero(), t9::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::div(t9::min_value(), t9::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::div(trt::t27::from_i64(5), t27::zero()));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::rem(trt::t27::from_i64(-5), t27::zero()));
}

TEST_CASE(am_3_2_min_div_neg1_is_max, "AM §3.2, Lang Ch.1 appendix") {
  // "There is no representable-overflow case for division: since MIN = −MAX,
  // div(MIN, −1) is simply MAX and is well-defined."
  CHECK(trt::div(trt::t9::min_value(), trt::t9::from_i64(-1)) == trt::t9::max_value());
  CHECK(trt::div(trt::t27::min_value(), trt::t27::from_i64(-1)) == trt::t27::max_value());
  CHECK(trt::rem(trt::t9::min_value(), trt::t9::from_i64(-1)) == trt::t9::zero());
  CHECK(trt::rem(trt::t27::min_value(), trt::t27::from_i64(-1)) == trt::t27::zero());
  CHECK(trt::div(trt::Tint<1>::from_i64(-1), trt::Tint<1>::from_i64(-1)) == trt::Tint<1>::from_i64(1));
  CHECK(trt::div(trt::Tint<1>::from_i64(1), trt::Tint<1>::from_i64(-1)) == trt::Tint<1>::from_i64(-1));
}
