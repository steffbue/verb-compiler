# JIT Import Support (FFI-V2-01) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `verb run` (LLVM MCJIT) execute programs using `import mod`, `import std io`, and `import std map`, with the same results as `verb build`.

**Architecture:** Approach A — trampoline forwarders + explicit symbol registration + dlopen. Codegen already emits `verb_alloc`/`verb_retain_value`/`verb_release_value` as in-module IR (trapped inside MCJIT) and emits external declarations for the imported functions. We (1) compile the std runtime into the `verb` binary and register those symbols into the engine, (2) `dlopen` user `import mod` libs and register their symbols, and (3) replace the aborting `verb_alloc`/retain/release host stubs with forwarders that call back into the JIT-compiled helpers, so the C++ side can allocate/refcount. The GC core and the entire AOT path are untouched.

**Tech Stack:** Rust, `inkwell` 0.9 (LLVM 20 / MCJIT), `cc` build crate, `libc` (dlopen/dlsym), C++17 runtime units.

## Global Constraints

- POSIX only for JIT imports. On a Windows host, `verb run` with any import must reject with a clear message; AOT is unchanged. (Consistent with the existing Windows `std io` AOT limitation.)
- No change to `src/codegen.rs`. No change to the AOT (`build`/`compile`) code paths. `verb build` output for every existing fixture must remain identical.
- Runtime helpers are always emitted by codegen (`src/codegen.rs` `new()` calls `build_alloc_fn`/`build_retain_value_fn`/`build_release_value_fn` unconditionally), so `ee.get_function_address("verb_alloc")` etc. always resolve in `run`.
- `-L<dir>` is already parsed globally into `ParsedArgs.lib_dirs` as raw `-L/path` strings (`src/main.rs:133`); reuse it, add no new flag.
- ABI mirror of a `VerbValue` is `#[repr(C)] { tag: i8, payload: i64 }` (see `runtime/verb.h`).
- The std symbol set is fixed and first-party: io = `read_line`, `file_read`, `file_write`, `file_append`, `tcp_connect`, `tcp_listen`, `tcp_accept`, `send_line`, `recv_line`, `close_conn` (`src/codegen.rs` `IO_FUNCS`); map = `map_new`, `map_set`, `map_get`, `map_has`, `map_remove`, `map_len` (`runtime/verb_map.cpp`); plus `verb_map_destroy_contents`.

---

## File Structure

- `build.rs` — also compile `runtime/verb_std_io.cpp` into the `verb` binary; add its rerun-if-changed.
- `Cargo.toml` — add `libc` dependency.
- `.cargo/config.toml` — add a dynamic-export linker flag to `rustflags` so the binary's `verb_alloc`/retain/release forwarders are visible to `dlopen`ed mod libs and `dlsym(RTLD_DEFAULT, …)` sees in-binary std symbols.
- `src/main.rs` —
  - Replace the aborting `verb_alloc`/`verb_retain_value`/`verb_release_value` stubs (`main.rs:44-58`) with forwarders through global function pointers + an installer.
  - Generalize `register_jit_runtime_symbols` (`main.rs:64-75`) to the full std set via `dlsym(RTLD_DEFAULT, …)`, guarded by `module.get_function(name).is_some()`.
  - Add `load_import_libs` (`dlopen`/`dlsym` mod-lib loader).
  - Rewrite the `"run"` arm (`main.rs:219-243`): remove the blanket rejection, add a Windows-host guard, wire registration + loader + forwarder install before `main.call()`.
- `tests/e2e.rs` — convert the three "run rejects imports" tests to "run executes imports"; add JIT leak-check helper + a JIT mod-lib run helper; add a mixed-imports JIT test.
- `tests/fixtures/jit_all_imports.verb` (+ `.expected`) — new mixed fixture (mod + std io + std map under `run`).
- `docs/...` and `.planning/REQUIREMENTS.md` — mark FFI-V2-01 done; update the cpp-import spec's out-of-scope note.

---

## Task 1: Foundation — compile std io into the binary, add libc, enable dynamic export

**Files:**
- Modify: `build.rs`
- Modify: `Cargo.toml` (`[dependencies]`)
- Modify: `.cargo/config.toml`
- Test: `tests/e2e.rs` (new test `in_binary_std_symbols_are_dynamically_resolvable`)

**Interfaces:**
- Produces: `runtime/verb_std_io.cpp` symbols (`file_read`, …) linked into the `verb` binary and present in its dynamic symbol table; `libc` crate available to `src/main.rs`.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` (near the other import tests). It proves the std io unit is compiled into the binary *and* dynamically exported (the mechanism both later tasks rely on). It links `libc` in the test via the crate's own dep once `Cargo.toml` has it; until then it fails to compile/link.

```rust
#[test]
fn in_binary_std_symbols_are_dynamically_resolvable() {
    // verb_std_io.cpp must be compiled into the `verb` test binary (build.rs)
    // and its symbols exported so dlsym(RTLD_DEFAULT, ...) can find them.
    // This is the resolution path the JIT uses for std io / std map.
    use std::ffi::CString;
    let name = CString::new("file_read").unwrap();
    let addr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    assert!(!addr.is_null(), "file_read not resolvable via dlsym(RTLD_DEFAULT); \
        verb_std_io.cpp not compiled in or dynamic export flag not effective");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e in_binary_std_symbols_are_dynamically_resolvable`
Expected: FAIL — compile error (`libc` not a dependency) or, once compiling, `file_read not resolvable` (std io not compiled in / not exported).

- [ ] **Step 3: Add `libc` dependency**

In `Cargo.toml` under `[dependencies]`:

```toml
libc = "0.2"
```

- [ ] **Step 4: Compile verb_std_io.cpp into the binary**

In `build.rs`, add the std io unit alongside `verb_map.cpp`:

```rust
    let map_cpp = runtime.join("verb_map.cpp");
    let std_io_cpp = runtime.join("verb_std_io.cpp");

    println!("cargo:rerun-if-changed={}", map_cpp.display());
    println!("cargo:rerun-if-changed={}", std_io_cpp.display());
    println!("cargo:rerun-if-changed={}", runtime.join("verb.h").display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&runtime)
        .file(&map_cpp)
        .file(&std_io_cpp)
        .compile("verb_runtime");
```

- [ ] **Step 5: Enable dynamic export**

In `.cargo/config.toml`, add per-OS rustflags so the executable exports its symbols dynamically. Keep the existing `[env]` block. Append:

```toml
[target.'cfg(target_os = "linux")']
rustflags = ["-C", "link-arg=-rdynamic"]

[target.'cfg(target_os = "macos")']
rustflags = ["-C", "link-arg=-Wl,-export_dynamic"]
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test --test e2e in_binary_std_symbols_are_dynamically_resolvable`
Expected: PASS.

- [ ] **Step 7: Confirm no regressions**

Run: `cargo test`
Expected: PASS (existing suite unaffected; std io/map/import tests still exercise the AOT path).

- [ ] **Step 8: Commit**

```bash
git add build.rs Cargo.toml Cargo.lock .cargo/config.toml tests/e2e.rs
git commit -m "feat(ffi-v2-01): compile std io into binary, add libc, export symbols dynamically"
```

---

## Task 2: Forwarders + generalized registration → std io and std map under `verb run`

**Files:**
- Modify: `src/main.rs:44-58` (stubs → forwarders), `src/main.rs:64-75` (registration), `src/main.rs:219-243` (`run` arm)
- Test: `tests/e2e.rs` — convert `run_rejects_programs_with_std_io_import` (~643) and `run_rejects_programs_with_std_map_import` (~780); add JIT leak-check helper.

**Interfaces:**
- Consumes: `libc` and the in-binary std symbols from Task 1.
- Produces:
  - `install_runtime_forwarders(ee: &ExecutionEngine)` — reads `ee.get_function_address("verb_alloc"|"verb_retain_value"|"verb_release_value")` and stores them into process-global pointers backing the exported `verb_alloc`/`verb_retain_value`/`verb_release_value` forwarders.
  - `register_jit_runtime_symbols(ee, module)` — now registers every name in the std set that the module declares.
  - `run` arm: on POSIX, executes std io / std map programs; on Windows, rejects imports.

- [ ] **Step 1: Write the failing tests**

In `tests/e2e.rs`, add a JIT leak-check helper next to `assert_no_leaks`:

```rust
/// Like `assert_no_leaks`, but drives the program through the JIT
/// (`verb run`) instead of a built binary. Proves the run-mode refcount
/// forwarders (verb_alloc/retain/release) reclaim every heap value.
fn assert_no_leaks_run(fixture: &str, lib_dirs: &[String]) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_verb"));
    cmd.arg("run").arg(format!("tests/fixtures/{fixture}.verb"));
    for d in lib_dirs { cmd.arg(d); }
    cmd.env("VERB_GC_DEBUG", "1");
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{fixture}: run failed:\n{}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("{fixture}: no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "{fixture}: leaked heap objects:\n{stdout}");
}
```

Replace the body of `run_rejects_programs_with_std_io_import` (rename to reflect the new behavior):

```rust
#[test]
fn run_executes_a_program_using_std_io_files() {
    // std io file round-trip under the JIT must match its build-mode output.
    let _ = std::fs::remove_file("verb_e2e_std_io_run.tmp");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_io_file_roundtrip.verb"])
        .output().unwrap();
    assert!(out.status.success(), "run failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_file_roundtrip.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}
```

Replace the body of `run_rejects_programs_with_std_map_import`:

```rust
#[test]
fn run_executes_a_program_using_std_map() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_map_basic.verb"])
        .output().unwrap();
    assert!(out.status.success(), "run failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_map_basic.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn run_std_map_with_heap_values_leaks_nothing() {
    // The critical forwarder path: map retains/releases heap VerbValues via
    // the JIT-compiled helpers. gc_live must return to 0.
    assert_no_leaks_run("gc_map_heap_values", &[]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e run_executes_a_program_using_std_io_files run_executes_a_program_using_std_map run_std_map_with_heap_values_leaks_nothing`
Expected: FAIL — current `run` arm prints "does not support imports" and exits 1.

- [ ] **Step 3: Replace the aborting stubs with forwarders**

In `src/main.rs`, replace the block at lines 44-58 (the three `#[no_mangle]` aborting stubs) with global pointers + forwarders + an installer. Keep the `VerbValueAbi` struct above it.

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

// Set once, at JIT startup, to the addresses of the module's emitted
// verb_alloc/verb_retain_value/verb_release_value. The C++ runtime units
// compiled into this binary (verb_map.cpp, verb_std_io.cpp) and any dlopen'd
// import-mod library call these forwarder symbols; the forwarders hop into
// the JIT-compiled helpers. Under AOT these forwarders are never linked
// (the object file carries its own emitted helpers), so AOT is unaffected.
static VERB_ALLOC_FP: AtomicUsize = AtomicUsize::new(0);
static VERB_RETAIN_FP: AtomicUsize = AtomicUsize::new(0);
static VERB_RELEASE_FP: AtomicUsize = AtomicUsize::new(0);

fn forwarder_addr(slot: &AtomicUsize, name: &str) -> usize {
    let a = slot.load(Ordering::Acquire);
    if a == 0 {
        eprintln!("internal error: {name} forwarder called before JIT runtime init");
        std::process::abort();
    }
    a
}

#[no_mangle]
pub extern "C" fn verb_alloc(n: i64) -> *mut std::ffi::c_void {
    let f: extern "C" fn(i64) -> *mut std::ffi::c_void =
        unsafe { std::mem::transmute(forwarder_addr(&VERB_ALLOC_FP, "verb_alloc")) };
    f(n)
}
#[no_mangle]
pub extern "C" fn verb_retain_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(forwarder_addr(&VERB_RETAIN_FP, "verb_retain_value")) };
    f(v)
}
#[no_mangle]
pub extern "C" fn verb_release_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(forwarder_addr(&VERB_RELEASE_FP, "verb_release_value")) };
    f(v)
}

/// Point the forwarders at the module's JIT-compiled helpers. Must run after
/// engine creation and before any Verb or C++ runtime code executes. Codegen
/// emits all three helpers into every module, so lookups always succeed.
fn install_runtime_forwarders(ee: &inkwell::execution_engine::ExecutionEngine) {
    for (slot, name) in [
        (&VERB_ALLOC_FP, "verb_alloc"),
        (&VERB_RETAIN_FP, "verb_retain_value"),
        (&VERB_RELEASE_FP, "verb_release_value"),
    ] {
        let addr = ee.get_function_address(name)
            .unwrap_or_else(|e| { eprintln!("JIT error: cannot resolve {name}: {e}"); exit(1); });
        slot.store(addr as usize, Ordering::Release);
    }
}
```

Note: `verb_map_destroy_contents` extern (lines ~31-34) is no longer referenced directly (registration uses `dlsym` — Step 4); remove that `extern "C"` block if it produces an unused-warning-as-error, otherwise leave it. If removed, also remove the now-unused import at the top of the file if any.

- [ ] **Step 4: Generalize `register_jit_runtime_symbols` to the std set**

Replace `register_jit_runtime_symbols` (lines 64-75) with a `dlsym(RTLD_DEFAULT, …)`-based registration over the full std set, guarded by presence in the module:

```rust
/// Registers the first-party std runtime symbols (io + map + the map
/// destructor) that codegen emits as external declarations. Their code is
/// compiled into this binary (build.rs) and exported dynamically, so
/// dlsym(RTLD_DEFAULT, name) yields the address. Only symbols the module
/// actually declares are registered.
fn register_jit_runtime_symbols<'ctx>(
    ee: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &inkwell::module::Module<'ctx>,
) {
    const STD_SYMBOLS: &[&str] = &[
        // io
        "read_line", "file_read", "file_write", "file_append",
        "tcp_connect", "tcp_listen", "tcp_accept", "send_line", "recv_line", "close_conn",
        // map
        "map_new", "map_set", "map_get", "map_has", "map_remove", "map_len",
        // always emitted by codegen's release path
        "verb_map_destroy_contents",
    ];
    for name in STD_SYMBOLS {
        let Some(f) = module.get_function(name) else { continue };
        let cname = std::ffi::CString::new(*name).unwrap();
        let addr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, cname.as_ptr()) };
        if addr.is_null() {
            eprintln!("internal error: std runtime symbol '{name}' not found in process");
            exit(1);
        }
        ee.add_global_mapping(&f, addr as usize);
    }
}
```

- [ ] **Step 5: Rewrite the `run` arm (drop rejection, add guard + forwarder install)**

Replace the `"run"` arm (lines 219-243). For this task, `import mod` is not yet loaded (Task 3), so keep rejecting when `!imports.is_empty()` but allow `std_imports`. On Windows, reject any import.

```rust
        "run" => {
            let has_imports = !imports.is_empty() || !std_imports.is_empty();
            if has_imports && cfg!(target_os = "windows") {
                eprintln!("error: 'verb run' does not support imports on Windows; use 'verb build'");
                exit(1);
            }
            if !imports.is_empty() {
                // import mod libraries land in Task 3.
                eprintln!(
                    "error: 'verb run' does not yet support 'import mod' ({}); use 'verb build'",
                    imports.join(", ")
                );
                exit(1);
            }
            let ee = cg
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| { eprintln!("JIT error: {e}"); exit(1); });
            register_jit_runtime_symbols(&ee, cg.module());
            install_runtime_forwarders(&ee);
            unsafe {
                let main_fn = ee
                    .get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test e2e run_executes_a_program_using_std_io_files run_executes_a_program_using_std_map run_std_map_with_heap_values_leaks_nothing`
Expected: PASS.

- [ ] **Step 7: Full suite**

Run: `cargo test`
Expected: PASS. (The old rejection tests are gone; AOT tests unchanged.)

- [ ] **Step 8: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat(ffi-v2-01): run std io and std map under verb run via forwarders + registration"
```

---

## Task 3: dlopen loader → `import mod` under `verb run`

**Files:**
- Modify: `src/main.rs` (add `load_import_libs`; wire into `run` arm; drop the Task-2 `import mod` rejection)
- Test: `tests/e2e.rs` — convert `run_rejects_programs_with_imports` (~365); add a JIT mod-lib run helper.

**Interfaces:**
- Consumes: `imports: &[String]`, `lib_dirs: &[String]` (raw `-L/path`), the module's extern declarations, the registration + forwarder machinery from Task 2.
- Produces: `load_import_libs(ee, module, imports, lib_dirs) -> Vec<*mut c_void>` — `dlopen`s each `lib<name>.{dylib,so}`, `dlsym`s and `add_global_mapping`s each still-unresolved extern the module declares, returns the (leaked) handles.

- [ ] **Step 1: Write the failing test**

In `tests/e2e.rs`, add a JIT mod-lib runner mirroring the existing `build_and_run_ok`, reusing `build_mathlib_fixture()`:

```rust
/// Runs `<fixture>.verb` through the JIT with `-L<lib_dir>`, asserting
/// success and matching `<fixture>.expected`.
fn run_with_lib_ok(fixture: &str, lib_dir: &std::path::Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            &format!("tests/fixtures/{fixture}.verb"),
            &format!("-L{}", lib_dir.display()),
        ])
        .output().unwrap();
    assert!(out.status.success(), "{fixture}: run failed:\n{}",
        String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string(format!("tests/fixtures/{fixture}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}
```

Replace the body of `run_rejects_programs_with_imports` (~365):

```rust
#[test]
fn run_imports_a_cpp_library_and_calls_extern_functions() {
    // import mod under the JIT: dlopen the fixture lib, resolve externs,
    // and (via c_shout returning a string) exercise the verb_alloc forwarder.
    let lib_dir = build_mathlib_fixture();
    run_with_lib_ok("import_mathlib", &lib_dir);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e run_imports_a_cpp_library_and_calls_extern_functions`
Expected: FAIL — the Task-2 `run` arm still rejects `import mod`.

- [ ] **Step 3: Implement the loader**

Add to `src/main.rs`:

```rust
/// dlopen each `import mod` library, resolve every extern the module
/// declares against the opened handles, and register the address with the
/// engine. `lib_dirs` entries are raw `-L/path` strings. Returns the opened
/// handles (leaked for the process lifetime). Aborts the process with a
/// clear message on any failure.
fn load_import_libs<'ctx>(
    ee: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &inkwell::module::Module<'ctx>,
    imports: &[String],
    lib_dirs: &[String],
) -> Vec<*mut std::ffi::c_void> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let dirs: Vec<&str> = lib_dirs.iter().map(|d| d.trim_start_matches("-L")).collect();
    let mut handles = Vec::new();

    for name in imports {
        // Find libNAME.<ext> in a -L dir, else fall back to the bare soname
        // so the loader's default search path applies.
        let mut candidate: Option<std::ffi::CString> = None;
        for dir in &dirs {
            let p = std::path::Path::new(dir).join(format!("lib{name}.{ext}"));
            if p.exists() {
                candidate = Some(std::ffi::CString::new(p.to_str().unwrap()).unwrap());
                break;
            }
        }
        let path = candidate.unwrap_or_else(|| {
            std::ffi::CString::new(format!("lib{name}.{ext}")).unwrap()
        });
        // RTLD_LOCAL (not GLOBAL): keep the lib's own symbols OUT of the
        // global namespace, so the resolution loop below can tell a genuine
        // in-process symbol (libc / std / forwarder) from a mod extern and
        // map the latter explicitly via dlsym(handle, ...). The lib's own
        // undefined refs (e.g. its verb_alloc callback) still resolve against
        // this executable's dynamically-exported forwarders regardless.
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            let err = unsafe { libc::dlerror() };
            let msg = if err.is_null() { String::new() }
                else { unsafe { std::ffi::CStr::from_ptr(err) }.to_string_lossy().into_owned() };
            eprintln!("error: cannot load import library 'lib{name}.{ext}' \
                (searched: {}): {msg}", dirs.join(", "));
            exit(1);
        }
        handles.push(handle);
    }

    // Resolve every still-unresolved external declaration in the module
    // (the mod externs) against the opened handles.
    let mut f = module.get_first_function();
    while let Some(func) = f {
        f = func.get_next_function();
        if func.count_basic_blocks() != 0 { continue; } // has a body -> not external
        let name = func.get_name().to_string_lossy().into_owned();
        // Already mapped by register_jit_runtime_symbols, or resolvable by
        // MCJIT itself (libc: malloc/printf/...). Skip anything dlsym finds
        // in-process; only map symbols that live in the dlopen'd libs.
        let cname = std::ffi::CString::new(name.clone()).unwrap();
        if !unsafe { libc::dlsym(libc::RTLD_DEFAULT, cname.as_ptr()) }.is_null() {
            continue;
        }
        for &handle in &handles {
            let addr = unsafe { libc::dlsym(handle, cname.as_ptr()) };
            if !addr.is_null() {
                ee.add_global_mapping(&func, addr as usize);
                break;
            }
        }
    }
    handles
}
```

- [ ] **Step 4: Wire the loader into the `run` arm; drop the `import mod` rejection**

In the `"run"` arm, remove the `if !imports.is_empty() { … exit(1) }` block added in Task 2, and call the loader after `register_jit_runtime_symbols` and before `install_runtime_forwarders`:

```rust
            register_jit_runtime_symbols(&ee, cg.module());
            let _import_handles = load_import_libs(&ee, cg.module(), &imports, &parsed.lib_dirs);
            install_runtime_forwarders(&ee);
```

Keep the Windows guard (`has_imports && cfg!(target_os = "windows")`) at the top of the arm.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test e2e run_imports_a_cpp_library_and_calls_extern_functions`
Expected: PASS.

- [ ] **Step 6: Full suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat(ffi-v2-01): run import mod libraries under verb run via dlopen"
```

---

## Task 4: Mixed-imports integration test + docs

**Files:**
- Create: `tests/fixtures/jit_all_imports.verb`, `tests/fixtures/jit_all_imports.expected`
- Modify: `tests/e2e.rs` (mixed-imports JIT test)
- Modify: `.planning/REQUIREMENTS.md`, `docs/superpowers/specs/2026-07-20-cpp-import-design.md`

**Interfaces:**
- Consumes: everything from Tasks 1-3, `build_mathlib_fixture()`, `run_with_lib_ok`.

- [ ] **Step 1: Create the mixed fixture**

`tests/fixtures/jit_all_imports.verb` — combines `import mod mathlib`, `import std io`, `import std map` in one program run through the JIT:

```
import mod mathlib;
import std io;
import std map;

assign m map_new();
map_set(m, "greet", c_shout("hi"));
print(map_get(m, "greet"));
print(c_add_int(2, 3));
assign ok file_write("verb_jit_all_imports.tmp", "data");
print(map_len(m));
```

- [ ] **Step 2: Generate the expected output**

Run the program once with the freshly built binary to capture golden output (mathlib built by the harness; run manually to author `.expected`):

Run: `c++ -std=c++17 -shared -fPIC tests/fixtures/cpp/mathlib.cpp -Iruntime -o /tmp/libmathlib.dylib && cargo run -- run tests/fixtures/jit_all_imports.verb -L/tmp`
Expected: prints the shouted greeting, `5`, and `1`. Write exactly that stdout into `tests/fixtures/jit_all_imports.expected`.

- [ ] **Step 3: Write the test**

In `tests/e2e.rs`:

```rust
#[test]
fn run_mixes_mod_std_io_and_std_map_imports() {
    let _ = std::fs::remove_file("verb_jit_all_imports.tmp");
    let lib_dir = build_mathlib_fixture();
    run_with_lib_ok("jit_all_imports", &lib_dir);
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test --test e2e run_mixes_mod_std_io_and_std_map_imports`
Expected: PASS.

- [ ] **Step 5: Update requirement tracking**

In `.planning/REQUIREMENTS.md`: move `FFI-V2-01` from the v2 "Native Interop" list to a completed entry (or mark it `[x]`), and add a `FFI-V2-01 | Phase 8 | Complete` row to the Traceability table (match the surrounding format).

In `docs/superpowers/specs/2026-07-20-cpp-import-design.md`: update the `verb run` note (~line 100) and the Out-of-scope line that reads "JIT-mode (`verb run`) extern support" to record that JIT imports are now supported (FFI-V2-01), referencing `docs/superpowers/specs/2026-07-23-jit-import-support-design.md`.

- [ ] **Step 6: Full suite + AOT-unchanged check**

Run: `cargo test`
Expected: PASS — including all pre-existing AOT `build` import/std/map tests, confirming the AOT path is unchanged.

- [ ] **Step 7: Commit**

```bash
git add tests/fixtures/jit_all_imports.verb tests/fixtures/jit_all_imports.expected tests/e2e.rs .planning/REQUIREMENTS.md docs/superpowers/specs/2026-07-20-cpp-import-design.md
git commit -m "test(ffi-v2-01): mixed mod+io+map JIT integration test; mark FFI-V2-01 done"
```

---

## Self-Review

**Spec coverage:**
- Direction 1 (module → std) → Task 1 (compile-in + export) + Task 2 (registration). ✓
- Direction 1 (module → mod libs) → Task 3 (dlopen loader). ✓
- Direction 2 (C++ → verb helpers) → Task 2 (forwarders + installer). ✓
- Drop rejection / Windows guard → Task 2 (std) + Task 3 (mod). ✓
- CLI `-L` reuse → Task 3 loader consumes `lib_dirs`. ✓
- Testing (std io, std map + gc_live=0, mod, mixed, AOT-unchanged) → Tasks 2-4. ✓
- Spikes: dynamic export → Task 1 test; `get_function_address` non-null → Task 2 install (aborts loudly if null); no leaks → Task 2 `run_std_map_with_heap_values_leaks_nothing`. ✓
- Out of scope (Windows JIT, cross JIT, Approach B) → honored; no tasks. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. Step 2 of Task 4 authors `.expected` from real output rather than guessing exact string — the run command and expected shape are given.

**Type consistency:** `install_runtime_forwarders`, `register_jit_runtime_symbols`, `load_import_libs` signatures consistent across tasks; `VerbValueAbi` reused for retain/release forwarders; forwarder pointer slots (`VERB_ALLOC_FP`/`VERB_RETAIN_FP`/`VERB_RELEASE_FP`) named consistently; std symbol list matches the spec's Global Constraints and `IO_FUNCS`/`verb_map.cpp`.
