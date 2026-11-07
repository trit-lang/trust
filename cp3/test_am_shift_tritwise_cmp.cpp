// cp3/test_am_shift_tritwise_cmp.cpp — AM §3.3 radix shifts, §3.4 trit-wise
// operations and splat, §3.5 comparison, and §4 fault codes.

#include "testharness.hpp"

TEST_CASE(am_3_3_shl, "AM §3.3") {
  using trt::t9, trt::t27;
  for (int k = 0; k < 9; ++k) {
    CHECK_LL(trt::shl_trap(t9::from_i64(1), k).to_i64(), pow3i(k));  // shl(a,k) = a·3^k
    CHECK_LL(trt::shl_wrap(t9::from_i64(1), k).to_i64(), pow3i(k));
    CHECK(trt::shl_wrap(t9::zero(), k) == t9::zero());
  }
  CHECK_LL(trt::shl_trap(t9::max_value(), 0).to_i64(), 9841);  // k = 0 is always fine
  CHECK_LL(trt::shl_wrap(trt::Tint<3>::from_i64(2), 2).to_i64(), wrapi(18, 3));
  CHECK_LL(trt::shl_wrap(t9::max_value(), 1).to_i64(), wrapi(9841 * 3, 9));
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::shl_trap(t9::from_i64(2), 8));   // exact 13122 > MAX
  CHECK_LL(trt::shl_trap(t9::from_i64(2), 7).to_i64(), 4374);
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::shl_trap(t9::max_value(), 1));
  CHECK_LL(trt::shl_wrap(t9::from_i64(2), 8).to_i64(), wrapi(13122, 9));    // wrapping loses high trits
  CHECK_LL(trt::shl_flag(t9::from_i64(2), 8).overflow.to_int(), 1);
  CHECK_LL(trt::shl_flag(t9::from_i64(-2), 8).overflow.to_int(), -1);
  CHECK_LL(trt::shl_trap(trt::t27::from_i64(1), 26).to_i64(), pow3i(26));   // 3^26 fits the word
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::shl_trap(trt::t27::from_i64(2), 26));
}

// "Shift amounts k are values in 0 … n−1 …; k outside that range is a trapping
// fault (not masked, not undefined)" — for both shifts, including negatives
// (TIR §3.1 G2.3: the amount is a same-width value; a negative amount faults).
TEST_CASE(am_3_3_shift_amount_faults, "AM §3.3 + TIR §3.1") {
  using trt::t9, trt::t27;
  const auto a = t9::from_i64(5);
  CHECK_LL(trt::shl_wrap(a, 0).to_i64(), 5);
  CHECK_LL(trt::shr(a, 0).to_i64(), 5);
  CHECK_LL(trt::shl_wrap(a, 8).to_i64(), wrapi(5 * 6561, 9));
  CHECK_LL(trt::shr(a, 8).to_i64(), 0);       // 5/3^8 is tiny → 0
  const long long bad[] = {-1, -2, 9, 10, 100};
  for (long long k : bad) {
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_wrap(a, k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_trap(a, k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_flag(a, k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_checked(a, k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_sat(a, k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shr(a, k));
  }
  const long long bad27[] = {-1, 27, 28};
  for (long long k : bad27) {
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shl_wrap(t27::from_i64(1), k));
    CHECK_FAULT(trt::Fault::F_SHIFT, trt::shr(t27::from_i64(1), k));
  }
}

// "shr(a, k) = div(a, 3ᵏ) — that is, arithmetic right shift rounds to nearest,
// consistent with §3.2 … because 3ᵏ is odd, ties are impossible: shr never
// invokes the tie-break rule."
TEST_CASE(am_3_3_shr_is_division, "AM §3.3") {
  using trt::t9;
  for (int k = 1; k < 9; ++k) {
    const auto p = t9::from_i64(static_cast<long long>(pow3i(k)));
    for (const auto a : boundary_values<9>()) {
      CHECK(trt::shr(a, k) == trt::div(a, p));
      CHECK_LL(trt::shr(a, k).to_i64(), div_rn(a.to_i64(), pow3i(k)));
    }
    Lcg l(0xFEEDFACEull + static_cast<unsigned>(k));
    for (int i = 0; i < 200; ++i) {
      const long long v = l.uniform(-9841, 9841);
      const auto a = t9::from_i64(v);
      CHECK(trt::shr(a, k) == trt::div(a, p));
      CHECK_LL(trt::shr(a, k).to_i64(), div_rn(v, pow3i(k)));
    }
  }
  // Discarding the low k trits of a balanced ternary number *is* round-to-
  // nearest division by 3^k (the truncate-is-round property).
  const auto x = trt::t9::wrap_of(trt::trits_from_string("1T"));      // 1T = 2
  CHECK_LL(trt::shr(x, 1).to_i64(), 1);                              // drop T → 1
  const auto y = trt::t9::wrap_of(trt::trits_from_string("1"));      // 1 = 1
  CHECK_LL(trt::shr(y, 1).to_i64(), 0);                              // drop 1 → 0
}

// AM §3.4: tmin/tmax/tmul/tneg apply *independently to each trit position*;
// the per-trit definitions are −a, min(a,b), max(a,b), a·b.
TEST_CASE(am_3_4_tritwise_per_trit, "AM §3.4") {
  const long long M = llmax(3);
  for (long long ia = -M; ia <= M; ++ia)
    for (long long ib = -M; ib <= M; ++ib) {
      const auto a = trt::Tint<3>::from_i64(ia), b = trt::Tint<3>::from_i64(ib);
      const auto mn = trt::tmin(a, b), mx = trt::tmax(a, b), mu = trt::tmul(a, b);
      for (int i = 0; i < 3; ++i) {
        const int x = a.trit(static_cast<std::size_t>(i)), y = b.trit(static_cast<std::size_t>(i));
        CHECK_LL(mn.trit(static_cast<std::size_t>(i)), std::min(x, y));
        CHECK_LL(mx.trit(static_cast<std::size_t>(i)), std::max(x, y));
        CHECK_LL(mu.trit(static_cast<std::size_t>(i)), x * y);
        // tmul: "nonzero iff both nonzero; sign composes".
        CHECK((mu.trit(static_cast<std::size_t>(i)) != 0) == (x != 0 && y != 0));
      }
    }
  // tmul is *not* multiplication: (−1)ᵏ patterns make that visible.
  const auto ones9 = trt::t9::splat(trt::Bt::from_int(1));
  const auto neg9 = trt::t9::splat(trt::Bt::from_int(-1));
  CHECK(trt::tmul(ones9, neg9) == neg9);
  CHECK(trt::tmul(neg9, neg9) == ones9);
  CHECK(trt::tmul(ones9, trt::t9::from_i64(0)) == trt::t9::from_i64(0));
  // A tmin/tmax example taken from a trit pattern, not from integer order:
  const auto p = trt::t9::wrap_of(trt::trits_from_string("1T01"));
  const auto q = trt::t9::wrap_of(trt::trits_from_string("T100"));
  CHECK(trt::tmin(p, q) == trt::t9::wrap_of(trt::trits_from_string("TT00")));
  CHECK(trt::tmax(p, q) == trt::t9::wrap_of(trt::trits_from_string("1101")));
}

// AM §3.4's splat: "every trit position takes the value of the single trit t".
TEST_CASE(am_3_4_splat_and_widening, "AM §3.4") {
  for (int t = -1; t <= 1; ++t) {
    const auto b = trt::Bt::from_int(t);
    const auto s9 = trt::t9::splat(b);
    const auto s27 = trt::t27::splat(b);
    for (int i = 0; i < 9; ++i) CHECK_LL(s9.trit(static_cast<std::size_t>(i)), t);
    for (int i = 0; i < 27; ++i) CHECK_LL(s27.trit(static_cast<std::size_t>(i)), t);
  }
  CHECK(trt::t9::splat(trt::Bt::from_int(1)) == trt::t9::max_value());
  CHECK(trt::t9::splat(trt::Bt::from_int(-1)) == trt::t9::min_value());
  CHECK(trt::t9::splat(trt::Bt::from_int(0)) == trt::t9::zero());
  // §3.4's justification for splat: widening is value-preserving and pads with
  // zeros, so a one-trit cmp result cannot combine with a word by widening.
  const auto neg1trit = trt::Tint<1>::from_bt(trt::Bt::from_int(-1));
  CHECK_LL((trt::widen<1, 9>(neg1trit).to_i64()), -1);
  CHECK(trt::widen<1, 9>(neg1trit) != trt::t9::splat(trt::Bt::from_int(-1)));
}
