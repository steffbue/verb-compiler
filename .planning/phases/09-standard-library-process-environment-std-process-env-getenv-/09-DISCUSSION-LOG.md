# Phase 9: Standard Library: Process & Environment - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-07-21
**Phase:** 9-Standard Library: Process & Environment
**Areas discussed:** Module split, Raw primitives exposure, spawn/wait API shape, exit/abort vs GC

---

## Module split

| Option | Description | Selected |
|--------|-------------|----------|
| Split | `import std env;` + `import std process;`, mirrors std io/std map | ✓ |
| Single module | One `import std process;` covers everything | |

**User's choice:** Split
**Notes:** Consistent with existing std io / std map convention of one module per concern.

| Option | Description | Selected |
|--------|-------------|----------|
| Require import | exit/abort/pid gated behind `import std process;` | |
| Core builtins, no import | Always available like `print` | ✓ |

**User's choice:** Core builtins, no import
**Notes:** exit/abort/pid need no OS-specific linking beyond libc.

| Option | Description | Selected |
|--------|-------------|----------|
| Build-only, matches precedent | Whole module rejected under `verb run` | ✓ |
| Allow simple calls under JIT | getenv/exit/pid work in JIT too | |

**User's choice:** Build-only, matches precedent
**Notes:** Reconciled with the prior answer — this restriction applies to the import-gated `std env`/`std process` functions only; exit/abort/pid are core builtins (not import-gated) and work under JIT the same as `print`.

---

## Raw primitives exposure

| Option | Description | Selected |
|--------|-------------|----------|
| Internal-only | spawn()/wait() only; fork/execve/waitpid/CreateProcess stay inside the runtime | ✓ |
| Expose raw primitives too | Advanced/unsafe low-level builtins available directly | |

**User's choice:** Internal-only
**Notes:** No way to call fork/execve/CreateProcess directly from Verb code.

| Option | Description | Selected |
|--------|-------------|----------|
| Claude's discretion | fork+execve, posix_spawn, whichever fits best | ✓ |
| fork()+execve() exactly | Use the literal syscalls as specified | |

**User's choice:** Claude's discretion
**Notes:** Original ask named fork/execve/waitpid but doesn't require the literal syscalls if a safer equivalent has the same effect.

---

## spawn/wait API shape

| Option | Description | Selected |
|--------|-------------|----------|
| Array of args | execve-style, no shell involved | ✓ |
| Single shell string | Goes through the shell to parse | |

**User's choice:** Array of args
**Notes:** Avoids shell-injection risk.

| Option | Description | Selected |
|--------|-------------|----------|
| Exit code only | Simplest runtime implementation | ✓ |
| Exit code + captured output | More useful, needs pipe/buffer management | |

**User's choice:** Exit code only

| Option | Description | Selected |
|--------|-------------|----------|
| Sentinel return, caller checks | Matches existing array/map nil-on-bad-input convention | ✓ |
| Abort/panic immediately | No error-checking path for the Verb programmer | |

**User's choice:** Sentinel return, caller checks

---

## exit/abort vs GC

| Option | Description | Selected |
|--------|-------------|----------|
| Immediate libc exit(), skip cleanup | Matches C exit() semantics | ✓ |
| Drain GC state first | Zero-leak invariant holds even on explicit exit | |

**User's choice:** Immediate libc exit(), skip cleanup

| Option | Description | Selected |
|--------|-------------|----------|
| libc abort() | Standard signal-based crash | ✓ |
| Verb-level panic | Custom message, controlled exit | |

**User's choice:** libc abort()

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, add a documented constraint | Keeps pattern consistent with Windows/std-io exception | ✓ |
| No, self-evident | No new doc needed | |

**User's choice:** Yes, add a documented constraint
**Notes:** Flagged as a planner/executor task to update PROJECT.md Constraints — not applied here since discuss-phase doesn't edit PROJECT.md.

---

## Claude's Discretion

- POSIX spawn() implementation: fork+execve vs. posix_spawn/vfork (D-05)
- Internal C++ helper naming in runtime/verb_process.cpp / runtime/verb_env.cpp (or combined file)
- Whether std env and std process are one or two runtime .cpp files

## Deferred Ideas

None — discussion stayed within phase scope.
