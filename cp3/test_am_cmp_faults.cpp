// cp3/test_am_cmp_faults.cpp — AM §3.5 comparison (one primitive three-way cmp,
// two-way predicates as projections) and the AM §4 fault-code surface.

#include "testharness.hpp"

TEST_CASE(am_3_5_cmp_primitive, "AM §3.5 + Lang Ch.1 §5") {
  auto sweep = [](auto tag) {
    constexpr int N = decltype(tag)::value;
    using T = trt::Tint<N>;
    const long long M = llmax(N);
    for (long long ia = -M; ia <= M; ++ia)
      for (long long ib = -M; ib <= M; ++ib) {
        const auto a = T::from_i64(ia), b = T::from_i64(ib);
        const auto c = trt::cmp(a, b);
        CHECK(c.to_int() >= -1 && c.to_int() <= 1);   // cmp yields a single trit
        CHECK_LL(c.to_int(), (ia > ib) - (ia < ib));
        CHECK_LL(trt::cmp(b, a).to_int(), -c.to_int());
        if (ia == ib) CHECK_LL(c.to_int(), 0);
        // The six two-way predicates are projections of cmp.
        CHECK((a < b) == is_neg(c));
        CHECK((a > b) == is_pos(c));
        CHECK((a <= b) == !is_pos(c));
        CHECK((a >= b) == !is_neg(c));
        CHECK((a == b) == is_zero(c));
        CHECK((a != b) == !is_zero(c));
      }
  };
  sweep(width<3>{});
  sweep(width<4>{});
  sweep(width<5>{});
}

TEST_CASE(am_3_5_cmp_t27_samples, "AM §3.5") {
  std::vector<trt::t27> xs;
  sample_values<27>(80, xs);
  for (const auto a : xs)
    for (const auto b : xs) {
      const auto c = trt::cmp(a, b);
      CHECK_LL(c.to_int(), (a.to_i64() > b.to_i64()) - (a.to_i64() < b.to_i64()));
      CHECK((a < b) == (a.to_i64() < b.to_i64()));
      CHECK((a <= b) == (a.to_i64() <= b.to_i64()));
      CHECK((a > b) == (a.to_i64() > b.to_i64()));
      CHECK((a >= b) == (a.to_i64() >= b.to_i64()));
    }
  // sign(x) ≡ x <=> 0 (Lang Ch.1 §6) matches the most-significant-nonzero-trit
  // reading (AM §1.2 consequence 4).
  for (const auto a : xs)
    CHECK_LL(trt::cmp(a, trt::t27::zero()).to_int(), trt::detail::tsign(a.trits()));
}

// AM §4: every fault the spec enumerates carries its own code.  F_ALIGN belongs
// to misaligned *memory* access and is therefore not raised by this arithmetic
// core (not one call here touches memory — see NOTES.md).  F_ILLEGAL/F_ADDRESS
// are reserved, raised by nothing in draft 0.1.
TEST_CASE(am_4_fault_codes, "AM §4") {
  using trt::t9;
  const auto one = t9::from_i64(1);
  CHECK_FAULT(trt::Fault::F_OVERFLOW, trt::add_trap(t9::max_value(), one));
  CHECK_FAULT(trt::Fault::F_DIVZERO, trt::rem(one, t9::zero()));
  CHECK_FAULT(trt::Fault::F_SHIFT, trt::shr(one, 9));
  CHECK_FAULT(trt::Fault::F_TRAP, trt::trap());
  CHECK_FAULT(trt::Fault::F_TRAP, trt::trap<trt::t9>());  // type `!`: any T works
  CHECK(std::string(trt::fault_name(trt::Fault::F_OVERFLOW)) == "F_OVERFLOW");
  CHECK(std::string(trt::fault_name(trt::Fault::F_DIVZERO)) == "F_DIVZERO");
  CHECK(std::string(trt::fault_name(trt::Fault::F_SHIFT)) == "F_SHIFT");
  CHECK(std::string(trt::fault_name(trt::Fault::F_ALIGN)) == "F_ALIGN");
  CHECK(std::string(trt::fault_name(trt::Fault::F_TRAP)) == "F_TRAP");
  // The reserved codes exist in the vocabulary but nothing here raises them
  // (AM §4: "decided, claimed, raised by nothing in draft 0.1").
  CHECK(std::string(trt::fault_name(trt::Fault::F_ILLEGAL)) == "F_ILLEGAL");
  CHECK(std::string(trt::fault_name(trt::Fault::F_ADDRESS)) == "F_ADDRESS");
  // Wrapping arithmetic never raises F_OVERFLOW; total ops never raise at all.
  bool any = false;
  try {
    for (long long a = -13; a <= 13; ++a)
      for (long long b = -13; b <= 13; ++b) {
        const auto x = trt::Tint<3>::from_i64(a), y = trt::Tint<3>::from_i64(b);
        (void)trt::add_wrap(x, y);
        (void)trt::sub_wrap(x, y);
        (void)trt::mul_wrap(x, y);
        (void)trt::neg(x);
        (void)trt::abs(x);
        (void)trt::mulh(x, y);
        if (b != 0) { (void)trt::div(x, y); (void)trt::rem(x, y); }
      }
  } catch (const trt::MachineFault&) { any = true; }
  CHECK(!any);
}
