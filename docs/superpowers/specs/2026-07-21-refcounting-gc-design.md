# Reference-counting garbage collector

## Context

Verb compiles to native LLVM IR (via inkwell); there is no bytecode VM and
no stack maps, so a precise or conservative tracing collector would need
either LLVM `gc.statepoint` intrinsics (not exposed by inkwell) or scanning
the native stack (fragile against optimized code, hard to verify). The
compiler fully controls every codegen site where a value is copied, so
reference counting can be inserted precisely at compile time instead,
with no runtime stack scanning at all.

Today, three kinds of values are heap-allocated via `malloc` and never
freed: string buffers (from `join`/concat), closure structs (24 bytes:
fn ptr, arity, always-null env), and "cells" (16-byte boxes that hold
every local variable's/parameter's `VerbValue`). Everything else
(nil/bool/int/float) is an unboxed tagged value with no heap identity.

Closures currently never capture anything — `make_closure` always stores
a null env pointer, and no function body ever reads its env parameter
(`get_nth_param(0)`). Cells never contain other cells. **This means no
reference cycle is currently constructible in Verb** — plain refcounting
is exact, not just a heuristic, and needs no cycle collector. If a future
feature adds variable capture (closures holding cells) or compound types
that can reference themselves, this invariant must be re-examined.

Arrays/maps are not implemented in Verb yet (open question in
`compiler_design_plan.md`) and are **out of scope** for this task. The
runtime dispatch is designed so adding a new heap-tagged type later is a
small addition (one more tag case in `verb_retain_value`/
`verb_release_value`, one more alloc site), not a redesign.

## Goals

- Implicit memory management: Verb programs never call free/retain/release
  themselves. Only heap-identity types (currently string, closure) do any
  refcount work; nil/bool/int/float are always no-ops.
- No leaks for any program expressible in current Verb (loops, recursion,
  string concatenation, closures used as first-class values).
- No new syntax, no observable semantic change — this is purely a memory
  behavior fix.

## Non-goals

- Cycle collection (not reachable given the invariant above).
- Arrays/maps or any new compound type.
- Concurrent/parallel collection (Verb has no concurrency).
- Compacting/moving GC.

## Design

### Heap object header

Every heap block gets an 8-byte `i64` refcount header immediately before
the pointer that Verb/`VerbValue` already carries:

```
[i64 refcount][payload bytes...]     <- existing pointer points here
```

This applies uniformly to all three kinds:

- **string buffer** — payload = NUL-terminated bytes, no nested refs to release.
- **closure struct** — payload = `{ fn_ptr, i64 arity, env_ptr }`, env is
  always null so releasing a closure never cascades.
- **cell** — payload = one `VerbValue` (`{i8 tag, i64 payload}`). On
  release, cascade: release the `VerbValue` stored inside, then free the
  cell block.

**String literals** (currently `build_global_string_ptr`, static rodata)
get the same header shape baked into the LLVM global: an `i64` sentinel
(e.g. `INT64_MIN`) immediately before the byte data, with the existing
string pointer left pointing at the data (not the sentinel). `verb_retain_value`/
`verb_release_value` always read the header at `payload - 8` for any
string, static or heap — the runtime checks for the sentinel and no-ops,
so codegen never needs to know statically whether a given string pointer
is static or heap-owned.

### Runtime API

New C functions declared in the module alongside today's `malloc`/`strlen`
declarations, and defined once in a small generated "runtime" block
(same pattern as `build_concat_fn` etc.) or in `runtime/verb.h`/a new
`runtime/verb_gc.c` linked into every build:

```c
void* verb_alloc(int64_t n);           // malloc(n+8); header=1; returns payload ptr
void  verb_retain_value(VerbValue v);  // tag==STR||CLOSURE: ++header; else no-op. STR: no-op if header==STATIC_SENTINEL
void  verb_release_value(VerbValue v); // tag==STR||CLOSURE: --header; ==0 -> free(header ptr). STR: skip entirely if STATIC_SENTINEL
void  verb_retain_cell(void* cell);    // ++header at cell-8
void  verb_release_cell(void* cell);   // --header; ==0 -> verb_release_value(*cell) then free(header ptr)
```

`verb_alloc` replaces raw `malloc` at the three existing allocation
sites (`malloc_bytes` for cells/closures, and the concat buffer in
`build_concat_fn`) — it must reserve and initialize the 8-byte header.
`malloc_bytes` becomes a thin wrapper over `verb_alloc`.

Codegen never branches on tag in Rust: it always emits an unconditional
call to `verb_retain_value`/`verb_release_value`/`verb_retain_cell`/
`verb_release_cell`. The tag switch (and the static-sentinel check) lives
once, in the small C runtime, not scattered across LLVM IR generation.

### Codegen insertion rule

One convention, applied uniformly: **every `gen_expr` result is an owned
temporary that whoever consumes it must dispose of** — either transfer it
(store into a freshly-created cell: no extra op, ownership just moves) or
release it (`verb_release_value`, once its use ends without being stored).

Concretely:

- `Expr::Ident` (variable read): call `verb_retain_value` on the loaded
  value before returning it — the cell keeps its own reference; this
  load produces an independent new one.
- Fresh allocations (`verb_concat` result, `make_closure`): born at
  refcount 1; that 1 is the temporary's ownership already, no extra retain.
- `Assign`/`Declare`/parameter binding (`cell = verb_alloc(16); store v`):
  the incoming temporary's ownership transfers straight into the new
  cell — no retain, no release.
- `Reassign`: load the old value from the cell, `verb_release_value` it,
  **then** store the new (already-owned) value — no retain needed on the new value.
- Statement-level/operand discard — `ExprStmt` result, condition value
  after `verb_truthy`, binary-op operands after `verb_add`/`verb_concat`/etc.
  read them: `verb_release_value` once the temporary's use is done.
- Scope pop (`Block`/`If`/`While` bodies, normal fn-body end): for every
  cell in the popped scope, `verb_release_cell`.
- **Early `return`**: before emitting `build_return`, walk every scope
  currently open in `self.scopes` (already isolated per-function via the
  existing `saved_scopes` swap in `Stmt::Fn`) and `verb_release_cell`
  every cell in every nested open block, in reverse (innermost first) —
  early return otherwise bypasses the normal scope-pop cleanup that would
  only run on the fall-through path. This is the one place needing new
  bookkeeping rather than a one-line change at an existing site — factor
  it into a `release_all_open_scopes(&self)` helper called both before
  `Stmt::Return`'s `build_return` and before the implicit end-of-body
  return already generated in `Stmt::Fn`.
- Runtime panics (`abort_at`, bad arity/type errors): no cleanup needed —
  the process exits immediately and the OS reclaims all memory.

### `extern`/`std io` contract change

Any C++ code that hands a heap string to Verb (`runtime/verb_std_io.cpp`'s
`file_read` etc., and user `import mod` externs per
`docs/superpowers/specs/2026-07-20-cpp-import-design.md`) must allocate
that string through a new helper, `verb_alloc_string(const char*)` (wraps
`verb_alloc` + copies bytes), instead of raw `malloc`/`strdup`. A string
pointer without a valid header at `payload - 8` will corrupt memory the
first time `verb_retain_value`/`verb_release_value` touches it. This
needs a small update to `runtime/verb.h` (add `verb_alloc_string`) and
`runtime/verb_std_io.cpp` (use it instead of `strdup`/raw `malloc`), plus
a note in the import docs about the new contract for extern authors.

### Testing

- `valgrind` (Linux CI) / `leaks` (macOS) run over existing example
  `.verb` programs (loops, recursion, string concat, closures-as-values,
  `import std io` file read) — expect zero leaked blocks after each run.
- Add one stress example: a tight loop that builds and discards strings
  each iteration, to catch any missed release site (a single miss shows
  up as unbounded RSS growth, not just a static leak count).
- Existing `tests/` suite must pass unchanged — this is a memory-behavior
  fix, not an observable semantic change.
