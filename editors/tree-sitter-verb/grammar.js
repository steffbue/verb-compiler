/**
 * @file Verb grammar for tree-sitter
 * @author Verb language
 * @license MIT
 *
 * Mirrors src/lexer.rs and src/parser.rs of the Verb compiler:
 *   - line comments:  %% ... \n
 *   - block comments: !?! ... !?!
 *   - statements: assign / declare / reassign (`x be expr;`) / make / return /
 *     check ... orelse / repeat / loop / begin...end / import (`mod`/`std`) /
 *     bare expression
 *   - expression precedence (low -> high): or, and, equals/differs,
 *     trails/beats/atmost/atleast, add/sub/join, times/div/mod, unary
 *     (not/neg), call
 */

/* eslint-disable arrow-parens */

const PREC = {
  or: 1,
  and: 2,
  equality: 3,
  comparison: 4,
  additive: 5,
  multiplicative: 6,
  unary: 7,
  call: 8,
};

module.exports = grammar({
  name: "verb",

  extras: ($) => [/\s/, $.line_comment, $.block_comment],

  word: ($) => $.identifier,

  rules: {
    source_file: ($) => repeat($._statement),

    // ----- statements -----

    _statement: ($) =>
      choice(
        $.import_statement,
        $.assign_statement,
        $.declare_statement,
        $.fn_statement,
        $.return_statement,
        $.if_statement,
        $.while_statement,
        $.for_statement,
        $.foreach_statement,
        $.block,
        $.reassign_statement,
        $.expression_statement,
      ),

    // `import mod <library>;` (generic C++ extern library) or
    // `import std <module>;` (built-in stdlib module, e.g. `io`) — see
    // README.md's "Importing C++ libraries" / "Standard library I/O"
    // sections. Both must appear before any other top-level statement;
    // that ordering rule is enforced by src/parser.rs, not the grammar.
    import_statement: ($) =>
      seq(
        "import",
        choice(
          seq("mod", field("library", $.identifier)),
          seq("std", field("module", $.identifier)),
        ),
        ";",
      ),

    assign_statement: ($) =>
      seq("assign", field("name", $.identifier), field("value", $._expression), ";"),

    declare_statement: ($) => seq("declare", field("name", $.identifier), ";"),

    // `x be expr;` — used both as a standalone statement and (without the
    // trailing `;`) as the update clause of a `loop` header, see below.
    reassign_statement: ($) => seq($._reassign_no_semi, ";"),

    _reassign_no_semi: ($) =>
      seq(field("name", $.identifier), "be", field("value", $._expression)),

    fn_statement: ($) =>
      seq(
        "make",
        field("name", $.identifier),
        field("parameters", $.parameters),
        field("body", $.block),
      ),

    parameters: ($) => seq("(", commaSep($.identifier), ")"),

    return_statement: ($) => seq("return", optional(field("value", $._expression)), ";"),

    if_statement: ($) =>
      seq(
        "check",
        field("condition", $._expression),
        field("consequence", $.block),
        optional(seq("orelse", field("alternative", choice($.if_statement, $.block)))),
      ),

    while_statement: ($) =>
      seq("repeat", field("condition", $._expression), field("body", $.block)),

    // `loop <init>; <condition>; <update> begin ... end`
    // <init> is `assign x expr;` or `x be expr;` (both already end in `;`).
    for_statement: ($) =>
      seq(
        "loop",
        field("initializer", choice($.assign_statement, $.reassign_statement)),
        field("condition", $._expression),
        ";",
        field("update", choice($._reassign_no_semi, $._expression)),
        field("body", $.block),
      ),

    // `each <var> in <collection> begin ... end` iterates an array/string/map;
    // `each <var> in <start> to <end> begin ... end` is a half-open integer
    // range (`start` up to, but not including, `end`). Mirrors src/parser.rs
    // `foreach_stmt` — the range form desugars to a `repeat` in the compiler,
    // but is written with `to` in source.
    foreach_statement: ($) =>
      seq(
        "each",
        field("variable", $.identifier),
        "in",
        field("iterable", $._expression),
        optional(seq("to", field("range_end", $._expression))),
        field("body", $.block),
      ),

    block: ($) => seq("begin", repeat($._statement), "end"),

    expression_statement: ($) => seq($._expression, ";"),

    // ----- expressions -----

    _expression: ($) =>
      choice(
        $.binary_expression,
        $.unary_expression,
        $.call_expression,
        $.parenthesized_expression,
        $.int,
        $.float,
        $.string,
        $.true,
        $.false,
        $.nil,
        $.identifier,
      ),

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    binary_expression: ($) =>
      choice(
        ...[
          [PREC.or, "or"],
          [PREC.and, "and"],
        ].map(([p, op]) =>
          prec.left(
            p,
            seq(field("left", $._expression), field("operator", op), field("right", $._expression)),
          ),
        ),
        prec.left(
          PREC.equality,
          seq(
            field("left", $._expression),
            field("operator", choice("equals", "differs")),
            field("right", $._expression),
          ),
        ),
        prec.left(
          PREC.comparison,
          seq(
            field("left", $._expression),
            field("operator", choice("trails", "beats", "atmost", "atleast")),
            field("right", $._expression),
          ),
        ),
        prec.left(
          PREC.additive,
          seq(
            field("left", $._expression),
            field("operator", choice("add", "sub", "join")),
            field("right", $._expression),
          ),
        ),
        prec.left(
          PREC.multiplicative,
          seq(
            field("left", $._expression),
            field("operator", choice("times", "div", "mod")),
            field("right", $._expression),
          ),
        ),
      ),

    unary_expression: ($) =>
      prec(PREC.unary, seq(field("operator", choice("not", "neg")), field("operand", $._expression))),

    call_expression: ($) =>
      prec(
        PREC.call,
        seq(field("function", $._expression), field("arguments", $.arguments)),
      ),

    arguments: ($) => seq("(", commaSep($._expression), ")"),

    // ----- literals & tokens -----

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    int: (_) => /[0-9]+/,
    float: (_) => /[0-9]+\.[0-9]+/,

    string: ($) =>
      seq('"', repeat(choice($.escape_sequence, /[^"\\\n]+/)), '"'),
    escape_sequence: (_) => /\\["\\nt]/,

    true: (_) => "true",
    false: (_) => "false",
    nil: (_) => "nil",

    line_comment: (_) => token(seq("%%", /[^\n]*/)),
    block_comment: (_) => token(seq("!?!", /[\s\S]*?/, "!?!")),
  },
});

function commaSep(rule) {
  return optional(seq(rule, repeat(seq(",", rule))));
}
