// cp3/test_main.cpp — drives every registered spec-transcribed case.

#include "testharness.hpp"

int main() {
  for (const Case& c : registry()) {
    ++g_cases;
    const long long before = g_fails;
    c.fn();
    if (g_fails != before)
      std::printf("  ^^ failures in case %s (%s)\n", c.name, c.spec);
  }
  std::printf("cases:      %lld\n", g_cases);
  std::printf("assertions: %lld\n", g_asserts);
  std::printf("failures:   %lld\n", g_fails);
  return g_fails == 0 ? 0 : 1;
}
