# Synthesis Summary

Ingest mode: `new`. Source: 22 classification JSON files in
`.planning/intel/classifications/`, covering
`docs/superpowers/plans/*.md` and `docs/superpowers/specs/*.md`.

## Doc counts by type

- ADR: 0
- SPEC: 12 (all `confidence: high`, all `locked: false`)
- PRD: 0
- DOC: 10 (all `confidence: medium` except 2 at `high`)
- UNKNOWN: 0

## Decisions (ADR intel)

0 locked decisions. No ADR documents present in this batch — see
`decisions.md` (fields marked absent, not fabricated).

## Requirements (PRD intel)

0 requirements extracted. No PRD documents present in this batch — see
`requirements.md` (fields marked absent, not fabricated).

## Constraints (SPEC intel)

12 constraints extracted to `constraints.md`, one per SPEC doc:
- api-contract: 3 (cpp-import-design, std-io-import-design,
  verb-export-macro-design)
- protocol: 5 (verb-compiler-design, cross-platform-compile-design,
  multi-file-linking-design, verb-formatter-design, vscode-extension-design)
- schema: 2 (arrays-design, maps-design)
- nfr: 2 (refcounting-gc-design, refcounting-gc-v2-design)

## Context (DOC intel)

10 topics extracted to `context.md`, one per implementation-plan doc
(verb-compiler, cpp-import, cross-platform-compile, multi-file-linking,
std-io-import, verb-export-macro, vscode-extension, arrays,
refcounting-gc v1, refcounting-gc v2). Each cites its companion SPEC via
cross-reference.

## Cycle detection

Ran DFS three-color cycle detection over the `cross_refs` graph built from
all 22 classifications (edges to non-classified files — test fixtures,
runtime sources, README.md — excluded from the classified-doc subgraph).
No cycles found. Max depth observed: 4 (e.g. refcounting-gc-v2-plan ->
refcounting-gc-v2-design -> refcounting-gc-design -> cpp-import-design),
well under the 50-hop cap.

## Conflicts

- BLOCKERS: 0
- WARNINGS (competing-variants): 1 — value-tag collision between the
  Arrays and Maps design specs (both independently claim `tag = 6`); see
  `../INGEST-CONFLICTS.md`.
- INFO (auto-resolved): 2 — refcounting-GC v2 spec/plan explicitly
  supersedes v1 (self-declared in the v2 source text, PR #11 closed
  unmerged); later specs in this batch (arrays, maps, refcounting-GC)
  fulfill items the original compiler-design spec had listed as "out of
  scope (v2+)" — expected roadmap evolution, not a contradiction.

Full detail: `../INGEST-CONFLICTS.md`

## Status

STATUS: AWAITING USER — 1 competing-variant warning needs resolution
(the Array/Map tag-6 collision) before downstream routing should treat
`constraints.md`'s Arrays entry as authoritative on that specific point.
No blockers; synthesis is otherwise complete and safe to read.

## Per-type intel files

- `decisions.md` — ADR intel (empty in this batch)
- `requirements.md` — PRD intel (empty in this batch)
- `constraints.md` — SPEC intel (12 entries)
- `context.md` — DOC intel (10 entries)
