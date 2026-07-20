# VSCode extension for Verb — LSP client, formatter, tree-sitter highlighting

## Goal

Add a VSCode extension that gives `.verb` files the same editing experience
the deployed Neovim config already has: LSP (hover, completion,
diagnostics), format-on-save, and tree-sitter-driven syntax highlighting.

## Non-goals

- No publishing to the VSCode Marketplace. This is a local dev tool,
  installed unpacked or via a locally-built `.vsix`, matching how
  `verb-lsp`/the nvim config are deployed today (build locally, point the
  editor at it).
- No automated extension-host test suite. Disproportionate for a
  single-user tool; verification is manual (documented steps below).
- No debugger integration, code snippets, or other editor features beyond
  LSP + formatting + highlighting.
- No incremental tree-sitter re-parsing (edit-range tracking). Verb source
  files are small; a full reparse per semantic-tokens request is cheap
  enough. Revisit only if it's ever noticeably slow.
- No TextMate grammar. Would be a 4th copy of Verb's syntax rules (after
  `lexer.rs`/`parser.rs`, `tree-sitter-verb/grammar.js`, and now this);
  rejected for the same reason the formatter design rejected a topiary
  dependency.

## Why tree-sitter WASM, not TextMate

VSCode's native/simplest highlighting mechanism is a TextMate grammar
(regex-based `.tmLanguage.json`). Considered and rejected: it would
duplicate grammar rules already expressed three other places in this repo,
with no shared source of truth — a change to Verb's syntax would need a
4th manual update to stay in sync.

**Chosen approach**: compile the existing `editors/tree-sitter-verb`
grammar to WASM (`tree-sitter build --wasm`) and load it at runtime with
`web-tree-sitter`. A semantic-tokens provider walks the parsed tree using
the same node/field patterns as `editors/tree-sitter-verb/queries/highlights.scm`
and emits VSCode semantic tokens. This reuses the real grammar — no new
syntax rules written, no duplication.

**Accepted trade-off**: needs a one-time WASM build step. No emscripten is
installed on this machine, but `tree-sitter build --wasm` falls back to
Docker automatically (Docker is present), pulling an emscripten image on
first run. If Docker ever becomes unavailable, the fallback is to install
emscripten directly, or (last resort) revisit a TextMate grammar — not
built now.

## Components

### 1. `editors/vscode-verb/package.json` (extension manifest)

- `contributes.languages`: id `verb`, `extensions: [".verb"]`,
  `configuration: "./language-configuration.json"`.
- `contributes.configuration`: setting `verb.lspPath` (string), default
  `/Users/steffen/Desktop/Projekte/compiler/target/release/verb-lsp`
  (same binary path the nvim config hardcodes).
- `contributes.configurationDefaults`: `"[verb]": { "editor.formatOnSave": true }`
  — mirrors the nvim `BufWritePre` autocmd, so format-on-save is on by
  default rather than requiring manual opt-in per workspace.
- `main`: `./out/extension.js`.
- `activationEvents`: `["onLanguage:verb"]`.
- `engines.vscode` pinned to the installed VSCode version's minor (`1.127.x`
  at design time).

### 2. `editors/vscode-verb/language-configuration.json`

- `comments`: `lineComment: "%%"`, `blockComment: ["!?!", "!?!"]`.
- `brackets`: `[["(", ")"]]`.
- `autoClosingPairs` / `surroundingPairs`: `( )`, `" "`.

### 3. `editors/vscode-verb/src/extension.ts`

- `activate(context)`:
  - Read `verb.lspPath` from configuration.
  - Start a `vscode-languageclient` `LanguageClient` with stdio transport,
    `command: <resolved lspPath>`, `documentSelector: [{ language: "verb" }]`.
  - Register the semantic tokens provider from `highlight.ts` for
    language `verb`.
  - Push both to `context.subscriptions` for clean disposal.

### 4. `editors/vscode-verb/src/highlight.ts`

- On extension activation, load `web-tree-sitter`'s runtime
  (`parsers/tree-sitter.wasm`) once, then load the Verb grammar
  (`parsers/tree-sitter-verb.wasm`) as a `Parser.Language`.
- Semantic tokens legend (custom, minimal — VSCode standard types that
  exist are reused, no new type names invented):
  `["comment", "string", "number", "keyword", "operator", "function", "variable", "parameter"]`
- `provideDocumentSemanticTokens(document)`:
  1. Parse `document.getText()` with the loaded grammar (full reparse).
  2. Walk the tree; for each node, match the same patterns as
     `highlights.scm` and map to a legend index:
     - `int` → number, `float` → number
     - `string`, `escape_sequence` → string
     - `true`, `false`, `nil` → keyword
     - `line_comment`, `block_comment` → comment
     - keyword literals (`assign`, `declare`, `be`, `make`, `return`,
       `check`, `orelse`, `repeat`, `loop`, `begin`, `end`) → keyword
     - operator literals (`add sub times div mod`, `equals differs trails
       beats atmost atleast`, `and or not neg join`) → operator
     - `fn_statement.name`, `call_expression.function` → function
     - `parameters` children → parameter
     - `assign_statement.name`, `declare_statement.name`,
       `reassign_statement.name`, bare `identifier` → variable
  3. Nodes with no mapping (punctuation, whitespace) are skipped —
     VSCode leaves them with default coloring.
  4. Build and return a `SemanticTokensBuilder` result.
- No modifiers used (YAGNI — nothing in the current highlight rules needs
  them).

### 5. Build pipeline

- `editors/tree-sitter-verb`: `npm install` (installs `tree-sitter-cli`
  already listed in `package.json`), then `npx tree-sitter generate &&
  npx tree-sitter build --wasm` → produces `tree-sitter-verb.wasm`.
- `editors/vscode-verb`: `npm run build` runs, in order:
  1. `tsc` (compiles `src/*.ts` → `out/`).
  2. Copy `node_modules/web-tree-sitter/tree-sitter.wasm` →
     `parsers/tree-sitter.wasm`.
  3. Copy `../tree-sitter-verb/tree-sitter-verb.wasm` →
     `parsers/tree-sitter-verb.wasm`.
- Result is self-contained: packaging with `vsce package` (or installing
  the unpacked folder directly) doesn't depend on sibling directories at
  runtime, unlike the nvim config's live symlink approach.
- `web-tree-sitter` version pinned to match `tree-sitter-cli`'s ABI
  (`^0.25`, same major as the `tree-sitter-cli` devDependency already in
  `tree-sitter-verb/package.json`) — mismatched ABI versions fail to load
  the compiled grammar at runtime.

## Data flow

- **Highlighting**: editor requests semantic tokens on open/edit →
  `highlight.ts` reparses full text with `web-tree-sitter` → walks tree →
  emits legend-indexed token ranges → VSCode paints via the user's theme.
  Fully local, no server round-trip.
- **LSP**: file open/edit → `didOpen`/`didChange` sent to `verb-lsp` over
  stdio → server reuses `lexer::lex` / `parser::parse_recovering` /
  `codegen::Codegen::compile_program` for diagnostics (already built, see
  `editors/README.md`), same `hover`/`completion`/`textDocument/formatting`
  handlers already implemented and manually verified. Format-on-save →
  VSCode's built-in save hook (enabled via `configurationDefaults`) →
  `textDocument/formatting` → single whole-document `TextEdit`.
- The two systems are independent processes/mechanisms and don't
  communicate with each other.

## Error handling

- Grammar WASM fails to parse mid-edit (syntax error): `web-tree-sitter`
  still returns a best-effort tree containing `ERROR` nodes. Those nodes
  have no legend mapping, so they're simply left with default/no semantic
  coloring — no crash, no exception surfaced to the user.
- `verb.lspPath` doesn't resolve to a runnable binary: `LanguageClient`
  start fails; VSCode shows its standard client-failed-to-start
  notification. Highlighting is unaffected (independent of the LSP
  client).

## Testing plan

No automated extension-host test suite (see Non-goals). Manual
verification, documented in `editors/vscode-verb/README.md`:

1. Build: `tree-sitter-verb` WASM build, then `vscode-verb`'s `npm run
   build`. Install unpacked or via a locally-packaged `.vsix`.
2. Open `examples/demo.verb` — confirm keywords/strings/numbers/comments
   are colored, and that the `check ... orelse ... orelse begin` chain and
   `loop init; cond; update begin ... end` don't mishighlight (the same
   constructs the formatter's design flagged as easy to get wrong via a
   lossy intermediate representation).
3. Trigger hover and completion — confirm same behavior as the nvim setup,
   proving the LSP client is wired correctly.
4. Introduce a syntax error — confirm a diagnostic squiggle appears and
   highlighting degrades gracefully (no crash) around the bad region.
5. Save a file with a stray double-space and a trailing `%%` comment —
   confirm format-on-save fixes spacing and preserves the comment (same
   fixture shape used for the formatter's manual LSP JSON-RPC
   verification).
