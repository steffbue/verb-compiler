# Phase 8: Integration Validation & Release Readiness - Context

**Gathered:** 2026-07-21
**Status:** Ready for planning

<domain>
## Phase Boundary

Verb clears its "feature-complete, self-hosted-capable" bar: a single real
example program exercises C++ FFI (`import mod`), `std io`, `std map`, and
arrays together, proving zero GC leaks and successful cross-compilation
across all supported targets. Additionally, the stale `TAG_ARRAY = 6` literal
in the Arrays design docs is corrected to `TAG_ARRAY = 7` to match shipped
code. No new language features or subsystems — this phase only proves
existing subsystems interoperate and cleans up one known doc/code mismatch.

</domain>

<decisions>
## Implementation Decisions

### Integration example program
- **D-01:** The example is a minimal mechanical combo, not a themed/narrative
  program — no real-world story needed. It should: call a C++ FFI function
  (via the existing `tests/fixtures/cpp/mathlib.cpp` test lib or an
  equivalent), write a result to a file via `std io`, tally call counts in a
  `std map`, and collect results in an array. ~15 lines is enough to exercise
  all four subsystems together.
- **D-02:** The program lives in `examples/` (e.g.
  `examples/integration_all.verb`) as a user-facing demo — NOT duplicated
  into `tests/fixtures/`. This is a deliberate deviation from the otherwise
  universal `tests/fixtures/{name}.verb` + `.expected` pairing pattern
  documented in `.planning/codebase/TESTING.md`.

### Verification approach
- **D-03:** Because the program lives in `examples/` and not
  `tests/fixtures/`, automated verification happens by adding a test in
  `tests/e2e.rs` that invokes `verb build examples/integration_all.verb`
  directly (no fixture duplicate) and checks `verb_gc_live=0` plus expected
  output. One source of truth for the program; the test drives it in place.
- **D-04:** GC leak verification is a **standalone test function**
  (e.g. `integration_example_zero_leaks()` calling the existing
  `assert_no_leaks()` helper from `tests/e2e.rs`) — NOT appended to the
  `gc_no_leaks_across_all_heap_kinds` fixture-name loop. This keeps
  INTEG-01's cross-cutting nature visible in the test name rather than
  buried among 16 other single-subsystem fixtures.

### Cross-compile scope for the FFI part
- **D-05:** Cross-compile validation for the FFI/mod-import piece is
  **build-only**, following the exact pattern of the existing
  `aot_cross_build_produces_binary_for_each_target` test in `tests/e2e.rs`:
  compile `mathlib.cpp` (or equivalent) for each of the 6 targets via
  `zig cc`, link, assert build success only. Non-host target binaries are
  never executed — this matches existing practice, avoids needing real
  execution environments for 6 OS×arch combos.

### Windows / std io exception
- **D-06:** Because `import std io` is not supported on Windows
  cross-compile (POSIX socket dependency, an existing documented
  constraint), build a **second, stdio-less variant** of the integration
  program specifically for `windows-x86_64`/`windows-arm64` (FFI + std map +
  arrays only, no file I/O). All 6 targets are expected to fully succeed in
  the `--target all` pass/fail summary — no target is a documented exception
  in this phase's success criteria beyond the pre-existing std io/Windows
  constraint itself.

### Housekeeping (HOUSEKEEP-01)
- **D-07:** Correct `TAG_ARRAY = 6` → `TAG_ARRAY = 7` in
  `docs/superpowers/specs/2026-07-21-arrays-design.md` and its companion
  plan `docs/superpowers/plans/2026-07-21-arrays.md` (Global Constraints
  section), matching shipped `src/value.rs` / `runtime/verb.h`. Doc-only
  change — no code changes, no re-tagging.

### Claude's Discretion
- Exact FFI function(s) exposed by the mathlib-equivalent test lib for the
  integration example (reuse `mathlib.cpp`'s existing exports vs. adding a
  new minimal one) — pick whichever is less code churn.
- Exact array/map operations used in the ~15-line example, as long as all
  four subsystems (FFI, std io, std map, arrays) are genuinely exercised.
- Naming of the new `tests/e2e.rs` test functions and the Windows-variant
  example file.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements & project state
- `.planning/REQUIREMENTS.md` — HOUSEKEEP-01, INTEG-01, INTEG-02 definitions and v1 traceability
- `.planning/PROJECT.md` — Core value statement, known doc/code mismatch note, constraints (Windows std io limitation)
- `.planning/ROADMAP.md` §Phase 8 — Goal and success criteria for this phase

### Stale doc requiring correction (HOUSEKEEP-01)
- `docs/superpowers/specs/2026-07-21-arrays-design.md` — contains stale `TAG_ARRAY = 6`
- `docs/superpowers/plans/2026-07-21-arrays.md` — Global Constraints section, same stale value
- `docs/superpowers/specs/2026-07-21-maps-design.md` — the competing spec claiming tag 6 for Map (`TAG_MAP = 6`); read for context, not modified

### Existing patterns to follow (from codebase map)
- `.planning/codebase/TESTING.md` — test fixture conventions, `assert_no_leaks()` helper pattern, existing `aot_cross_build_produces_binary_for_each_target` cross-compile test pattern
- `tests/e2e.rs` — existing test functions to model new ones on: `assert_no_leaks()`, `aot_cross_build_produces_binary_for_each_target()`, `build_mathlib_fixture()`
- `tests/fixtures/cpp/mathlib.cpp` — existing C++ FFI test library, reusable for the integration example's `import mod` call
- `tests/fixtures/gc_stress_all_kinds.verb` — closest existing fixture combining map+array+GC (missing FFI/std io/cross-compile), useful as a starting reference for the new example's style

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `tests/fixtures/cpp/mathlib.cpp`: existing compiled-per-test-run C++ FFI library (built via `build_mathlib_fixture()` in `tests/e2e.rs`) — reuse for the integration example's `import mod` call instead of writing a new C++ file.
- `assert_no_leaks()` helper in `tests/e2e.rs`: builds a fixture, runs it with `VERB_GC_DEBUG=1`, asserts `verb_gc_live=0`. Reuse directly for the new standalone GC test.
- `zig_available()` helper in `tests/e2e.rs`: guards cross-compile tests when `zig` isn't on PATH — reuse for the new cross-compile test.

### Established Patterns
- Fixture pairing convention (`{name}.verb` + `{name}.expected`) is the norm for `tests/fixtures/`, but this phase deliberately places the demo program in `examples/` instead (D-02) — the new e2e test builds from `examples/` directly rather than duplicating into `tests/fixtures/`.
- Cross-compile tests assert build-success only for non-host targets, never execution — established by `aot_cross_build_produces_binary_for_each_target`.

### Integration Points
- `build.rs` / `cc` crate compiles C++ runtime and test fixtures — no changes needed here since `mathlib.cpp` is reused as-is.
- `src/targets.rs` defines the 6 target combos already; no new target support needed.

</code_context>

<specifics>
## Specific Ideas

No specific narrative requirements — user explicitly chose the minimal
mechanical combo over a themed program (D-01). The two concrete artifacts to
produce are: `examples/integration_all.verb` (full combo, host + 5 targets)
and a Windows-specific stdio-less variant (D-06), plus corresponding
`tests/e2e.rs` test functions.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 8-Integration Validation & Release Readiness*
*Context gathered: 2026-07-21*
