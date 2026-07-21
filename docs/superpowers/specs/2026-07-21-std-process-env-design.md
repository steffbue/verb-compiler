# `import std env` / `import std process` — Design Spec (v1)

Date: 2026-07-21
Status: approved

## Purpose

Give Verb programs environment-variable access, process introspection
(cwd, executable path, pid), process termination (exit, abort), and
child-process spawn/wait — via two curated stdlib modules plus a small
set of core builtins, following the exact shape of the existing
`std io` / `std map` modules ([2026-07-20-std-io-import-design.md](2026-07-20-std-io-import-design.md)).

This design was originally brainstormed and approved in a parallel
worktree (`agent-a645b92ce5246fec0`, phase 9 planning docs) before any
implementation existed there. This spec re-derives the same approved
decisions against `refcounting-gc-v2`'s actual current structure
(`build.rs` + `src/main.rs`, not the other worktree's hypothetical
`build.rs`-registration-for-everything description) and is the version
that gets implemented.

## Language surface

```
import std env;
import std process;

assign home env_get("HOME");
check home eq nil begin
  print("no HOME set");
end

env_set("FOO", "bar");
print(env_get("FOO"));
env_unset("FOO");

print(cwd());
print(exe_path());

assign pid spawn("echo", list("hi"));
check pid eq nil begin
  print("spawn failed");
end orelse begin
  assign code wait(pid);
  print(code);
end

print(get_pid());
exit(0);
```

- Two new `std` module names: `env` and `process` (parser's known-module
  list grows from `io, map` to `io, map, env, process`).
- `exit`, `abort`, `get_pid` are **core builtins** — resolved the same
  tier as `print`/`len`/`get`/`set`/`push`/`pop` in `gen_call`, no import
  required, and (unlike `std env`/`std process`) usable under `verb run`
  (JIT) exactly like `print` is.

## Module split and gating

- `std env`: `env_get`, `env_set`, `env_unset` (not `getenv`/`setenv`/
  `unsetenv` — those names collide at C-linkage level with libc's own
  functions of the same name, which `runtime/verb_env.cpp` also needs
  to call; discovered during implementation, see that file's header
  comment).
- `std process`: `cwd`, `exe_path`, `spawn`, `wait`.
- `exit`, `abort`, `get_pid`: core builtins, not gated behind any
  import — trivial libc calls (`exit`/`abort`) or a one-line OS-primitive
  wrapper (`get_pid`) that need no extra runtime linking beyond what's
  already always-linked (see Build integration).
- `import std env;` and `import std process;` are **build-only**,
  exactly like `std io`/`std map` today: `verb run` already rejects any
  program with a non-empty `std_imports` list (`src/main.rs:220-224`),
  so both new module names fall under that existing check automatically
  — no new rejection code needed. `exit`/`abort`/`get_pid` are exempt
  from this because they never touch `std_imports` at all.

## Raw platform primitives: internal-only

`fork`/`execve`/`waitpid` (POSIX) and `CreateProcess`/
`WaitForSingleObject`/`GetExitCodeProcess`/`GetCurrentProcessId`
(Windows) are implementation details inside `runtime/verb_process.cpp`
and `runtime/verb_builtins.cpp`. They are never exposed as Verb-callable
names — the only Verb-facing calls are `spawn`/`wait`/`get_pid`.

## `spawn`/`wait` API shape

- `spawn(cmd, args)` — `cmd` is a string (the executable name/path,
  resolved via `PATH` on POSIX / the normal Windows search rules),
  `args` is a Verb array of strings (extra argv entries, **not**
  including `argv[0]`). No shell is invoked to parse anything — avoids
  shell-injection risk entirely. Returns the child's pid (int) on
  success, `nil` if `cmd`/`args` are the wrong Verb types or the OS
  couldn't create a process at all (`fork()` failure on POSIX,
  `CreateProcess` failure on Windows). **Asymmetry, by design, not a
  bug:** on Windows, `CreateProcess` validates the target executable
  exists before returning, so a missing binary makes `spawn()` itself
  return `nil`. On POSIX, `fork()` always succeeds regardless of
  whether `cmd` exists — the child only discovers the binary is
  missing when it calls `execvp`, by which point it's a separate
  process that can't hand a `VerbValue` back to `spawn()`'s caller. So
  on POSIX, a missing binary makes `spawn()` return a valid pid, and
  the failure surfaces later as `wait(pid)` returning exit code 127
  (the same convention a POSIX shell uses for "command not found").
  Detecting this synchronously would require a self-pipe between
  parent and child to relay `execvp`'s `errno` before the child exits
  — deliberately out of scope for v1 (see Out of scope).
- `wait(pid)` — blocks until the given pid exits, returns its exit code
  (int). Returns `nil` if `pid` doesn't correspond to a live child of
  this process (bad `waitpid`/`WaitForSingleObject` result). No stdout/
  stderr capture — keeps the runtime free of pipe/buffer management.
- Both follow the existing "sentinel return, caller checks" convention
  (same as array/map operations on bad input) rather than raising an
  exception.

## `exit`/`abort` semantics vs GC

- `exit(code)` calls libc `exit()` immediately — **skips refcount
  cleanup**, matching C `exit()` semantics exactly. Does not need to
  prove `verb_gc_live == 0` first.
- `abort()` calls libc `abort()` — a hard SIGABRT-style crash, not a
  "friendly" Verb-level panic. No new message-formatting machinery.
- `.planning/PROJECT.md`'s Constraints section gains a new bullet
  alongside the existing Windows/`std io` exception: `exit`/`abort` are
  excluded from the zero-leak (GC) guarantee, by design, same as C.

## AST / parser changes

- `src/parser.rs`'s `import_stmt` known-module check grows from
  `name != "io" && name != "map"` to also accept `"env"` and
  `"process"`; the error message's module list grows to match.
- No other grammar changes. `exit`/`abort`/`get_pid` are ordinary
  function-call expressions resolved in codegen, not new keywords.

## Codegen

Two new static arity tables mirroring `IO_FUNCS`/`MAP_FUNCS`:

```rust
const ENV_FUNCS: &[(&str, usize)] = &[
    ("env_get", 1),
    ("env_set", 2),
    ("env_unset", 1),
];
const PROCESS_FUNCS: &[(&str, usize)] = &[
    ("cwd", 0),
    ("exe_path", 0),
    ("spawn", 2),
    ("wait", 1),
];
```

`gen_call` gains two more `std_imports`-gated tiers (checked the same
way as the existing `io`/`map` tiers, reusing `gen_std_io_call` — the
name is generic despite saying "io", it just does "declare once by
arity, call, release args").

`exit`, `abort`, `get_pid` are matched by name directly in `gen_call`
next to `print`/`len` — always available, never gated by
`std_imports`. Each declares (once, memoized in `externs` like the
`io`/`map` tiers do) a `value_ty -> value_ty` (or 0-arity) extern and
calls it via `call_named`, releasing its argument(s) same as `print`
does.

## Runtime: two new C++ units

### `runtime/verb_env.cpp` (build-only, `std env`)

No POSIX/Windows divergence needed at the libc-call level (`getenv`,
`_putenv_s`/`setenv`, `_putenv`/`unsetenv` all exist both places under
slightly different names) — one `#ifdef _WIN32` pair of thin wrappers,
same file. Failure (`unsetenv`/`setenv` of a malformed name) returns
`verb_nil()`/`verb_bool(0)`, matching the `io`/`map` convention.

### `runtime/verb_process.cpp` (build-only, `std process`)

- `cwd()` — `getcwd()` (POSIX) / `GetCurrentDirectoryA()` (Windows).
- `exe_path()` — `readlink("/proc/self/exe", ...)` (Linux) /
  `_NSGetExecutablePath()` (macOS) / `GetModuleFileNameA()` (Windows).
- `spawn(cmd, args)` — reads `cmd` as `VERB_STRING` and `args` as a
  Verb array (`{ int64_t len, cap; VerbValue* elems }`, matching
  `src/value.rs`'s `TAG_ARRAY = 7` layout exactly — `verb.h` gains a
  `VERB_ARRAY = 7` enum value so this file can recognize the tag; no
  new public accessor beyond what this file needs, same spirit as
  `verb_map.cpp` reaching into its own opaque payload type). Builds a
  NUL-terminated `argv[]` (`cmd` as `argv[0]`, each array element —
  must be `VERB_STRING`, else `spawn` fails and returns `nil` — as
  `argv[1..]`), then:
  - **POSIX**: `fork()` + `execvp()` in the child (`execve` plus a
    `PATH` search, so `spawn("sh", ...)` resolves without a hardcoded
    absolute path — every other constraint above still holds: no
    shell is invoked to parse anything, `execvp` just locates the
    binary the same way a shell's own bare-word lookup would),
    `waitpid`-free in the parent (parent just returns the child pid
    immediately — blocking happens in `wait()`, not `spawn()`).
  - **Windows**: `CreateProcess()` with a quoted/escaped command line
    built from `argv[]`, returns `dwProcessId` on success. The
    `PROCESS_INFORMATION.hProcess` handle is stashed (see below) so a
    later `wait()` can call `WaitForSingleObject`/`GetExitCodeProcess`
    on it instead of a pid.
- `wait(pid)` — **POSIX**: `waitpid(pid, &status, 0)`, returns
  `WEXITSTATUS(status)` if `WIFEXITED`, else `nil`. **Windows**: pid
  alone can't `OpenProcess`+wait reliably for a child this process
  itself started without `PROCESS_QUERY_INFORMATION` rights in all
  cases, so the Windows `spawn()` keeps a small process-local
  `pid -> HANDLE` map (a plain `std::unordered_map`, not a Verb map)
  alive for exactly this purpose; `wait()` looks the handle up, calls
  `WaitForSingleObject` + `GetExitCodeProcess`, then closes and erases
  the handle. A `wait()` on an unknown pid returns `nil`.

### `runtime/verb_builtins.cpp` (always linked, core builtins)

- `builtin_exit(VerbValue code)` — `exit((int)verb_as_int(code))`, `-> VerbValue` never returns (but needs the signature for codegen's uniform call convention; codegen ignores the "returned" value since the call never falls through).
- `builtin_abort()` — `abort()`.
- `builtin_get_pid()` — `getpid()` (POSIX) / `GetCurrentProcessId()`
  (Windows), wrapped as `verb_int(...)`.

These need OS-conditional code (`get_pid` specifically) but must work
under **both** `verb run` (JIT, host platform only) and every AOT
target — see Build integration for why this means dual registration,
matching `verb_map.cpp`'s existing precedent exactly.

## Build integration

- **`build.rs`**: `runtime/verb_builtins.cpp` is added to the
  always-compiled-into-the-`verb`-binary set (alongside
  `verb_map.cpp`), since `get_pid`/`exit`/`abort` are core builtins
  that must resolve under `verb run` (JIT) too, and (per the existing
  comment on `verb_map.cpp` in `build.rs`) MCJIT needs the real
  symbols compiled directly into the host process, not merely
  registered as a stub — `main.rs`'s `register_jit_runtime_symbols`
  path grows the same `add_global_mapping` treatment for
  `builtin_exit`/`builtin_abort`/`builtin_get_pid`.
- **`src/main.rs`** (AOT, `build`/`compile`):
  - `runtime/verb_builtins.cpp` joins `runtime/verb_map.cpp` as
    **unconditionally** compiled and linked (matches the existing
    "verb_map.cpp is now unconditional" precedent/comment in
    `build_aot_host` — same reasoning: a core builtin can appear in
    any program, and there's no cheap way to detect "this program
    definitely never calls `exit`/`abort`/`get_pid`" ahead of linking).
  - `runtime/verb_env.cpp` is compiled+linked only when `std_imports`
    contains `env` (new `compile_env_obj`, mirrors
    `compile_std_io_obj`).
  - `runtime/verb_process.cpp` is compiled+linked only when
    `std_imports` contains `process` (new `compile_process_obj`, same
    pattern).
  - Both new conditional objects follow `build_aot_cross`'s existing
    per-target compile pattern (native `cc`/`c++` for host, `zig c++
    -target <triple>` for cross) — no new Windows-cross restriction
    needed for `env` (libc `getenv`/`_putenv` are available under the
    mingw cross toolchain) or `process` (`CreateProcess` compiles fine
    under mingw headers), unlike `std io`'s existing POSIX-socket
    restriction.

## Testing

- Parser unit tests: `import std env;`/`import std process;` parsing,
  updated unknown-module-name error message (now lists `env, io, map,
  process`), coexistence with `import mod`/`import std io`.
- Codegen unit tests: arity mismatch against `ENV_FUNCS`/`PROCESS_FUNCS`
  → compile error; `exit`/`abort`/`get_pid` resolve without any
  `std_imports` present.
- E2e fixtures (extends the existing `verb build` + execute + diff-stdout
  harness):
  - JIT-rejection test: `verb run` on a program with `import std env;`
    (and separately `import std process;`) errors out the same way
    `import std io;` does today.
  - `get_pid()`/`exit`/`abort` work under `verb run` (JIT) with no
    import at all — proves D-02/D-03's "core builtin, not gated"
    distinction actually holds.
  - `env_get`/`env_set`/`env_unset` roundtrip.
  - `cwd()`/`exe_path()` return non-nil strings.
  - `spawn` + `wait` roundtrip: spawn a trivial child (e.g. the
    system's `true`/`false` or `cmd.exe /c exit <n>` equivalent) and
    assert the exit code `wait()` reports matches.
  - `spawn` of a nonexistent executable returns `nil`.
  - `exit(0)` inside a Verb program short-circuits before any trailing
    `print` — proves the "skip GC cleanup" semantics (D-09) rather
    than silently also running a leak check that would need to pass.

## `.planning/PROJECT.md` changes

- Constraints section gains: **`exit()`/`abort()` are excluded from the
  zero-leak (GC) guarantee — they call libc `exit`/`abort` directly and
  never run refcount cleanup, by design (matches C's own `exit`/`abort`
  semantics)**, placed next to the existing Windows/`std io`
  POSIX-socket exception.
- Tech stack bullet listing `verb.h`, `verb_map.cpp`, `verb_std_io.cpp`
  grows to include `verb_env.cpp`, `verb_process.cpp`,
  `verb_builtins.cpp`.

## Out of scope (future spec)

Captured stdout/stderr from `spawn`ed children; signals (`kill`,
custom handlers); non-blocking `wait` (`waitpid(WNOHANG)`/
polling); environment as a bulk snapshot (`environ` iteration —
`env_get`/`env_set`/`env_unset` are per-key only); process groups/
sessions; raw fork/execve/CreateProcess exposed directly to Verb code;
a self-pipe (or `posix_spawn`-based) mechanism to make `spawn()` itself
detect a missing POSIX executable synchronously — v1 accepts that this
surfaces via `wait()`'s exit code 127 instead (see spawn/wait API
shape).
A `spawn`ed pid that's never passed to `wait()` leaks a zombie entry
(POSIX) or an open process handle (Windows) — accepted v1 limitation,
same class as the existing unbounded-but-accepted cyclic-array leak.
