// Block comments nest (Ch. 0 §1.2), which a regular expression cannot say.
//
// Everything else in this grammar is regular; this is the one token that is
// not, so it is the one thing here written in C.

#include "tree_sitter/parser.h"

enum TokenType { BLOCK_COMMENT };

void *tree_sitter_trust_external_scanner_create(void) { return NULL; }
void tree_sitter_trust_external_scanner_destroy(void *payload) { (void)payload; }
void tree_sitter_trust_external_scanner_deserialize(void *payload, const char *buffer,
                                                    unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}
unsigned tree_sitter_trust_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

bool tree_sitter_trust_external_scanner_scan(void *payload, TSLexer *lexer,
                                             const bool *valid_symbols) {
  (void)payload;
  if (!valid_symbols[BLOCK_COMMENT]) {
    return false;
  }
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\n' ||
         lexer->lookahead == '\r') {
    lexer->advance(lexer, true);
  }
  if (lexer->lookahead != '/') {
    return false;
  }
  lexer->advance(lexer, false);
  if (lexer->lookahead != '*') {
    return false;
  }
  lexer->advance(lexer, false);

  // An unterminated comment is not a comment: returning false leaves the
  // text to the rest of the grammar, which reports it where it starts.
  unsigned depth = 1;
  while (depth > 0) {
    if (lexer->eof(lexer)) {
      return false;
    }
    if (lexer->lookahead == '/') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '*') {
        lexer->advance(lexer, false);
        depth++;
      }
    } else if (lexer->lookahead == '*') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '/') {
        lexer->advance(lexer, false);
        depth--;
      }
    } else {
      lexer->advance(lexer, false);
    }
  }
  lexer->result_symbol = BLOCK_COMMENT;
  return true;
}
