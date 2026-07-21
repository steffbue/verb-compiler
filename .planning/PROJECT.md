# verb

## What This Is

Verb is a dynamically-typed programming language and compiler, implemented in
Rust 2021 using inkwell/LLVM 20.1. `verb run` lexes, parses, and JIT-compiles a
`.verb` source file to LLVM IR and executes it immediately; `verb build`
ahead-of-time compiles the same pipeline to a native binary, optionally
cross-compiled to any of 6 OS×arch targets via `zig cc`. A small C++17 runtime
(`runtime/`) provides the value ABI, standard-library I/O and map modules, and
reference-counted memory management for every heap allocation. Editor tooling
(a formatter, `verb-lsp`, and a VSCode extension with tree-sitter highlighting)
is being built alongside the language itself.

## Core Value

A developer can write a real, nontrivial Verb program — combining C++/stdlib
imports, arrays, maps, and cross-platform AOT compilation — and have it
compile and run correctly with zero memory leaks.

## Requirements

### Validated

<!-- Shipped and confirmed via the existing test suite (tests/e2e.rs and friends). -->

- ✓ **LANG-01**: Core dynamically-typed language (Int/Float/String/Boolean/Nil,
  word-operators, lexical block scoping, reference-capturing closures) — Phase 1
- ✓ **LANG-02**: `verb run` JIT-compiles and executes a `.verb` program via inkwell/MCJIT — Phase 1
- ✓ **LANG-03**: `verb build` AOT-compiles to a native binary via the host C compiler — Phase 1
- ✓ **FFI-01**: `import mod <name>;` calls extern C++ functions through the shared VerbValue ABI (build-only) — Phase 2
- ✓ **FFI-02**: `VERB_EXPORT(name, arity, fn)` macro generates FFI wrapper boilerplate in `runtime/verb.h` — Phase 2
- ✓ **STDIO-01**: `import std io;` — stdin, file read/write/append, blocking TCP (build-only) — Phase 3
- ✓ **MULTI-01**: `verb run`/`verb build` accept and link multiple `.verb` files, with per-file error attribution — Phase 3
- ✓ **ARR-01**: Growable arrays via `list e1, ..., en` + `get`/`set`/`push`/`pop`/`len` — Phase 4
- ✓ **MAP-01**: `import std map;` hash map (`map_new/set/get/has/remove/len`) — Phase 4
- ✓ **XPLAT-01**: `verb build --target <os>-<arch>` cross-compiles via `zig cc` (6 combos) — Phase 5
- ✓ **XPLAT-02**: `verb build --target all` best-effort builds all 6 targets with a summary — Phase 5
- ✓ **GC-01**: Every heap allocation carries a refcount header, allocated via `verb_alloc` — Phase 6
- ✓ **GC-02**: Codegen-inserted retain/release for strings, closures, cells, and globals — Phase 6
- ✓ **GC-03**: Cascading release of array elements / map keys-and-values on refcount-zero — Phase 6
- ✓ **GC-04**: Zero leaks (`verb_gc_live=0`) across all acyclic fixtures; confined, bounded leak on cyclic containers — Phase 6
- ✓ **TOOL-01**: `.verb` formatter wired into `verb-lsp` and Neovim format-on-save — Phase 7
- ✓ **TOOL-02**: VSCode extension — LSP client, format-on-save, tree-sitter highlighting — Phase 7

### Active

<!-- Current scope. Building toward these. -->

- [ ] **HOUSEKEEP-01**: Correct the Arrays design spec/plan's stale `TAG_ARRAY = 6` literal to `TAG_ARRAY = 7`, matching shipped `src/value.rs`/`runtime/verb.h`
- [ ] **INTEG-01**: A single nontrivial Verb program combining a C++ import, `std io`, `std map`, and arrays compiles and runs via `verb build` with zero GC leaks
- [ ] **INTEG-02**: That program (or an equivalent) cross-compiles via `verb build --target all` across all supported targets

### Out of Scope

<!-- Explicit boundaries. Includes reasoning to prevent re-adding. -->

- Array/map deep or structural equality — Arrays design spec keeps pointer/reference equality; not a gap to fix
- Array slicing, for-each sugar — explicitly out of scope in Arrays design spec
- In-source `import`/`include` syntax for linking multiple files — multi-file-linking spec deliberately chose CLI-only linking (`verb run a.verb b.verb ...`)
- "Library file" convention / cross-file duplicate-symbol detection — explicitly rejected; later definition silently shadows, matching existing intra-file behavior
- `VERB_EXPORT` arity > 6, unsupported C++ types, lambdas/function objects — explicitly out of scope in the macro's design spec
- `verb targets` introspection command, target auto-detection, packaging/installers — explicit non-goals of cross-platform-compile design
- VSCode Marketplace publish, extension-host test suite, debugger integration, incremental tree-sitter reparse, TextMate grammar — explicit non-goals of the VSCode extension design
- Standalone `verb fmt` CLI, line-wrapping/reflow, trailing-comment column alignment — explicit non-goals of the formatter design
- `break`/`continue`, anonymous functions, string methods, result-style error handling — deferred by the original v1 language design spec; never picked up by a later spec in this batch
- Windows cross-compile for `import std io` — `verb_std_io.cpp` depends on POSIX sockets; not supported in v1

## Context

**Brownfield project, substantial existing implementation.** Planning docs were
bootstrapped by ingesting 12 SPEC + 10 DOC (implementation-plan) documents from
`docs/superpowers/specs/` and `docs/superpowers/plans/` (dated 2026-07-19 through
2026-07-21) plus a fresh codebase map (`.planning/codebase/`). No ADRs or PRDs
existed in the source material, so v1 requirements above were derived directly
from the SPEC contracts (the authoritative technical source) rather than a
product-requirements doc.

**Current git branch:** `refcounting-gc-v2`. This branch's refcounting-GC v2 plan
(8 tasks) is fully implemented and tested — the final task's exit criteria
(`gc_no_leaks_across_all_heap_kinds`, `gc_stress_all_kinds_leaks_nothing`,
`gc_cyclic_array_leak_is_confined_not_corrupting`) are all committed and
passing per recent commit history. This branch is **not yet merged to `main`**.

**GC v2 explicitly supersedes v1**: a v1 refcounting-GC design (strings/closures
only) shipped as PR #11, which was closed unmerged because `main` diverged too
far for a clean rebase before arrays/maps/globals/the export macro landed. v2
re-applies the same core design against current `main` and extends it to
arrays, maps, and global bindings. v1's spec/plan docs are retained for
provenance only — v2 is authoritative.

**Known doc/code mismatch (not a live bug):** the Arrays design spec and its
companion plan both state `TAG_ARRAY = 6`; the Maps design spec independently
claims the same tag 6. The shipped code resolves this as `TAG_MAP = 6` /
`TAG_ARRAY = 7` — Arrays was bumped to 7 during implementation and its design
docs were never updated to match. Tracked as HOUSEKEEP-01 (Phase 8), not
treated as an implementation defect.

**Heavy parallel-worktree development style**: the repo has 20+ local/remote
branches (`worktree-agent-*`, `worktree-merge-pr*`, topic branches for
std-thread/stdlib-time/file-import/debugger/formatter/LSP/tree-sitter sync),
indicating features are frequently developed in isolated worktrees and merged
back. None of those other branches are in scope for this roadmap.

## Constraints

- **Tech stack**: Rust 2021 + inkwell 0.9 (`llvm20-1` feature) + LLVM 20.1 via
  Homebrew (`LLVM_SYS_201_PREFIX`) — required for building the compiler itself
- **Tech stack**: C++17 runtime (`runtime/verb.h`, `verb_map.cpp`,
  `verb_std_io.cpp`) compiled via `build.rs`/the `cc` crate and linked into
  every generated binary
- **Dependency**: `zig` must be on PATH only for `--target` cross-compilation;
  the default (no `--target`) host build path has no new dependency
- **Compatibility**: `verb run` (JIT) rejects any program using `import mod`,
  `import std io`, or `import std map` — these are build-only (AOT) features
- **Compatibility**: Windows cross-compile targets don't support `import std io`
  (POSIX socket dependency in `verb_std_io.cpp`)
- **Architecture**: single-threaded compiler and JIT; inkwell/MCJIT are not
  thread-safe for concurrent compilation
- **Memory model**: reference-counting GC has no cycle collector yet;
  self-referential arrays/maps leak in a small, bounded, accepted way (proven
  by `gc_cyclic_array_leak_is_confined_not_corrupting`)

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Reference-counting GC over a tracing collector | inkwell doesn't expose `gc.statepoint`; no bytecode VM/stack maps; stack-scanning is fragile against optimized native code | ✓ Good |
| `zig cc` for cross-compilation instead of requiring native per-target toolchains | Avoids requiring 6 separate installed toolchains; zig ships prebuilt cross-linkers | ✓ Good |
| VerbValue struct reused as-is for the C-ABI boundary (no marshalling layer) | Simplicity — no per-function signatures or type annotations needed for `import mod` | ✓ Good |
| Token-stream formatter instead of AST-unparse | `ast::Stmt`/`Expr` carry no comments and desugar `loop`; unparsing the AST would destroy both | ✓ Good |
| refcounting-gc-v2 supersedes v1 (PR #11 abandoned) | `main` diverged too far for a clean rebase after arrays/maps/globals/export-macro landed | ✓ Good — v2 complete on `refcounting-gc-v2` branch, not yet merged |
| Cycle collection deferred to a later sub-project | Keeps v1 GC scope to correct, tested acyclic refcounting; cycles proven confined, not corrupting | — Pending |
| Word-style array literal (`list e1, ..., en`, no bracket tokens) | Avoids adding new lexer tokens; accepted trade-off: no nested non-final array literals | ✓ Good |

---
*Last updated: 2026-07-21 after initial ingest-based roadmap creation*
