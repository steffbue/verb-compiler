---
phase: 08-integration-validation-release-readiness
plan: 02
subsystem: testing
tags: [verb-lang, ffi, gc, integration-test, e2e]

# Dependency graph
requires:
  - phase: 08-integration-validation-release-readiness (plan 01)
    provides: HOUSEKEEP-01 doc correction (TAG_ARRAY = 7), no code dependency
provides:
  - examples/integration_all.verb — the single nontrivial program combining FFI (import mod), std io, std map, and arrays, satisfying INTEG-01
  - examples/integration_all_windows.verb — the std-io-less variant plan 03 cross-compiles to Windows targets
  - tests/e2e.rs::integration_example_zero_leaks() — standalone in-place e2e test proving zero GC leaks + deterministic output
affects: [08-integration-validation-release-readiness (plan 03 — cross-compile all targets)]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "In-place example test: build+run tests/e2e.rs against an examples/*.verb file directly (no tests/fixtures/ duplicate, no .expected file) — inlines the assert_no_leaks()/build_and_run_ok() body since both hardcode tests/fixtures/ paths"
    - "check ... equals ... begin ... end orelse begin ... end used as a self-verifying summary line: the demo's final print is conditioned on the map tally matching the expected value, so a map_set-duplicate-key regression would flip the printed output rather than silently passing"

key-files:
  created:
    - examples/integration_all.verb
    - examples/integration_all_windows.verb
  modified:
    - tests/e2e.rs

key-decisions:
  - "Used only c_add_int (int-returning FFI) for the mathlib call, not c_shout (string-returning) — c_shout's underlying shout_impl in mathlib.cpp returns a raw std::malloc'd buffer, and runtime/verb.h's wrap<const char*> (verb_string) wraps that raw pointer without allocating through verb_alloc's refcount-header convention. Assigning the result to a variable (assign shout c_shout(...)) triggers a GC v2 retain that writes 8 bytes before the malloc'd buffer, corrupting heap metadata and crashing with SIGTRAP on the next allocator call. Confirmed via a minimal repro (assign+print crashes; direct print(c_shout(...)) does not, since it never retains the value across a scope boundary). This is a pre-existing runtime limitation (documented in runtime/verb.h's own comments about unheadered pointers), not introduced by this plan, and out of scope per the phase objective (no new language feature/subsystem — prove existing subsystems interoperate only). The file_write payload uses a fixed string literal instead of an FFI-derived string, sidestepping the bug entirely while still exercising std io."
  - "The example's deterministic summary line is produced via check calls equals first begin/orelse rather than string-concatenating the tallied count into the message — join (the language's only string-concat op) requires both operands to be strings and errors on ints (src/codegen.rs:754), and there is no int-to-string builtin in the shipped language. The check/orelse pattern still ties the printed text's correctness to the map_set update-in-place behavior (the INTEG-01 adjacency edge case), just without embedding the raw digit."

requirements-completed: [INTEG-01]

coverage:
  - id: D1
    description: "examples/integration_all.verb exercises FFI (import mod mathlib), std io (file_write/file_read), std map (map_new/map_set/map_get), and arrays (list/get) together in one program"
    requirement: "INTEG-01"
    verification:
      - kind: e2e
        ref: "tests/e2e.rs#integration_example_zero_leaks"
        status: pass
    human_judgment: false
  - id: D2
    description: "verb build examples/integration_all.verb -L<mathlib dir> produces a working native binary; running it with VERB_GC_DEBUG=1 prints verb_gc_live=0 and a deterministic summary line"
    requirement: "INTEG-01"
    verification:
      - kind: e2e
        ref: "tests/e2e.rs#integration_example_zero_leaks"
        status: pass
    human_judgment: false
  - id: D3
    description: "examples/integration_all_windows.verb is the FFI + std map + arrays variant with no std io — the Windows-cross-compile-safe form for plan 03"
    requirement: "INTEG-01"
    verification:
      - kind: unit
        ref: "grep-based structural check (no import std io / file_write / file_read / file_append present; import mod mathlib, import std map, list all present) — run manually during task execution"
        status: pass
      - kind: manual_procedural
        ref: "host build+run of examples/integration_all_windows.verb (verb build ... && ./bin) confirmed correct output (prints tallied count then integration_summary: ok) — not exercised by an automated test in this plan (cross-compile execution to windows-x86_64/arm64 is plan 03's job; zig was unavailable in this environment to even attempt a cross-build)"
        status: pass
    human_judgment: true
    rationale: "No automated tests/e2e.rs test builds/runs this Windows-variant file in this plan (D-05: cross-compile validation is build-only and belongs to plan 03, which also lacked a zig toolchain here to confirm). A human (or plan 03's own tests) should confirm it cross-compiles cleanly once zig is available."

# Metrics
duration: 25min
completed: 2026-07-21
status: complete
---

# Phase 8 Plan 02: Integration Example Program + Zero-Leaks E2E Test Summary

**A single Verb program (`examples/integration_all.verb`) now exercises a C++ FFI import, `std io`, `std map`, and arrays together, builds via `verb build`, runs with zero GC leaks, and is proven by a standalone `tests/e2e.rs` test — plus a std-io-less Windows-safe variant for plan 03.**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-07-21T21:35:00+02:00 (approx.)
- **Completed:** 2026-07-21T21:52:49+02:00
- **Tasks:** 3/3 completed
- **Files modified:** 3 (2 created, 1 modified)

## Accomplishments
- Created `examples/integration_all.verb`, a minimal mechanical combo that calls `c_add_int` via `import mod mathlib;`, collects results into an array (`list`/`get`), tallies a call count in a map (`map_new`/`map_set`/`map_get`, exercising update-in-place on a repeated key), round-trips a value through the filesystem via `import std io;` (`file_write`/`file_read`), and ends with a deterministic summary line gated on the map tally being correct.
- Created `examples/integration_all_windows.verb`, the same FFI+map+array combo with `import std io;` and the file round-trip removed (Windows cross-compile does not support `std io`'s POSIX-socket-dependent runtime), printing the tallied value directly instead.
- Added `tests/e2e.rs::integration_example_zero_leaks()`, a standalone test that builds `examples/integration_all.verb` in place (no `tests/fixtures/` duplicate) with `-L` pointing at `build_mathlib_fixture()`'s output, runs it with both `VERB_GC_DEBUG=1` and `DYLD_LIBRARY_PATH` set, and asserts `verb_gc_live=0` plus the program's deterministic summary substring. Confirmed passing (`cargo test --test e2e integration_example_zero_leaks`); full e2e suite (76 tests) still passes after the addition.
- Discovered and worked around a genuine, pre-existing GC v2 limitation: FFI functions that return raw `std::malloc`'d strings (e.g. `c_shout`) are unsafe to assign to a Verb variable under the current refcounting GC, because `verb_string()` wraps the raw pointer without the `verb_alloc` refcount header the retain/release paths expect. Documented as a deviation below rather than silently worked around.

## Task Commits

Each task was committed atomically:

1. **Task 1: Write examples/integration_all.verb (full four-subsystem combo)** - `7f7208c` (feat)
2. **Task 2: Write examples/integration_all_windows.verb (std-io-less variant)** - `0b94680` (feat)
3. **Task 3: Add integration_example_zero_leaks() test to tests/e2e.rs** - `d9232d2` (test)

**Plan metadata:** committed separately by the orchestrator after wave completion (worktree mode — this plan does not update STATE.md/ROADMAP.md itself).

## Files Created/Modified
- `examples/integration_all.verb` - the four-subsystem integration demo (FFI + std io + std map + arrays), banner-commented per `examples/files.verb` convention
- `examples/integration_all_windows.verb` - the std-io-less variant (FFI + std map + arrays only) for Windows cross-compile in plan 03
- `tests/e2e.rs` - added `integration_example_zero_leaks()`, an in-place build+run+assert test for the new example

## Decisions Made
- Used `c_add_int` (int-returning) rather than `c_shout` (string-returning) for the FFI call inside the array/map logic, to avoid a pre-existing GC v2 crash when retaining a raw-`malloc`'d FFI string result (see Deviations below for full detail).
- The file-written content in `examples/integration_all.verb` is a fixed string literal rather than an FFI-derived string, since (a) the language's `join` operator requires both operands to be strings and cannot concatenate an int result into a string (no int-to-string builtin exists), and (b) using an FFI-derived string here would hit the same GC bug as above.
- The deterministic "single summary line" requirement is satisfied via a `check ... equals ... begin/orelse` construct whose branch depends on the map tally being correct, rather than embedding the numeric tally as text in the print — this keeps the summary genuinely self-verifying (a map_set-duplicate-key regression flips the printed branch) without needing string/int concatenation.
- Placed the new e2e test at the end of `tests/e2e.rs`, after the existing GC-leak-focused tests (`gc_no_leaks_across_all_heap_kinds`, `gc_stress_all_kinds_leaks_nothing`, `gc_cyclic_array_leak_is_confined_not_corrupting`), since it is thematically a GC-leak assertion first and an FFI/std-io/map/array integration test second.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug avoidance / design adjustment] Avoided a pre-existing GC v2 crash on FFI-returned strings**
- **Found during:** Task 1 (manual verification of `examples/integration_all.verb` before committing)
- **Issue:** The plan's action item suggested optionally using an FFI string result (e.g. `c_shout`) and writing "a string built from the results" to the file. A draft using `assign shout c_shout("verb"); ... file_write(path, shout);` built successfully but crashed at runtime with exit code 133 (SIGTRAP), both with and without `VERB_GC_DEBUG=1`. Root cause: `tests/fixtures/cpp/mathlib.cpp`'s `shout_impl` returns a `std::malloc`'d buffer; `runtime/verb.h`'s `wrap<const char*>` (used by the `VERB_EXPORT` macro) calls `verb_string(s)`, which wraps that raw pointer directly without allocating it through `verb_alloc` (the function that prefixes an 8-byte GC refcount header, per the header's own comment at `runtime/verb.h:19-25`). Assigning the FFI result to a Verb variable triggers a GC v2 retain, which writes a refcount increment 8 bytes before the raw `malloc`'d buffer — corrupting the allocator's heap metadata and crashing on the next allocation. A minimal repro confirmed: `print(c_shout("hi"))` called directly (never retained across a scope) works fine; `assign x c_shout("hi"); print(x);` crashes identically.
- **Fix:** Rewrote `examples/integration_all.verb` to use only `c_add_int` (an int-returning FFI call — ints carry no heap pointer, so retain/release is a no-op) for the array/map logic, and to `file_write` a fixed string literal instead of an FFI-derived string. Verified the corrected program builds, runs, prints the expected output, and reports `verb_gc_live=0` with no crash.
- **Files modified:** `examples/integration_all.verb` (drafted then corrected before first commit — the committed version already reflects the fix)
- **Verification:** Manual build+run with `VERB_GC_DEBUG=1` and `DYLD_LIBRARY_PATH` set against a locally-built `libmathlib.dylib`; confirmed `verb_gc_live=0`, exit 0, and the expected `integration_summary: ok` line. Also confirmed via `cargo test --test e2e integration_example_zero_leaks` (Task 3) and the full `cargo test --test e2e` suite (76/76 passing).
- **Committed in:** `7f7208c` (Task 1 commit already contains the corrected, non-crashing version — no separate fix-up commit was needed since the issue was caught before the first commit)
- **Not fixed (explicitly out of scope):** The underlying runtime limitation itself — `VERB_EXPORT`'s `wrap<const char*>` accepting a raw, unheadered `const char*` from arbitrary C++ callables and silently constructing a `VerbValue` the GC will later try to retain/release — was NOT patched in `runtime/verb.h` or `tests/fixtures/cpp/mathlib.cpp`. This phase's objective is explicitly "no new language feature, builtin, or subsystem — this phase only proves existing subsystems interoperate" (plan frontmatter prohibition #3), and the plan's own read_first note says "no new C++ needed." Fixing the FFI/GC ABI contract for externally-allocated strings is an architectural change (Rule 4 territory) affecting the `VERB_EXPORT` macro's type-safety contract, not a task-scoped bug fix. It is already tracked as known tech debt in `.planning/PROJECT.md`'s Blockers/Concerns section ("unvalidated refcount headers for externally-`malloc`'d pointers"). Recommend a future hardening-milestone task to either (a) require FFI callables returning strings to allocate via `verb_alloc`-compatible storage, or (b) have `wrap<const char*>` defensively copy into a properly-headed buffer.

---

**Total deviations:** 1 auto-fixed (Rule 1 — design/workaround, no runtime code changed).
**Impact on plan:** No scope creep. The example program still genuinely exercises all four subsystems (FFI, std io, std map, arrays) as INTEG-01 requires; only the specific FFI function chosen for the array/map path changed (int-returning instead of string-returning) to avoid tripping a pre-existing, out-of-scope runtime bug. The underlying bug is flagged for future work, not fixed here.

## Issues Encountered
None beyond the deviation documented above (which was fully resolved within this plan's scope by choosing a different, safe FFI call).

## User Setup Required

None - no external service configuration required.

## Known Stubs

None. Both example programs are fully wired (no hardcoded empty values, no placeholder text, no unconnected data sources) and the e2e test genuinely builds and executes them.

## Threat Flags

None beyond what the plan's own `<threat_model>` already anticipated (T-08-03 FFI marshaling, T-08-04 file_write path disclosure, T-08-05 GC DoS via cycles) — no new network endpoints, auth paths, or schema changes were introduced. One additional note for the record: the FFI-string/GC interaction bug discovered during this plan (see Deviations) is a correctness/robustness gap, not a newly-introduced threat surface — it was pre-existing and is now more precisely characterized and documented.

## Next Phase Readiness

`examples/integration_all_windows.verb` is ready for plan 03 to cross-compile via `verb build --target all` (or per-target) as the Windows-safe (and general cross-compile) variant. Plan 03 will need `zig` on PATH to actually attempt cross-builds — it was not available in this execution environment, so cross-compile itself was not attempted here (out of this plan's scope; D-05 assigns build-only cross-compile verification to the FFI/mod-import piece, which plan 03 owns). No blockers for plan 03 are known.

---
*Phase: 08-integration-validation-release-readiness*
*Completed: 2026-07-21*
