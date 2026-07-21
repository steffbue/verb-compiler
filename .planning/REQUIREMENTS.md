# Requirements: verb

**Defined:** 2026-07-21
**Core Value:** A developer can write a real, nontrivial Verb program — combining C++/stdlib imports, arrays, maps, and cross-platform AOT compilation — and have it compile and run correctly with zero memory leaks.

**Source note:** No ADR or PRD documents existed in the ingested source material
(`docs/superpowers/specs/`, `docs/superpowers/plans/`) — 0 decisions, 0
requirements extracted by type. The requirements below were derived directly
from the 12 SPEC documents (the authoritative technical contracts) and cross-
checked against the current codebase map, since this is a brownfield project
where most of this scope is already implemented.

## v1 Requirements

Requirements for the current milestone. Each maps to a roadmap phase.

### Core Language & Compiler

- [x] **LANG-01**: Developer can write a Verb program using dynamically-typed
  values (Int/Float/String/Boolean/Nil), word-style operators, lexical block
  scoping, and closures that capture enclosing variables by reference

- [x] **LANG-02**: Developer can run `verb run program.verb` and have it lexed,
  parsed, JIT-compiled to LLVM IR (via inkwell/MCJIT), and executed immediately
  with correct output and exit codes

- [x] **LANG-03**: Developer can run `verb build program.verb -o out` and get a
  native, standalone binary linked by the host C compiler

### Native C++ Interop

- [x] **FFI-01**: Developer can write `import mod <name>;` at the top of a
  program and call an extern C++ function through the shared VerbValue ABI
  when using `verb build`; `verb run` rejects any program containing imports

- [x] **FFI-02**: Developer can expose a C++ function to Verb with a single
  `VERB_EXPORT(name, arity, callable)` line in `runtime/verb.h`, instead of
  hand-writing the `extern "C" VerbValue` wrapper

### Standard Library I/O & Multi-File Programs

- [x] **STDIO-01**: Developer can write `import std io;` and read stdin,
  read/write/append files, and use blocking TCP sockets from Verb code
  (build-only; `verb run` rejects it)

- [x] **MULTI-01**: Developer can pass multiple `.verb` files to `verb run`/
  `verb build` and have them compile and link as a single program, with
  compile errors attributed to the correct source file (`file:line:col`)

### Compound Data Types

- [x] **ARR-01**: Developer can create a growable array with
  `list e1, ..., en` and operate on it with `get`, `set`, `push`, `pop`, and
  `len` builtins

- [x] **MAP-01**: Developer can write `import std map;` and create/read/
  update/delete key-value pairs with `map_new`/`map_set`/`map_get`/`map_has`/
  `map_remove`/`map_len`, keyed by nil/bool/int/float/string

### Cross-Platform Build & Distribution

- [x] **XPLAT-01**: Developer can run `verb build program.verb -o out --target
  <os>-<arch>` for any of 6 supported OS×arch combinations, using `zig cc`,
  with `.exe` auto-appended for Windows targets

- [x] **XPLAT-02**: Developer can run `verb build program.verb -o out --target
  all` and get all 6 target binaries built in one pass, with a best-effort
  pass/fail summary (exit 0 only if all 6 succeed)

### Memory Management (Reference-Counting GC v2)

- [x] **GC-01**: Every heap allocation (string, closure, cell, array, map) is
  created via `verb_alloc` and carries an 8-byte refcount header

- [x] **GC-02**: Verb-compiled programs automatically retain/release strings,
  closures, and boxed locals/params/globals as they are created, passed,
  reassigned, and go out of scope, with zero new language syntax

- [x] **GC-03**: Verb-compiled programs automatically release every array
  element (and every map key/value) when the container's refcount reaches
  zero, without double-freeing

- [x] **GC-04**: Setting `VERB_GC_DEBUG=1` on a built binary and running any
  acyclic Verb program reports `verb_gc_live=0` at exit; a self-referential
  (cyclic) array/map leaks only a small, bounded, documented footprint instead
  of corrupting memory or growing unbounded

### Developer Tooling

- [x] **TOOL-01**: Developer can format a `.verb` file (2-space indent,
  normalized operator spacing, preserved comments) via `verb-lsp`'s
  `textDocument/formatting` or a Neovim format-on-save autocmd

- [x] **TOOL-02**: Developer can install the VSCode extension for Verb and get
  LSP-backed hover/completion/diagnostics, format-on-save, and tree-sitter-
  based syntax highlighting for `.verb` files

### Integration & Housekeeping

- [x] **HOUSEKEEP-01**: The Arrays design spec and its companion plan are
  corrected to state `TAG_ARRAY = 7` (matching shipped `src/value.rs`/
  `runtime/verb.h`), resolving the stale tag-6 collision with the Maps spec

- [x] **INTEG-01**: A single nontrivial Verb program combining a C++ import,
  `import std io`, `import std map`, and arrays compiles and runs correctly
  end-to-end via `verb build`, producing zero GC leaks (`verb_gc_live=0`)

- [ ] **INTEG-02**: That same integration program (or an equivalent)
  cross-compiles successfully via `verb build --target all`, confirming
  imports/stdlib/arrays/maps/GC all function under every supported target's
  build path (excluding the documented Windows `std io` limitation)

## v2 Requirements

Deferred to a future milestone. Tracked but not in current roadmap.

### Memory Management

- **GC-CYCLE-01**: A backup cycle collector reclaims self-referential
  arrays/maps that pure reference-counting cannot free (explicitly named as
  "a separate later sub-project" in the refcounting-gc-v2 design spec)

### Standard Library

- **STDLIB-V2-01**: Map iteration — enumerate a map's keys/values (no
  iteration function exists in v1, even though arrays now provide a data
  structure to return results into)

- **STDLIB-V2-02**: Additional `import std` modules beyond `io`/`map` (e.g.
  generic containers/templates, non-blocking/async sockets, UDP, TLS)

### Native Interop

- **FFI-V2-01**: JIT-mode (`verb run`) support for imports
- **FFI-V2-02**: Typed, checked per-function extern signatures, beyond the
  current untyped VerbValue ABI

- **FFI-V2-03**: A package manager / C++ header parsing for imports (classes,
  templates, overloads, name demangling)

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Array/map deep or structural equality | Arrays design spec keeps pointer/reference equality; called out as accepted, not a gap |
| Array slicing, for-each sugar | Explicit non-goals of the Arrays design spec |
| In-source `import`/`include` syntax for linking multiple files | Multi-file-linking spec deliberately chose CLI-only linking (`verb run a.verb b.verb ...`) |
| "Library file" convention / cross-file duplicate-symbol detection | Explicitly rejected; later definition silently shadows, matching existing intra-file behavior |
| `VERB_EXPORT` arity > 6, unsupported C++ types, lambdas/function objects | Explicit non-goals of the VERB_EXPORT macro design |
| `verb targets` introspection command, target auto-detection, packaging/installers | Explicit non-goals of the cross-platform-compile design |
| VSCode Marketplace publish, extension-host test suite, debugger integration, incremental tree-sitter reparse, TextMate grammar | Explicit non-goals of the VSCode extension design |
| Standalone `verb fmt` CLI, line-wrapping/reflow, trailing-comment column alignment | Explicit non-goals of the formatter design |
| `break`/`continue`, anonymous functions, string methods, result-style error handling | Deferred by the original v1 language design spec; never picked up by a later spec in this ingest batch |
| Windows cross-compile for `import std io` | `verb_std_io.cpp` depends on POSIX sockets; not supported in v1 |

## Traceability

Which phases cover which requirements.

| Requirement | Phase | Status |
|-------------|-------|--------|
| LANG-01 | Phase 1 | Complete |
| LANG-02 | Phase 1 | Complete |
| LANG-03 | Phase 1 | Complete |
| FFI-01 | Phase 2 | Complete |
| FFI-02 | Phase 2 | Complete |
| STDIO-01 | Phase 3 | Complete |
| MULTI-01 | Phase 3 | Complete |
| ARR-01 | Phase 4 | Complete |
| MAP-01 | Phase 4 | Complete |
| XPLAT-01 | Phase 5 | Complete |
| XPLAT-02 | Phase 5 | Complete |
| GC-01 | Phase 6 | Complete |
| GC-02 | Phase 6 | Complete |
| GC-03 | Phase 6 | Complete |
| GC-04 | Phase 6 | Complete |
| TOOL-01 | Phase 7 | Complete |
| TOOL-02 | Phase 7 | Complete |
| HOUSEKEEP-01 | Phase 8 | Complete |
| INTEG-01 | Phase 8 | Complete |
| INTEG-02 | Phase 8 | Pending |

**Coverage:**

- v1 requirements: 20 total
- Mapped to phases: 20
- Unmapped: 0 ✓

---
*Requirements defined: 2026-07-21*
*Last updated: 2026-07-21 after initial ingest-based roadmap creation*
