// cp3/core.hpp — balanced-ternary arithmetic core.
//
// Implemented from the specification alone:
//   spec/00-abstract-machine.md   (AM)        — trit, tN values, arithmetic, faults
//   spec/01-types.md              (Lang Ch.1) — t9/t27/taddr, flavors, mulh, literals context
//   spec/tir-0.1.md               (TIR)       — arbitrary tN, .wrap/.trap/.flag, mulh, shifts
//
// Representation note: the AM is explicit that it "is not a description of any
// implementation" and requires only observable-behavior equivalence.  Values are
// held as their unique balanced-ternary expansion (AM §1.2: exactly one n-trit
// representation per integer of the symmetric range), least-significant trit
// first so that index i is t_i of the spec's Σ t_i·3^i.

#pragma once

#include <array>
#include <cstdint>
#include <optional>
#include <span>
#include <stdexcept>
#include <vector>

namespace trt {

// ---------------------------------------------------------------------------
// Faults (AM §4).  A fault is "a defined, observable machine halt with a fault
// code"; in this library the halt is realized as a C++ exception carrying the
// code, which is the observable the tests check.
// ---------------------------------------------------------------------------
enum class Fault {
  F_OVERFLOW,  // trapping-flavor arithmetic overflow (AM §3.1)
  F_DIVZERO,   // division or remainder by zero (AM §3.2)
  F_SHIFT,     // shift amount out of range (AM §3.3)
  F_ALIGN,     // misaligned access (AM §2.3) — memory is outside this core's
               // scope; declared for completeness, never raised here.
  F_TRAP,      // explicit trap instruction (AM §4)
  // Reserved: "decided, claimed, raised by nothing in draft 0.1" (AM §4).
  F_ILLEGAL,   // reserved
  F_ADDRESS,   // reserved
};

inline const char* fault_name(Fault f) {
  switch (f) {
    case Fault::F_OVERFLOW: return "F_OVERFLOW";
    case Fault::F_DIVZERO:  return "F_DIVZERO";
    case Fault::F_SHIFT:    return "F_SHIFT";
    case Fault::F_ALIGN:    return "F_ALIGN";
    case Fault::F_TRAP:     return "F_TRAP";
    case Fault::F_ILLEGAL:  return "F_ILLEGAL";
    case Fault::F_ADDRESS:  return "F_ADDRESS";
  }
  return "F_UNKNOWN";
}

class MachineFault : public std::exception {
 public:
  explicit MachineFault(Fault c) : code_(c) {}
  Fault code() const { return code_; }
  const char* what() const noexcept override { return fault_name(code_); }
 private:
  Fault code_;
};

// Language Ch.1 §6: trap() stops the program; its type is ! — a value of no
// type may appear wherever any type is wanted, so trap<T>() is well-formed
// (and never returns) for any result type T.
template <class T = void>
[[noreturn]] T trap() { throw MachineFault(Fault::F_TRAP); }

// ---------------------------------------------------------------------------
// Bt — the single trit (AM §1.1): exactly one of −1, 0, +1.
// Written T / 0 / 1 in the spec's notation (see literals.hpp for parsing).
// ---------------------------------------------------------------------------
class Bt {
 public:
  constexpr Bt() = default;
  static constexpr Bt from_int(int v) {
    if (v < -1 || v > 1)
      throw std::out_of_range("Bt: a trit is -1, 0 or +1 (AM §1.1)");
    return Bt(static_cast<int8_t>(v));
  }
  constexpr int to_int() const { return v_; }
  friend constexpr bool operator==(Bt, Bt) = default;
 private:
  explicit constexpr Bt(int8_t v) : v_(v) {}
  int8_t v_ = 0;
};

// AM §3.4 trit-wise operations on a single trit.
constexpr Bt tneg(Bt t) { return Bt::from_int(-t.to_int()); }
constexpr Bt tmin(Bt a, Bt b) { return Bt::from_int(a.to_int() < b.to_int() ? a.to_int() : b.to_int()); }
constexpr Bt tmax(Bt a, Bt b) { return Bt::from_int(a.to_int() > b.to_int() ? a.to_int() : b.to_int()); }
constexpr Bt tmul(Bt a, Bt b) { return Bt::from_int(a.to_int() * b.to_int()); }
// neg is tneg for a trit (AM §3.4 closing paragraph); abs is total (AM §3.1).
constexpr Bt neg(Bt t) { return tneg(t); }
constexpr Bt abs(Bt t) { return Bt::from_int(t.to_int() < 0 ? -t.to_int() : t.to_int()); }
// AM §3.5: three-way comparison yields a trit.
constexpr Bt cmp(Bt a, Bt b) {
  return a.to_int() < b.to_int() ? Bt::from_int(-1)
       : a.to_int() > b.to_int() ? Bt::from_int(+1)
                                 : Bt::from_int(0);
}
// Language Ch.1 §6 sanctioned trit↔bool bridge (no `as` path exists).
constexpr bool is_pos(Bt t)  { return t.to_int() > 0; }
constexpr bool is_zero(Bt t) { return t.to_int() == 0; }
constexpr bool is_neg(Bt t)  { return t.to_int() < 0; }
constexpr Bt bt_from_bool(bool b) { return Bt::from_int(b ? 1 : 0); }  // b.to_trit(): false→0t, true→1t

namespace detail {

// Unbounded exact balanced-ternary integer: trits t[0] (units) … t[n-1].
// Every finite sequence denotes Σ t_i·3^i (AM §1.2); the empty sequence is 0.
// Sequences need not be normalized (trailing zeros denote the same value);
// tlen/tsign/tcmp look past them.
using Trits    = std::vector<int8_t>;
using TritSpan = std::span<const int8_t>;

// Sign = sign of the most significant *nonzero* trit (AM §1.2, consequence 4).
inline int tsign(TritSpan t) {
  for (std::size_t i = t.size(); i-- > 0;)
    if (t[i] != 0) return t[i];
  return 0;
}

inline std::size_t tlen(TritSpan t) {  // length with trailing zero trits stripped
  std::size_t n = t.size();
  while (n > 0 && t[n - 1] == 0) --n;
  return n;
}

inline void tnorm(Trits& t) { t.resize(tlen(t)); }

inline int tcmp(TritSpan a, TritSpan b) {
  const int sa = tsign(a), sb = tsign(b);
  if (sa != sb) return sa < sb ? -1 : 1;
  if (sa == 0) return 0;
  const std::size_t na = tlen(a), nb = tlen(b);
  if (na != nb) {  // same sign: longer expansion = larger magnitude
    const bool a_shorter = na < nb;
    return (sa > 0) ? (a_shorter ? -1 : +1) : (a_shorter ? +1 : -1);
  }
  for (std::size_t i = na; i-- > 0;)   // most significant differing trit decides
    if (a[i] != b[i]) return a[i] < b[i] ? -1 : +1;
  return 0;
}

inline Trits tneg(TritSpan a) {
  Trits r(a.begin(), a.end());
  for (auto& x : r) x = static_cast<int8_t>(-x);   // AM §1.2 consequence 3
  return r;
}
inline Trits tabs(TritSpan a) { return tsign(a) < 0 ? tneg(a) : Trits(a.begin(), a.end()); }

// Split v into a balanced trit d ∈ {-1,0,+1} and carry c with v = 3c + d.
inline int8_t center3(long long v, long long& c) {
  int d = static_cast<int>(v % 3);
  if (d > 1) d -= 3;
  else if (d < -1) d += 3;
  c = (v - d) / 3;
  return static_cast<int8_t>(d);
}

inline Trits tadd(TritSpan a, TritSpan b) {
  const std::size_t n = a.size() > b.size() ? a.size() : b.size();
  Trits r;
  r.reserve(n + 1);
  long long c = 0;
  for (std::size_t i = 0; i < n; ++i) {
    const long long v = (i < a.size() ? a[i] : 0) + (i < b.size() ? b[i] : 0) + c;
    r.push_back(center3(v, c));
  }
  if (c != 0) r.push_back(static_cast<int8_t>(c));
  return r;
}
inline Trits tsub(TritSpan a, TritSpan b) { return tadd(a, tneg(b)); }

inline Trits tmul(TritSpan a, TritSpan b) {
  if (tsign(a) == 0 || tsign(b) == 0) return {};
  std::vector<long long> acc(a.size() + b.size() + 1, 0);
  for (std::size_t i = 0; i < a.size(); ++i)
    for (std::size_t j = 0; j < b.size(); ++j)
      acc[i + j] += static_cast<long long>(a[i]) * b[j];
  Trits r;
  r.reserve(acc.size() + 1);
  long long c = 0;
  for (long long v0 : acc) {
    const long long v = v0 + c;
    r.push_back(center3(v, c));
  }
  while (c != 0) r.push_back(center3(c, c));
  tnorm(r);
  return r;
}

inline void tadd_i32(Trits& a, std::int32_t w) {  // in-place add of a small constant
  long long c = w;
  std::size_t i = 0;
  while (c != 0) {
    if (i == a.size()) a.push_back(0);
    const long long v = a[i] + c;
    a[i] = center3(v, c);
    ++i;
  }
}

inline Trits tshl(TritSpan a, int k) {  // a·3^k, exact: k zero trits enter at the bottom
  Trits r(static_cast<std::size_t>(k), 0);
  r.insert(r.end(), a.begin(), a.end());
  return r;
}
inline Trits tpow3(int k) { Trits r(static_cast<std::size_t>(k), 0); r.push_back(1); return r; }

// True iff the value is representable in n trits, i.e. lies in the symmetric
// range −(3^n−1)/2 … +(3^n−1)/2 (AM §1.2): all trits above position n−1 vanish.
inline bool tfits(TritSpan t, int n) {
  for (std::size_t i = static_cast<std::size_t>(n); i < t.size(); ++i)
    if (t[i] != 0) return false;
  return true;
}

// AM §3.1 overflow trit: +1 when the exact result exceeded MAX, −1 when it fell
// below MIN, 0 when there was no overflow.
inline int toverflow_trit(TritSpan t, int n) {
  return tfits(t, n) ? 0 : tsign(t);
}

// ---- unbalanced base-3 (digits {0,1,2}), used only for division -------------

using U3 = std::vector<int>;

inline void unorm(U3& u) { while (!u.empty() && u.back() == 0) u.pop_back(); }

inline int ucmp(const U3& a, const U3& b) {
  U3 x = a, y = b;
  unorm(x); unorm(y);
  if (x.size() != y.size()) return x.size() < y.size() ? -1 : +1;
  for (std::size_t i = x.size(); i-- > 0;)
    if (x[i] != y[i]) return x[i] < y[i] ? -1 : +1;
  return 0;
}

inline U3 uadd(const U3& a, const U3& b) {
  U3 r;
  int c = 0;
  const std::size_t n = a.size() > b.size() ? a.size() : b.size();
  for (std::size_t i = 0; i < n; ++i) {
    const int v = (i < a.size() ? a[i] : 0) + (i < b.size() ? b[i] : 0) + c;
    r.push_back(v % 3);
    c = v / 3;
  }
  if (c) r.push_back(c);
  return r;
}

inline U3 usub(const U3& a, const U3& b) {  // requires a ≥ b
  U3 r(a.size(), 0);
  int borrow = 0;
  for (std::size_t i = 0; i < a.size(); ++i) {
    int v = a[i] - (i < b.size() ? b[i] : 0) - borrow;
    if (v < 0) { v += 3; borrow = 1; } else borrow = 0;
    r[i] = v;
  }
  unorm(r);
  return r;
}

inline U3 balanced_to_u3(TritSpan t) {  // requires t ≥ 0
  U3 u;
  Trits cur(t.begin(), t.end());
  while (tsign(cur) != 0) {
    const int t0 = cur.empty() ? 0 : cur[0];
    const int digit = t0 < 0 ? t0 + 3 : t0;  // ≡ t0 (mod 3), in {0,1,2}
    u.push_back(digit);
    tadd_i32(cur, -digit);                   // now divisible by 3
    cur.erase(cur.begin());                  // exact ÷3: drop the zero low trit
  }
  unorm(u);
  return u;
}

inline Trits u3_to_balanced(const U3& u) {
  Trits r;
  int c = 0;
  for (int d : u) {
    const int v = d + c;
    if (v <= 1)      { r.push_back(static_cast<int8_t>(v)); c = 0; }
    else if (v == 2) { r.push_back(-1); c = 1; }
    else             { r.push_back(0);  c = 1; }  // v == 3
  }
  if (c) r.push_back(1);
  return r;
}

inline std::pair<U3, U3> udivmod3(const U3& x, const U3& d) {  // x ≥ 0, d > 0, floor
  U3 q(x.size(), 0), r;
  const U3 d2 = uadd(d, d);
  for (std::size_t i = x.size(); i-- > 0;) {
    r.insert(r.begin(), x[i]);  // r = 3r + x_i
    unorm(r);
    int digit = 0;
    if (ucmp(r, d2) >= 0)      { digit = 2; r = usub(r, d2); }
    else if (ucmp(r, d) >= 0)  { digit = 1; r = usub(r, d);  }
    q[i] = digit;
  }
  unorm(q);
  return {q, r};
}

// AM §3.2: q is the integer nearest a/b; ties (|r| = |b|/2, possible only for
// even b) round away from zero.  r = a − q·b, so a = q·b + r with |r| ≤ |b|/2.
inline std::pair<Trits, Trits> tdivmod_nearest(TritSpan a, TritSpan b) {
  if (tsign(b) == 0) throw MachineFault(Fault::F_DIVZERO);
  if (tsign(a) == 0) return {Trits{}, Trits{}};
  const int sa = tsign(a), sb = tsign(b);
  const Trits A = tabs(a), B = tabs(b);
  auto [qu, ru] = udivmod3(balanced_to_u3(A), balanced_to_u3(B));
  (void)ru;
  Trits qmag = u3_to_balanced(qu);              // floor(|a|/|b|)
  const Trits r0 = tsub(A, tmul(qmag, B));      // floor remainder, 0 ≤ r0 < B
  // nearest = round up in magnitude iff 2·r0 ≥ B; == is the tie, away from zero.
  if (tcmp(tadd(r0, r0), B) >= 0) tadd_i32(qmag, 1);
  Trits q = (sa == sb) ? qmag : tneg(qmag);     // div(−a,b)=div(a,−b)=−div(a,b)
  Trits r = tsub(a, tmul(q, Trits(b.begin(), b.end())));
  tnorm(q);
  tnorm(r);
  return {q, r};
}

// Host-integer bridges (convenience; the AM semantics never depend on these).
inline Trits t_from_i128(__int128 v) {
  Trits r;
  while (v != 0) {
    int d = static_cast<int>(v % 3);
    if (d > 1) d -= 3;
    else if (d < -1) d += 3;
    r.push_back(static_cast<int8_t>(d));
    v = (v - d) / 3;
  }
  return r;
}
inline __int128 t_to_i128(TritSpan t) {
  __int128 v = 0;
  for (std::size_t i = t.size(); i-- > 0;) v = v * 3 + t[i];
  return v;
}

}  // namespace detail

// ---------------------------------------------------------------------------
// Tint<N> — the width-N signed balanced-ternary integer, N ≥ 1 (TIR §2: tN is
// arbitrary-width; Lang Ch.1 §2 instantiates t9, t27, taddr; the trit is N = 1).
// Invariant: each of the N trits is −1, 0 or +1 (AM §1.2 bijection).
// ---------------------------------------------------------------------------
template <int N>
  requires(N >= 1)
class Tint {
 public:
  constexpr Tint() = default;

  // Symmetric residue mod 3^N (AM §3.1 wrapping): keep the low N trits.
  static Tint wrap_of(detail::TritSpan t) {
    Tint r;
    const std::size_t m = t.size() < static_cast<std::size_t>(N)
                              ? t.size() : static_cast<std::size_t>(N);
    for (std::size_t i = 0; i < m; ++i) r.t_[i] = t[i];
    return r;
  }
  // Checked construction from an exact value; empty when it does not fit.
  // Literal rule (Lang Ch.1 §3): "never a silent wrap".
  static std::optional<Tint> try_of(detail::TritSpan t) {
    if (!detail::tfits(t, N)) return std::nullopt;
    return wrap_of(t);
  }
  static Tint from_i64(long long v) {  // host convenience; throws when out of range
    const detail::Trits t = detail::t_from_i128(static_cast<__int128>(v));
    if (!detail::tfits(t, N)) throw std::out_of_range("value does not fit this tN width");
    return wrap_of(t);
  }
  long long to_i64() const {
    static_assert(N <= 40, "to_i64 requires the whole tN range to fit in 64 bits");
    return static_cast<long long>(detail::t_to_i128(trits()));
  }

  static constexpr Tint zero() { return Tint{}; }
  static Tint max_value() { Tint r; r.t_.fill(1);  return r; }  // +(3^N−1)/2
  static Tint min_value() { Tint r; r.t_.fill(-1); return r; }  // −(3^N−1)/2 = −MAX
  static Tint from_bt(Bt t) { Tint r; r.t_[0] = static_cast<int8_t>(t.to_int()); return r; }
  // AM §3.4 splat: every trit position takes the value of the single trit t.
  static Tint splat(Bt t) { Tint r; r.t_.fill(static_cast<int8_t>(t.to_int())); return r; }

  detail::TritSpan trits() const { return detail::TritSpan(t_.data(), static_cast<std::size_t>(N)); }
  int8_t lst() const { return t_[0]; }        // t0(x), the least significant trit (AM A.5)
  int8_t trit(std::size_t i) const { return t_[i]; }

  friend bool operator==(const Tint&, const Tint&) = default;

 private:
  std::array<int8_t, N> t_{};  // least-significant trit first
};

// AM §3.4 trit-wise operations, applied independently to each trit position:
//   tneg(a) = −a per trit; tmin/tmax = min/max per trit; tmul = a·b per trit.
// tneg is also arithmetic neg (AM §1.2 consequence 3, §3.4 closing paragraph).
// (Free templates rather than hidden friends, so qualified calls trt::neg(x)
// work — everything here is built on the public span/trit access only.)
template <int N> Tint<N> neg(Tint<N> a) { return Tint<N>::wrap_of(detail::tneg(a.trits())); }
template <int N> Tint<N> tneg(Tint<N> a) { return neg(a); }
template <int N> Tint<N> tmin(Tint<N> a, Tint<N> b) {
  detail::Trits r(static_cast<std::size_t>(N), 0);
  for (int i = 0; i < N; ++i)
    r[static_cast<std::size_t>(i)] = a.trit(i) < b.trit(i) ? a.trit(i) : b.trit(i);
  return Tint<N>::wrap_of(r);
}
template <int N> Tint<N> tmax(Tint<N> a, Tint<N> b) {
  detail::Trits r(static_cast<std::size_t>(N), 0);
  for (int i = 0; i < N; ++i)
    r[static_cast<std::size_t>(i)] = a.trit(i) > b.trit(i) ? a.trit(i) : b.trit(i);
  return Tint<N>::wrap_of(r);
}
template <int N> Tint<N> tmul(Tint<N> a, Tint<N> b) {
  detail::Trits r(static_cast<std::size_t>(N), 0);
  for (int i = 0; i < N; ++i)
    r[static_cast<std::size_t>(i)] = static_cast<int8_t>(a.trit(i) * b.trit(i));
  return Tint<N>::wrap_of(r);
}

// abs is total: MIN = −MAX, so neither neg nor abs can overflow (AM §3.1, P1).
template <int N> Tint<N> abs(Tint<N> a) {
  return detail::tsign(a.trits()) < 0 ? neg(a) : a;
}

// AM §3.5 cmp — the primitive comparison, yielding a single trit.
template <int N> Bt cmp(Tint<N> a, Tint<N> b) {
  return Bt::from_int(detail::tcmp(a.trits(), b.trits()));
}
// Two-valued predicates are projections of cmp (Lang Ch.1 §5).
template <int N> bool operator< (Tint<N> a, Tint<N> b) { return is_neg(cmp(a, b)); }
template <int N> bool operator> (Tint<N> a, Tint<N> b) { return is_pos(cmp(a, b)); }
template <int N> bool operator<=(Tint<N> a, Tint<N> b) { return !is_pos(cmp(a, b)); }
template <int N> bool operator>=(Tint<N> a, Tint<N> b) { return !is_neg(cmp(a, b)); }
template <int N> bool operator!=(Tint<N> a, Tint<N> b) { return !(a == b); }

// ---------------------------------------------------------------------------
// Overflow flavors (AM §3.1; TIR §3: .wrap / .trap / .flag).  The exact
// mathematical result is computed first; the flavor decides what overflow does.
// checked_* / saturating_* are the language's library-level compositions of the
// primitives plus comparison (AM §3.1, Lang Ch.1 P4) — see NOTES.md reader Q4.
// ---------------------------------------------------------------------------
template <int N> struct Ovf {
  Tint<N> value;    // the wrapped value
  Bt      overflow; // the overflow trit: +1 above MAX, −1 below MIN, 0 none
};

namespace detail {
template <int N> Tint<N> apply_wrap(TritSpan exact) { return Tint<N>::wrap_of(exact); }
template <int N> Tint<N> apply_trap(TritSpan exact) {
  if (!tfits(exact, N)) throw MachineFault(Fault::F_OVERFLOW);
  return Tint<N>::wrap_of(exact);
}
template <int N> Ovf<N> apply_flag(TritSpan exact) {
  return {Tint<N>::wrap_of(exact), Bt::from_int(toverflow_trit(exact, N))};
}
template <int N> std::optional<Tint<N>> apply_checked(TritSpan exact) {
  if (!tfits(exact, N)) return std::nullopt;
  return Tint<N>::wrap_of(exact);
}
template <int N> Tint<N> apply_sat(TritSpan exact) {
  switch (toverflow_trit(exact, N)) {
    case +1: return Tint<N>::max_value();
    case -1: return Tint<N>::min_value();
    default: return Tint<N>::wrap_of(exact);
  }
}
}  // namespace detail

// add / sub / mul: five flavors each (AM §3.1 + Lang Ch.1 §4 naming).
#define TRT_FLAVORED_BINOP(op, EXACT)                                        \
  template <int N> Tint<N> op##_wrap(Tint<N> a, Tint<N> b)                   \
    { return detail::apply_wrap<N>(EXACT); }                                 \
  template <int N> Tint<N> op##_trap(Tint<N> a, Tint<N> b)                   \
    { return detail::apply_trap<N>(EXACT); }                                 \
  template <int N> Ovf<N> op##_flag(Tint<N> a, Tint<N> b)                    \
    { return detail::apply_flag<N>(EXACT); }                                 \
  template <int N> std::optional<Tint<N>> op##_checked(Tint<N> a, Tint<N> b) \
    { return detail::apply_checked<N>(EXACT); }                              \
  template <int N> Tint<N> op##_sat(Tint<N> a, Tint<N> b)                    \
    { return detail::apply_sat<N>(EXACT); }                                  \
  static_assert(true)

TRT_FLAVORED_BINOP(add, detail::tadd(a.trits(), b.trits()));
TRT_FLAVORED_BINOP(sub, detail::tsub(a.trits(), b.trits()));
TRT_FLAVORED_BINOP(mul, detail::tmul(a.trits(), b.trits()));
#undef TRT_FLAVORED_BINOP

// AM §3.3 shifts: k must lie in 0 … N−1; outside that range is a trapping
// fault, "not masked, not undefined" (TIR §3.1: negative amounts fault too).
template <int N> void shift_check(long long k) {
  if (k < 0 || k >= static_cast<long long>(N)) throw MachineFault(Fault::F_SHIFT);
}

// shl(a,k) = a·3^k, with the §3.1 flavor system on overflow.
template <int N> Tint<N> shl_wrap(Tint<N> a, long long k)
  { shift_check<N>(k); return detail::apply_wrap<N>(detail::tshl(a.trits(), static_cast<int>(k))); }
template <int N> Tint<N> shl_trap(Tint<N> a, long long k)
  { shift_check<N>(k); return detail::apply_trap<N>(detail::tshl(a.trits(), static_cast<int>(k))); }
template <int N> Ovf<N> shl_flag(Tint<N> a, long long k)
  { shift_check<N>(k); return detail::apply_flag<N>(detail::tshl(a.trits(), static_cast<int>(k))); }
template <int N> std::optional<Tint<N>> shl_checked(Tint<N> a, long long k)
  { shift_check<N>(k); return detail::apply_checked<N>(detail::tshl(a.trits(), static_cast<int>(k))); }
template <int N> Tint<N> shl_sat(Tint<N> a, long long k)
  { shift_check<N>(k); return detail::apply_sat<N>(detail::tshl(a.trits(), static_cast<int>(k))); }

// AM §3.2 div/rem.  There is no representable-overflow case (MIN = −MAX), so
// apply_trap below can never raise F_OVERFLOW; only b = 0 faults (F_DIVZERO).
template <int N> Tint<N> div(Tint<N> a, Tint<N> b) {
  return detail::apply_trap<N>(detail::tdivmod_nearest(a.trits(), b.trits()).first);
}
template <int N> Tint<N> rem(Tint<N> a, Tint<N> b) {
  return detail::apply_trap<N>(detail::tdivmod_nearest(a.trits(), b.trits()).second);
}

// AM §3.3: shr(a,k) = div(a,3^k) — exact round-to-nearest; 3^k is odd, so ties
// are impossible and the §3.2 tie-break is never invoked.
template <int N> Tint<N> shr(Tint<N> a, long long k) {
  shift_check<N>(k);
  return div(a, Tint<N>::wrap_of(detail::tpow3(static_cast<int>(k))));
}

// mulh (Lang Ch.1 §4, TIR §3.1, ISA §4.1): the high N trits of the exact
// 2N-trit product.  Total, no flavors: the high half always fits in N trits.
// With mul_wrap it reconstructs the product: P = mulh·3^N + mul_wrap.
template <int N> Tint<N> mulh(Tint<N> a, Tint<N> b) {
  const detail::Trits p = detail::tmul(a.trits(), b.trits());
  detail::Trits hi;
  if (p.size() > static_cast<std::size_t>(N)) hi.assign(p.begin() + N, p.end());
  return detail::apply_trap<N>(hi);  // never faults (spec guarantee)
}

// Conversions (Lang Ch.1 §6, TIR §3.5): widening is value-preserving (there is
// only one extension — zero-padding, which changes nothing), narrowing wraps.
template <int M, int N> Tint<N> widen(Tint<M> a) {
  static_assert(M < N, "widen requires M < N");
  return Tint<N>::wrap_of(a.trits());
}
template <int M, int N> Tint<M> narrow(Tint<N> a) {  // TIR §3.5 `trunc`
  static_assert(M < N, "narrow requires M < N");
  return Tint<M>::wrap_of(a.trits());
}
template <int M, int N> std::optional<Tint<M>> checked_narrow(Tint<N> a) {
  static_assert(M < N, "checked_narrow requires M < N");
  return Tint<M>::try_of(a.trits());
}

// The widths the spec defines (AM §1.3, Lang Ch.1 §2).  taddr is the target's
// pointer-width integer: word-width on the AM.
using trit  = Bt;
using t9    = Tint<9>;   // the tryte: −9841 … +9841
using t27   = Tint<27>;  // the word:  ±3 812 798 742 493
using taddr = Tint<27>;

}  // namespace trt
