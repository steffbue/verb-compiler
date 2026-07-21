# Requirements (PRD intel)

No PRD-classified documents were found in this ingest batch
(`CLASSIFICATIONS_DIR` contains only `DOC` and `SPEC` classifications — see
`/Users/steffen/Desktop/Projekte/compiler/.planning/intel/classifications/`).

- source: absent
- description: absent
- acceptance: absent
- scope: absent

Several plan docs (classified `DOC`) contain "Requirements"/"Usage" sections
(e.g. `docs/superpowers/plans/2026-07-19-verb-compiler.md`), but these are
build/toolchain prerequisites (Rust version, LLVM version, `cc`), not
product requirements with acceptance criteria — they were left in
`context.md` under their originating DOC entry rather than promoted here,
per the classifier's type assignment (which this synthesizer does not
override).

If PRDs are added in a later ingest batch, this file should gain one
`## REQ-{slug}` entry per requirement, per the standard schema:

```
## REQ-{slug}
- source: {path}
- description: {text}
- acceptance: {criteria}
- scope: {scope}
