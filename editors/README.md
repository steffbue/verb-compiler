# Editor support

## verb-lsp

`src/bin/verb-lsp.rs` is a minimal LSP server, built as part of this same
Cargo package (`cargo build --release`, binary at
`target/release/verb-lsp`). No async runtime — the whole protocol is
handled as one blocking read/dispatch/write loop over stdio
(Content-Length-framed JSON-RPC via `serde_json`), which is plenty for a
language this small.

Diagnostics reuse the real compiler pipeline directly — `lexer::lex` ->
`parser::parse_recovering` -> `codegen::Codegen::compile_program` (the
same checks `verb run` does, minus JIT execution) — so whatever the LSP
flags is exactly what the compiler would reject, both syntax errors and
semantic ones (undefined variables, arity mismatches, type errors, etc.).

**Multiple syntax errors surface at once.** `parser::parse_recovering`
(`src/parser.rs`) is a second entry point alongside the compiler's normal
`parse` — same grammar, but instead of stopping at the first bad
statement, it synchronizes to the next likely statement boundary (a
`;`/`end`, or a keyword that starts a new statement) and keeps going,
collecting every statement it could parse and every error it hit. The
CLI/compiler still use plain `parse`, which stops at the first error like
a normal compiler — this recovery mode exists only for the LSP.

Semantic errors from codegen are still one-at-a-time: codegen bails at
the first `CompileError` (its LLVM builder position and scope stack
aren't something you can resume cleanly mid-error), so once a file
parses cleanly, only the first type/scope error shows. Fixing that would
mean redesigning how codegen tracks state across a broken statement — a
much bigger change than recovering the parser was.

Recovery correctness is guarded by `tests/parser_recovery_fuzz.rs`, which
deletes single and paired tokens from every real `.verb` fixture and
asserts `parse_recovering` always terminates (each run happens on its
own thread with a hard deadline, so a regression fails the test instead
of hanging `cargo test`) — this is how a real infinite-loop bug in the
recovery logic (a `return` outside a function fails without consuming
its own token, and the naive synchronizer treated that same token as
already-safe) got caught before shipping, rather than by hanging
someone's editor.

Also implements:
- `textDocument/hover` — keyword/operator docs, plus best-effort
  "variable"/"function" info for identifiers, from a lightweight AST walk
  (not full semantic analysis — no scoping).
- `textDocument/completion` — all keywords/operators, plus identifiers
  found in the current document.
- `textDocument/formatting` — backed by `src/formatter.rs`, a single
  left-to-right pass over `lexer::lex_with_comments`'s token+comment
  stream (not the AST — `Stmt`/`Expr` carry no comment info, and
  `parser.rs` desugars `loop` into a fabricated `assign`+`repeat`, so
  unparsing the AST would drop comments and rewrite every `loop`).
  Canonicalizes spacing/indentation (2 spaces per `begin`/`end` level,
  one statement per line inside a block) and reinserts every comment at
  its original position (trailing vs. own-line), collapsing runs of
  blank lines to at most one. Only ever formats source that
  `parser::parse` already accepts — a syntax error leaves the buffer
  untouched (same document the diagnostic already points at). Wired up
  as Neovim format-on-save below.

## verb-lsp: on-save formatting (deployed)

`~/.config/nvim/lua/steffbue/plugins/verb-lsp.lua`'s `config` function
registers a `BufWritePre` autocmd for `*.verb` that calls
`vim.lsp.buf.format({ async = false })` before every save — no separate
plugin or `verb fmt` CLI, it reuses the already-attached `verb-lsp`
client's `textDocument/formatting`. Rebuild `verb-lsp` (`cargo build
--release`) and `:LspRestart` after editing `src/formatter.rs` to pick up
formatter changes.

To make this work, the project moved from a single `main.rs` binary to a
`lib.rs` + `src/bin/*.rs` layout: `ast`/`codegen`/`error`/`lexer`/`parser`/
`value` are now `pub mod`s in `src/lib.rs`, and both `src/main.rs` (the
`verb` CLI) and `src/bin/verb-lsp.rs` depend on them as the `verb` library
crate.

**Deployed setup** (already done on this machine): registered in
`/Users/steffen/.config/nvim/lua/steffbue/plugins/verb-lsp.lua` via
Neovim 0.11's native `vim.lsp.config`/`vim.lsp.enable` (not
nvim-lspconfig/mason — this server isn't in either registry). Verified
headless against the real nvim config: client attaches on `.verb`
buffers, zero false-positive diagnostics on valid code, correct
diagnostic + range on both a syntax error and a semantic one
(`tests/fixtures/err_undef.verb`), and hover resolves both keyword docs
and variable/function info.

Rebuild after editing the server: `cargo build --release` in this repo
(the nvim config's `cmd` points at the `target/release/verb-lsp` path
directly, so `:LspRestart` after a rebuild picks up the new binary).

## tree-sitter-verb

`tree-sitter-verb/` is a standalone [tree-sitter](https://tree-sitter.github.io/tree-sitter/)
grammar for the Verb language, mirroring `src/lexer.rs` and `src/parser.rs`:
statements (`assign`/`declare`/`x be ...`/`make`/`return`/`check ... orelse`/
`repeat`/`loop`/`begin...end`), the full binary/unary operator precedence
table, string escapes, and both comment styles (`%% line` and `!?! block !?!`).

It ships pre-generated (`src/parser.c`, `src/grammar.json`,
`src/node-types.json` are committed) so consumers only need a C compiler to
build it — Node/the `tree-sitter` CLI are only needed if you're editing
`grammar.js` itself. To regenerate after a grammar change:

```sh
cd editors/tree-sitter-verb
npm install          # pulls in tree-sitter-cli as a devDependency
npm run generate     # tree-sitter generate --abi=14 (see below — do not run
                      # `npx tree-sitter generate` bare)
npx tree-sitter test        # corpus tests in test/corpus/
npx tree-sitter parse ../../examples/demo.verb   # sanity check
```

**Always regenerate via the `generate` (or `build:wasm`) npm script, never
bare `npx tree-sitter generate`.** `tree-sitter.json`'s presence in this
directory changes the CLI's *default* ABI to 15, but `src/parser.c` and the
other committed generated files are ABI 14 — the npm scripts pin
`--abi=14` explicitly; a bare `npx tree-sitter generate` does not, and will
silently regenerate `src/parser.c` at the wrong ABI.

The canonical query files live in `tree-sitter-verb/queries/`:

- `highlights.scm` — syntax highlighting
- `locals.scm` — scope/variable-reference tracking
- `indents.scm` — auto-indent hints

`nvim/queries/verb/*.scm` are symlinks back to these — don't edit them
directly, edit the `tree-sitter-verb/queries/` originals.

## Neovim (nvim-treesitter)

`nvim/plugin/verb.lua` targets the **classic/`master` branch** API of
[nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)
(`nvim-treesitter.configs` + `nvim-treesitter.parsers`, `install_info =
{ url, files }`, `ensure_installed`/`highlight.enable` style config) —
that's what's actually installed for this setup, not the newer rewritten
`main` branch, which uses a different registration mechanism entirely.
On load it registers `*.verb` as the `verb` filetype and adds a `verb`
entry to nvim-treesitter's parser registry pointing at the local
`../tree-sitter-verb` directory (`install_info.url` as a local path, not
a git URL — nvim-treesitter detects this and installs in place instead of
cloning). Highlighting/indent then come for free from the existing global
`highlight.enable = true` / `indent.enable = true` config — no
`vim.treesitter.start()` call needed on top, unlike the `main`-branch
integration pattern.

**Deployed setup** (already done on this machine):

- `editors/nvim` is loaded as a lazy.nvim local plugin via
  `/Users/steffen/.config/nvim/lua/steffbue/plugins/verb-treesitter.lua`,
  which sets `dir` to this repo's `editors/nvim` and depends on
  `nvim-treesitter/nvim-treesitter` (so it's guaranteed to load after it).
- The parser was compiled once with `:TSInstallSync verb` (needs a C
  compiler — `cc`/`gcc`/`clang` — on `PATH`; not Node or the `tree-sitter`
  CLI, since `src/parser.c` is pre-generated and committed).
- Verified headless against `examples/demo.verb`: filetype resolves to
  `verb`, the parser attaches and parses with zero error nodes, and
  highlight captures resolve correctly (e.g. `assign` → `@keyword`, the
  `%%` header → `@comment`).

To reuse this setup elsewhere (a different machine, or after moving the
compiler repo), update the `dir` path in `verb-treesitter.lua` to match
the new location, then run `:TSInstallSync verb` again there — the
compiled parser lives under nvim-treesitter's own install dir, not in
this repo, so it must be rebuilt per machine.

### Troubleshooting

- `:checkhealth nvim-treesitter` reports on parser install status.
- If `:TSInstall verb` can't find the `verb` parser, confirm
  `verb-treesitter.lua`'s `dir` still points at a valid `editors/nvim`
  checkout and that lazy.nvim actually loaded it (`:Lazy` → look for
  `verb-treesitter`).
- `:InspectTree` (or `:Inspect` under the cursor) shows the live parse
  tree for the current buffer — useful when tweaking `queries/*.scm`.
- If you're on nvim-treesitter's newer `main` branch instead, the
  registration mechanism is different (a `User TSUpdate` autocmd
  assigning into `require('nvim-treesitter.parsers')` directly, plus
  `install_info.queries` for symlinking query dirs, plus explicit
  `vim.treesitter.start()` in a `FileType` autocmd) — ask for that
  variant specifically if this project ever upgrades.
