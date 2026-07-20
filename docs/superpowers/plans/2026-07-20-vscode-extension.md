# VSCode extension for Verb — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: `superpowers:subagent-driven-development`.
> This plan is split into tasks with an explicit **Developer** sub-checklist and a
> separate **Reviewer** sub-checklist per task — a task is not done until its
> Reviewer checklist passes against the actual diff, not against the developer's
> self-report. Tasks are grouped into waves; tasks in the same wave have no
> dependency on each other and may be assigned to different agents concurrently.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ship `editors/vscode-verb`, a local (unpublished) VSCode extension giving
`.verb` files LSP (hover/completion/diagnostics/format-on-save) + tree-sitter
semantic-token highlighting, per the accepted design.

**Spec:** `docs/superpowers/specs/2026-07-20-vscode-extension-design.md` — read it
first; this plan does not repeat rule-level detail already pinned there (semantic
token legend, node→token mapping table, error-handling behavior). Where this plan
and the spec conflict, the spec wins; flag the conflict to the user instead of
silently picking one.

**Prior art already in the repo (do not re-derive):**
- `editors/tree-sitter-verb/` — grammar ships pre-generated (`src/parser.c`,
  `src/grammar.json`, `src/node-types.json` committed), plus
  `queries/highlights.scm` this task reuses the *patterns* from (not the file
  itself — VSCode semantic tokens are computed in TS, not via `.scm` queries).
- `src/bin/verb-lsp.rs` — LSP server is complete and already implements
  `hover`/`completion`/`textDocument/formatting`; this plan only *points* an
  extension at its binary path, it does not modify the server.
- `editors/nvim/`, `editors/README.md` — the Neovim integration this extension
  mirrors; useful as a reference for "what correct behavior looks like" but not
  code this task reuses directly (different client mechanism).

## Environment facts (verified at plan time — re-verify if stale)

- `node v24.11.1`, `npm 11.6.2` — present.
- `docker 24.0.7` — present, engine running. Required: no emscripten installed on
  this machine, so `tree-sitter build --wasm` needs Docker's fallback.
- `vsce` (bare package) is **not** installed and is deprecated; use
  `@vscode/vsce` (scoped package) via `npx @vscode/vsce package`, not `npx vsce`.
- No VSCode version was queried yet — Task 1 must run `code --version` (or check
  `Code.app`'s `Info.plist`/About dialog if the `code` CLI shim isn't on `PATH`)
  and pin `engines.vscode` to that installed minor, not guess.

## Global constraints

- Directory root for all new code: `editors/vscode-verb/`. Nothing outside
  `editors/` is touched by this plan (the LSP server and grammar are consumed,
  not modified).
- `verb.lspPath` setting default is an absolute path
  (`/Users/steffen/Desktop/Projekte/compiler/target/release/verb-lsp`) per spec —
  this is intentionally machine-specific, matching the nvim config's existing
  hardcoded-path convention. Do not make it relative or try to auto-discover it.
- No Marketplace publish, no extension-host test suite, no TextMate grammar, no
  incremental tree-sitter reparse — see spec's Non-goals. A task that starts
  adding any of these is out of scope; stop and flag it instead.
- `web-tree-sitter` version must match `tree-sitter-cli`'s ABI (`^0.25`, same
  major as `tree-sitter-verb/package.json`'s `tree-sitter-cli` devDependency).
  Mismatched ABI fails to load the compiled grammar at runtime with an opaque
  error — if Task 4 hits a load failure, check this first.
- Every task's Reviewer checklist includes: does the diff stay inside this
  task's declared file list, and does it avoid touching sibling tasks' files
  (waves run concurrently — cross-file edits cause merge conflicts between
  agents, not just review nits).

## Task graph

```
Wave 1 (solo, blocks everything else):
  Task 1: extension scaffold (package.json, tsconfig, language-configuration.json)

Wave 2 (parallel, both depend only on Task 1):
  Task 2: tree-sitter WASM build pipeline
  Task 3: extension.ts — LSP client wiring

Wave 3 (depends on Task 1 + Task 2; may start once Task 2 lands, doesn't need Task 3):
  Task 4: highlight.ts — semantic tokens provider

Wave 4 (depends on Tasks 1-4, integrates everything):
  Task 5: build pipeline wiring + packaging

Wave 5 (depends on Task 5, final gate):
  Task 6: manual verification + editors/vscode-verb/README.md
```

---

### Task 1: Extension scaffold

**Files:**
- Create: `editors/vscode-verb/package.json`, `editors/vscode-verb/tsconfig.json`,
  `editors/vscode-verb/language-configuration.json`,
  `editors/vscode-verb/.gitignore` (ignore `out/`, `node_modules/`, `parsers/*.wasm`)
- Create empty dirs (via placeholder or first real file from Task 3/4):
  `editors/vscode-verb/src/`, `editors/vscode-verb/parsers/`

**Interfaces:**
- Produces: the `package.json` contract every later task builds against —
  `main: "./out/extension.js"`, `activationEvents: ["onLanguage:verb"]`,
  `contributes.languages`/`configuration`/`configurationDefaults` per spec §1,
  `scripts.build` (stub for now — Task 5 fills in the real pipeline; Task 1 can
  leave it as `"echo 'see Task 5' && exit 1"` or a `tsc`-only stub, developer's
  call, just don't leave it silently succeeding with no output).
- Consumes: nothing.

- [ ] **Step 1:** Run `code --version` (or equivalent) to get the actual
  installed VSCode version; record it in a code comment or commit message —
  Reviewer needs to verify `engines.vscode` matches what was actually found,
  not a guess.
- [ ] **Step 2:** Write `package.json` per spec §1. Include
  `devDependencies`: `typescript`, `@types/vscode` (pinned to the `engines.vscode`
  minor), `@types/node`, `vscode-languageclient`, `web-tree-sitter` (`^0.25`).
- [ ] **Step 3:** Write `language-configuration.json` per spec §2
  (`lineComment: "%%"`, `blockComment: ["!?!", "!?!"]`, `brackets: [["(", ")"]]`,
  matching `autoClosingPairs`/`surroundingPairs`).
- [ ] **Step 4:** Write `tsconfig.json` — `target` compatible with the
  `@types/vscode` version chosen, `module: "commonjs"` (VSCode extension host
  requirement, not ESM), `outDir: "out"`, `rootDir: "src"`, `strict: true`.
- [ ] **Step 5:** `npm install` inside `editors/vscode-verb/` — verify it
  resolves with no peer-dep errors and produces `package-lock.json`.
- [ ] **Step 6: Reviewer checklist**
  - [ ] `engines.vscode` matches the version from Step 1, not the spec's
    placeholder `1.127.x` verbatim (unless that happens to be what Step 1 found).
  - [ ] `contributes.configurationDefaults` sets `editor.formatOnSave: true`
    scoped to `"[verb]"`, not globally.
  - [ ] `verb.lspPath` default is the exact absolute path from spec §1, not a
    relative or templated path.
  - [ ] `npm install` was actually run (lockfile present) and committed.
- [ ] **Step 7: Commit**
  ```bash
  git add editors/vscode-verb/package.json editors/vscode-verb/package-lock.json \
          editors/vscode-verb/tsconfig.json editors/vscode-verb/language-configuration.json \
          editors/vscode-verb/.gitignore
  git commit -m "chore: scaffold vscode-verb extension manifest"
  ```

---

### Task 2: tree-sitter WASM build pipeline

**Depends on:** Task 1 (needs `editors/vscode-verb/` to exist as a copy target;
otherwise touches only `editors/tree-sitter-verb/`).

**Files:**
- Modify: `editors/tree-sitter-verb/package.json` (add a `build:wasm` script)
- No source grammar changes — `grammar.js` is untouched by this task.

**Interfaces:**
- Produces: `editors/tree-sitter-verb/tree-sitter-verb.wasm` (build artifact,
  gitignored — Task 5's build script copies it into `vscode-verb/parsers/` at
  build time, it is not committed to the repo directly).
- Consumes: existing `editors/tree-sitter-verb/grammar.js` + pre-generated
  `src/parser.c` (per `editors/README.md`, only `npx tree-sitter generate` needs
  the CLI; this task additionally needs `npx tree-sitter build --wasm`).

- [ ] **Step 1:** `cd editors/tree-sitter-verb && npm install` (pulls
  `tree-sitter-cli` devDependency, already listed).
- [ ] **Step 2:** `npx tree-sitter generate` — regenerate to confirm the
  committed `src/parser.c` is still in sync with `grammar.js` (should be a
  no-op diff; if it's not, stop and flag — that means grammar.js and the
  committed parser have already drifted, which is outside this task's scope
  to fix).
- [ ] **Step 3:** `npx tree-sitter build --wasm`. First run pulls an
  emscripten Docker image — expect several minutes. Verify
  `tree-sitter-verb.wasm` is produced in `editors/tree-sitter-verb/`.
- [ ] **Step 4:** Sanity-check the WASM loads: write a throwaway Node script
  (not committed, or committed as `editors/tree-sitter-verb/test/wasm-smoke.mjs`
  if useful for Task 4 to reuse) that does
  `Parser.init() → Parser.Language.load(wasmBytes) → parser.parse(demo.verb source)`
  and asserts the resulting tree has no top-level `ERROR` node on
  `examples/demo.verb`.
- [ ] **Step 5:** Add `editors/tree-sitter-verb/package.json` script:
  `"build:wasm": "tree-sitter generate && tree-sitter build --wasm"`.
- [ ] **Step 6: Reviewer checklist**
  - [ ] `.wasm` artifact is gitignored, not committed (build artifacts derive
    from source already in the repo — committing it invites drift between the
    binary and `grammar.js`).
  - [ ] Step 2's regenerate produced no diff against the committed `src/parser.c`
    — if it did, this was flagged, not silently committed.
  - [ ] The Docker-fallback path was actually exercised (not skipped because
    emscripten happened to already be on someone's machine) — the plan's
    accepted trade-off is specifically about Docker availability.
- [ ] **Step 7: Commit**
  ```bash
  git add editors/tree-sitter-verb/package.json editors/tree-sitter-verb/.gitignore
  # (.gitignore addition for *.wasm if not already covered by root .gitignore)
  git commit -m "chore: add tree-sitter-verb wasm build script"
  ```

---

### Task 3: `extension.ts` — LSP client wiring

**Depends on:** Task 1 only (does not need the WASM artifact or `highlight.ts`
to compile or to be reviewed — semantic tokens registration can reference a
not-yet-existing `./highlight` import; Task 4 fills that module in. If TS's
`strict` mode blocks compiling with a missing module, stub
`editors/vscode-verb/src/highlight.ts` with a minimal placeholder export and
leave a comment that Task 4 replaces it — do not implement highlighting logic
here).

**Files:**
- Create: `editors/vscode-verb/src/extension.ts`

**Interfaces:**
- Produces: `activate(context: ExtensionContext)` / `deactivate()` exports
  (standard VSCode extension entry points), matching `package.json`'s
  `main: "./out/extension.js"`.
- Consumes: `vscode-languageclient/node`, `vscode` workspace configuration API,
  and (stub or real) `./highlight`'s exported provider-registration function.

- [ ] **Step 1:** Implement `activate()` per spec §3:
  - Read `verb.lspPath` from `workspace.getConfiguration("verb")`.
  - Construct `LanguageClient` with stdio transport
    (`ServerOptions = { command: resolvedLspPath }`),
    `documentSelector: [{ language: "verb" }]`.
  - Start the client; push to `context.subscriptions`.
  - Call `./highlight`'s registration function (semantic tokens provider) and
    push its `Disposable` to `context.subscriptions` too.
- [ ] **Step 2:** Implement `deactivate()` — stop the language client if
  running (standard `vscode-languageclient` teardown), return its promise.
- [ ] **Step 3:** `npm run build` (or bare `tsc` if Task 5's script isn't
  landed yet) — verify `out/extension.js` compiles with no type errors.
- [ ] **Step 4: Reviewer checklist**
  - [ ] `verb.lspPath` is read from configuration, not hardcoded a second time
    in `extension.ts` (single source of truth is `package.json`'s default).
  - [ ] Both the language client and the semantic tokens provider are pushed
    to `context.subscriptions` (spec explicitly calls out "clean disposal" —
    a missing push here leaks the client process on extension deactivate).
  - [ ] No highlighting *logic* was implemented in this file — that's Task 4's
    file boundary; `extension.ts` only registers whatever `./highlight` exports.
- [ ] **Step 5: Commit**
  ```bash
  git add editors/vscode-verb/src/extension.ts
  git commit -m "feat: vscode-verb LSP client activation"
  ```

---

### Task 4: `highlight.ts` — semantic tokens provider

**Depends on:** Task 1 (scaffold) + Task 2 (needs a loadable `.wasm`, at least
locally during development — the *committed* copy step is Task 5's job, but
Task 4's developer needs a local `tree-sitter-verb.wasm` + `web-tree-sitter`'s
own `tree-sitter.wasm` in `parsers/` to test against; copy them manually for
now, per spec §5 steps 2-3, without wiring the npm script yet).

**Files:**
- Create: `editors/vscode-verb/src/highlight.ts`
- Modify: `editors/vscode-verb/src/extension.ts` only if Task 3 landed a stub
  that needs its real signature restored — coordinate via the stub's exported
  function signature agreed in Task 3, don't redesign it here.

**Interfaces:**
- Produces: a function `extension.ts` calls to register a
  `DocumentSemanticTokensProvider` for language `verb`, using the legend from
  spec §4 (`["comment", "string", "number", "keyword", "operator", "function",
  "variable", "parameter"]`) — exact exported name/signature is this task's
  call as long as it matches what Task 3's stub declared.
- Consumes: `web-tree-sitter`, the compiled grammar WASM from `parsers/`.

- [ ] **Step 1:** On first activation (lazy, once — not per-request), load
  `web-tree-sitter`'s runtime WASM, then `Parser.Language.load()` the Verb
  grammar WASM from `parsers/tree-sitter-verb.wasm`.
- [ ] **Step 2:** Implement `provideDocumentSemanticTokens(document)` per
  spec §4 step-by-step: full reparse of `document.getText()`, walk the tree,
  map nodes to legend indices using the exact table in the spec (do not
  invent additional mappings — punctuation/whitespace nodes are intentionally
  left unmapped).
- [ ] **Step 3:** Build and return results via `SemanticTokensBuilder`.
- [ ] **Step 4:** Confirm `ERROR` nodes (malformed source mid-edit) don't
  throw — they simply have no legend mapping and get skipped, per spec's
  Error handling section. Add a quick manual check: introduce a syntax error
  in a scratch `.verb` buffer inside the dev extension host, confirm no
  exception in the extension host log.
- [ ] **Step 5: Reviewer checklist**
  - [ ] Node→token mapping matches the spec's table exactly (cross-check each
    line — this is the kind of one-to-one table an implementer easily
    transposes incorrectly, e.g. swapping `function`/`variable` for
    `fn_statement.name` vs. bare `identifier`).
  - [ ] No modifiers are used (spec explicitly says YAGNI on this).
  - [ ] Grammar WASM load happens once at activation, not on every
    `provideDocumentSemanticTokens` call (spec accepts full-reparse-per-request
    as a cost, but reloading the *grammar* per request is a different,
    unaccepted cost).
  - [ ] `ERROR` nodes verified non-crashing per Step 4, not just assumed.
- [ ] **Step 6: Commit**
  ```bash
  git add editors/vscode-verb/src/highlight.ts
  git commit -m "feat: vscode-verb tree-sitter semantic tokens"
  ```

---

### Task 5: Build pipeline wiring + packaging

**Depends on:** Tasks 1-4 all landed.

**Files:**
- Modify: `editors/vscode-verb/package.json` (`scripts.build`, add
  `@vscode/vsce` devDependency)
- Create: `editors/vscode-verb/.vscodeignore` (excludes `src/`, `node_modules/`
  dev-only entries, `*.ts`, from the packaged `.vsix`)

**Interfaces:**
- Produces: `npm run build` runs, in order, per spec §5: `tsc` →
  copy `node_modules/web-tree-sitter/tree-sitter.wasm` → `parsers/` → copy
  `../tree-sitter-verb/tree-sitter-verb.wasm` → `parsers/`. Use a small Node
  copy script or `cpx`/`shx` (pick one, note the choice — avoid a shell-only
  `cp` in `scripts` since it won't be portable, though portability is a minor
  concern for a single-machine local tool; developer's call, just be explicit).
- Consumes: Task 2's `build:wasm` script (this task's build assumes the WASM
  already exists — it does NOT invoke `tree-sitter build --wasm` itself each
  time, since that's a slow Docker build; document that `build:wasm` in
  `tree-sitter-verb` must be run at least once first).

- [ ] **Step 1:** Wire `scripts.build` per spec §5's 3 steps.
- [ ] **Step 2:** Add `@vscode/vsce` as a devDependency (not the deprecated
  `vsce`); add `scripts.package: "vsce package"` (resolves to
  `@vscode/vsce`'s binary via npm's bin shimming — confirm this actually
  resolves rather than silently invoking a different global `vsce` if one
  exists on `PATH`).
- [ ] **Step 3:** Run the full pipeline from clean: `rm -rf out parsers/*.wasm`,
  then `npm run build`, then `npm run package`. Verify a `.vsix` is produced
  and `npm run build` alone leaves `parsers/` populated with both WASM files.
- [ ] **Step 4:** Confirm the packaged `.vsix` is self-contained per spec's
  claim ("doesn't depend on sibling directories at runtime") — e.g. copy the
  `.vsix` to a scratch dir and `code --install-extension` it, or at minimum
  inspect the `.vsix` zip contents for both `.wasm` files and `out/extension.js`.
- [ ] **Step 5: Reviewer checklist**
  - [ ] `.vscodeignore` excludes `node_modules/` and `src/*.ts` (a `.vsix`
    shipping raw `node_modules` would be enormous and pull in `web-tree-sitter`
    twice — once as source dep, once bundled).
  - [ ] `web-tree-sitter`'s `^0.25` pin (Task 1) actually matches whatever
    `tree-sitter-cli` version Task 2 used to build the WASM — mismatch fails
    silently/opaquely at runtime, this is the one thing worth double-checking
    by hand even though nothing here automates the check.
  - [ ] Clean-build verification (Step 3) was actually run, not just "should
    work" — build scripts are exactly the kind of thing that pass on a dirty
    tree with stale artifacts and fail from clean.
- [ ] **Step 6: Commit**
  ```bash
  git add editors/vscode-verb/package.json editors/vscode-verb/.vscodeignore \
          editors/vscode-verb/package-lock.json
  git commit -m "chore: vscode-verb build + packaging pipeline"
  ```

---

### Task 6: Manual verification + README (final gate)

**Depends on:** Task 5.

**Files:**
- Create: `editors/vscode-verb/README.md`

**Interfaces:** none — this task is verification + documentation, no source
changes. This is the task a **human reviewer**, not another agent, should
likely sign off on, since spec explicitly rejects an automated test suite and
the acceptance criteria are all manual/visual.

- [ ] **Step 1:** `cargo build --release` in the repo root (LSP binary must be
  current before manual testing).
- [ ] **Step 2:** Install the extension (unpacked folder or the Task 5 `.vsix`)
  into a real VSCode instance.
- [ ] **Step 3:** Run spec's 5-step testing plan verbatim:
  1. Build succeeds, extension installs.
  2. Open `examples/demo.verb` — keywords/strings/numbers/comments colored;
     `check ... orelse ... orelse begin` chain and
     `loop init; cond; update begin ... end` highlight correctly (not
     mis-highlighted the way a lossy AST-based approach would).
  3. Hover + completion work (proves LSP client wiring).
  4. Introduce a syntax error — diagnostic squiggle appears, highlighting
     degrades gracefully around the bad region, no crash.
  5. Save a file with a stray double-space + trailing `%%` comment —
     format-on-save fixes spacing, preserves the comment.
- [ ] **Step 4:** Write `editors/vscode-verb/README.md` documenting: build
  steps (link to Task 2's `build:wasm` + Task 5's `build`/`package`), install
  steps, and the `verb.lspPath` setting — mirroring `editors/README.md`'s
  existing style for the nvim/tree-sitter sections.
- [ ] **Step 5: Reviewer checklist (human)**
  - [ ] All 5 manual steps actually reproduced, not asserted from reading code.
  - [ ] README's build steps work when followed literally from a clean clone
    (no "assumes you already ran X" gaps).
- [ ] **Step 6: Commit**
  ```bash
  git add editors/vscode-verb/README.md
  git commit -m "docs: vscode-verb build/install instructions"
  ```
