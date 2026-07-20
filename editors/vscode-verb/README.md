# vscode-verb

VSCode language support for Verb: LSP client (hover, completion,
diagnostics, format-on-save via `verb-lsp`) plus tree-sitter-driven syntax
highlighting. Local dev tool only — not published to the Marketplace;
install unpacked or from a locally-built `.vsix`, same as this repo's
Neovim setup (`../nvim`, `../tree-sitter-verb`).

## Build

Two build steps, in order — the grammar WASM first, then the extension:

```sh
# 1. Build the tree-sitter grammar to WASM (first run pulls an emscripten
#    Docker image if you don't have emscripten installed directly — expect
#    several minutes; subsequent runs are fast).
cd ../tree-sitter-verb
npm install
npm run build:wasm   # always use this script, never bare `npx tree-sitter
                      # build --wasm` — see ../README.md's tree-sitter-verb
                      # section for why (ABI pin)

# 2. Build and package the extension
cd ../vscode-verb
npm install
npm run build         # tsc + copies both WASM files into parsers/
npm run package        # produces vscode-verb-<version>.vsix
```

Also make sure the LSP binary this extension points at is built:

```sh
cargo build --release   # from the repo root; produces target/release/verb-lsp
```

## Install

```sh
code --install-extension vscode-verb-<version>.vsix
```

Or install unpacked: open this directory in VSCode and use "Run Extension"
from the Debug panel (Extension Development Host), or `code --extensionDevelopmentPath=.`.

## Settings

- `verb.lspPath` (string) — absolute path to the `verb-lsp` binary. Defaults
  to `/Users/steffen/Desktop/Projekte/compiler/target/release/verb-lsp`
  (this machine's repo path, matching the Neovim config's hardcoded-path
  convention — see `../README.md`). Override in VSCode settings if the repo
  lives elsewhere.
- `editor.formatOnSave` is enabled by default for `.verb` files
  (`configurationDefaults` in `package.json`) — no separate opt-in needed.

## What's implemented

- **LSP client** (`src/extension.ts`): stdio-connects to `verb-lsp`,
  wired for `.verb` files. Gets you hover, completion, diagnostics, and
  format-on-save for free from the server — see `../README.md`'s
  `verb-lsp` section for what the server itself supports.
- **Syntax highlighting** (`src/highlight.ts`): a VSCode semantic tokens
  provider that parses each document with the `tree-sitter-verb` grammar
  (compiled to WASM, loaded once at activation via `web-tree-sitter`) and
  maps parse-tree nodes to the semantic token legend
  `["comment", "string", "number", "keyword", "operator", "function",
  "variable", "parameter"]`. Reuses the real grammar — no separate/duplicated
  syntax rules, no TextMate grammar.

## Manual verification

No automated extension-host test suite (disproportionate for a
single-user local tool). After installing, verify by hand:

1. Build succeeds, extension installs without error.
2. Open `../../examples/demo.verb` — keywords/strings/numbers/comments are
   colored, and the `check ... orelse ... orelse begin` chain and
   `loop init; cond; update begin ... end` highlight correctly (not
   mis-highlighted the way a lossy AST-based approach would be).
3. Hover over an identifier or keyword, and trigger completion — both
   resolve, proving the LSP client is wired correctly.
4. Introduce a syntax error (e.g. an unmatched `(`) — a diagnostic squiggle
   appears at the right location, and highlighting degrades gracefully
   around the bad region (no crash, no exception in the extension host log).
5. Save a file with a stray double space and a trailing `%%` comment —
   format-on-save fixes the spacing and preserves the comment.

## Troubleshooting

- **Extension fails to start / "client failed to start" notification:**
  `verb.lspPath` doesn't resolve to a runnable binary — check the setting
  and that `cargo build --release` has been run. Highlighting still works
  independently (it doesn't depend on the LSP client).
- **No highlighting, or everything looks unstyled:** check the Extension
  Host output panel for a `web-tree-sitter`/WASM load error — usually means
  `npm run build` wasn't run before packaging (missing `parsers/*.wasm`), or
  the extension was packaged before `npm run build:wasm` produced
  `../tree-sitter-verb/tree-sitter-verb.wasm`.
- **Regenerating the grammar:** see `../README.md`'s tree-sitter-verb
  section — always use the `generate`/`build:wasm` npm scripts, never a bare
  `npx tree-sitter generate` (ABI pin, explained there).
