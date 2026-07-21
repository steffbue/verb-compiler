## Conflict Detection Report

### BLOCKERS (0)

None. No ADR-classified documents exist in this ingest batch, so no
LOCKED-vs-LOCKED contradiction is possible; no PRD-classified documents
exist, so no competing-requirement contradiction is possible; no cross-ref
cycles were detected in the classified doc set; no `UNKNOWN`/low-confidence
classifications were found.

### WARNINGS (1)

[WARNING] Value-tag collision between Arrays and Maps design specs
  Found: docs/superpowers/specs/2026-07-21-arrays-design.md ("New tag | Tag
    6 | Array | ...") and its companion plan
    docs/superpowers/plans/2026-07-21-arrays.md (Global Constraints: "TAG_ARRAY
    = 6, appended after the existing tags 0-5") both assign value tag 6 to
    Array.
  Found: docs/superpowers/specs/2026-07-21-maps-design.md ("New tag `VERB_MAP
    = 6` in `runtime/verb.h` / `TAG_MAP` in `src/value.rs`") independently
    assigns value tag 6 to Map — same scope ("VerbValue tag enumeration"),
    both SPEC-precedence, both dated 2026-07-21, neither locked.
  Impact: Two co-equal-precedence SPEC sources disagree on a literal
    constant for the same enumeration slot. Synthesis cannot pick a winner
    without risking silently endorsing a stale document. (For transparency:
    the actual shipped code — src/value.rs, runtime/verb.h — resolves this
    as TAG_MAP=6 / TAG_ARRAY=7, i.e. Arrays was bumped to 7 and the Arrays
    design/plan docs were never updated to match. This was observed by
    reading the current codebase, not asserted as a source document, so it
    is reported here as impact context rather than used to silently
    auto-resolve the conflict.)
  → Update docs/superpowers/specs/2026-07-21-arrays-design.md (and its
    companion plan's Global Constraints) to read `TAG_ARRAY = 7`, matching
    the shipped implementation, or explicitly mark one doc Superseded if a
    re-tag is intended. Re-run ingest after the correction.

### INFO (2)

[INFO] Auto-resolved: refcounting-GC v2 supersedes v1 (same precedence tier, self-declared)
  Note: docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md states
    directly: "docs/superpowers/specs/2026-07-21-refcounting-gc-design.md
    designed and implemented a refcounting GC for strings and closures (PR
    #11)... PR #11 is closed unmerged — main diverged too far for a clean
    rebase... This spec re-applies the original design (unchanged) against
    current main, and extends it to cover arrays, maps, and the new
    global-binding mechanism." The same relationship holds for the paired
    plans (docs/superpowers/plans/2026-07-21-refcounting-gc.md vs
    2026-07-21-refcounting-gc-v2.md). Both v1 and v2 SPEC/DOC pairs were
    extracted separately into constraints.md/context.md (nothing was
    dropped), each annotated with this supersession. v2 is authoritative
    for current/future work; v1 is retained for provenance. This matches
    the current git branch (refcounting-gc-v2) and recent commit history
    ("feat(gc): wire array builtin call sites", "test(gc): verify zero
    leaks across all heap kinds").

[INFO] Auto-resolved: later specs fulfill the original compiler design's declared "out of scope (v2+)" list
  Note: docs/superpowers/specs/2026-07-19-verb-compiler-design.md lists
    "Out of scope (v2+): Arrays/maps, GC, break/continue, anonymous fns,
    string methods, modules/imports, result-style error handling" and
    states "No GC in v1... malloc'd, never freed" as a "deliberate
    simplification." The Arrays, Maps, and both refcounting-GC specs in
    this same ingest batch are exactly those deferred v2 items being
    delivered. This is expected roadmap evolution, not a contradiction —
    no auto-resolution was needed beyond noting it for downstream
    (gsd-roadmapper) context, since precedence rules only adjudicate
    genuine contradictions and this is sequential delivery of previously
    out-of-scope work.
