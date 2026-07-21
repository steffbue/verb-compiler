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

Three builtin functions, available whenever `import std time;` is
present, wired the same way `std io`/`std map` functions are
(`src/codegen.rs` `gen_std_io_call`/`IO_FUNCS`/`MAP_FUNCS`, generalized
to a third `TIME_FUNCS` table):

| Function | Arity | Returns |
|---|---|---|
| `now_ms()` | 0 | int: milliseconds since the Unix epoch (`std::chrono::system_clock`) — wall-clock, can jump if the system clock is adjusted |
| `monotonic_ms()` | 0 | int: milliseconds from a monotonic clock (`std::chrono::steady_clock`) — never goes backwards; only meaningful as a difference between two calls, so this is what elapsed-time measurement should use, not `now_ms()` |
| `sleep_ms(ms)` | 1 | nil: blocks the calling thread for `ms` milliseconds (`std::this_thread::sleep_for`); `ms <= 0` is a no-op rather than an error |

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
- A `tests/fixtures/std_time_basic.verb` fixture exercising
  `monotonic_ms`/`sleep_ms`/`now_ms`, built and run via `verb build`,
  output diffed against `tests/fixtures/std_time_basic.expected`. Unlike
  `std_map_basic`, raw timestamps aren't deterministic, so the fixture
  prints boolean facts derived from them instead (elapsed monotonic time
  after a `sleep_ms(20)` is at least 20ms; `now_ms()` is positive and
  non-decreasing across two calls) rather than diffing exact values.
- Parser tests: `import std time;` parses; `import std vector;` still
  rejects with `io`, `map`, and `time` all listed as known modules.
- Codegen/unit-level coverage for arity mismatches on `sleep_ms`,
  mirroring the existing `IO_FUNCS`/`MAP_FUNCS` arity-check tests.
- Cross-build test covering a non-host, non-Windows target (like `std
  map`, no separate Windows-rejection test needed).

README gets a new section documenting `import std time` next to the
existing `import std io`/`import std map` sections, and the "known std
modules" line changes from "io, map" to "io, map, time".
