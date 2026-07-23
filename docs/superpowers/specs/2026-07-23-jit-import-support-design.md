# JIT Import Support (FFI-V2-01) — Design Spec

Date: 2026-07-23
Status: approved
Requirement: FFI-V2-01 (JIT-mode `verb run` support for imports)

## Purpose

Make `verb run` (the LLVM MCJIT path) execute programs that use imports,
with the same semantics as `verb build` (AOT). Today the JIT hard-rejects
any program with imports (`src/main.rs:220-228`); the C++ import feature
(see `2026-07-20-cpp-import-design.md`) explicitly deferred JIT support to
v2. This spec closes that gap for all three import kinds:

- `import mod <lib>` — user-provided C++ FFI libraries (`-l<name>` / `-L<dir>`).
- `import std io` — bundled `runtime/verb_std_io.cpp` (file/socket helpers).
- `import std map` — bundled `runtime/verb_map.cpp` (map API + refcounting hooks).

No language-surface change. No AOT change. No codegen change.

## Background: why imports don't work under JIT today

`verb run` builds one LLVM module and runs it with MCJIT (via `inkwell`,
`create_jit_execution_engine`). Two facts collide:

1. **The runtime helpers `verb_alloc`, `verb_retain_value`,
   `verb_release_value` are emitted as in-module LLVM IR by codegen**
   (`src/codegen.rs`: `verb_alloc` at ~239, `verb_retain_value` at ~1159,
   `verb_release_value` at ~1225). Under MCJIT they live inside the JIT's
   own memory and are **not** in the process's dynamic symbol table.

2. **The C++ runtime units and user `import mod` libraries call *back* into
   those helpers.** `runtime/verb_std_io.cpp` calls `verb_alloc`;
   `runtime/verb_map.cpp` calls `verb_alloc`/`verb_retain_value`/
   `verb_release_value`; a user mod lib calls `verb_alloc` whenever it
   builds a heap value (e.g. a string).

In AOT these callbacks resolve through the normal linker (the emitted
helpers land in the object file and are linked). Under JIT they are
trapped inside MCJIT, unreachable by any C++ code. So enabling imports in
JIT is fundamentally a **two-direction symbol-resolution problem**, not
merely "dlopen the library."

Additionally, `verb_std_io.cpp` is **not** compiled into the `verb` binary
today (only `verb_map.cpp` is, via `build.rs`), so its symbols
(`read_line`, `file_read`, `tcp_*`, …) have no home in-process for MCJIT to
resolve against. And `main.rs:45-58` deliberately provides *aborting* host
stubs for `verb_alloc`/`retain`/`release` — correct only under the current
invariant that imports/maps can never exist under `run`.

## Approach: trampoline forwarders + explicit registration + dlopen

Chosen over (B) moving the refcount runtime into C++ and (C) a duplicated
ABI-compatible runtime. Rationale: A leaves the GC core and the entire AOT
path untouched (zero regression surface for existing programs), keeps a
single source of truth for the helpers (the codegen-emitted module), and
confines the change to `src/main.rs` + `build.rs` + one build-config flag.

### Direction 1 — module → runtime/library code

The module *declares* external symbols (`read_line`, `map_set`, `c_sqrt`,
…) and must find their code.

**std io / std map (compiled into the `verb` binary):**

- `build.rs` compiles `runtime/verb_std_io.cpp` into the binary in addition
  to `runtime/verb_map.cpp` (same `cc::Build`, same `-std=c++17`, same
  include dir). Its symbols now exist in-process.
- Generalize `register_jit_runtime_symbols` from its current single entry
  to cover the full first-party set:
  - io (10): `read_line`, `file_read`, `file_write`, `file_append`,
    `tcp_connect`, `tcp_listen`, `tcp_accept`, `send_line`, `recv_line`,
    `close_conn`.
  - map (6): `map_new`, `map_set`, `map_get`, `map_has`, `map_remove`,
    `map_len`.
  - plus `verb_map_destroy_contents` (already registered).
- Each is declared in a Rust `extern "C"` block and its address taken
  **directly** (`fn_name as *const () as usize`), exactly as
  `verb_map_destroy_contents` is today — no `dlsym`, no dynamic-export
  dependency for in-binary symbols. Register only symbols the module
  actually declares: guard with `module.get_function(name).is_some()`, so a
  program that imports `io` but not `map` doesn't force map registration
  (and vice versa). Symbols the program never references are simply absent
  from the module and skipped.

The canonical io symbol list lives at `src/codegen.rs` `IO_FUNCS`; the map
API list is the `extern "C"` functions in `runtime/verb_map.cpp`. The
registration table must stay in sync with those; a divergence surfaces as
an MCJIT "undefined symbol" at run time for the missing name.

**import mod libraries (external `.dylib` / `.so`):**

- Add a run-time loader driven by `lib_dirs` (already parsed globally from
  `-L<dir>` at `main.rs:133`) and `imports`.
- For each imported library name `N`: search each `-L` directory for
  `libN.dylib` (macOS) / `libN.so` (Linux); `dlopen` the first match with
  `RTLD_NOW | RTLD_GLOBAL`. `RTLD_GLOBAL` so the lib's symbols are visible
  for resolution; `RTLD_NOW` to surface unresolved symbols eagerly. If no
  match is found in any `-L` dir, fall back to a bare `libN.<ext>` name so
  the dynamic loader's default search path applies; if that also fails,
  emit a clear error naming the library and the searched directories.
- For each module declaration still unresolved after the in-binary
  registration (i.e. the mod externs), `dlsym` it from the opened handles
  and `add_global_mapping`. Leak the handles (kept open for the process
  lifetime; the process exits after `main`).

### Direction 2 — runtime/library code → verb helpers

`verb_std_io.cpp`, `verb_map.cpp`, and mod libs call back into
`verb_alloc`/`verb_retain_value`/`verb_release_value`, which live as IR
inside MCJIT.

- Replace the aborting stubs at `main.rs:45-58` with **forwarders**: real,
  exported `verb_alloc`/`verb_retain_value`/`verb_release_value` functions
  in the `verb` binary, each calling through a process-global function
  pointer (e.g. an `AtomicPtr`/`static mut` set once at startup). Before the
  pointer is set, calling a forwarder is a programming error and aborts with
  a clear message (unreachable in normal flow — pointers are set before any
  Verb/C++ code runs).
- After `create_jit_execution_engine`, set the pointers from
  `ee.get_function_address("verb_alloc")` / `"verb_retain_value"` /
  `"verb_release_value"` (the module's emitted helpers).
- **Ordering (critical):** perform *all* `add_global_mapping` calls first,
  then resolve the forwarder addresses (this forces the module to JIT), then
  `get_function("main")` and call. All mappings must precede any symbol
  lookup/finalization.
- **Dynamic export:** the forwarder symbols must be in the binary's dynamic
  symbol table so a `dlopen`ed mod lib resolves its `verb_alloc` callback
  against the binary. Add the linker flag via cargo build config:
  `-rdynamic` on Linux, `-Wl,-export_dynamic` on macOS (e.g. through
  `.cargo/config.toml` `rustflags`, or `build.rs`
  `cargo:rustc-link-arg`). In-binary std io/map units do **not** need this —
  they call the forwarder symbol resolved at binary-link time.

### Removing the rejection

Delete the blanket rejection at `main.rs:220-228`. Replace with:

- On POSIX hosts: proceed through the registration + loader path above.
- On Windows hosts: keep a clear rejection for `verb run` with imports
  ("`verb run` imports are not supported on Windows; use `verb build`"),
  consistent with the documented Windows `std io` AOT limitation. (This
  spec's mechanism is POSIX `dlopen`-based; a Windows `LoadLibrary`/
  `GetProcAddress` port is out of scope.)

## CLI

No new flags. `-L<dir>` is already parsed globally into `lib_dirs`
(`main.rs:133`) and thus already accepted on `verb run`; it was simply
unused there. The stored form is the raw `-L/path` string — the loader
strips the `-L` prefix to get the directory.

## Files touched

- `build.rs` — add `runtime/verb_std_io.cpp` to the `cc::Build` file list;
  add its `rerun-if-changed`.
- `src/main.rs` —
  - Replace aborting stubs (45-58) with forwarders through global pointers.
  - Generalize `register_jit_runtime_symbols` (64-75) to the io+map+destroy
    set, guarded by `module.get_function(name).is_some()`.
  - Add the `dlopen`/`dlsym` mod-library loader.
  - Set forwarder pointers from `ee.get_function_address(...)` after engine
    creation, before `main.call()`.
  - Remove rejection (220-228); add Windows-host guard.
- Build config — dynamic-export linker flag (`.cargo/config.toml` or
  `build.rs` `rustc-link-arg`); add `libc` dependency for
  `dlopen`/`dlsym`/`RTLD_*` (or `libloading` with `RTLD_GLOBAL`).
- `src/codegen.rs` — **no change** (externs already emitted as plain
  external declarations; the io/map arity tables are the source of the
  symbol names).

## Testing

- **e2e (primary):** extend the golden/e2e harness (currently JIT-runs only
  import-free programs) to also `verb run` the import fixtures:
  - `import mod mathlib` under `run` — reuse the existing
    `tests/fixtures/cpp/mathlib` built to a shared library; pass its dir via
    `-L`; diff stdout against the same program under `verb build`.
  - `import std io` under `run` — file read/write round-trip (sockets
    optional / kept as they are in the AOT e2e).
  - `import std map` under `run` — **the critical path**: exercises the
    retain/release forwarders and map destruction. Assert `verb_gc_live=0`
    (no leaks), matching the AOT INTEG-01 guarantee.
  - one program mixing all three under `run`.
- **Unit:** forwarder-pointer wiring (a forwarder called after setup reaches
  the emitted helper); registration guard skips symbols the module didn't
  declare (io-only program does not register map symbols).
- **Regression:** confirm `verb build` output for the same fixtures is
  unchanged (AOT path is untouched by this work).

## Spikes to verify early (de-risk before full build)

1. **Dynamic export works** on both macOS and Linux: `dlopen` a trivial mod
   lib whose function calls `verb_alloc`, confirm it resolves against the
   binary's exported forwarder (not an "undefined symbol").
2. **`get_function_address` returns the emitted helper**: the helpers have
   external linkage and `OptimizationLevel::None` prevents DCE/inlining, so
   their addresses are obtainable by name. Confirm non-null and callable.
3. **No leaks with std map under run**: a create/populate/drop map program
   run via `verb run` reports `verb_gc_live=0` — proving the cross-boundary
   alloc (forwarder) / release (forwarder) pairing is consistent.

## Out of scope

- Windows JIT imports (`LoadLibrary`/`GetProcAddress`) — deferred, matching
  the existing Windows `std io` limitation.
- Cross-target JIT (JIT is always host-target; unchanged).
- Any typed/checked extern signatures (that is FFI-V2-02).
- Package management / header parsing (FFI-V2-03).
- Moving the refcount runtime into C++ (Approach B) — explicitly not taken.
