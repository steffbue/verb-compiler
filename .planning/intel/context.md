# Context (DOC intel)

10 DOC-classified documents were synthesized, all task-by-task implementation
plans (`confidence: medium` except the two 2026-07-21 array/gc plans, which
are `high`), all `locked: false`, all `precedence: null`. Each plan is a
process/execution artifact that implements a companion SPEC (cross-referenced
below); per the classifier notes, the authoritative technical contract lives
in the SPEC files (see `constraints.md`), not here. Entries are topic-keyed
by feature, ordered by source date.

---

## Topic: Verb Compiler (initial implementation plan)
- source: docs/superpowers/plans/2026-07-19-verb-compiler.md
- Task-by-task implementation plan (file structure, code snippets, task
  steps) for building the verb Rust/inkwell compiler producing LLVM IR.
  Cross-ref: docs/superpowers/specs/2026-07-19-verb-compiler-design.md (SPEC).
- Global Constraints: crate/binary name `verb`; inkwell 0.9 (`llvm20-1`
  feature), `LLVM_SYS_201_PREFIX=/opt/homebrew/opt/llvm`; value struct
  `{i8 tag, i64 payload}` with tags 0-5 (nil/bool/int/float/string/closure);
  no GC; compile errors to stderr, runtime errors to stdout then exit(1);
  Lox-style truthiness/`and`/`or` semantics.
  README content proposed by this plan: Requirements (Rust 2021, LLVM 20.1,
  a C compiler); Usage (`cargo run -- run/build`); Known v1 limitations —
  "No GC", "No arrays/maps, no break/continue, no anonymous functions",
  captured variables must be declared before the enclosing `fn` (no mutual
  recursion), shadowing `print` has no effect. Each of these listed
  limitations is the subject of a later spec/plan pair in this same batch
  (arrays, maps, refcounting GC).

## Topic: C++ Import (`import mod`)
- source: docs/superpowers/plans/2026-07-20-cpp-import.md
- Task-by-task implementation plan for letting Verb programs import and
  call extern C++ library functions via a shared VerbValue ABI struct.
  Cross-ref: docs/superpowers/specs/2026-07-20-cpp-import-design.md (SPEC).
- Global Constraints: `import mod <ident>;` bare identifier, top-level only,
  must precede other statements, repeatable/deduplicated; ABI is
  byte-identical `%verb.value`/C struct, no marshalling; arity fixed by
  first call site; `verb run` (JIT) rejects imports, `verb build` (AOT) is
  where imports work; links `c++` when imports non-empty else `cc`; new
  `-L<dir>` CLI flag; platform assumption is macOS + Homebrew LLVM.

## Topic: Cross-platform compile (`--target`)
- source: docs/superpowers/plans/2026-07-20-cross-platform-compile.md
- Task-by-task implementation plan to replace the stubbed AOT build and add
  `--target` cross-compilation via `zig cc`.
  Cross-ref: docs/superpowers/specs/2026-07-20-cross-platform-compile-design.md
  (SPEC).
- Global Constraints: default build path has no new dependency (host `cc`);
  `--target`/`--target all` require `zig` on PATH (fails fast with an
  install hint otherwise); 6 os/arch combos, `x86` aliases `x86_64`;
  windows targets get `.exe` auto-appended; `--target all` is best-effort
  with an end-of-run summary, exit 0 only if all 6 succeed; confirms (via
  codebase check at plan time) no datalayout-dependent codegen exists yet,
  so retargeting the same module across triples in a loop is safe.

## Topic: Multi-file `.verb` linking
- source: docs/superpowers/plans/2026-07-20-multi-file-linking.md
- Task-by-task implementation plan for letting `verb run`/`build` accept
  and link multiple `.verb` files with per-file error attribution. No
  companion SPEC cross-ref recorded in this doc's classification, but its
  Global Constraints explicitly cite spec sections by name ("spec:
  Non-goals", "spec: Semantics", "spec: Goals", "spec: Accepted
  limitation") pointing at
  docs/superpowers/specs/2026-07-20-multi-file-linking-design.md.
- Global Constraints: no in-source import/include syntax (CLI-only linking
  only); no "library file" restriction; no new duplicate-function-name
  detection across files (later definition shadows earlier, matching
  existing intra-file behavior); runtime (JIT-generated) errors keep bare
  `[line:col]`, no filename — accepted v1 limitation, explicitly not to be
  fixed as part of this plan; at least one file required; `-o` still
  consumes its following argument regardless of position.

## Topic: `import std io`
- source: docs/superpowers/plans/2026-07-20-std-io-import.md
- Task-by-task implementation plan for adding a blocking std io module
  (stdin, file, TCP) to Verb via a new std import mechanism.
  Cross-ref: docs/superpowers/specs/2026-07-20-std-io-import-design.md (SPEC).
- Global Constraints: follow the spec exactly — do not add modules beyond
  `io`, no streaming/handle-based file I/O, no non-blocking sockets/UDP
  (all explicitly out of scope for v1); every `io` function returns
  `verb_nil()` on failure, never a C++ exception across the FFI boundary;
  file/socket handles reuse the existing `VERB_INT` tag; `import std io;`
  must coexist with `import mod <lib>;` in the same file; every new Rust
  test must run in `cargo test` (no `#[ignore]`) except where it depends on
  `zig` being installed (guarded skip, matching existing pattern).

## Topic: `VERB_EXPORT` macro
- source: docs/superpowers/plans/2026-07-20-verb-export-macro.md
- Step-by-step task plan to implement a C++17 `VERB_EXPORT` macro in
  `runtime/verb.h` for exposing C++ functions to Verb imports without
  hand-writing wrappers.
  Cross-refs: docs/superpowers/specs/2026-07-20-verb-export-macro-design.md
  (SPEC), tests/e2e.rs, tests/verb_export_macro.rs,
  tests/fixtures/cpp/mathlib.cpp, tests/fixtures/import_mathlib.verb/.expected.
- Global Constraints: C++17 only (matches the existing `-std=c++17` fixture
  build flag); header-only, all new code in `runtime/verb.h`, guarded so
  the existing C-compatible section is untouched; exactly the same 4
  supported Verb value types as the SPEC (`int64_t`/`double`/`const char*`/
  `int`-as-bool) plus `void`-return; arity 0-6 only, no lambdas/function
  objects; no changes to `src/` or the `.verb` language.

## Topic: VSCode extension for Verb
- source: docs/superpowers/plans/2026-07-20-vscode-extension.md
- Task-by-task implementation plan for shipping an unpublished VSCode
  extension with LSP and tree-sitter semantic highlighting for `.verb`
  files.
  Cross-refs: docs/superpowers/specs/2026-07-20-vscode-extension-design.md
  (SPEC), editors/tree-sitter-verb/, editors/nvim/, editors/README.md,
  src/bin/verb-lsp.rs, editors/vscode-verb/README.md.
- Global constraints: all new code confined to `editors/vscode-verb/`
  (LSP server and grammar are consumed, not modified); `verb.lspPath`
  default is an intentionally machine-specific absolute path, matching the
  nvim config's existing hardcoded-path convention — do not make it
  relative/auto-discovered; no Marketplace publish, no extension-host test
  suite, no TextMate grammar, no incremental tree-sitter reparse (per
  spec's Non-goals — any task attempting these should be flagged, not
  built); `web-tree-sitter` version must match `tree-sitter-cli`'s ABI
  (`^0.25`); every task's reviewer checklist must confirm the diff stays
  inside that task's declared file list (waves run concurrently — cross-
  file edits between agents cause merge conflicts).

## Topic: Arrays (`list`, get/set/push/pop/len)
- source: docs/superpowers/plans/2026-07-21-arrays.md
- Task-by-task TDD implementation plan for growable arrays (list literal,
  get/set/push/pop/len) in the Verb compiler, per the arrays design spec.
  Cross-ref: docs/superpowers/specs/2026-07-21-arrays-design.md (SPEC).
- Global Constraints: no new bracket tokens, word-style array syntax; the
  no-closing-delimiter `list` parsing limitation is accepted, not to be
  "fixed"; no GC (array headers/element buffers `malloc`'d, never freed);
  **`TAG_ARRAY = 6`, appended after existing tags 0-5** (see tag-collision
  note in `constraints.md` and `INGEST-CONFLICTS.md` — the maps spec/plan
  independently also claims tag 6, and the shipped codebase actually uses
  `TAG_ARRAY = 7`); every runtime error uses the existing `abort_at`
  helper, asserted on stdout; array equality is explicitly out of scope
  for new codegen (falls through to existing pointer-equality default);
  maps, slicing, for-each sugar out of scope for this plan.

## Topic: Reference-counting GC (v1)
- source: docs/superpowers/plans/2026-07-21-refcounting-gc.md
- Task-by-task implementation plan to add refcounted heap allocations
  (strings, closures) to Verb's codegen with zero new syntax.
  Cross-refs: docs/superpowers/specs/2026-07-21-refcounting-gc-design.md
  (SPEC), tests/e2e.rs, tests/formatter_roundtrip.rs,
  tests/parser_recovery_fuzz.rs.
- Global Constraints: no new language syntax, no new heap types (arrays/
  maps explicitly out of scope for this plan); only string and closure
  values are refcounted, nil/bool/int/float never touch retain/release;
  every existing test suite must keep passing after every task (`cargo
  test` run at the end of each task, not just at the end of the plan);
  `cargo build` must produce zero new warnings at the end of every task.
  **Per this repo's own git history and the v2 spec/plan below, this
  plan's PR (#11) was closed unmerged** — `main` diverged too far for a
  clean rebase before arrays/maps/globals/the export macro landed. See
  `INGEST-CONFLICTS.md` INFO and the v2 entry below, which is the current,
  authoritative implementation plan for this feature area.

## Topic: Reference-counting GC v2 (strings, closures, arrays, maps)
- source: docs/superpowers/plans/2026-07-21-refcounting-gc-v2.md
- Task-by-task implementation plan for adding refcounting GC to Verb's
  heap allocations, implementing a separate design spec; re-applies the v1
  plan's design (unchanged) against current `main` and extends it to
  arrays, maps, closures, and globals.
  Cross-ref: docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md
  (SPEC).
- Global Constraints: no new language syntax, no cycle detection/collection
  anywhere in this plan; only heap-identity tags (STR/CLOSURE/ARRAY/MAP)
  ever touch retain/release, nil/bool/int/float always no-ops; every
  existing test suite (`tests/e2e.rs`, `tests/formatter_roundtrip.rs`,
  `tests/parser_recovery_fuzz.rs`, `tests/verb_export_macro.rs`) must keep
  passing after every task; `cargo build` zero new warnings per task; all
  code in this plan is pinned against `main` @ `1bc678f` (branch
  `refcounting-gc-v2`) — if a local file doesn't match a "Find" block
  verbatim, the plan instructs the implementer to stop and report rather
  than guess. This is the currently-active plan per repo git history (see
  recent commits: "test(gc): verify zero leaks across all heap kinds",
  "feat(gc): wire array builtin call sites").
