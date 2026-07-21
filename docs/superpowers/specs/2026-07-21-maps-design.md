# Maps (`import std map`) — design

## Context

README's "Known v1 limitations" lists "No arrays/maps". This adds a hash-map
(dictionary) type, following the same opt-in pattern already established by
`import std io` (see `docs/superpowers/specs/2026-07-20-std-io-import-design.md`):
a new `runtime/verb_map.cpp`, gated behind `import std map;`, usable only
with `verb build`/`compile` (not `verb run`/JIT) — for the same reason `std
io` is build-only: the C++ object is only compiled and linked for AOT
builds.

Arrays/lists remain out of scope; this spec covers maps only.

## Value representation

- New tag `VERB_MAP = 6` in `runtime/verb.h` / `TAG_MAP` in `src/value.rs`.
  Tag 5 (closure) already documents "never crosses the C boundary"; maps
  don't have that restriction because the C++ side treats a `VerbValue`
  payload opaquely when storing it (no need to interpret a closure to copy
  its bytes) — so a closure *can* be stored as a map value, just not
  meaningfully used as a map key (see below).
- Payload = pointer to a heap-allocated `std::unordered_map`-backed struct,
  `new`'d and (consistent with the rest of v1) never freed — no GC, no
  `map_free`.
- `verb_type_name` gets a `"map"` case; `verb_print` prints `<map>` (no
  entry dump, matching `<fn>` for closures); `verb_eq` needs no change —
  its existing default (same-tag payload/pointer equality) already gives
  maps correct reference equality.

## Key semantics

Any value can be a map **value**. Map **keys** are restricted to nil, bool,
int, float, string — closures and nested maps are rejected as keys (no
sensible equality/hash). Numeric keys follow the same cross-tag equality
`verb_eq` already uses elsewhere in the language: int `1` and float `1.0`
hash and compare equal as keys, so `map_set(m, 1, "x")` and
`map_get(m, 1.0)` agree. String keys compare by content (`strcmp`), not
pointer identity.

## API surface

Six builtin functions, available whenever `import std map;` is present,
wired the same way `std io` functions are (`src/codegen.rs`
`gen_std_io_call`/`IO_FUNCS`, generalized to a second `MAP_FUNCS` table —
`gen_std_io_call` is already module-agnostic, just declares an extern of
known arity and calls it):

| Function | Arity | Returns |
|---|---|---|
| `map_new()` | 0 | new empty map |
| `map_set(m, k, v)` | 3 | `m` (chainable); `nil` if `m` isn't a map or `k` isn't a valid key type |
| `map_get(m, k)` | 2 | value at `k`, or `nil` if absent/invalid |
| `map_has(m, k)` | 2 | `true`/`false`; `false` if `m`/`k` invalid |
| `map_remove(m, k)` | 2 | `true` if a key was removed, else `false` |
| `map_len(m)` | 1 | int entry count; `0` if `m` isn't a map |

No iteration/keys/values function in v1 — the language has no arrays to
return a key list into, so that's naturally deferred along with
arrays/lists.

Failure mode follows the `std io` convention already documented in the
README ("every function returns nil on failure") rather than aborting the
program — invalid usage (wrong-typed `m`, unsupported key type) degrades to
a nil/false/0 result instead of a runtime panic.

## Plumbing changes (mirroring `std io` exactly)

- `src/parser.rs`: `import_stmt` allow-list gains `"map"` (currently
  `if name != "io"` → also `map`; error message lists both).
- `src/codegen.rs`: `MAP_FUNCS` table + `map_func_arity`, extend `gen_call`'s
  std-module dispatch to also check `self.std_imports` for `"map"`.
- `src/main.rs`: parallel `wants_map`/`compile_map_obj`/`MAP_CPP` alongside
  the existing `wants_std_io`/`compile_std_io_obj`/`STD_IO_CPP`, duplicated
  the same way `build_aot_host`/`build_aot_cross` already duplicate the io
  logic rather than introducing a generic "std modules" abstraction —
  matches existing style, only two modules exist.
- `runtime/verb_map.cpp` + update to `runtime/verb.h` (new tag + `verb_map`/
  `verb_as_map` constructors, mirroring `verb_string`/`verb_as_string`).
- No windows-cross-compile restriction needed (unlike `std io`'s POSIX
  socket dependency) — `std::unordered_map` is portable; map programs can
  cross-compile to Windows targets.
- `formatter.rs` needs no change (import statements are formatted
  token-generically already).

## Testing

Mirror the `std_io_*` e2e fixtures/tests in `tests/e2e.rs` and
`tests/fixtures/`:

- `runtime/verb_map.cpp` compiles standalone (like
  `verb_std_io_cpp_compiles_standalone`).
- `verb run` rejects a program with `import std map;` with an error
  mentioning `std map`.
- A `tests/fixtures/std_map_basic.verb` fixture exercising
  `map_new`/`map_set`/`map_get`/`map_has`/`map_remove`/`map_len`, including
  an int/float key-equality case and a missing-key case, built and run via
  `verb build`, output diffed against `tests/fixtures/std_map_basic.expected`.
- Parser tests: `import std map;` parses; `import std vector;` still
  rejects with both `io` and `map` listed as known modules.
- Codegen/unit-level coverage for arity mismatches on `map_set`/`map_get`/etc,
  mirroring the existing `IO_FUNCS` arity-check tests.

README gets a new section documenting `import std map` next to the existing
`import std io` section, and the "Known v1 limitations" line changes from
"No arrays/maps" to "No arrays" (maps ship, arrays remain deferred).
