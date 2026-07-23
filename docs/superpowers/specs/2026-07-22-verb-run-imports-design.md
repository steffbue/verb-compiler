# `verb run` import support — design

**Date:** 2026-07-22
**Status:** approved
**Scope:** Make `verb run` (the MCJIT path) execute programs that use `import std io`, `import std map`, and `import mod <lib>`, which it currently rejects. `verb build`/`compile` (AOT) already support all three and are unchanged.

## Problem

`verb run` JIT-compiles the program module and calls `main` in-process. The module is self-contained for import-free programs, so the JIT path works. For import-using programs it bails out early (`src/main.rs`, the `"run"` arm) with:

```
error: 'verb run' does not support imports (...); use 'verb build' instead
```

Three distinct reasons it can't run imports today:

1. **`import std io` / `import std map`** need the runtime entry points (`read_line`, `map_new`, …) available in-process. `verb_map.cpp` is compiled into the `verb` binary (via `build.rs`); `verb_std_io.cpp` is **not**.
2. **The shared value runtime.** `verb_map.cpp` and `verb_std_io.cpp` call `verb_alloc` / `verb_retain_value` / `verb_release_value`. Those C++ units are compiled into the `verb` host binary, so their calls bind — at host link time — to the host binary's copies of those symbols. Today those host copies are **`abort()` stubs** (`src/main.rs`), deliberately, because `run` "cannot use maps". The real definitions are emitted as LLVM IR *into the program module* by `src/codegen.rs`, but MCJIT module symbols are not visible to already-linked host/C++ code, so the host C++ can never reach them.
3. **`import mod <lib>`** links an external library with `-l<lib>`. Under MCJIT nothing loads that library, so its symbols are unresolved.

## Approach

Chosen: **true in-process JIT** (resolve everything inside the `verb` process). Rejected alternative: compiling to a temp executable and `exec`-ing it.

### 1. Host runtime symbols become forwarding thunks

The key move. Instead of reimplementing the value runtime in C++, make the host binary's `verb_alloc` / `verb_retain_value` / `verb_release_value` **forward to the program module's own JIT-compiled definitions**.

- `src/codegen.rs` already emits real IR bodies for all three into every module (`build_alloc_fn`, `build_retain_value_fn`, `build_release_value_fn`). Unchanged.
- At JIT init (after the execution engine is created, before `main` is called), look up their addresses with `ee.get_function_address("verb_alloc")` etc. and store each in a process-global slot.
- The host `#[no_mangle] extern "C"` functions in `src/main.rs` stop aborting; each loads its slot, transmutes to the correct `extern "C"` fn pointer, and calls through.

Consequences:
- `verb_map.cpp` retaining/releasing arbitrary nested values (arrays, maps, closures, cells) works, because the *module's* real tag-dispatching implementation runs — no C++ reimplementation, single source of truth.
- `verb_gc_live` stays consistent: every alloc/release, whether triggered from module code or from host C++, goes through the module's counter-touching implementation.
- The slots are set before `main` runs and only read during `main`; single-threaded, so an `AtomicUsize` per slot (Relaxed) is sufficient. A thunk called with an unset slot (should never happen) aborts loudly, preserving the current fail-loud contract.

The thunks affect the JIT path only. AOT-generated executables link the module's `verb_alloc` etc. directly and never reference the `verb` binary's copies, so `build`/`compile` are untouched.

### 2. Register std runtime entry points

- `build.rs`: add `runtime/verb_std_io.cpp` to the compiled runtime unit(s), alongside the existing `verb_map.cpp`, so `read_line` … `close_conn` have in-process addresses. (Note: `verb_std_io.cpp`'s POSIX socket code is what makes it host-only; this is the same code `build` already compiles.)
- `src/main.rs`, JIT init: extend the existing `register_jit_runtime_symbols` array (currently just `verb_map_destroy_contents`) to also `add_global_mapping` the entry points the module may reference:
  - map: `map_new`, `map_set`, `map_get`, `map_has`, `map_remove`, `map_len`
  - io: `read_line`, `file_read`, `file_write`, `file_append`, `tcp_connect`, `tcp_listen`, `tcp_accept`, `send_line`, `recv_line`, `close_conn`
- Each mapping stays guarded by `if let Some(f) = module.get_function(name)` (existing pattern), so only symbols the program actually references are wired, and the array is the single place to extend for future std functions.
- These host addresses require `extern "C"` declarations in `src/main.rs` for the functions being mapped.

### 3. `import mod <lib>` via dynamic loading

- Before creating/running the JIT engine, for each `import mod <lib>`:
  - Resolve a shared library file: search each `-L<dir>` (from `parsed.lib_dirs`, already parsed but currently ignored by `run`), then system defaults, for `lib<name>.dylib` (macOS) / `lib<name>.so` (Linux).
  - `inkwell::support::load_library_permanently(path)` it.
- Call `inkwell::support::load_visible_symbols()` once so MCJIT's symbol search can see process + loaded-library symbols.
- `gen_extern_call`'s no-body externs then resolve through MCJIT's dynamic symbol search. Mod-lib functions that themselves call `verb_alloc` resolve to the host thunk → module implementation.
- **v1 limitation:** only shared libraries can be `dlopen`-ed; static `.a` archives are unsupported under `run` (they remain build-only). If no shared library is found for an `import mod` name, `run` fails with a clear message naming the library and the searched dirs, and pointing at `verb build`.

### 4. Remove the rejection

Delete the `if !imports.is_empty() || !std_imports.is_empty()` bail-out in the `"run"` arm of `src/main.rs`. Replace with the registration + dyn-load wiring above.

## Components touched

| File | Change |
|------|--------|
| `build.rs` | Compile `verb_std_io.cpp` into the `verb` binary too. |
| `src/main.rs` | Thunk-forwarding `verb_alloc`/`retain`/`release`; extern decls + `add_global_mapping` for map/io entry points; mod shared-lib resolution + `load_library_permanently`/`load_visible_symbols`; remove imports rejection; use `parsed.lib_dirs` in `run`. |
| `src/codegen.rs` | None expected (module already emits the runtime it needs). |

No changes to the parser, lexer, or the value ABI.

## Error handling

- Thunk with unset slot → `abort()` with a message (unreachable-in-practice guard).
- `import mod <lib>` with no findable shared library → exit non-zero, message naming the lib + searched `-L` dirs, suggests `verb build` for static linking.
- `load_library_permanently` failure → exit non-zero with the path and OS error.
- Unknown `std` module is still rejected earlier by the parser (`io`/`map` only) — unchanged.
- `import std io` socket/file errors keep returning `nil`/error values as the C++ runtime already defines — unchanged.

## Testing

- **Unit (`src/main.rs`):** mod shared-library resolution — given a set of `-L` dirs and a name, picks the right `lib<name>.<ext>`; returns a clear error when absent. Pure path logic, no linking.
- **E2E `verb run`:**
  - `import std io`: `read_line` echo (stdin→stdout), `file_write` then `file_read` round-trip in a temp dir.
  - `import std map`: `map_set`/`map_get`/`map_has`/`map_remove`/`map_len`, including a map holding array/nested-map values to exercise the forwarding retain/release path; assert `VERB_GC_DEBUG` reports `verb_gc_live=0` at exit (proves the thunk/counter consistency).
  - `import mod <lib>`: build a tiny shared library exposing one `extern "C" VerbValue f(VerbValue)` using `runtime/verb.h`, `verb run -L<dir>` a program that calls it, assert output; assert a missing lib produces the documented error.
  - Regression: an import-free program still runs via the unchanged fast path.
- **Cross-check:** the same import-using programs produce identical output under `verb build` + exec and `verb run`.

## Out of scope (v1)

- Static `.a` mod libraries under `run`.
- Windows `verb run` with `import std io` (mirrors the existing cross-compile limitation).
- Any change to AOT `build`/`compile` behavior or the value ABI.
