# Features Backlog — Implementation Plans

Line-verified implementation plans for post-v1 feature work on Verb, grouped by tier.
All line numbers reference branch `refcounting-gc-v2`. Each plan is grounded in real
`file:line` locations, with approach, files/functions to change, test strategy, and risks.

| Tier | File | Scope | Tasks |
|------|------|-------|-------|
| 2 | [TIER-2-hardening.md](TIER-2-hardening.md) | Correctness / hardening (release-readiness gaps) | `--target all` `-L` fix (SC2 blocker), FFI string SIGTRAP, overflow checks, OOM handling |
| 3 | [TIER-3-language-features.md](TIER-3-language-features.md) | Net-new language features | structs, real closures (capture), enums + match, result errors, string methods |
| 4 | [TIER-4-tooling.md](TIER-4-tooling.md) | Tooling / ecosystem (low language-design risk) | `verb targets`, optimizer `-O0..3`, REPL, `std net`/UDP, typed externs, DWARF |

## Recommended entry points

- **Ship v1 cleanly** → Tier 2 Task 1 (`--target all`, fixes verification SC2) + Task 2 (FFI string
  bug); both are real blockers and touch disjoint files (fully parallel). Then Tier 4 A+B as cheap wins.
- **Grow the language** → Tier 3 structs + real closures — the two features that most change what
  Verb can express. Closure env-passing plumbing already exists (`codegen.rs:2002-2008`).

## Parallelizable across worktrees
T2-1, T2-2, T3-structs, T3-closures, T4-A, T4-B, T4-D all touch disjoint files.
