# Verb formatter + Neovim format-on-save — design

## Goal

Add a code formatter for the Verb language and wire it into the deployed
Neovim setup so `.verb` files reformat on save. Comments (`%% line` and
`!?! block !?!`) are pervasive in real Verb source (`examples/demo.verb`
has 20+) and must survive formatting — a formatter that drops them is a
non-starter for format-on-save.

## Non-goals

- No standalone `verb fmt` CLI subcommand. Integration is LSP-only
  (`textDocument/formatting`), matching how this project already ships
  editor tooling (`verb-lsp`). Nothing else consumes formatting.
- No line-wrapping / long-expression reflow. Verb source in this repo is
  already one-statement-per-line; the formatter normalizes spacing,
  indentation, and comment placement, not line breaking.
- No column-alignment of trailing comments across a block (e.g. lining up
  every `%%` in a function body). Trailing comments get one normalizing
  space before `%%`, nothing fancier.

## Why token-stream, not AST-unparse

Two other approaches were considered and rejected:

- **AST-based pretty-printer**: simplest to build, but `ast::Stmt`/`Expr`
  carry no comment info at all (the lexer discards comments today), and
  `parser.rs` *desugars* `loop init; cond; update begin...end` into
  `Stmt::Block([Assign, Stmt::While{..}])` — unparsing the AST would
  literally rewrite every `loop` into an `assign` + fabricated `repeat`,
  destroying the construct the user wrote. Rejected.
- **tree-sitter/topiary-based**: `editors/tree-sitter-verb`'s CST already
  keeps comments as `extras`, so this would preserve them for free. But
  it pulls in a new toolchain (topiary binary + query language) separate
  from the Rust compiler crate, for a language whose grammar already
  lives natively in Rust twice (`lexer.rs`/`parser.rs`) and once more in
  `tree-sitter-verb/grammar.js`. Rejected as an unnecessary fourth
  grammar copy plus a new external dependency.

**Chosen approach**: a token-stream printer that walks `lexer.rs`'s token
stream directly, structured like `parser.rs`'s recursive-descent grammar
but emitting formatted text instead of an AST. This preserves `loop` and
all comments by construction, since nothing is ever discarded or
resynthesized from a lossy intermediate form.

**Accepted trade-off**: this is a *third* place that encodes Verb's
grammar shape (after `parser.rs` and `tree-sitter-verb/grammar.js`). A
future grammar change means updating three places. This repo already
accepted that cost once (for the tree-sitter grammar); the design does
not attempt to eliminate it, only avoids adding a fourth copy via
topiary.

## Components

### 1. `src/lexer.rs` (additive change)

- Internal scan loop is refactored so both comment styles are captured
  instead of silently skipped.
- New public types/functions:
  ```rust
  pub struct Comment { pub text: String, pub line: u32, pub col: u32 }
  pub fn lex_with_comments(src: &str) -> Result<(Vec<Token>, Vec<Comment>), CompileError>;
  ```
- Existing `pub fn lex(src: &str) -> Result<Vec<Token>, CompileError>`
  keeps its exact current signature and behavior (implemented as
  `lex_with_comments(src).map(|(t, _)| t)` or equivalent) — no change to
  any existing caller, no change to any existing lexer/parser/codegen
  test.

### 2. `src/formatter.rs` (new module, `pub mod formatter` in `lib.rs`)

```rust
pub fn format(src: &str) -> Result<String, CompileError>
```

Steps:

1. **Validate first.** Run the real `lexer::lex` + `parser::parse` (the
   same pipeline the compiler and LSP diagnostics use). If it errors,
   return that `CompileError` unchanged and do not attempt to format.
   This guarantees the formatter only ever touches syntactically valid
   source, and lets the printer below assume well-formed input rather
   than reimplementing error recovery.
2. **Re-lex with comments** via `lex_with_comments`, and merge tokens +
   comments into one stream ordered by source position (line, col).
3. **Print.** A recursive-descent printer shaped like `parser.rs`'s
   `statement()`/`expression()` functions (assign / declare / reassign /
   `make` / `return` / `check`/`orelse` / `repeat` / `loop` / `begin...end`
   / bare expression, then the binary/unary/call precedence chain) walks
   the merged stream and writes to a `String` output buffer with an
   indent-depth counter, rather than building `Stmt`/`Expr` nodes.

Formatting rules:

- **Indent**: 2 spaces per `begin`/`end` nesting level.
- **Operators**: exactly one space around every binary/keyword operator
  (`add`, `sub`, `equals`, `and`, `join`, …) and after unary keyword
  operators (`neg x`, `not ready`).
- **Punctuation**: no space before `;` or `,`; one space after `,`; no
  space just inside `(` `)`.
- **Blocks**: `begin` stays on the same line as the construct that opens
  it (`check cond begin`, `orelse check cond begin`, `make f(params)
  begin`, `repeat cond begin`, `loop init; cond; update begin`); `end` is
  on its own line, dedented to match the opening construct's indent;
  `end orelse ...` stays on one line, same as current `demo.verb` style.
- **Blank lines**: at most one consecutive blank line is preserved
  between statements (source had ≥1 blank line → exactly one in output;
  source had none → none inserted).
- **Comments**: reinserted at their original source position relative to
  surrounding tokens.
  - Comment shared a source line with the token before it → emitted as a
    trailing comment on that same output line (`<code>  %% ...`).
  - Otherwise → emitted on its own output line at the current indent.
  - Block comments (`!?! ... !?!`) are preserved verbatim internally
    (interior text/newlines untouched); only the placement (own-line vs
    trailing) and the opening line's indent follow the rules above.

### 3. `src/bin/verb-lsp.rs`

- `initialize_result()` capabilities gain `"documentFormattingProvider": true`.
- New handled method `"textDocument/formatting"`:
  - Look up the document text by URI from the existing `docs` map (same
    pattern as `hover`/`completion`).
  - Call `formatter::format(src)`.
  - On `Ok(new_text)`: respond with a single `TextEdit` covering the
    whole document (range from `(0,0)` to end-of-file, computed from
    `src`), `newText: new_text`.
  - On `Err(_)`: respond with `null` (no edits) — matches existing
    diagnostics already surfacing the syntax error; formatting silently
    no-ops rather than erroring the LSP request.

### 4. Neovim config (outside this repo)

`~/.config/nvim/lua/steffbue/plugins/verb-lsp.lua` — inside the existing
`config = function() ... end`, after `vim.lsp.enable("verb")`, add:

```lua
vim.api.nvim_create_autocmd("BufWritePre", {
  pattern = "*.verb",
  callback = function()
    vim.lsp.buf.format({ async = false })
  end,
})
```

No new plugin, no conform.nvim — reuses the already-attached `verb-lsp`
client. Requires a `cargo build --release` of this repo first so the
deployed binary has the new capability.

## Testing plan

- Unit tests in `formatter.rs` (`#[cfg(test)]`, following this repo's
  existing test style):
  - Idempotency: `format(&format(src)?)? == format(src)?` for hand-written
    snippets covering every statement kind.
  - Comment preservation: standalone line comment, trailing line comment,
    standalone block comment, trailing block comment — each still present
    verbatim (modulo placement rule) in the output.
  - Indentation: nested `begin`/`end` (including `make` inside `check`
    inside `repeat`) produces the expected 2-space-per-level output.
  - `check ... orelse check ... orelse begin` chain stays on the
    documented single-line-`end orelse`-style.
  - `loop init; cond; update begin ... end` round-trips as a `loop`, not
    as a desugared `assign` + `repeat` (the case that ruled out
    AST-unparse).
  - Blank-line collapsing: 0, 1, and 3 blank lines between two statements
    all produce the same output (none / one / one).
  - Invalid input (e.g. missing `;`) returns `Err` and does not panic.
- Integration: round-trip every valid fixture (`tests/fixtures/*.verb`
  minus the `err_*.verb` ones) and `examples/demo.verb` through
  `format()`; assert the result still lexes+parses successfully and is
  idempotent.
- Manual verification: rebuild `verb-lsp` in release mode, save a
  `.verb` file in the deployed Neovim config, confirm it reformats and
  every comment (line + block, standalone + trailing) survives.
