// cp3/literals.hpp — the numeric literal notations of the spec, with parsing.
//
// Sources (only files under spec/ were consulted):
//   spec/01-types.md              Lang Ch.1 §3, §3.1 — three radices, heptavintimal
//                                  digit table, the leading-zero warning, the two
//                                  lexer questions G1.2 (maximal munch) and G1.3
//                                  (lowercase t synonym), trit literals.
//   spec/language/00-syntax.md    Lang Ch.0 §1.4 — literals are Ch.1 §3 unchanged.
//   spec/00-abstract-machine.md   AM §1.1 — trit-string notation `1T0` = 6.
//   spec/isa/assembly-0.1.md      §2.2–2.3 — same three radices; `t` accepted in
//                                  trit strings, normalized to `T`; 0x/0o rejected.

#pragma once

#include <algorithm>
#include <string>
#include <string_view>

#include "core.hpp"

namespace trt {

class ParseError : public std::runtime_error {
 public:
  using std::runtime_error::runtime_error;
};

// ---------------------------------------------------------------------------
// Heptavintimal digits (Lang Ch.1 §3.1).  A digit is a *balanced* group of
// three trits, denoting −13 … +13.  'D' is the zero digit, not '0' — this is
// why a leading zero is not neutral: 0hJ = 6, 0h0J = −345, 0h00J = −9822.
// Digits are '0'-'9' then 'A'-'Q' (uppercase only; see NOTES.md reader Q1).
// ---------------------------------------------------------------------------
inline constexpr std::string_view kHeptDigits = "0123456789ABCDEFGHIJKLMNOPQ";

inline int hept_value_of(char c) {  // digit value −13…+13, or 999 when invalid
  if (c >= '0' && c <= '9') return (c - '0') - 13;         // '0'→−13 … '9'→−4
  if (c >= 'A' && c <= 'Q') return (c - 'A') + 10 - 13;    // 'A'→−3 … 'Q'→+13
  return 999;                                              // 'D'→0 via 'A'..: 'D' = 13−13
}
inline char hept_digit_of(int v) {  // v ∈ −13…+13 → its digit character
  return kHeptDigits[static_cast<std::size_t>(v + 13)];
}

// ---------------------------------------------------------------------------
// Trit strings (AM §1.1): 'T'/'t', '0', '1', MST first; '_' is ignored and the
// canonical output never carries the lowercase (Lang Ch.1 §3 G1.3, Asm §2.2).
// ---------------------------------------------------------------------------
inline detail::Trits trits_from_string(std::string_view s) {
  detail::Trits r;
  r.reserve(s.size());
  for (char c : s) {
    switch (c) {
      case '1': r.push_back(1); break;
      case '0': r.push_back(0); break;
      case 'T': case 't': r.push_back(-1); break;
      case '_': break;
      default:
        throw ParseError(std::string("not a trit character: '") + c + "' (AM §1.1)");
    }
  }
  std::reverse(r.begin(), r.end());  // written MST first → stored LST first
  detail::tnorm(r);
  return r;
}

inline std::string to_trit_string(detail::TritSpan t) {
  std::string s;
  const std::size_t n = detail::tlen(t);  // canonical: no leading zeros
  if (n == 0) return "0";
  s.reserve(n);
  for (std::size_t i = n; i-- > 0;)
    s.push_back(t[i] > 0 ? '1' : t[i] < 0 ? 'T' : '0');  // canonical uppercase
  return s;
}

// Heptavintimal rendering: one digit per balanced 3-trit group, minimal digit
// count.  "A formatter must not zero-pad a 0h literal" (Lang Ch.1 §3.1), and
// zero itself prints as 'D', never '0'.
inline std::string to_heptavintimal(detail::TritSpan t) {
  detail::Trits cur(t.begin(), t.end());
  detail::tnorm(cur);
  if (cur.empty()) return "D";
  std::string rev;
  while (!cur.empty()) {
    const int dv = (cur.size() > 0 ? cur[0] : 0)
                 + 3 * (cur.size() > 1 ? cur[1] : 0)
                 + 9 * (cur.size() > 2 ? cur[2] : 0);   // balanced group, −13…+13
    rev.push_back(hept_digit_of(dv));
    cur.erase(cur.begin(), cur.begin() + std::min<std::size_t>(3, cur.size()));
    detail::tnorm(cur);
  }
  return std::string(rev.rbegin(), rev.rend());
}

// ---------------------------------------------------------------------------
// Literal parsing (Lang Ch.1 §3 with §3.1's two lexer rules):
//   decimal                       6, -9841, 3_812_798
//   balanced ternary, prefix 0t   0t1T0 = 6, digits 1/0/T (+lowercase t), MSMST first
//   heptavintimal, prefix 0h      0hJ = 6, digits 0-9 A-Q, 3 trits per digit
//   trit literals                 -1t, 0t, 1t (only these may wear the t suffix)
// Maximal munch (G1.2): 0t immediately followed by a balanced-ternary character
// (0, 1, T/t, _) opens a balanced literal; standing alone it is the trit zero.
// A leading '-' is permitted on 0t (redundant: -0t1T = 0tT1); this parser also
// accepts it on the other radices — see NOTES.md reader Q2 and Q3.
// ---------------------------------------------------------------------------
struct ParsedLiteral {
  enum class Kind { Integer, Trit } kind;
  detail::Trits exact;               // the unbounded exact value
  Bt trit_value() const {            // valid when kind == Trit
    if (kind != Kind::Trit) throw ParseError("not a trit literal");
    detail::Trits n = exact;
    detail::tnorm(n);
    if (n.size() > 1) throw ParseError("trit literal out of trit range");
    return Bt::from_int(n.empty() ? 0 : n[0]);
  }
};

inline ParsedLiteral parse_literal(std::string_view text) {
  using Kind = ParsedLiteral::Kind;
  if (text.empty()) throw ParseError("empty literal");
  const std::string whole(text);
  std::size_t pos = 0;
  bool negative = false;
  if (text[pos] == '-') {
    negative = true;
    if (++pos == text.size()) throw ParseError("a bare '-' is not a literal");
  }
  const std::string_view rest = text.substr(pos);
  detail::Trits exact;
  Kind kind = Kind::Integer;

  if (rest.size() >= 2 && rest[0] == '0' && rest[1] == 't') {
    const std::string_view body = rest.substr(2);
    bool balanced_chars = true, any_digit = false;
    for (char c : body) {
      if (c == '0' || c == '1' || c == 'T' || c == 't') any_digit = true;
      else if (c != '_') { balanced_chars = false; break; }
    }
    if (body.empty()) {
      // Maximal munch did not fire: `0t` standing alone is the trit zero (G1.2).
      kind = Kind::Trit;
      exact = {};
    } else if (balanced_chars && any_digit) {
      exact = trits_from_string(body);
    } else if (balanced_chars) {
      // Only separators after 0t: reader Q3 — a literal needs at least one digit.
      throw ParseError("0t literal with separators but no digits: " + whole);
    } else {
      // A non-balanced character stops the munch at `0t`; for this single-literal
      // API the remainder is trailing junk (in a full lexer it would re-tokenize).
      throw ParseError("trailing characters after trit literal `0t`: " + whole);
    }
  } else if (rest.size() >= 2 && rest[0] == '0' && rest[1] == 'h') {
    const std::string_view body = rest.substr(2);
    bool any = false;
    detail::Trits acc;
    for (char c : body) {
      if (c == '_') continue;
      const int dv = hept_value_of(c);
      if (dv == 999)
        throw ParseError(std::string("bad heptavintimal digit '") + c +
                         "' (digits are 0-9 A-Q, Lang Ch.1 §3.1): " + whole);
      any = true;
      acc = detail::tshl(acc, 3);  // one digit = one 27-group = a 3-trit shift
      detail::tadd_i32(acc, dv);
    }
    if (!any) throw ParseError("0h literal with no digits: " + whole);
    exact = std::move(acc);
  } else if (rest.size() >= 2 && rest[0] == '0' &&
             (rest[1] == 'x' || rest[1] == 'X' || rest[1] == 'o' || rest[1] == 'O')) {
    // "There is no hexadecimal or octal: both are artifacts of the bit." (Ch.1 §3)
    throw ParseError("there is no hexadecimal or octal in this language: " + whole +
                     " (Lang Ch.1 §3)");
  } else if (!rest.empty() && rest[0] >= '0' && rest[0] <= '9') {
    detail::Trits acc;
    std::size_t i = 0;
    for (; i < rest.size(); ++i) {
      const char c = rest[i];
      if (c == '_') continue;
      if (c < '0' || c > '9') break;
      acc = detail::tadd(detail::tshl(acc, 2), acc);  // acc·10 = acc·3^2 + acc
      detail::tadd_i32(acc, c - '0');
    }
    if (i < rest.size() && rest[i] == 't') {
      // trit suffix: only the magnitudes 0 and 1 may wear it — "2t is ill-formed".
      detail::Trits n = acc;
      detail::tnorm(n);
      const bool unit = n.empty() || (n.size() == 1 && n[0] == 1);
      if (!unit)
        throw ParseError("only -1t, 0t, 1t are trit literals (Lang Ch.1 §3, G1.2): " + whole);
      kind = Kind::Trit;
      ++i;
    }
    if (i != rest.size())
      throw ParseError(std::string("bad character in decimal literal: '") + rest[i] +
                       "': " + whole);
    exact = std::move(acc);
  } else {
    throw ParseError("not a numeric literal (Lang Ch.1 §3): " + whole);
  }

  if (negative) exact = detail::tneg(exact);  // −0 is 0: no negative zero (AM §1.2)
  return {kind, std::move(exact)};
}

// Literal typed at width N (Lang Ch.1 §3): "a literal that does not fit its
// inferred type is a compile-time error, never a silent wrap."  The empty
// optional is this library's compile-time-error analogue, for integer literals.
template <int N>
std::optional<Tint<N>> parse_as(std::string_view text) {
  const ParsedLiteral p = parse_literal(text);
  if (p.kind != ParsedLiteral::Kind::Integer) return std::nullopt;  // no implicit trit→int (P2)
  return Tint<N>::try_of(p.exact);
}

}  // namespace trt
