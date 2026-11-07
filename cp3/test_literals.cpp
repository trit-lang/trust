// cp3/test_literals.cpp — Lang Ch.1 §3/§3.1 literal notations (decimal, 0t
// balanced ternary, 0h heptavintimal, trit literals), the maximal-munch rules,
// and the pinned leading-zero-not-neutral heptavintimal warning.

#include "testharness.hpp"

TEST_CASE(lit_decimal, "Lang Ch.1 §3 + Ch.0 §4") {
  CHECK_LL(lit("6"), 6);                  // the table's decimal example
  CHECK_LL(lit("-9841"), -9841);          // the table's negative decimal example
  CHECK_LL(lit("0"), 0);
  CHECK_LL(lit("-0"), 0);                 // no negative zero (AM §1.2)
  CHECK_LL(lit("42"), 42);                // Ch.0 §4 integer-literal pattern
  CHECK_LL(lit("3_812_798"), 3812798);    // "_ permitted as a separator in all three"
  CHECK_LL(lit("3_812_798_742_493"), 3812798742493LL);  // the AM word's MAX, §1.3
  CHECK_LL(lit("-3_812_798_742_493"), -3812798742493LL);
  CHECK(trt::parse_literal("6").kind == trt::ParsedLiteral::Kind::Integer);
}

TEST_CASE(lit_balanced_ternary, "Lang Ch.1 §3 + AM §1.1") {
  CHECK_LL(lit("0t1T0"), 6);              // both the AM §1.1 and the Ch.1 §3 example
  CHECK_LL(lit("0t1"), 1);                // "So 0t1 is the integer 1"
  CHECK_LL(lit("0t0"), 0);
  CHECK_LL(lit("0tT"), -1);
  CHECK_LL(lit("0t11"), 4);
  CHECK_LL(lit("0tT11"), -5);             // −9 + 3 + 1
  CHECK_LL(lit("0t1T0_T01"), 154);        // separators in the ternary form
  // "0t literals are trit-exact: each character is one trit of the representation"
  CHECK(trt::to_trit_string(trt::parse_literal("0t1T0").exact) == "1T0");
  CHECK(trt::to_trit_string(trt::parse_literal("0tT0110T").exact) == "T0110T");
  // "A leading `-` on a `0t` literal is permitted but redundant (-0t1T = 0tT1)"
  CHECK_LL(lit("-0t1T"), -2);
  CHECK_LL(lit("0tT1"), -2);
  CHECK(lit("-0t1T") == lit("0tT1"));
  // Canonical forms never carry the lowercase (G1.3).
  CHECK(trt::to_trit_string(trt::parse_literal("0t1t").exact) == "1T");
  CHECK_LL(lit("0t1t"), 2);               // "0t1t reads as 0t1T, which is 2"
  CHECK_LL(lit("0t1t1tt"), lit("0t1T1TT"));
  // Parsing into a width: the same value as the decimal spelling.
  CHECK(trt::parse_as<9>("0t1T0") == trt::t9::from_i64(6));
  CHECK(trt::parse_as<27>("0t1T0") == trt::t27::from_i64(6));
}

TEST_CASE(lit_trit_literals_and_maximal_munch, "Lang Ch.1 §3 (G1.2, G1.3)") {
  using Kind = trt::ParsedLiteral::Kind;
  // "The three `trit` literals of §2 (-1t, 0t, 1t) are the only decimal
  // magnitudes that may wear the t suffix."
  CHECK(trt::parse_literal("-1t").kind == Kind::Trit);
  CHECK_LL(trt::parse_literal("-1t").trit_value().to_int(), -1);
  CHECK(trt::parse_literal("0t").kind == Kind::Trit);       // standing alone: trit 0
  CHECK_LL(trt::parse_literal("0t").trit_value().to_int(), 0);
  CHECK(trt::parse_literal("1t").kind == Kind::Trit);
  CHECK_LL(trt::parse_literal("1t").trit_value().to_int(), 1);
  CHECK_LL(trt::parse_literal("-0t").trit_value().to_int(), 0);
  // Maximal munch: immediately followed by a balanced-ternary character
  // (0, 1, T/t, _), 0t opens a balanced literal instead.
  CHECK(trt::parse_literal("0t1").kind == Kind::Integer);
  CHECK(trt::parse_literal("0t0").kind == Kind::Integer);
  CHECK(trt::parse_literal("0tT").kind == Kind::Integer);
  CHECK(trt::parse_literal("0tt").kind == Kind::Integer);   // lowercase counts too
  CHECK(trt::parse_literal("0t1t").kind == Kind::Integer);  // it is *not* trit "0t" + "1t"
  CHECK(trt::parse_literal("0t_1").kind == Kind::Integer);  // reader Q3 interpretation
  CHECK_LL(lit("0t_1"), 1);
  // "2t is ill-formed", and larger magnitudes as well.
  CHECK_PARSE_ERR(trt::parse_literal("2t"));
  CHECK_PARSE_ERR(trt::parse_literal("-2t"));
  CHECK_PARSE_ERR(trt::parse_literal("10t"));
  // trit_value is only for trit-kind literals.
  CHECK_PARSE_ERR(trt::parse_literal("0t1").trit_value());
  // `0t` the literal is the trit zero — it is *not* an integer literal, and the
  // language has no implicit trit→integer conversion (P2).
  CHECK(!trt::parse_as<9>("0t").has_value());
}

// THE load-bearing warning of Lang Ch.1 §3.1: a leading zero is NOT neutral.
// "D is the zero digit, not 0. So 0hJ is 6, 0h0J is −13 × 27 + 6 = −345, and
// 0h00J is −9822."
TEST_CASE(lit_heptavintimal_leading_zero_warning, "Lang Ch.1 §3.1") {
  CHECK_LL(lit("0hJ"), 6);
  CHECK_LL(lit("0h0J"), -345);     // −13·27 + 6
  CHECK_LL(lit("0h00J"), -9822);   // −13·729 − 13·27 + 6
  CHECK(lit("0hJ") != lit("0h0J"));
  CHECK(lit("0h0J") != lit("0h00J"));
  // Independent arithmetic of the warning, digit by digit:
  CHECK_LL(-13 * 27 + 6, -345);
  CHECK_LL(-13 * 729 - 13 * 27 + 6, -9822);
  // Notational padding must use D: 0hDJ and 0hDDJ are both exactly 6,
  // the same value as 0hJ — unlike the 0-padded forms above.
  CHECK_LL(lit("0hDJ"), 6);
  CHECK_LL(lit("0hDDJ"), 6);
  CHECK(lit("0hDJ") == lit("0hJ"));
  // Prepending one '0' (the −13 digit) contributes exactly −13·27 = −351 here.
  CHECK_LL(lit("0h0J") - lit("0hJ"), -351);
}

TEST_CASE(lit_heptavintimal_examples, "Lang Ch.1 §3.1 + Ch.0 §4") {
  CHECK_LL(lit("0h2C9"), -8050);   // the spec's retained warning-example: −11·729 − 27 − 4
  CHECK_LL(-11 * 729 - 27 - 4, -8050);
  CHECK_LL(lit("0hDDE"), 1);       // Ch.0 §4's pattern literal
  CHECK_LL(lit("0hD"), 0);         // D is the zero digit
  CHECK_LL(lit("0hDD"), 0);
  CHECK_LL(lit("0h0"), -13);
  CHECK_LL(lit("0hQ"), 13);
  CHECK_LL(lit("0hC"), -1);
  CHECK_LL(lit("0hE"), 1);
  CHECK_LL(lit("0h9"), -4);
  CHECK_LL(lit("0hA"), -3);
  CHECK_LL(lit("0hP"), 12);
  // Balanced digits denote negatives without a sign (§3.1: "the only reading
  // under which a 0h literal can denote a negative number without a sign").
  CHECK(lit("0h0") < 0 && lit("0hC") < 0 && lit("0h9") < 0 && lit("0hA") < 0);
  // With a leading '-' as well (reader Q2 interpretation).
  CHECK_LL(lit("-0hJ"), -6);
  CHECK_LL(lit("-0h0J"), 345);
  CHECK_LL(-lit("0h0J"), lit("-0h0J"));
  // The two extremes of the word: nine digits, as §3 promises ("a t27 value is
  // at most 9 digits").
  CHECK_LL(lit("0hQQQQQQQQQ"), 3812798742493LL);
  CHECK_LL(lit("0h000000000"), -3812798742493LL);
  CHECK_LL(lit("0hQQQ"), 9841);            // and of the tryte: three digits
  CHECK_LL(lit("0h000"), -9841);
  CHECK(!trt::parse_as<9>("0hQQQQ").has_value());   // does not fit: never wraps
  CHECK(trt::parse_as<27>("0hQQQQQQQQQ") == trt::t27::max_value());
  CHECK(trt::parse_as<27>("0h000000000") == trt::t27::min_value());
}

// §3.1: "A heptavintimal digit is a balanced group of three trits, so it
// denotes −13 … +13 and not 0 … 26" — digit-exactness over the underlying
// radix, for all 27 digits, against the table transcribed from the spec.
TEST_CASE(lit_heptavintimal_digit_table_and_groups, "Lang Ch.1 §3.1") {
  const std::string_view digits = "0123456789ABCDEFGHIJKLMNOPQ";
  const int table[27] = {-13, -12, -11, -10, -9, -8, -7, -6, -5, -4, -3, -2, -1,
                         0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13};
  for (int k = 0; k < 27; ++k) {
    char text[4] = {'0', 'h', digits[static_cast<std::size_t>(k)], 0};
    CHECK_LL(lit(text), table[k]);
    CHECK_LL(trt::hept_value_of(digits[static_cast<std::size_t>(k)]), table[k]);
    // …and the digit is exactly the balanced 3-trit group of that value.
    long long v = table[k];
    std::string group(3, '0');
    for (int pos = 2; pos >= 0; --pos) {
      long long r = v % 3;
      if (r > 1) r -= 3;
      else if (r < -1) r += 3;
      group[static_cast<std::size_t>(pos)] = r == 1 ? '1' : r == -1 ? 'T' : '0';
      v = (v - r) / 3;
    }
    CHECK_LL(v, 0);
    CHECK_LL(trt::detail::t_to_i128(trt::trits_from_string(group)), table[k]);
    CHECK(trt::to_heptavintimal(trt::parse_literal(text).exact)
          == std::string(1, digits[static_cast<std::size_t>(k)]));
  }
  CHECK(trt::to_heptavintimal(trt::t9::zero().trits()) == "D");
}

// "A formatter must not zero-pad a 0h literal" — rendering is minimal and uses
// D for the zero group; note that a leading 0 in the *output* still happens
// exactly when the value's most significant group is −13 (like 0J = −345).
TEST_CASE(lit_heptavintimal_roundtrip, "Lang Ch.1 §3.1") {
  auto roundtrip = [](long long v) {
    const auto x = trt::t27::from_i64(v);
    const std::string s = "0h" + trt::to_heptavintimal(x.trits());
    CHECK_LL(lit(s), v);
    CHECK(trt::parse_as<27>(s).has_value());
    CHECK(*trt::parse_as<27>(s) == x);
  };
  for (long long v : {0LL, 1LL, -1LL, 6LL, -6LL, -345LL, -9822LL, -8050LL,
                      9841LL, -9841LL, 27LL, -27LL, 26LL, 13LL, -13LL,
                      3812798742493LL, -3812798742493LL})
    roundtrip(v);
  Lcg l(0xABCDEF01ull);
  const long long M = llmax(27);
  for (int i = 0; i < 200; ++i) roundtrip(l.uniform(-M, M));
  // No zero padding: the renderings of small values stay one digit long.
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(6).trits()) == "J");
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(1).trits()) == "E");
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(0).trits()) == "D");
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(-345).trits()) == "0J");
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(-9822).trits()) == "00J");
  CHECK(trt::to_heptavintimal(trt::t27::from_i64(-8050).trits()) == "2C9");
  CHECK(trt::to_heptavintimal(trt::t27::max_value().trits()) == "QQQQQQQQQ");
  CHECK(trt::to_heptavintimal(trt::t27::min_value().trits()) == "000000000");
}

TEST_CASE(lit_separators_and_errors, "Lang Ch.1 §3, Assembly §2.2, reader Q1–Q3") {
  CHECK_LL(lit("3_8"), 38);
  CHECK_LL(lit("0hQ_Q"), 13 * 27 + 13);         // 364
  CHECK_LL(lit("0t1_T"), 2);
  // No hexadecimal or octal: "both are artifacts of the bit" (Ch.1 §3), and an
  // assembler must reject them with a diagnostic *saying so* (Assembly §2.2).
  CHECK_PARSE_ERR(trt::parse_literal("0x10"));
  CHECK_PARSE_ERR(trt::parse_literal("0X10"));
  CHECK_PARSE_ERR(trt::parse_literal("0o17"));
  CHECK_PARSE_ERR(trt::parse_literal("0O17"));
  bool named = false;
  try { (void)trt::parse_literal("0x10"); }
  catch (const trt::ParseError& e) {
    named = std::string(e.what()).find("hexadecimal") != std::string::npos &&
            std::string(e.what()).find("octal") != std::string::npos;
  }
  CHECK(named);
  // Lowercase heptavintimal digits: the spec grants lowercase acceptance only
  // for the balanced-ternary T (G1.3).  Implemented: rejected (reader Q1).
  CHECK_PARSE_ERR(trt::parse_literal("0hj"));
  CHECK_PARSE_ERR(trt::parse_literal("0hJq"));
  // 'R' is past 'Q' and not a digit.
  CHECK_PARSE_ERR(trt::parse_literal("0hR"));
  // Degenerate inputs.
  CHECK_PARSE_ERR(trt::parse_literal(""));
  CHECK_PARSE_ERR(trt::parse_literal("-"));
  CHECK_PARSE_ERR(trt::parse_literal("abc"));
  CHECK_PARSE_ERR(trt::parse_literal("0h"));
  CHECK_PARSE_ERR(trt::parse_literal("0t_"));       // separators but no digits (Q3)
  CHECK_PARSE_ERR(trt::parse_literal("12abc"));
  CHECK_PARSE_ERR(trt::parse_literal("1t1"));
  // Literal range checks: "a literal that does not fit its inferred type is a
  // compile-time error, never a silent wrap" (Ch.1 §3).
  CHECK(trt::parse_as<9>("9841").has_value());
  CHECK(!trt::parse_as<9>("9842").has_value());
  CHECK(!trt::parse_as<9>("-9842").has_value());
  CHECK(!trt::parse_as<3>("14").has_value());
  CHECK(trt::parse_as<3>("13")->to_i64() == 13);
  // Ten digits where the top group is the zero-valued D: still exactly MIN of
  // the word (nine −13 groups below).  It fits; ten *nonzero* low groups do not.
  CHECK(trt::parse_as<27>("0hD000000000") == trt::t27::min_value());
  CHECK(!trt::parse_as<27>("0h0000000000").has_value());  // −(3^30−1)/2: no silent wrap
  CHECK(trt::parse_as<9>("0t1T0")->to_i64() == 6);
  // After the maximal munch a single-literal parse must consume everything
  // (in a program text 0t2 would lex as trit-zero followed by integer 2):
  CHECK_PARSE_ERR(trt::parse_literal("0t2"));
}
