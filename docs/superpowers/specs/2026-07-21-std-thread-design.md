# `import std thread`: OS threads, mutex, channel

## Context

`import std io` (`runtime/verb_std_io.cpp`) and `import std map`
(`runtime/verb_map.cpp`) are the two existing first-party `std` modules.
Both follow the same shape: a fixed name→arity table in `src/codegen.rs`
(`IO_FUNCS`/`MAP_FUNCS`), dispatched through `gen_std_io_call` — a
generic "call a known-arity C-ABI extern, VerbValue in/out" helper that
is not actually io-specific despite its name (`map`'s functions already
reuse it verbatim). `main.rs` conditionally compiles and links the
module's `.cpp` file only when the program's `std_imports` requests it.

This spec adds a third module, `import std thread;`, giving Verb
programs real OS threads, a mutex, and a blocking channel.

**Why this is more than "add another runtime file":** Verb's
refcounting GC (`docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md`)
is not thread-safe — retain/release on strings, arrays, maps, and
closures use plain (non-atomic) increment/decrement. Any heap-tagged
`VerbValue` touched by two threads concurrently is a data race. The
design below avoids this entirely rather than making the GC atomic:
**no heap-tagged value (`STRING`/`ARRAY`/`MAP`/`CLOSURE`) may cross a
thread boundary.** Only `NIL`/`BOOL`/`INT`/`FLOAT` do.

This is compatible with the language as it stands today for a second
reason: closures already cannot capture anything (`env` is always
null — see the refcounting spec's Context section). A spawned closure
cannot smuggle a captured heap value into another thread even if we
wanted it to. The only way a spawned thread touches shared state is
top-level globals (module-level LLVM globals, `self.globals`), which is
exactly what `mutex_*` exists to protect.

## Goals

- `thread_spawn`/`thread_join` — run a 0-arity closure on a new OS
  thread, wait for it to finish.
- `thread_sleep_ms` — pause the current thread.
- `mutex_new`/`mutex_lock`/`mutex_unlock` — protect shared globals.
- `channel_new`/`channel_send`/`channel_recv` — blocking
  single-producer/consumer-agnostic queue for explicit handoff,
  restricted to primitive payloads.
- No changes to the GC. No atomics added to retain/release.

## Non-goals

- Passing strings/arrays/maps/closures across threads (spawn args,
  closure return value, or channel payload). Deferred to a future spec
  that would need to make refcounting atomic or add deep-copy-on-cross.
- `thread_spawn` forwarding call arguments to the closure — v1 closures
  are 0-arity only. Initial data reaches a thread via globals (written
  before `thread_spawn`, read from the closure body) or via a channel
  created before spawning and captured the same way (global).
- Cross-compiling `import std thread` programs to Windows — same
  restriction `import std io` already has for its POSIX socket code.
  Native compilation on Windows is out of scope for this spec, same as
  today's `import std io`.
- Timeouts on `mutex_lock`/`channel_recv`, try-lock, buffered/bounded
  channels, multi-value channels, thread priorities/names/cancellation.
- Recovering from a panicking/aborting spawned closure — undefined,
  same trust level the rest of the runtime already extends to the
  program (e.g. an out-of-bounds `get()` already aborts the process).

## API surface

All functions live behind `import std thread;`, dispatched the same way
`io`/`map` are: unbound name, present in `std_imports`, looked up in a
fixed table. Prefixed (`thread_`/`mutex_`/`channel_`) rather than bare
(`spawn`/`lock`/`send`) to avoid colliding with likely user identifiers
— following `map_`'s precedent over `io`'s unprefixed names, since `io`
picked already-distinctive names (`file_read`, `tcp_connect`) while
`thread`'s natural verbs (`lock`, `send`, `join`) are generic.

| Function | Arity | Returns | Notes |
|---|---|---|---|
| `thread_spawn(closure)` | 1 | thread handle | `closure` must be a 0-arity closure (checked via existing `verb_check_call`, same arity-mismatch abort behavior as any other bad call). Return value of the closure is discarded. |
| `thread_join(handle)` | 1 | `nil` | Blocks until the thread finishes. Frees the handle. Joining an already-joined or bogus handle is undefined (same trust level as `close_conn` on a bad fd). |
| `thread_sleep_ms(ms)` | 1 | `nil` | `ms` truncated to `int64`. |
| `mutex_new()` | 0 | mutex handle | Heap `std::mutex`. |
| `mutex_lock(h)` | 1 | `nil` | Blocks. |
| `mutex_unlock(h)` | 1 | `nil` | Unlocking a mutex not held by the caller is undefined (matches `std::mutex`). |
| `channel_new()` | 0 | channel handle | Heap unbounded queue, internal `std::mutex` + `std::condition_variable`. |
| `channel_send(h, v)` | 2 | `bool` | `false` (no-op) if `v`'s tag is `STRING`/`ARRAY`/`MAP`/`CLOSURE` — this is the runtime enforcement of "primitives only cross threads" (can't be checked at compile time; the language is dynamically typed). `true` on success. |
| `channel_recv(h)` | 1 | value | Blocks until a value is available, then pops and returns it (always `NIL`/`BOOL`/`INT`/`FLOAT` — `send` already rejected anything else). |

All handles reuse `VERB_INT`'s payload-as-pointer-cast-to-int64 pattern,
same precedent as `import std io` reusing `VERB_INT` for POSIX fds and
socket fds — no changes to `verb.h`'s tag set.

## Codegen

Every function above except `thread_spawn` needs zero new codegen: they
take/return plain `VerbValue`s, so they go through the *existing*
`gen_std_io_call` generic path exactly like `map_*` does today. Add a
`THREAD_FUNCS: &[(&str, usize)]` table (mirroring `IO_FUNCS`/`MAP_FUNCS`)
and one dispatch arm in `gen_call` gated on
`self.std_imports.iter().any(|m| m == "thread")`.

`thread_spawn` is the one bespoke case, because a closure's `VerbValue`
cannot cross the C++ boundary (verb.h's documented rule — "Tag 5
(closure) never crosses this boundary"). Its codegen:

1. `gen_expr` the closure argument → `cv`.
2. Reuse the exact `verb_check_call(cv, 0, line, col)` call `gen_call`'s
   own fallback tail already emits for ordinary closure calls, to
   arity-check and unwrap → `clos_ptr`.
3. Load `fp` (`fn_ptr` field) from `clos_ptr` via `closure_ty`'s GEP —
   same field access `gen_call` already does. `env` is always null, so
   skip loading it and pass a null pointer constant directly.
4. Call a new extern `void* thread_spawn_raw(void* fn_ptr, void* env)`
   (raw pointers, not `VerbValue` — sidesteps the closure-ABI boundary
   entirely) that spawns a `std::thread` running
   `((VerbValue(*)(void*, void*))fn_ptr)(env, /*empty argv*/ nullptr)`,
   heap-allocates a handle struct wrapping the `std::thread`, and
   returns it as an opaque pointer.
5. Wrap the returned pointer as `VerbValue{tag: VERB_INT, payload:
   (int64_t)ptr}` — this part goes through the ordinary post-call
   codegen every other extern call already uses.

`thread_join(handle)` is an ordinary `gen_std_io_call` extern taking one
`VerbValue`, unwrapping the pointer, joining the `std::thread`, and
`delete`-ing the handle struct.

## Linking (`main.rs`)

Mirror `wants_std_io`/`std_io_obj` exactly:
- `wants_std_thread = std_imports.iter().any(|m| m == "thread")`.
- New `compile_std_thread_obj` analogous to `compile_std_io_obj`,
  compiling `runtime/verb_std_thread.cpp`.
- Linker choice (`cc` vs `c++`) already switches to `c++` whenever any
  first-party module needing C++ is present — extend that condition
  with `wants_std_thread`.
- Link against pthreads: on Linux add `-pthread` (macOS's libc++
  already threads without an extra flag; `std::thread` needs pthread on
  Linux specifically). Mirror wherever `wants_std_io`'s socket linking
  already branches per-platform, if it does; otherwise add the flag
  unconditionally when `wants_std_thread` and non-Windows.
- Windows cross-compile (`target.is_windows()`) rejected with a
  `CompileError` the same way `import std io` already rejects Windows
  cross-compilation for its socket code (same message shape, naming
  `import std thread` instead).

## Parser

`src/parser.rs`'s std-module allow-list (`name != "io" && name !=
"map"`) gains `&& name != "thread"`. Mirror the existing
`parses_std_io_import`/`parses_std_map_import`-style tests for `import
std thread;`, including the "unknown module" error message still
listing all three names.

## Testing

- Parser: `import std thread;` accepted; combined with `io`/`map`
  imports; dedup on repeated `import std thread;`; unknown-module error
  message mentions `thread` alongside `io`/`map`.
- Codegen: arity-mismatch compile errors for each `THREAD_FUNCS` entry
  (mirroring `std_io_arity_mismatch_is_a_compile_error`); `thread_spawn`
  rejects a non-0-arity closure via the existing arity-abort path (no
  new error message needed — same runtime abort every other bad call
  already produces); std-thread names ignored without `import std
  thread` (mirroring `std_io_name_ignored_without_import_std_io`).
- Runtime (`tests/`, end-to-end compiled-and-run, mirroring the
  existing std-io fixture tests): spawn+join round trip (global counter
  incremented by the spawned thread, observed after join); several
  threads incrementing a shared global under `mutex_lock`/
  `mutex_unlock` reach the exact expected total (proves mutual
  exclusion, not just "doesn't crash"); channel send/recv handoff
  between main and a spawned thread; `channel_send` of a string/array
  returns `false` and does not deadlock the receiver.
- GC: new leak-check fixture for `import std thread`, isolated from
  other temp-file-using fixtures the same way the recent std-io
  leak-check isolation commit already established — thread/mutex/
  channel handles are `VERB_INT`-tagged (not refcounted) so there is
  nothing to leak-check on the Verb-value side, but confirm the fixture
  process itself doesn't leave dangling C++ heap allocations (handle
  structs) after every `thread_join`/no unmatched `mutex_new`/
  `channel_new`.

## Error handling summary

Consistent with the rest of the runtime's existing trust model (fd
reuse, `close_conn` on a bad fd, `get()` aborting on out-of-bounds): the
only *checked* failure this module introduces is `channel_send` on a
non-primitive value (returns `false`, program stays running). Bad
handles, double-join, unlock-without-lock are undefined behavior at the
C++ level, matching how POSIX fd misuse is already handled (or rather,
not defended against) in `verb_std_io.cpp`.
