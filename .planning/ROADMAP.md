# Roadmap: verb

## Overview

Verb starts as a dynamically-typed language with a Rust/inkwell compiler
producing LLVM IR (JIT or AOT). From there it grows in four directions at
once: native C++ interop (imports, the `VERB_EXPORT` macro), a curated
build-only standard library plus multi-file programs, compound data types
(arrays, maps), and cross-platform AOT distribution via `zig cc`. Once all
four exist, reference-counting memory management is retrofitted across every
heap kind they introduced (strings, closures, cells, arrays, maps, globals).
Editor tooling (formatter, VSCode extension) is built in parallel on top of
the core lexer/parser. The final phase proves the whole system holds together
on one real, nontrivial program and closes out a known documentation gap.

Phases 1–7 reflect work already implemented and tested on the current branch
(`refcounting-gc-v2`) as of this roadmap's creation — see STATE.md for exact
position. Phase 8 is the only phase with open work.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

- [x] **Phase 1: Core Language & Compiler Foundation** - Dynamically-typed Verb programs lex, parse, JIT-run, and AOT-build to native binaries
- [x] **Phase 2: Native C++ Interop** - Verb programs call extern C++ functions via `import mod` and the `VERB_EXPORT` macro
- [x] **Phase 3: Standard Library I/O & Multi-File Programs** - Verb programs read/write files and sockets via `import std io`, and link across multiple `.verb` files
- [x] **Phase 4: Compound Data Types (Arrays & Maps)** - Verb programs store collections via growable arrays and `import std map` hash maps
- [x] **Phase 5: Cross-Platform Build & Distribution** - `verb build --target` cross-compiles to any of 6 OS×arch combinations via `zig cc`
- [x] **Phase 6: Reference-Counting Memory Management (GC v2)** - Every heap allocation across all value kinds is automatically retained/released with zero leaks (acyclic case)
- [x] **Phase 7: Developer Tooling (Formatter & VSCode Extension)** - Verb has a working formatter and a VSCode extension with LSP + syntax highlighting
- [ ] **Phase 8: Integration Validation & Release Readiness** - A single real program proves imports+stdlib+arrays+maps+GC+cross-compile work together, and the stale Arrays doc is fixed

## Phase Details

### Phase 1: Core Language & Compiler Foundation
**Goal**: Developers can write and execute a dynamically-typed Verb program end-to-end via the compiler CLI
**Depends on**: Nothing (first phase)
**Requirements**: LANG-01, LANG-02, LANG-03
**Success Criteria** (what must be TRUE):
  1. Developer can write a `.verb` program using Int/Float/String/Boolean/Nil literals, word-operators, lexical block scoping, and reference-capturing closures
  2. Developer can run `verb run program.verb` and see it lexed, parsed, JIT-compiled, and executed immediately with correct output and exit code
  3. Developer can run `verb build program.verb -o out` and get a native, standalone binary linked by the host C compiler
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-19-verb-compiler-design.md`)

### Phase 2: Native C++ Interop
**Goal**: Developers can extend Verb programs with hand-written or macro-generated C++ functionality
**Depends on**: Phase 1
**Requirements**: FFI-01, FFI-02
**Success Criteria** (what must be TRUE):
  1. Developer can declare `import mod <name>;` at the top of a program and call an extern C++ function through it when using `verb build`
  2. `verb run` (JIT) rejects any program containing imports with a clear compile-time error
  3. Developer can expose a C++ function to Verb with a single `VERB_EXPORT(name, arity, fn)` line instead of hand-writing the `extern "C" VerbValue` wrapper
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-20-cpp-import-design.md`, `docs/superpowers/specs/2026-07-20-verb-export-macro-design.md`)

### Phase 3: Standard Library I/O & Multi-File Programs
**Goal**: Developers can build larger programs split across files and reach the outside world via a curated, safe stdlib
**Depends on**: Phase 1, Phase 2
**Requirements**: STDIO-01, MULTI-01
**Success Criteria** (what must be TRUE):
  1. Developer can write `import std io;` and read stdin, read/write/append files, and open blocking TCP connections from Verb code
  2. Developer can pass multiple `.verb` files to `verb run`/`verb build` and have them compile and link as a single program
  3. Compile errors in a multi-file build are attributed to the correct source file (`file:line:col`)
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-20-std-io-import-design.md`, `docs/superpowers/specs/2026-07-20-multi-file-linking-design.md`)

### Phase 4: Compound Data Types (Arrays & Maps)
**Goal**: Developers can store and manipulate collections of values, not just scalars
**Depends on**: Phase 1, Phase 3
**Requirements**: ARR-01, MAP-01
**Success Criteria** (what must be TRUE):
  1. Developer can create a growable array with `list e1, ..., en` and call `get`/`set`/`push`/`pop`/`len` on it
  2. Developer can write `import std map;` and create/read/update/delete key-value pairs with `map_new`/`map_set`/`map_get`/`map_has`/`map_remove`/`map_len`
  3. Both data types coexist without value-tag ambiguity at the implementation level (shipped `TAG_ARRAY=7`, `TAG_MAP=6` — no runtime collision, though the design docs still need correcting; tracked in Phase 8)
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-21-arrays-design.md`, `docs/superpowers/specs/2026-07-21-maps-design.md`)

### Phase 5: Cross-Platform Build & Distribution
**Goal**: Developers can ship a compiled Verb program to any of 3 major OSes on 2 architectures without owning the target hardware
**Depends on**: Phase 2
**Requirements**: XPLAT-01, XPLAT-02
**Success Criteria** (what must be TRUE):
  1. Developer can run `verb build program.verb -o out --target <os>-<arch>` for any of 6 supported combinations and get a correctly named, working binary (`.exe` auto-appended for Windows)
  2. Developer can run `verb build program.verb -o out --target all` and get all 6 binaries built in one pass, with a clear best-effort pass/fail summary
  3. Cross-compilation uses `zig cc` and fails fast with an install hint if `zig` isn't on PATH, without affecting the default (no `--target`) host build path
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-20-cross-platform-compile-design.md`)

### Phase 6: Reference-Counting Memory Management (GC v2)
**Goal**: Developers get automatic memory management for every heap-allocated Verb value with no new syntax and no manual frees
**Depends on**: Phase 4
**Requirements**: GC-01, GC-02, GC-03, GC-04
**Success Criteria** (what must be TRUE):
  1. Every heap allocation (string, closure, cell, array, map) is created via `verb_alloc` and carries a refcount header
  2. Values are automatically retained/released as they're assigned, passed, reassigned, and scoped in/out — for locals, params, and top-level globals alike
  3. Setting `VERB_GC_DEBUG=1` on a built binary and running any acyclic Verb program reports `verb_gc_live=0` at exit
  4. A self-referential (cyclic) array or map leaks only its own small, bounded footprint — never corrupting memory or growing unbounded
**Plans**: Complete — all 8 tasks of `docs/superpowers/plans/2026-07-21-refcounting-gc-v2.md` implemented and tested on the current branch (`refcounting-gc-v2`, not yet merged to `main`)

### Phase 7: Developer Tooling (Formatter & VSCode Extension)
**Goal**: Developers get an editing experience (formatting, highlighting, LSP features) comparable to a mainstream language
**Depends on**: Phase 1
**Requirements**: TOOL-01, TOOL-02
**Success Criteria** (what must be TRUE):
  1. Developer can format a `.verb` file (consistent 2-space indent, normalized operator spacing, preserved comments) via the LSP or a Neovim format-on-save autocmd
  2. Developer can install the VSCode extension and get hover/completion/diagnostics from `verb-lsp`, format-on-save, and tree-sitter-based syntax highlighting
**Plans**: Complete (pre-dates GSD tracking; see `docs/superpowers/specs/2026-07-20-verb-formatter-design.md`, `docs/superpowers/specs/2026-07-20-vscode-extension-design.md`; built `.vsix` present at `editors/vscode-verb/`)

### Phase 8: Integration Validation & Release Readiness
**Goal**: Verb demonstrably clears its "feature-complete, self-hosted-capable" bar — a real program exercising every major subsystem together compiles, runs, and cross-compiles cleanly — and a known stale doc is fixed
**Depends on**: Phase 2, Phase 3, Phase 4, Phase 5, Phase 6
**Requirements**: HOUSEKEEP-01, INTEG-01, INTEG-02
**Success Criteria** (what must be TRUE):
  1. Developer can point to a single `.verb` example program that uses a C++ import, `std io`, `std map`, and arrays together, and `verb build` produces a working binary with zero GC leaks
  2. Developer can cross-compile that same program for all supported targets via `verb build --target all` and get a clear pass/fail summary
  3. Developer reading the Arrays design spec sees `TAG_ARRAY = 7`, matching the shipped code, with no unresolved tag-collision note
**Plans**: 3 plans
- [ ] 08-01-PLAN.md — Correct stale TAG_ARRAY=6→7 in the Arrays design spec + plan (HOUSEKEEP-01)
- [ ] 08-02-PLAN.md — Integration example (FFI+std io+std map+arrays) + Windows variant + host build/run/zero-leaks test (INTEG-01)
- [ ] 08-03-PLAN.md — Cross-compile build-only test across all 6 targets with per-target libmathlib (INTEG-02)

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Core Language & Compiler Foundation | Pre-GSD | Complete | 2026-07-19 |
| 2. Native C++ Interop | Pre-GSD | Complete | 2026-07-20 |
| 3. Standard Library I/O & Multi-File Programs | Pre-GSD | Complete | 2026-07-20 |
| 4. Compound Data Types (Arrays & Maps) | Pre-GSD | Complete | 2026-07-21 |
| 5. Cross-Platform Build & Distribution | Pre-GSD | Complete | 2026-07-20 |
| 6. Reference-Counting Memory Management (GC v2) | Pre-GSD | Complete | 2026-07-21 |
| 7. Developer Tooling (Formatter & VSCode Extension) | Pre-GSD | Complete | 2026-07-20 |
| 8. Integration Validation & Release Readiness | 0/3 | Planned | - |
