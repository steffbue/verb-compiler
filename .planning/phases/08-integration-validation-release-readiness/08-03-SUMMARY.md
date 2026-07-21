---
phase: 08-integration-validation-release-readiness
plan: 03
subsystem: testing
tags: [verb-lang, ffi, cross-compile, zig, e2e]

# Dependency graph
requires:
  - phase: 08-integration-validation-release-readiness (plan 02)
    provides: "examples/integration_all.verb and examples/integration_all_windows.verb (the two source programs), plus the host-build zero-leaks e2e test pattern to model from"
provides:
  - "tests/e2e.rs::build_mathlib_for_target() — per-target static libmathlib build helper (zig c++ -target <triple> + zig ar)"
  - "tests/e2e.rs::integration_example_cross_builds_all_targets() — build-only cross-compile test proving INTEG-02 across all 6 supported targets"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Per-target static libmathlib (zig c++ -target <triple> -c ... && zig ar rcs ...): a host-built dylib cannot link into a foreign-target binary, so each of the 6 targets gets its own target-matched static archive built fresh, mirroring build_mathlib_fixture()'s host-dylib pattern but per-label and per-triple"
    - "Build-only cross-compile assertion (existing pattern, reused): assert exit status + non-empty output file only, never execute the produced binary — matches aot_cross_build_produces_binary_for_each_target exactly"

key-files:
  created: []
  modified:
    - tests/e2e.rs

key-decisions:
  - "Used a fixed 6-element (label, zig_triple) array inline in the test rather than importing src/targets.rs::ALL/zig_triple() from the test binary — tests/e2e.rs is a black-box integration test crate (invokes the compiled `verb` binary via Command, does not depend on the `verb` lib crate), so there is no existing precedent or clean path to reach src/targets.rs's private-to-binary types from here. The triples are transcribed verbatim from src/targets.rs::Target::zig_triple() and cross-checked against it during this plan."
  - "libmathlib is archived as a static .a (via `zig ar rcs`) rather than built as a cross dylib/so — a static archive is sufsimpler to produce per-target with zig and matches the plan's explicit guidance (D-05: build-only, no runtime execution of the resulting binary, so a dynamic loader path is never exercised for the foreign target anyway)."

requirements-completed: [INTEG-02]

coverage:
  - id: D1
    description: "A per-target libmathlib build helper (build_mathlib_for_target) compiles tests/fixtures/cpp/mathlib.cpp unchanged via zig c++ -target <triple> and archives a target-matched libmathlib.a, so the FFI import resolves at cross-link time for each of the 6 targets"
    requirement: "INTEG-02"
    verification:
      - kind: unit
        ref: "cargo test --test e2e --no-run (test crate compiles with the new helper)"
        status: pass
    human_judgment: false
  - id: D2
    description: "integration_example_cross_builds_all_targets() cross-compiles examples/integration_all.verb (4 non-Windows targets) and examples/integration_all_windows.verb (2 Windows targets) via `verb build --target <label>` with a per-target -L libmathlib dir, asserting build success and a non-empty output file only — never executing a foreign-target binary — and skips cleanly when zig is absent"
    requirement: "INTEG-02"
    verification:
      - kind: e2e
        ref: "tests/e2e.rs#integration_example_cross_builds_all_targets"
        status: pass
    human_judgment: true
    rationale: "zig is not installed on this execution host, so the test's actual per-target build/link path (the core INTEG-02 claim: a target-matched libmathlib resolves the FFI import for all 6 targets) executed only the zig_available() early-return/skip branch here, not the real build loop. The test is proven to compile, to correctly detect zig's absence, and to skip cleanly (matching the existing aot_cross test's behavior) -- but the cross-build-success assertion itself needs a host with zig on PATH to actually exercise. A human (or a future CI run with zig installed) should confirm all 6 targets build successfully."

# Metrics
duration: 22min
completed: 2026-07-21
status: complete
---

# Phase 8 Plan 03: Cross-Compile Build-Only E2E Test for the Integration Example Summary

**A new build-only e2e test cross-compiles the FFI-importing integration example for all 6 supported targets, linking a fresh per-target-built static libmathlib so `import mod mathlib;` resolves at cross-link time — closing the last gap in INTEG-02.**

## Performance

- **Duration:** ~22 min
- **Started:** 2026-07-21T19:39:00Z (approx.)
- **Completed:** 2026-07-21T20:01:09Z
- **Tasks:** 2/2 completed
- **Files modified:** 1

## Accomplishments
- Added `build_mathlib_for_target(label, zig_triple)` to `tests/e2e.rs`: compiles `tests/fixtures/cpp/mathlib.cpp` (unchanged) via `zig c++ -target <triple> -c` and archives the object into a static `libmathlib.a` per target directory, so each of the 6 targets gets its own target-matched library for `-L<dir>` — a host-built `.dylib` (as used by the existing host-only `build_mathlib_fixture()`) cannot link into a foreign-target binary, which is exactly the failure INTEG-02 exists to prove does not occur.
- Added `integration_example_cross_builds_all_targets()`: iterates all 6 `(label, zig_triple)` pairs matching `src/targets.rs::ALL`, builds `examples/integration_all.verb` for the 4 non-Windows targets and `examples/integration_all_windows.verb` for the 2 Windows targets (D-06), passing the per-target libmathlib's `-L<dir>` on each `verb build --target <label>` invocation. Asserts build success and a non-empty output artifact only (`.exe` suffix accounted for on Windows targets) — never executes any produced foreign-target binary (D-05).
- Guarded the new test with the existing `zig_available()` helper (matching `aot_cross_build_produces_binary_for_each_target`'s pattern exactly), so the suite passes with or without zig on PATH.
- Verified: `cargo test --test e2e --no-run` compiles cleanly; `cargo test --test e2e integration_example_cross_builds_all_targets` passes by printing `skipping: zig not on PATH` and returning (zig is not installed on this execution host); the full `cargo test --test e2e` suite passes 77/77 (76 pre-existing + this new test) with no regressions.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add a per-target libmathlib build helper to tests/e2e.rs** - `210e5db` (feat)
2. **Task 2: Add the cross-compile build-only test for all 6 targets** - `3d76a41` (test)

**Plan metadata:** committed separately by the orchestrator after wave completion (worktree mode — this plan does not update STATE.md/ROADMAP.md itself).

## Files Created/Modified
- `tests/e2e.rs` - added `build_mathlib_for_target()` (per-target static libmathlib builder via zig c++/zig ar) and `integration_example_cross_builds_all_targets()` (build-only cross-compile test for all 6 targets, guarded by `zig_available()`)

## Decisions Made
- Transcribed the 6 `(label, zig_triple)` pairs as a fixed inline array in the test rather than reaching into `src/targets.rs` from the black-box `tests/e2e.rs` integration crate (which only invokes the compiled `verb` binary via `Command`, matching every other test in the file) — values verified against `src/targets.rs::Target::zig_triple()` and `ALL` during this plan.
- Built `libmathlib.a` as a static archive (`zig c++ -target <triple> -c` then `zig ar rcs`) rather than a cross dylib, since the test is build-only and never loads/runs the resulting binary — a static archive is the simplest artifact that satisfies the cross-linker's `-lmathlib` requirement at link time.
- Placed both additions at the end of `tests/e2e.rs`, immediately after `integration_example_zero_leaks()` (plan 02's test), keeping all integration-example-related tests grouped together.

## Deviations from Plan

None - plan executed exactly as written. Both tasks matched their `<action>` and `<acceptance_criteria>` blocks without requiring any Rule 1-4 deviation.

One process note (not a deviation): the plan's Task 1 `<verify>` grep-based automated check (`cargo test --test e2e --no-run 2>&1 | grep -qiE 'Compiling|Finished|warning|error\['`) relies on cargo re-emitting build diagnostics; because cargo caches unchanged builds and prints nothing on a no-op second invocation, that literal grep can transiently fail even though the crate compiles successfully (confirmed via exit code 0 and explicit `Compiling`/`Finished` output on the actual build run immediately prior). The real acceptance criterion — the test crate compiles without errors — was independently confirmed both times.

## Issues Encountered
None. One incidental untracked runtime artifact (`verb_e2e_gc_v2_roundtrip.tmp`) was left in the working tree by an unrelated pre-existing test (`std_io_file_roundtrip_allocates_through_verb_alloc`) during the full-suite verification run; it was removed as harmless cleanup (not committed, not part of this plan's scope).

## User Setup Required

None - no external service configuration required.

**Note for future verification:** `zig` is not installed on this execution host, so `integration_example_cross_builds_all_targets()` has only been proven to compile and to skip cleanly via its `zig_available()` guard. On a host with zig on PATH, re-running `cargo test --test e2e integration_example_cross_builds_all_targets` will exercise the actual 6-target build/link path and should be confirmed to pass (all 6 targets build successfully) before treating INTEG-02 as fully closed end-to-end.

## Known Stubs

None. Both the helper and the test are fully wired — no hardcoded empty values, no placeholder assertions, and no unconnected code paths. The only "unexercised" aspect is environmental (zig absence on this host), not a stub in the code itself.

## Threat Flags

None. This plan added no new network endpoints, auth paths, file access patterns, or schema changes at a trust boundary — it only extends the existing build-only cross-compile test surface already covered by the plan's own `<threat_model>` (T-08-06 cross-compile artifact integrity, T-08-07 elevation-of-privilege via executing foreign binaries, T-08-08 DoS via zig absence), all of which were mitigated exactly as specified (target-matched libmathlib, build-only assertions, zig_available() guard).

## Next Phase Readiness

INTEG-02 is now structurally satisfied: `tests/e2e.rs` contains a build-only cross-compile test covering all 6 supported targets with per-target-matched FFI libraries. The one open item is environmental, not code-level: this execution host lacks `zig`, so the actual 6-target build/link success has not been exercised end-to-end here (only the skip path was exercised). Recommend running the full suite once on a host with zig installed to confirm all 6 targets genuinely build before considering the phase's release-readiness bar fully met. No blockers for any subsequent phase/plan.

---
*Phase: 08-integration-validation-release-readiness*
*Completed: 2026-07-21*
