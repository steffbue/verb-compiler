# Reference-counting GC v2: strings, closures, arrays, maps

## Context

`docs/superpowers/specs/2026-07-21-refcounting-gc-design.md` designed and
implemented a refcounting GC for strings and closures (PR #11). That
branch was cut before arrays (`list`, `TAG_ARRAY`), `import std map`
(`TAG_MAP`), closure scoping (`globals`), and a C++ export macro all
landed on `main` via separately-merged PRs. PR #11 is closed unmerged —
`main` diverged too far for a clean rebase, and neither branch has what
the other built. This spec re-applies the original design (unchanged)
against current `main`, and extends it to cover arrays, maps, and the
new global-binding mechanism.

Today, on `main`, four kinds of values are heap-allocated and never
freed:

- **string buffers** (`join`/concat results) — plain bytes, no nested refs.
- **closure structs** (`{fn_ptr, i64 arity, env_ptr}`, 24 bytes) — `env`
  is always null (`make_closure`, unchanged since the original spec);
  nested `make` never sees an enclosing function's scope at all, only
  its own params/locals and top-level globals (`src/codegen.rs`'s
  `lookup`/`globals` comments). Closures still cannot capture anything,
  so they still never cascade.
- **cells** (16-byte boxes, one `VerbValue` each, for locals/params).
- **array headers** (`{i64 len, i64 cap, ptr elems}`, 24 bytes) plus a
  separately-`malloc`'d `elems` buffer holding `len` (really `cap`)
  `VerbValue`s. Elements are stored by reference: `push`/`get`/`set`
  mutate the same underlying array a variable points at, so two
  variables can alias one array, and an array can hold another array or
  a closure.
- **map objects** (`TAG_MAP`, payload = pointer to a
  `runtime/verb_map.cpp` `VerbMapImpl`, a real
  `std::unordered_map<VerbValue, VerbValue, KeyHash, KeyEq>`) — bare
  `new`, never `delete`d. Map functions (`map_new`/`map_get`/`map_set`/
  `map_has`/`map_remove`/`map_len`) are wired through the same
  `gen_std_io_call`-style extern-call path used by `import std io`
  (`MAP_FUNCS`, arity-checked like `IO_FUNCS`).

**Globals are new plumbing, not a new heap kind.** Top-level bindings
(`Stmt::Assign`/`Stmt::Declare` outside any function/block) now live in
module-level LLVM globals (`self.globals: HashMap<String,
PointerValue>`), not malloc'd cells — permanent storage slots, created
lazily via `global_slot`, read through the same `lookup` used for
regular cells. `bind()` picks a cell or a global slot depending on
whether `self.scopes` is empty.

**Cycles are now really possible.** Arrays are mutable, reference-stored
containers with no self-reference check anywhere in `push`/`set` —
`assign a list 1, 2; push(a, a);` is valid, unrejected Verb today, and
makes `a`'s own `elems` buffer point back to `a`'s own header. The same
is possible through two arrays referencing each other, or a map holding
a value that (transitively) holds the map. **Plain refcounting cannot
reclaim these — this is a known, explicit limitation of this
sub-project**, carried forward to a separate, later sub-project (a
backup cycle collector). This spec's job is only to make refcounting
correct and complete for the acyclic case, and to prove (via a test)
that the cyclic case fails safely — a confined leak, not corruption or
a crash.

## Goals

- Implicit memory management, extended to cover strings, closures,
  arrays, maps, and globals: no Verb program calls free/retain/release
  itself. Only heap-identity tags (STR, CLOSURE, ARRAY, MAP) do any
  refcount work; nil/bool/int do not.
- No leaks for any *acyclic* program expressible in current Verb —
  including arrays, maps, nested arrays, arrays of closures, map entries
  holding heap values, and repeated top-level reassignment.
- Fix the pre-existing, adjacent leak in `push`'s grow path (old `elems`
  buffer never freed on reallocation) as part of this work, since GC
  already touches that exact code path.
- No new syntax, no observable semantic change to any existing program's
  output — purely a memory-behavior fix.

## Non-goals

- Cycle collection — deferred to a separate sub-project. This spec
  documents the limitation and tests that it fails safely, but does not
  solve it.
- Any new language feature (no new array/map operations, no capture).
- Concurrent/parallel collection.
- Compacting/moving GC.

## Design

### Heap object header (unchanged from v1)

Every heap block gets an 8-byte `i64` refcount header immediately before
the pointer Verb already carries: `[i64 refcount][payload...]`. String
literals get the same shape with a sentinel (`i64::MIN`) instead of a
live count, baked into the LLVM global at `payload - 8`, so
`verb_retain_value`/`verb_release_value` can always read a header there
regardless of whether a given string is static or heap-owned.

### Runtime API — extended tag dispatch

```c
void* verb_alloc(int64_t n);           // malloc(n+8); header=1; returns payload ptr
void  verb_retain_value(VerbValue v);  // STR/CLOSURE/ARRAY/MAP: ++header (STR: no-op if sentinel)
void  verb_release_value(VerbValue v); // STR/CLOSURE/ARRAY/MAP: --header; ==0 -> cascade + free
void  verb_retain_cell(void* cell);
void  verb_release_cell(void* cell);   // --header; ==0 -> verb_release_value(*cell) then free
```

`verb_alloc` replaces raw `malloc` at every existing allocation site:
`malloc_bytes` (cells, closures, array headers), `malloc_bytes_dyn`
(array `elems` buffers, sized at runtime), the concat buffer, and (new)
`map_new`'s `VerbMapImpl` allocation.

Cascade behavior at refcount zero, added to `verb_release_value`:

- **STR**: unchanged — just `free`, no cascade.
- **CLOSURE**: unchanged — `env` is always null, just `free`.
- **ARRAY**: release every element `0..len` (cascading into any
  heap-owned element — a string, closure, nested array, or map), then
  `free(elems)`, then `free(header)`.
- **MAP**: call a new `extern "C" void verb_map_destroy_contents(void*
  payload)` in `runtime/verb_map.cpp`. It iterates every key/value pair,
  calling back into the LLVM-defined `verb_release_value` for each
  (cascading the same way as arrays), then explicitly runs
  `impl->~VerbMapImpl()` (placement-new requires an explicit destructor
  call, `delete` would double-free). The header's `free()` still happens
  in the one generic LLVM-side path afterward — every heap kind frees
  its header in exactly one place.

As before, codegen never branches on tag in Rust — it always emits an
unconditional call to the four runtime functions; the tag switch and
sentinel/cascade logic live once, in the runtime.

### Codegen insertion rule (unchanged convention, new sites)

The same rule as v1: every `gen_expr` result is an owned temporary,
transferred (stored, no extra op) or released (discarded). Everywhere
v1 already wired this in (`Expr::Ident`, `Reassign`, discard sites,
scope-pop, early-return-unwind, the extern/std-io argument-release
convention) is re-applied unchanged against `main`'s current line
numbers. New sites, specific to this sub-project:

- **`build_array_get_fn`**: retain the returned element (mirrors
  `Expr::Ident` — the array's own slot keeps its reference; `get` hands
  back an independent copy).
- **`build_array_set_fn`**: release the old value at that slot before
  overwriting (mirrors `Reassign`).
- **`build_array_push_fn`**: the pushed value transfers in, no extra op
  (same as any argument-to-param-cell transfer). On the grow path, after
  copying `elems` into the new, larger buffer, `free()` the *old*
  `elems` buffer — a plain `free`, not a refcounted release, since
  `elems` is never independently aliased outside its owning header.
  This closes the pre-existing leak.
- **`build_array_pop_fn`**: the returned value transfers out, no extra
  op (unchanged from today's behavior — the slot isn't cleared, so no
  release is needed there either; this matches how `pop` already reads
  today).
- **Map function calls** (`gen_std_io_call`'s `MAP_FUNCS` path):
  arguments already get released after the call via the existing
  extern-call convention (arrays and maps both route through this same
  function) — no new code needed there. `map_get`'s return value needs
  an explicit retain in the runtime dispatch, mirroring array `get` —
  the map's internal entry keeps its own reference; the caller gets an
  independent copy.
- **`bind()`'s global-slot branch**: before storing the new value, load
  the slot's current value and release it — unconditionally, every
  time, including the first bind (a freshly-created global slot is
  zero-initialized to `{tag: NIL, payload: 0}`, and releasing a NIL
  value is always a no-op, so no special-casing is needed for "first
  bind vs. rebind"). This is the same leak class as the cell
  re-declaration bug fixed in v1, applied to the global-slot case.
- **Program exit** (`compile_program`'s implicit top-level return): after
  releasing all open (top-level) scopes as in v1, also release every
  global's current value — iterate `self.globals.values()`, load, and
  release each. Globals are a separate `HashMap`, not part of
  `self.scopes`, so v1's scope-release logic never reaches them; without
  this, any program with top-level state would never report
  `verb_gc_live=0`.

### `extern`/`std io`/`std map` contract (unchanged from v1, reconfirmed)

Any C++ code handing a heap value back to Verb must allocate through
`verb_alloc`, not raw `malloc`/`new`/`strdup` — reconfirmed for
`runtime/verb_std_io.cpp` (as in v1) and newly applied to
`runtime/verb_map.cpp`'s `map_new`.

### The cycle limitation, made concrete

No detection, no rejection, no runtime check for self-reference is added
anywhere (`push`, `set`, map insertion all stay exactly as fast and
simple as today). A program that constructs a cycle (`push(a, a)`, or
two arrays/maps referencing each other) will leak that structure for the
lifetime of the process — refcounts on the participants never reach
zero. This is validated, not hidden: a fixture builds a self-referential
array, and the test asserts the leak is *bounded* (a small, fixed
non-zero `verb_gc_live`, matching exactly the cyclic structure's own
block count) rather than growing unbounded or crashing — proving the
failure mode is "leaks precisely what a refcounter can't reach," not
memory corruption.

### Testing

Same `VERB_GC_DEBUG`/`verb_gc_live` oracle as v1, extended with fixtures
for: nested arrays (`arrays_of_arrays`-style), arrays of closures, a
push-driven regrowth loop (proving the grow-path fix), a map holding
heap-valued entries, repeated top-level (global) reassignment, and the
confined-cycle-leak proof described above. Every existing fixture
(`arrays_*`, `std_map_*`, `closures.verb`, etc.) must keep passing with
identical output — this is a memory-behavior change only.
