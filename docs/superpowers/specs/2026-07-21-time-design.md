# Time (`import std time`) — design

## Context

Verb has no way to measure elapsed time, get a wall-clock timestamp, or
pause execution. This adds a third first-party `std` module, following
the same opt-in pattern already established by `import std io` and
`import std map` (see `docs/superpowers/specs/2026-07-20-std-io-import-
design.md` and `docs/superpowers/specs/2026-07-21-maps-design.md`): a new
`runtime/verb_time.cpp`, gated behind `import std time;`, usable only
with `verb build`/`compile` (not `verb run`/JIT) — for the same reason
`std io`/`std map` are build-only: the C++ object is only compiled and
linked for AOT builds.

## Value representation

No new `VerbValue` tag — every function here takes/returns plain
int64_t (millisecond counts) or nothing, so `VERB_INT`/`VERB_NIL` cover
the whole surface. Unlike `std io`/`std map`, this means the
implementation doesn't need hand-written `extern "C" VerbValue`
wrappers at all: it uses `VERB_EXPORT` (`runtime/verb.h`, see
`docs/superpowers/specs/2026-07-20-verb-export-macro-design.md`), the
same macro `import mod` C++ bindings use, to generate the boundary
wrappers from plain C++ functions.

## API surface

Nine builtin functions, available whenever `import std time;` is
present, wired the same way `std io`/`std map` functions are
(`src/codegen.rs` `gen_std_io_call`/`IO_FUNCS`/`MAP_FUNCS`, generalized
to a third `TIME_FUNCS` table).

Portable (every platform, `<chrono>`/`<thread>`/`<ctime>` only):

| Function | Arity | Returns |
|---|---|---|
| `now_ms()` | 0 | int: milliseconds since the Unix epoch (`std::chrono::system_clock`) — wall-clock, can jump if the system clock is adjusted |
| `monotonic_ms()` | 0 | int: milliseconds from a monotonic clock (`std::chrono::steady_clock`) — never goes backwards; only meaningful as a difference between two calls, so this is what elapsed-time measurement should use, not `now_ms()` |
| `sleep_ms(ms)` | 1 | nil: blocks the calling thread for `ms` milliseconds (`std::this_thread::sleep_for`); `ms <= 0` is a no-op rather than an error |
| `clock_ms()` | 0 | int: CPU time consumed by this process, in milliseconds (`std::clock()` converted from `clock_t` ticks) — the same quantity C's `clock()` reports; does not advance while blocked/sleeping, unlike `monotonic_ms()` |
| `difftime_ms(later, earlier)` | 2 | int: `later - earlier` — offered under C's familiar `difftime` name even though it's just `sub`, so callers don't have to remember `sub`'s operand order |

Platform-specific (only defined under the matching preprocessor guard in
`runtime/verb_time.cpp`, direct OS API bindings rather than `<chrono>`/
`<thread>` wrappers):

| Function | Arity | Guard | Returns |
|---|---|---|---|
| `linux_clock_gettime_ns(clock_id)` | 1 | `__linux__` | int: nanoseconds from `clock_gettime`; `clock_id` is the raw Linux `clockid_t` value (`0` = `CLOCK_REALTIME`, `1` = `CLOCK_MONOTONIC`) |
| `linux_nanosleep_ns(ns)` | 1 | `__linux__` | nil: `nanosleep`; `ns <= 0` is a no-op |
| `win_filetime_100ns()` | 0 | `_WIN32` | int: raw `FILETIME` (`GetSystemTimeAsFileTime`) as 100ns intervals since 1601-01-01, packed into an `int64_t` via `ULARGE_INTEGER` |
| `win_sleep_ms(ms)` | 1 | `_WIN32` | nil: `Sleep`; `ms <= 0` is a no-op |

`TIME_FUNCS` lists all nine names unconditionally — arity is a
language-level signature check independent of target platform, same as
every other std-module name. What's *defined* in the object file is
target-dependent: the Linux/Windows preprocessor blocks in
`runtime/verb_time.cpp` are mutually exclusive (`#if defined(__linux__) /
#elif defined(_WIN32) / #endif`), so calling a `linux_*` function in a
program built for Windows (or vice versa) compiles fine but fails at
link time with an undefined symbol — the same "footgun accepted, v1"
tradeoff `gen_extern_call` already documents for generic `import mod`
externs, just applied to a first-party module for the first time.
Cross-compiling relies on zig's clang frontend setting the correct
predefined macros from `-target` regardless of the host OS (verified:
`c++ -target x86_64-linux-gnu -dM -E -` defines `__linux__`; `-target
x86_64-windows-gnu` defines `_WIN32` — this is a frontend-level fact,
independent of whether a full sysroot for that target is installed),
plus zig's bundled per-target libc headers/sysroot providing `<time.h>`'s
`clockid_t`/`nanosleep` for Linux and `<windows.h>` for Windows — the
same reason the project already routes all cross builds through zig
rather than the host's own `cc`/`c++`.

No `now()`/formatting/timezone/date-arithmetic functions in v1 — Verb has
no struct/record type to return a broken-down calendar time into, and
formatting is naturally deferred until strings gain more manipulation
functions.

## Plumbing changes (mirroring `std io`/`std map`)

- `src/parser.rs`: `import_stmt` allow-list gains `"time"` (currently
  `if name != "io" && name != "map"` → also `time`; error message lists
  all three).
- `src/codegen.rs`: `TIME_FUNCS` table + `time_func_arity`, extend
  `gen_call`'s std-module dispatch to also check `self.std_imports` for
  `"time"`.
- `src/main.rs`: parallel `wants_time`/`compile_time_obj`/`TIME_CPP`
  alongside the existing `wants_std_io`/`wants_map` triples in
  `build_aot_host`/`build_aot_cross`, duplicated the same way those two
  already duplicate each other rather than introducing a generic "std
  modules" abstraction — matches existing style.
- `runtime/verb_time.cpp` (new) — no `runtime/verb.h` changes needed (no
  new tag, no new constructor).
- No Windows cross-compile restriction needed (unlike `std io`'s POSIX
  socket dependency) — `<chrono>`/`<thread>` are portable; time programs
  can cross-compile to every v1 target, matching `std map`.
- `formatter.rs` needs no change (import statements are formatted
  token-generically already).

## Testing

Mirror the `std_map_*`/`std_io_*` fixtures/tests in `tests/e2e.rs` and
`tests/fixtures/`:

- `runtime/verb_time.cpp` compiles standalone (like
  `verb_map_cpp_compiles_standalone`).
- `verb run` rejects a program with `import std time;` with an error
  mentioning `std time`.
- A `tests/fixtures/std_time_basic.verb` fixture exercising the five
  portable functions (`monotonic_ms`/`sleep_ms`/`now_ms`/`clock_ms`/
  `difftime_ms`), built and run via `verb build`, output diffed against
  `tests/fixtures/std_time_basic.expected`. Unlike `std_map_basic`, raw
  timestamps aren't deterministic, so the fixture prints boolean facts
  derived from them instead (elapsed monotonic time after a
  `sleep_ms(20)` is at least 20ms via both `sub` and `difftime_ms`;
  `now_ms()`/`clock_ms()` are non-negative and non-decreasing across two
  calls) rather than diffing exact values.
- `tests/fixtures/std_time_linux.verb`/`std_time_windows.verb` exercise
  the platform-specific functions, each cross-built (via zig,
  `linux-x86_64`/`windows-x86_64`) and checked for link success + a
  non-empty binary — not executed, same as every other foreign-target
  cross-build test, since the point is confirming the right preprocessor
  branch actually compiles and links for that target, not running it.
- Parser tests: `import std time;` parses; `import std vector;` still
  rejects with `io`, `map`, and `time` all listed as known modules.
- Codegen/unit-level coverage for arity mismatches on `sleep_ms`,
  mirroring the existing `IO_FUNCS`/`MAP_FUNCS` arity-check tests.
- Cross-build test covering a non-host, non-Windows target for the
  portable fixture (like `std map`, no separate Windows-rejection test
  needed) plus the two platform-specific cross-build tests above.

README gets a new section documenting `import std time` next to the
existing `import std io`/`import std map` sections, and the "known std
modules" line changes from "io, map" to "io, map, time".
