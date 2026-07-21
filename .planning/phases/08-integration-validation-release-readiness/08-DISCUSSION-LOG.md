# Phase 8: Integration Validation & Release Readiness - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-07-21
**Phase:** 8-Integration Validation & Release Readiness
**Areas discussed:** Integration example program design, Verification approach, Cross-compile scope for the FFI part, Windows / std io exception handling, GC leak-check integration

---

## Integration example program design

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal mechanical combo | No real-world story; just exercises FFI+stdio+map+arrays in ~15 lines | ✓ |
| Small themed program | e.g. word-frequency counter; more realistic, more code | |
| Let Claude decide | Pick whichever minimizes plan risk | |

**User's choice:** Minimal mechanical combo
**Notes:** None.

| Option | Description | Selected |
|--------|-------------|----------|
| tests/fixtures/ + tests/e2e.rs | Matches existing fixture-pairing pattern exactly | |
| examples/ directory | User-facing demo program instead of a test fixture | ✓ |

**User's choice:** examples/ directory
**Notes:** Deliberate deviation from the universal fixture-pairing convention — required a follow-up on how verification would work automatically.

---

## Verification approach

| Option | Description | Selected |
|--------|-------------|----------|
| e2e.rs builds+runs examples/ path directly | New test fn invokes `verb build examples/integration_all.verb` directly, no fixture duplicate | ✓ |
| Duplicate into tests/fixtures/ too | Keep examples/ as demo, also copy into tests/fixtures/ to plug into existing GC loop | |

**User's choice:** e2e.rs builds+runs examples/ path directly
**Notes:** Avoids keeping two copies in sync.

| Option | Description | Selected |
|--------|-------------|----------|
| Standalone test function | New `integration_example_zero_leaks()` calling `assert_no_leaks()` | ✓ |
| Add to existing list | Append to `gc_no_leaks_across_all_heap_kinds` fixture loop | |

**User's choice:** Standalone test function
**Notes:** Keeps INTEG-01's cross-cutting nature visible in the test name.

---

## Cross-compile scope for the FFI part

| Option | Description | Selected |
|--------|-------------|----------|
| Build-only, like existing aot_cross_build test | Cross-compile mathlib.cpp per target via zig cc, assert build success only, no execution of non-host binaries | ✓ |
| Host-only FFI, stdlib-only for cross targets | Two programs: FFI (host-only) and stdlib-only (cross-compiled) | |

**User's choice:** Build-only, like existing aot_cross_build test
**Notes:** Matches existing `aot_cross_build_produces_binary_for_each_target` pattern exactly.

---

## Windows / std io exception handling

| Option | Description | Selected |
|--------|-------------|----------|
| Documented skip in the pass/fail summary | Same program for all targets; Windows expected to fail/be excluded per existing std io limitation | |
| Stdio-less variant for Windows | Second variant without `import std io` specifically for windows-x86_64/windows-arm64 | ✓ |

**User's choice:** Stdio-less variant for Windows
**Notes:** All 6 targets expected to fully succeed with this approach — no exception carve-out needed in the pass/fail summary beyond the pre-existing documented std io/Windows constraint.

---

## Claude's Discretion

- Exact FFI function(s) exposed by the mathlib-equivalent test lib (reuse existing `mathlib.cpp` exports vs. adding a new minimal one)
- Exact array/map operations used in the ~15-line example
- Naming of the new `tests/e2e.rs` test functions and the Windows-variant example file

## Deferred Ideas

None — discussion stayed within phase scope.
