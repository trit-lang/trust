// cp3/test_am_repr.cpp — AM §1.1–§1.3 (representation, symmetric range, the four
// consequences of §1.2, storage-unit ranges) and total neg/abs (AM §3.1).

#include "testharness.hpp"

// The unique n-trit expansion of every value at small widths (AM §1.2), checked
// against the expansion obtained by peeling centered base-3 remainders.
template <int N>
static void unique_expansion_sweep() {
  for (long long v = -llmax(N); v <= llmax(N); ++v) {
    const auto x = trt::Tint<N>::from_i64(v);
    CHECK_LL(x.to_i64(), v);  // "denotes the integer Σ t_i·3^i"
    long long rest = v;
    for (int i = 0; i < N; ++i) {
      long long r = rest % 3;
      if (r > 1) r -= 3;
      else if (r < -1) r += 3;
      CHECK_LL(x.trit(static_cast<std::size_t>(i)), r);
      rest = (rest - r) / 3;
    }
    CHECK_LL(rest, 0);  // representable in exactly N trits, no more
  }
}

TEST_CASE(am_1_1_trit_string_example, "AM §1.1") {
  // "`1T0` denotes (+1)·9 + (−1)·3 + 0 = 6"
  CHECK_LL(trt::detail::t_to_i128(trt::trits_from_string("1T0")), 6);
  // Bt is exactly the three-valued trit: −1, 0, +1.
  CHECK_LL(trt::Bt::from_int(-1).to_int(), -1);
  CHECK_LL(trt::Bt::from_int(0).to_int(), 0);
  CHECK_LL(trt::Bt::from_int(1).to_int(), 1);
  bool threw = false;
  try { (void)trt::Bt::from_int(2); } catch (const std::out_of_range&) { threw = true; }
  CHECK(threw);
  threw = false;
  try { (void)trt::Bt::from_int(-2); } catch (const std::out_of_range&) { threw = true; }
  CHECK(threw);
}

TEST_CASE(am_1_2_unique_representation, "AM §1.2") {
  unique_expansion_sweep<1>();
  unique_expansion_sweep<2>();
  unique_expansion_sweep<3>();
  unique_expansion_sweep<4>();
  unique_expansion_sweep<5>();
}

// "Every n-trit sequence denotes a unique integer in the symmetric range
// −(3^n − 1)/2 … +(3^n − 1)/2" — enumerate all 3^n sequences and require
// exactly that set of integers, each appearing exactly once.
TEST_CASE(am_1_2_bijection, "AM §1.2") {
  for (int n = 1; n <= 4; ++n) {
    std::vector<long long> seen;
    const long long total = static_cast<long long>(pow3i(n));
    for (long long code = 0; code < total; ++code) {
      trt::detail::Trits t;
      long long c = code;
      for (int i = 0; i < n; ++i) {
        t.push_back(static_cast<int8_t>(c % 3 - 1));
        c /= 3;
      }
      const long long v = static_cast<long long>(trt::detail::t_to_i128(t));
      CHECK(v >= -llmax(n) && v <= llmax(n));
      seen.push_back(v);
    }
    std::sort(seen.begin(), seen.end());
    bool complete = static_cast<long long>(seen.size()) == total;
    for (long long i = 0; i < total; ++i)
      complete = complete && seen[static_cast<std::size_t>(i)] == -llmax(n) + i;
    CHECK(complete);
  }
}

TEST_CASE(am_1_3_storage_unit_ranges, "AM §1.3") {
  CHECK_LL(trt::t9::max_value().to_i64(), 9841);       // tryte: −9841 … +9841
  CHECK_LL(trt::t9::min_value().to_i64(), -9841);
  CHECK_LL(trt::t27::max_value().to_i64(), 3812798742493LL);
  CHECK_LL(trt::t27::min_value().to_i64(), -3812798742493LL);
  // min/max of every width are symmetric (AM §1.2 consequence 2).
  CHECK(trt::t9::min_value() == trt::neg(trt::t9::max_value()));
  CHECK(trt::t27::min_value() == trt::neg(trt::t27::max_value()));
  // "No distinct representation of negative zero."
  CHECK(trt::neg(trt::t9::zero()) == trt::t9::zero());
  CHECK(trt::to_trit_string(trt::t9::zero().trits()) == "0");
  // A value that does not fit its width is rejected, never silently wrapped.
  bool threw = false;
  try { (void)trt::t9::from_i64(9842); } catch (const std::out_of_range&) { threw = true; }
  CHECK(threw);
  threw = false;
  try { (void)trt::t9::from_i64(-9842); } catch (const std::out_of_range&) { threw = true; }
  CHECK(threw);
}

// Consequence 2 (negation total, MIN = −MAX), consequence 3 (negation is
// trit-wise, and tneg is the same operation per AM §3.4), consequence 4 (sign
// = sign of the most significant nonzero trit), and §3.1's total abs.
template <int N>
static void consequences_sweep() {
  using T = trt::Tint<N>;
  const long long M = llmax(N);
  for (long long v = -M; v <= M; ++v) {
    const T x = T::from_i64(v);
    const T nx = trt::neg(x);
    CHECK_LL(nx.to_i64(), -v);                 // total, never overflows
    CHECK(trt::neg(nx) == x);
    for (int i = 0; i < N; ++i)                // trit-wise, carry-free
      CHECK_LL(nx.trit(static_cast<std::size_t>(i)), -x.trit(static_cast<std::size_t>(i)));
    CHECK(trt::tneg(x) == nx);                 // AM §3.4 closing paragraph
    CHECK_LL(trt::cmp(x, T::zero()).to_int(), (v > 0) - (v < 0));
    CHECK_LL(trt::abs(x).to_i64(), v < 0 ? -v : v);
  }
  CHECK(trt::neg(T::max_value()) == T::min_value());
  CHECK(trt::neg(T::min_value()) == T::max_value());
  CHECK(trt::abs(T::min_value()) == T::max_value());  // "abs(MIN) negative" cannot occur
  CHECK(trt::abs(T::max_value()) == T::max_value());
  // The sign of a value is the sign of its most significant nonzero trit.
  for (int i = 0; i < N; ++i) {
    trt::detail::Trits p(static_cast<std::size_t>(N), 0);
    p[static_cast<std::size_t>(i)] = 1;
    CHECK_LL(trt::cmp(T::wrap_of(p), T::zero()).to_int(), 1);
    p[static_cast<std::size_t>(i)] = -1;
    CHECK_LL(trt::cmp(T::wrap_of(p), T::zero()).to_int(), -1);
  }
}

TEST_CASE(am_1_2_consequences_2_3_4, "AM §1.2 + §3.1") {
  consequences_sweep<1>();
  consequences_sweep<2>();
  consequences_sweep<3>();
  consequences_sweep<4>();
  consequences_sweep<5>();
  consequences_sweep<9>();   // full tryte range, 19683 values
  // t27 is NOT swept here: 7.6e12 values is too many.  See the sampled case next.
}

// t27 / t81 fixed-seed samples and boundary values for the same consequences
// (t81 additionally exercises widths no host integer can express, per TIR §2).
TEST_CASE(am_1_2_consequences_sampled_t27, "AM §1.2 + §3.1") {
  std::vector<trt::t27> xs;
  sample_values<27>(200, xs);
  for (const auto x : xs) {
    CHECK(trt::neg(trt::neg(x)) == x);
    CHECK(trt::tneg(x) == trt::neg(x));
    CHECK(trt::tneg(trt::tneg(x)) == x);
    CHECK_LL(trt::neg(x).to_i64(), -x.to_i64());
    CHECK_LL(trt::abs(x).to_i64(), x.to_i64() < 0 ? -x.to_i64() : x.to_i64());
    CHECK_LL(trt::cmp(x, trt::t27::zero()).to_int(), (x.to_i64() > 0) - (x.to_i64() < 0));
  }
  // t81 (reserved as a language name but a legal tN width in TIR §2): the same
  // total-negation/abs facts hold at widths the host integer cannot express.
  const auto w = trt::Tint<81>::wrap_of(trt::trits_from_string("1T000111TTT01T1T000111TTT01T"));
  CHECK(trt::neg(trt::neg(w)) == w);
  CHECK(trt::tneg(trt::tneg(w)) == w);
  CHECK(trt::abs(trt::Tint<81>::min_value()) == trt::Tint<81>::max_value());
  CHECK(trt::neg(trt::Tint<81>::min_value()) == trt::Tint<81>::max_value());
  CHECK(trt::Tint<81>::splat(trt::Bt::from_int(1)) == trt::Tint<81>::max_value());
  CHECK(trt::Tint<81>::splat(trt::Bt::from_int(-1)) == trt::Tint<81>::min_value());
}

TEST_CASE(lang_2_trit_bool_projections, "Lang Ch.1 §2, §6") {
  // b.to_trit(): false→0t, true→1t.  trit→bool has no `as` path, only predicates.
  CHECK_LL(trt::bt_from_bool(false).to_int(), 0);
  CHECK_LL(trt::bt_from_bool(true).to_int(), 1);
  CHECK(trt::is_pos(trt::Bt::from_int(1)) && !trt::is_pos(trt::Bt::from_int(0)));
  CHECK(trt::is_neg(trt::Bt::from_int(-1)) && !trt::is_neg(trt::Bt::from_int(1)));
  CHECK(trt::is_zero(trt::Bt::from_int(0)));
  // trit as wider integer is value-preserving (Ch.1 §2: semantically the 1-trit int).
  for (int t = -1; t <= 1; ++t) {
    const auto b = trt::Bt::from_int(t);
    CHECK_LL(trt::t9::from_bt(b).to_i64(), t);
    CHECK_LL(trt::t27::from_bt(b).to_i64(), t);
    CHECK(trt::widen<1, 9>(trt::Tint<1>::from_bt(b)) == trt::t9::from_bt(b));
    CHECK(trt::widen<1, 27>(trt::Tint<1>::from_bt(b)) == trt::t27::from_bt(b));
  }
  // Per-trit trit-wise definitions (AM §3.4) on the single trit type.
  for (int a = -1; a <= 1; ++a)
    for (int b = -1; b <= 1; ++b) {
      const auto x = trt::Bt::from_int(a), y = trt::Bt::from_int(b);
      CHECK_LL(trt::tmin(x, y).to_int(), std::min(a, b));
      CHECK_LL(trt::tmax(x, y).to_int(), std::max(a, b));
      CHECK_LL(trt::tmul(x, y).to_int(), a * b);
      CHECK_LL(trt::cmp(x, y).to_int(), (a > b) - (a < b));
    }
  CHECK_LL(trt::tneg(trt::Bt::from_int(-1)).to_int(), 1);
}
