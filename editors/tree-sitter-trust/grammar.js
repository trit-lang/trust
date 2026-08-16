/// A tree-sitter grammar for Trust, for editors that highlight with one.
///
/// This is a **second** grammar. `compiler/src/lang/parse.rs` is the first,
/// and it is the one that decides what a program means; this one exists
/// because Zed and its neighbours have no other way to colour a file. Two
/// grammars can drift, so `scripts/grammar.sh` parses every file in
/// `examples/trust/` with this one and fails on a single ERROR node. That is
/// the whole of the discipline holding them together, and it is worth saying
/// plainly that it catches what this grammar *rejects* and not what it
/// accepts too freely.
///
/// Where it is deliberately looser than the compiler:
///
///   * comparison and `<=>` chain here. `a < b < c` is a syntax error in
///     Trust (Ch. 0 §2.1) and is coloured rather than refused, because a
///     highlighter that gives up on a file is worse than one that is wrong
///     about a line of it.
///   * a number is one token of digits and letters, exactly as the lexer
///     reads it (§1.4); whether `0t1T0` is a legal literal is the compiler's
///     answer, not a colour.
///   * `_` reserved words (`mod`, `unsafe`, …) parse as identifiers. The
///     compiler refuses them by name; the highlighter has nothing to say.

const PREC = {
  assign: 0,
  or: 1,
  and: 2,
  compare: 3,
  spaceship: 4,
  shift: 5,
  sum: 6,
  product: 7,
  cast: 8,
  unary: 9,
  postfix: 10,
};

const sepBy1 = (sep, rule) => seq(rule, repeat(seq(sep, rule)));
const sepBy = (sep, rule) => optional(sepBy1(sep, rule));
/// A comma-separated list with an optional trailing comma, which §6 writes
/// `A,*` and every list in this language allows.
const commaSep = (rule) => seq(sepBy(',', rule), optional(','));
const commaSep1 = (rule) => seq(sepBy1(',', rule), optional(','));

module.exports = grammar({
  name: 'trust',

  extras: ($) => [/[\s]/, $.line_comment, $.block_comment],

  word: ($) => $.identifier,

  externals: ($) => [$.block_comment],

  supertypes: ($) => [$._expression, $._type, $._pattern, $._item],

  conflicts: ($) => [
    // `if a { … }`, `match a { … }` and `where n <= a.len()` all put an
    // expression immediately before a `{`, and `a { … }` is also how a
    // struct literal is written (§2.8). The compiler carries a flag that
    // switches struct literals off in those places; here the parser tries
    // both and keeps the one that parses, which is the same answer reached
    // by a different road.
    [$._expression, $.struct_expression],
    // In a `where` clause a name may begin a bound or a predicate, so `T <`
    // is either a type argument list or a comparison and only what follows
    // says which. Nowhere else in the language are the two reachable at
    // once, which is why this is the only conflict about `<`.
    [$._expression, $.generic_type],
    // `x as Foo < y` — the cast's type could take an argument list, or the
    // cast could be one side of a comparison.
    [$._type, $.generic_type],
  ],

  rules: {
    source_file: ($) => repeat($._item),

    // ------------------------------------------------------------ comments

    line_comment: (_) => token(seq('//', /.*/)),

    // ------------------------------------------------------------- items

    _item: ($) =>
      choice(
        $.mod_item,
        $.use_item,
        $.macro_item,
        $.function_item,
        $.struct_item,
        $.enum_item,
        $.const_item,
        $.trait_item,
        $.impl_item,
      ),

    /// `macro name($a, $($x),*) { … }` (Ch. 7 §6).
    macro_item: ($) =>
      seq(
        optional('pub'),
        'macro',
        field('name', $.identifier),
        '(',
        commaSep(choice($.macro_repetition, $.macro_parameter)),
        ')',
        field('body', $.block),
      ),

    macro_parameter: ($) => seq('$', $.identifier),

    macro_repetition: ($) => seq('$', '(', '$', $.identifier, ')', ',', '*'),

    /// `name!(args)` (Ch. 7 §6).
    macro_call: ($) =>
      prec(
        PREC.postfix + 2,
        seq(field('macro', $.identifier), '!', field('arguments', $.arguments)),
      ),

    /// `$( … )*` inside a body, which holds statements and stands as one.
    macro_repeat: ($) => seq('$', '(', repeat($._statement), ')', '*'),

    /// `mod name;` — a module, which is a file (Ch. 6 §1.2).
    mod_item: ($) => seq(optional('pub'), 'mod', field('name', $.identifier), ';'),

    /// `use a::b::c;` (Ch. 6 §3.2).
    use_item: ($) => seq('use', $.module_path, ';'),

    module_path: ($) => sepBy1('::', $.identifier),

    attribute: ($) =>
      seq(
        '#',
        '[',
        $.identifier,
        optional(seq('(', commaSep($.identifier), ')')),
        ']',
      ),

    function_item: ($) =>
      seq(
        repeat($.attribute),
        optional('pub'),
        'fn',
        field('name', $.identifier),
        optional($.generic_parameters),
        $.parameters,
        optional(seq('->', field('return_type', $._type))),
        optional($.where_clause),
        choice(field('body', $.block), ';'),
      ),

    parameters: ($) => seq('(', commaSep(choice($.self_parameter, $.parameter)), ')'),

    self_parameter: ($) =>
      seq(
        optional(seq('&', optional($.lifetime))),
        optional('mut'),
        'self',
        optional(seq(':', field('type', $._type))),
      ),

    parameter: ($) => seq(field('name', $.identifier), ':', field('type', $._type)),

    generic_parameters: ($) =>
      seq('<', commaSep(choice($.lifetime, $.const_parameter, $.type_parameter)), '>'),

    type_parameter: ($) => seq($.identifier, optional($.bounds)),

    const_parameter: ($) => seq('const', $.identifier, ':', $._type),

    bounds: ($) => seq(':', sepBy1('+', choice($.lifetime, $.bound))),

    bound: ($) => seq($.identifier, optional($.type_arguments)),

    /// `where T: Bound` and `where n <= a.len()` share a clause: the second
    /// is a predicate the caller must have established (Ch. 4 §2.8), and is
    /// an expression rather than a bound.
    /// A trailing comma is allowed and the `{` that follows it opens the
    /// item's body, not another predicate — so the clause reduces as soon as
    /// it can rather than reaching for one more.
    where_clause: ($) =>
      prec.left(
        seq('where', commaSep1(choice($.where_bound, field('predicate', $._expression)))),
      ),

    where_bound: ($) => seq(choice($.lifetime, $.identifier), $.bounds),

    struct_item: ($) =>
      seq(
        repeat($.attribute),
        optional('pub'),
        'struct',
        field('name', $.identifier),
        optional($.generic_parameters),
        optional($.where_clause),
        choice($.field_declarations, seq($.tuple_fields, ';'), ';'),
      ),

    field_declarations: ($) => seq('{', commaSep($.field_declaration), '}'),

    field_declaration: ($) =>
      seq(optional('pub'), field('name', $.identifier), ':', field('type', $._type)),

    tuple_fields: ($) => seq('(', commaSep($._type), ')'),

    enum_item: ($) =>
      seq(
        repeat($.attribute),
        optional('pub'),
        'enum',
        field('name', $.identifier),
        optional($.generic_parameters),
        optional($.where_clause),
        '{',
        commaSep($.variant),
        '}',
      ),

    variant: ($) =>
      seq(
        field('name', $.identifier),
        optional(choice($.tuple_fields, $.field_declarations)),
        optional(seq('=', $._expression)),
      ),

    const_item: ($) =>
      seq(
        optional('pub'),
        'const',
        field('name', $.identifier),
        ':',
        field('type', $._type),
        optional(seq('=', field('value', $._expression))),
        ';',
      ),

    trait_item: ($) =>
      seq(
        optional('pub'),
        'trait',
        field('name', $.identifier),
        optional($.generic_parameters),
        optional($.bounds),
        optional($.where_clause),
        $.declaration_list,
      ),

    impl_item: ($) =>
      seq(
        'impl',
        optional($.generic_parameters),
        optional('!'),
        field('trait', $._type),
        optional(seq('for', field('type', $._type))),
        optional($.where_clause),
        $.declaration_list,
      ),

    declaration_list: ($) =>
      seq('{', repeat(choice($.function_item, $.associated_type, $.const_item)), '}'),

    associated_type: ($) =>
      seq('type', $.identifier, optional($.bounds), optional(seq('=', $._type)), ';'),

    // ------------------------------------------------------------- types

    _type: ($) =>
      choice(
        $.primitive_type,
        $.never_type,
        $.self_type,
        $.unit_type,
        $.tuple_type,
        $.array_type,
        $.reference_type,
        $.dynamic_type,
        $.impl_fn_type,
        $.generic_type,
        $.qualified_type,
        $.identifier,
      ),

    primitive_type: (_) => choice('trit', 'bool', 't9', 't27', 'taddr', 'char', 'str'),
    never_type: (_) => '!',
    self_type: (_) => 'Self',
    unit_type: (_) => seq('(', ')'),
    tuple_type: ($) => seq('(', commaSep1($._type), ')'),
    /// `[T; N]` is an array and `[T]` a slice; §6 writes them as one rule.
    array_type: ($) => seq('[', $._type, optional(seq(';', $._expression)), ']'),
    reference_type: ($) =>
      seq('&', optional($.lifetime), optional('mut'), field('type', $._type)),
    dynamic_type: ($) => seq('dyn', $._type),
    impl_fn_type: ($) =>
      prec.right(
        seq(
          'impl',
          field('trait', $.identifier),
          '(',
          commaSep($._type),
          ')',
          optional(seq('->', $._type)),
        ),
      ),
    generic_type: ($) => seq(field('name', $.identifier), $.type_arguments),
    /// `T::Item` and `Self::Item` — an associated type (Ch. 4 §1.7).
    qualified_type: ($) =>
      prec.left(seq(choice($.identifier, $.self_type, $.generic_type), repeat1(seq('::', $.identifier)))),

    type_arguments: ($) => seq('<', commaSep1(choice($._type, $.lifetime, $.associated_binding)), '>'),

    associated_binding: ($) => seq($.identifier, '=', $._type),

    // -------------------------------------------------------- statements

    block: ($) => seq('{', repeat($._statement), optional($._expression), '}'),

    _statement: ($) =>
      choice(
        $.let_statement,
        $._item,
        $.expression_statement,
      ),

    let_statement: ($) =>
      seq(
        'let',
        optional('mut'),
        field('pattern', $._pattern),
        optional(seq(':', field('type', $._type))),
        optional(seq('=', field('value', $._expression))),
        ';',
      ),

    /// An expression used as a statement. One written with braces needs no
    /// semicolon (§5.1), which is why the two forms are separate here.
    expression_statement: ($) =>
      choice(seq($._expression, ';'), prec(1, $._block_expression)),

    // ------------------------------------------------------- expressions

    _expression: ($) =>
      choice(
        $._literal,
        $.identifier,
        $.self_expression,
        $.path,
        $.unit_expression,
        $.tuple_expression,
        $.array_expression,
        $.parenthesized_expression,
        $.struct_expression,
        $.call_expression,
        $.method_call_expression,
        $.field_expression,
        $.index_expression,
        $.try_expression,
        $.unary_expression,
        $.reference_expression,
        $.binary_expression,
        $.assignment_expression,
        $.cast_expression,
        $.range_expression,
        $.closure_expression,
        $.macro_call,
        $.macro_parameter,
        $.break_expression,
        $.continue_expression,
        $.return_expression,
        $._block_expression,
      ),

    /// The five written with braces, which may stand as statements.
    _block_expression: ($) =>
      choice(
        $.block,
        $.if_expression,
        $.match_expression,
        $.loop_expression,
        $.while_expression,
        $.for_expression,
        // `$( … )*` ends at its `*`, so it stands as a statement (§3).
        $.macro_repeat,
      ),

    self_expression: (_) => 'self',
    unit_expression: (_) => prec(1, seq('(', ')')),
    parenthesized_expression: ($) => seq('(', $._expression, ')'),
    tuple_expression: ($) => seq('(', $._expression, ',', commaSep($._expression), ')'),

    array_expression: ($) =>
      seq('[', choice(seq($._expression, ';', $._expression), commaSep($._expression)), ']'),

    /// `Name { field: value }`, `Name { field }` and `Name::Variant { … }`.
    struct_expression: ($) =>
      prec.dynamic(
        -1,
        seq(field('name', choice($.identifier, $.path, $.generic_type)), $.field_initializers),
      ),

    field_initializers: ($) => seq('{', commaSep($.field_initializer), '}'),

    field_initializer: ($) =>
      seq(field('name', $.identifier), optional(seq(':', field('value', $._expression)))),

    call_expression: ($) =>
      prec(PREC.postfix, seq(field('function', $._expression), field('arguments', $.arguments))),

    arguments: ($) => seq('(', commaSep($._expression), ')'),

    method_call_expression: ($) =>
      prec(
        PREC.postfix + 1,
        seq(
          field('receiver', $._expression),
          '.',
          field('method', $.identifier),
          optional($.turbofish),
          field('arguments', $.arguments),
        ),
      ),

    field_expression: ($) =>
      prec(PREC.postfix, seq(field('value', $._expression), '.', field('field', choice($.identifier, $.integer_literal)))),

    index_expression: ($) =>
      prec(PREC.postfix, seq($._expression, '[', $._expression, ']')),

    try_expression: ($) => prec(PREC.postfix, seq($._expression, '?')),

    unary_expression: ($) => prec(PREC.unary, seq(choice('-', '!', '*'), $._expression)),

    reference_expression: ($) => prec(PREC.unary, seq('&', optional('mut'), $._expression)),

    cast_expression: ($) => prec.left(PREC.cast, seq($._expression, 'as', $._type)),

    binary_expression: ($) => {
      const levels = [
        [PREC.or, '||'],
        [PREC.and, '&&'],
        [PREC.compare, choice('==', '!=', '<', '<=', '>', '>=')],
        [PREC.spaceship, '<=>'],
        [PREC.shift, choice('<<', '>>')],
        [PREC.sum, choice('+', '-')],
        [PREC.product, choice('*', '/', '%')],
      ];
      return choice(
        ...levels.map(([p, op]) =>
          prec.left(p, seq(field('left', $._expression), field('operator', op), field('right', $._expression))),
        ),
      );
    },

    assignment_expression: ($) =>
      prec.right(
        PREC.assign,
        seq(
          field('left', $._expression),
          field('operator', choice('=', '+=', '-=', '*=', '/=', '%=', '<<=', '>>=')),
          field('right', $._expression),
        ),
      ),

    /// `a..b` — sugar for `Range { start: a, end: b }` (Ch. 0 §5.6). `..=` is
    /// reserved and is not a range here either.
    range_expression: ($) => prec.left(PREC.assign + 1, seq($._expression, '..', $._expression)),

    // A closure body reaches as far right as it can: `|x| a + b` is one
    // closure and not a closure plus a sum.
    closure_expression: ($) =>
      prec.right(
        PREC.assign,
        seq(
          choice('||', seq('|', commaSep($.closure_parameter), '|')),
          choice(seq('->', $._type, $.block), $._expression),
        ),
      ),

    closure_parameter: ($) => seq($.identifier, optional(seq(':', $._type))),

    if_expression: ($) =>
      prec.right(
        seq(
          'if',
          field('condition', $._condition),
          field('consequence', $.block),
          optional(seq('else', field('alternative', choice($.block, $.if_expression)))),
        ),
      ),

    match_expression: ($) =>
      seq('match', field('value', $._condition), '{', commaSep($.match_arm), '}'),

    match_arm: ($) =>
      seq(
        field('pattern', $.match_pattern),
        '=>',
        field('value', $._expression),
      ),

    match_pattern: ($) =>
      seq(sepBy1('|', $._pattern), optional(seq('if', field('guard', $._expression)))),

    loop_expression: ($) => seq('loop', $.block),
    while_expression: ($) => seq('while', field('condition', $._condition), $.block),
    for_expression: ($) =>
      seq('for', field('pattern', $._pattern), 'in', field('value', $._condition), $.block),

    /// The expression before a block, where a `{` opens the block and not a
    /// struct literal (§2.8). The compiler has a flag for this; here it is a
    /// separate rule that simply has no struct literal in it.
    _condition: ($) => prec(2, $._expression),

    break_expression: ($) => prec.right(seq('break', optional($._expression))),
    continue_expression: (_) => 'continue',
    return_expression: ($) => prec.right(seq('return', optional($._expression))),

    // ---------------------------------------------------------- patterns

    _pattern: ($) =>
      choice(
        $.wildcard_pattern,
        $._literal,
        $.negative_literal,
        $.identifier,
        $.path_pattern,
        $.tuple_struct_pattern,
        $.struct_pattern,
        $.tuple_pattern,
        $.binding_pattern,
      ),

    wildcard_pattern: (_) => '_',
    negative_literal: ($) => seq('-', $._literal),
    binding_pattern: ($) => seq($.identifier, '@', $._pattern),
    path_pattern: ($) => prec.left(seq($.identifier, repeat1(seq('::', $.identifier)))),
    tuple_struct_pattern: ($) =>
      seq(field('name', choice($.identifier, $.path_pattern)), '(', commaSep($._pattern), ')'),
    struct_pattern: ($) =>
      seq(field('name', choice($.identifier, $.path_pattern)), '{', commaSep($.field_pattern), '}'),
    field_pattern: ($) => seq($.identifier, optional(seq(':', $._pattern))),
    tuple_pattern: ($) => seq('(', commaSep($._pattern), ')'),

    // ----------------------------------------------------------- atoms

    /// A path with `::`, and the turbofish that may sit in it (Ch. 4 §2.3).
    path: ($) =>
      prec.left(seq($.identifier, repeat1(seq('::', choice($.identifier, $.turbofish))))),

    turbofish: ($) => seq('<', commaSep1($._type), '>'),

    _literal: ($) =>
      choice($.integer_literal, $.boolean_literal, $.char_literal, $.string_literal),

    /// One run of digits and letters, which is exactly what the lexer takes
    /// before handing it to `trit-core` — decimal, `0t…` balanced ternary,
    /// `0h…` heptavintimal, and the `1t`/`0t`/`-1t` trit literals (§1.4).
    integer_literal: (_) => token(/[0-9][0-9a-zA-Z_]*/),

    boolean_literal: (_) => choice('true', 'false'),

    /// A character literal, and a lifetime, both open with `'`. They are told
    /// apart by length — `'a'` is three characters and `'a` is two — which is
    /// maximal munch and what the compiler's lexer does (Ch. 0 §1.4).
    char_literal: (_) =>
      token(seq("'", choice(/[^'\\\n]/, seq('\\', choice(/[nrt\\'"0]/, seq('u', '{', /[0-9a-fA-F]{1,6}/, '}')))), "'")),

    string_literal: (_) =>
      token(
        seq(
          '"',
          repeat(choice(/[^"\\\n]/, seq('\\', choice(/[nrt\\'"0]/, seq('u', '{', /[0-9a-fA-F]{1,6}/, '}'))))),
          '"',
        ),
      ),

    lifetime: (_) => token(seq("'", /[a-zA-Z_][a-zA-Z0-9_]*/)),

    identifier: (_) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});
