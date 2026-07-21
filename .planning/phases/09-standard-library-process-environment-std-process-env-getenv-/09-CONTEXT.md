# Phase 9: Standard Library: Process & Environment - Context

**Gathered:** 2026-07-21
**Status:** Ready for planning

<domain>
## Phase Boundary

Verb programs get environment-variable access, process introspection (cwd,
executable path, pid), process termination (exit, abort), and child-process
spawn/wait — via two curated stdlib modules (`std env`, `std process`) plus
platform-specific primitives underneath (Linux: fork/execve/waitpid;
Windows: CreateProcess). No new language syntax — this is a stdlib addition
following the same shape as the existing `std io` / `std map` modules.

</domain>

<decisions>
## Implementation Decisions

### Module split
- **D-01:** Split into two modules, mirroring the existing `std io`/`std map`
  precedent: `import std env;` (getenv/setenv/unsetenv) and
  `import std process;` (cwd, executable path, spawn, wait).
- **D-02:** `exit`, `abort`, and `get pid` are **core builtins** — always
  available with no `import` required (like `print`), since they're trivial
  libc calls needing no extra runtime linking.
- **D-03:** `import std env;` and `import std process;` are **build-only**,
  exactly like `std io`/`std map` today — `verb run` (JIT) rejects any
  program using them. This restriction does NOT apply to `exit`/`abort`/pid
  since those are core builtins (D-02), not import-gated — they work under
  JIT the same as `print` does.

### Raw platform primitives exposure
- **D-04:** fork/execve/waitpid (Linux) and CreateProcess (Windows) are
  **internal-only** — implementation details inside
  `runtime/verb_process.cpp`. They are NOT exposed as Verb-callable
  builtins. The only Verb-facing calls are `spawn()`/`wait()`.
- **D-05:** On POSIX, whether `spawn()` uses `fork()`+`execve()` literally
  or `posix_spawn()`/`vfork()` is **Claude's discretion** — pick whichever
  is simplest/safest in the runtime, as long as `spawn()`/`wait()` behave
  correctly. The user's original ask named fork/execve/waitpid, but does
  not require the literal syscalls if a safer equivalent has the same
  observable effect.

### spawn/wait API shape
- **D-06:** `spawn()` takes a command plus an **array of args** (execve-style
  — e.g. `spawn("ls", list "-l", "/tmp")`). No shell is invoked to parse
  arguments — avoids shell-injection risk entirely.
- **D-07:** `wait()` returns **exit code only** — no captured stdout/stderr.
  Keeps the runtime implementation simple (no pipe/buffer management).
- **D-08:** A failed `spawn()` (e.g. command not found) returns a **sentinel**
  (nil/-1) that the caller checks — consistent with the existing convention
  where array/map operations on bad input return nil/error codes rather than
  raising exceptions.

### exit/abort semantics vs GC
- **D-09:** `exit(code)` calls libc `exit()` immediately, **skipping GC/
  refcount cleanup** — matches C `exit()` semantics exactly. Explicit
  process exit does not need to prove `verb_gc_live=0`.
- **D-10:** `abort()` calls libc `abort()` — a hard SIGABRT-style crash with
  abnormal exit status, not a "friendly" Verb-level panic. No new
  message-formatting machinery needed.
- **D-11:** Because `exit`/`abort` bypass GC cleanup, PROJECT.md's
  Constraints section should gain a new documented exception (alongside the
  existing Windows/`std io` POSIX-socket note) stating that `exit()`/
  `abort()` are excluded from the zero-leak (GC-04) guarantee. This is a
  planner/executor task, not something to be silently assumed.

### Claude's Discretion
- Exact C++ implementation of `spawn()`/`wait()` on POSIX (fork+execve vs.
  posix_spawn/vfork) — see D-05.
- Naming of internal C++ helper functions in `runtime/verb_process.cpp` and
  `runtime/verb_env.cpp` (or however the two modules are split at the
  runtime-file level).
- Whether `std env` and `std process` are one or two `.cpp` runtime files —
  follow whatever's cleanest given the existing `verb_map.cpp` / `verb_io.cpp`
  one-file-per-module convention, but a single `verb_process.cpp` covering
  both if that's simpler is acceptable since D-01 only mandates two Verb-level
  import names, not two runtime files.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements & project state
- `.planning/REQUIREMENTS.md` — v1 requirements traceability (Phase 9 has no
  REQ-IDs assigned yet — Requirements: TBD in ROADMAP.md; planner should
  propose IDs, e.g. `ENV-01`, `PROC-01`, `PROC-02`, following the existing
  `AREA-NN` convention)
- `.planning/PROJECT.md` — Core value statement, existing Windows/`std io`
  documented exception (the pattern D-11's new exception should follow)
- `.planning/ROADMAP.md` §Phase 9 — Goal and scope bullets for this phase

### Existing patterns to follow (from codebase map)
- `.planning/codebase/STRUCTURE.md` §"New Standard Library Module" — the
  exact recipe this phase follows: parser recognition of `import std foo`,
  new C++ unit in `runtime/`, `build.rs` compile-step registration, linking
  into both JIT (`register_jit_runtime_symbols`) and AOT binaries
- `.planning/codebase/INTEGRATIONS.md` §"Standard Library Modules" — exact
  shape of the existing `std io`/`std map` function lists and their
  build-only/JIT-rejected convention, to mirror for `std env`/`std process`
- `.planning/codebase/ARCHITECTURE.md` — VerbValue tagged-union model
  (`src/value.rs`), codegen builtin-function pattern, JIT vs AOT runtime
  symbol registration
- `runtime/verb_std_io.cpp` — closest existing runtime unit to model
  `verb_env.cpp`/`verb_process.cpp` on (build-only module, POSIX/Windows
  divergence already handled here for sockets)
- `runtime/verb_map.cpp` — second existing runtime module reference,
  simpler (no OS divergence) — useful for `std env`'s getenv/setenv/unsetenv
  which also have no real POSIX/Windows divergence (both have `getenv`/
  `_putenv`-equivalent libc calls)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/main.rs` stub-function pattern (lines ~44–58) for JIT-mode rejection
  of build-only features — reuse the same "abort loudly if called under
  JIT" stub approach for `std env`/`std process`, matching how `std io`/
  `std map` already reject JIT.
- `assert_no_leaks()` helper in `tests/e2e.rs` — reusable for any std
  process/env test that doesn't explicitly exercise exit/abort (which by
  D-09/D-10 are expected to skip the leak check, so those tests need a
  different assertion shape).

### Established Patterns
- New stdlib module recipe (STRUCTURE.md): parser → runtime `.cpp` unit →
  `build.rs` registration → JIT+AOT symbol linking. `std env`/`std process`
  follow this exactly for their import-gated functions; `exit`/`abort`/pid
  (D-02, core builtins) instead follow the plain "new builtin function"
  recipe (codegen `build_*_fn` methods), not the module-import recipe.
- Windows-exception documentation pattern: `std io`'s POSIX-socket
  limitation is called out explicitly in PROJECT.md Constraints — D-11
  extends this same pattern for exit/abort bypassing GC cleanup.

### Integration Points
- `src/targets.rs` / cross-compile: `CreateProcess` (Windows) implementation
  needs to compile cleanly for `windows-x86_64`/`windows-arm64` zig-cc
  cross-targets, same as other runtime units already do.
- `build.rs`: new `.cpp` file(s) need registration in the compile-step list,
  same as `verb_map.cpp`/`verb_std_io.cpp` today.

</code_context>

<specifics>
## Specific Ideas

No themed/narrative requirements — this is a mechanical stdlib addition.
Concrete function list from the original ask:

- `std env`: getenv, setenv, unsetenv
- `std process`: current working directory, executable path, spawn, wait
- Core builtins (no import): exit, abort, get pid
- Internal-only platform primitives: Linux fork/execve/waitpid, Windows
  CreateProcess (never exposed to Verb code directly — D-04)

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 9-Standard Library: Process & Environment*
*Context gathered: 2026-07-21*
