# `verb run` Import Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `verb run` (the MCJIT path) execute programs that use `import std map`, `import std io`, and `import mod <lib>`, which it currently rejects.

**Architecture:** Under MCJIT the program module already emits real `verb_alloc`/`verb_retain_value`/`verb_release_value` bodies. The C++ runtime units (`verb_map.cpp`, `verb_std_io.cpp`) are compiled into the `verb` host binary and their calls to those three symbols bind to the host binary's copies — today `abort()` stubs. We turn those host copies into **forwarding thunks** that call the module's JIT-compiled definitions (addresses fetched at JIT init), register the map/io entry points into MCJIT via `add_global_mapping`, and `dlopen` mod shared libraries via `inkwell::support::load_library_permanently`. AOT (`build`/`compile`) and the value ABI are untouched.

**Tech Stack:** Rust, inkwell 0.9 (LLVM 20, MCJIT), C++17 runtime, `cc` build dependency.

## Global Constraints

- No changes to `src/parser.rs`, `src/lexer.rs`, or the value ABI (`{ i8, i64 }`, refcount header at payload-8).
- No changes to AOT `build`/`compile` behavior. The host thunks live in the `verb` binary only; AOT-generated executables never reference them.
- `import mod` under `run` supports **shared libraries only** (`lib<name>.dylib` / `lib<name>.so`); static `.a` archives stay build-only.
- Follow the existing e2e conventions: fixtures in `tests/fixtures/<name>.verb` with `<name>.expected`, invoked with `Command::new(env!("CARGO_BIN_EXE_verb"))`.
- Reuse the existing `VerbValueAbi` struct in `src/main.rs` (`#[repr(C)] { tag: i8, payload: i64 }`).
- Reuse the existing `register_jit_runtime_symbols` guarded pattern (`if let Some(f) = module.get_function(name) { ee.add_global_mapping(&f, addr) }`) as the single place to wire runtime symbols.

---

### Task 1: `import std map` runs under `verb run`

Turns the host `verb_alloc`/`verb_retain_value`/`verb_release_value` stubs into forwarding thunks, registers the map entry points, and removes the import rejection. This is the atomic "maps work under run" deliverable — thunks and registration are only meaningful together (registration alone aborts in the stubs; thunks alone have nothing to forward to).

**Files:**
- Modify: `src/main.rs` (host stubs → thunks; extern decls + registration; `run` arm)
- Test: `tests/e2e.rs`

**Interfaces:**
- Produces: forwarding host symbols `verb_alloc`/`verb_retain_value`/`verb_release_value`; the extended `register_jit_runtime_symbols` (now also maps `map_new`, `map_set`, `map_get`, `map_has`, `map_remove`, `map_len`); an `assert_no_leaks_under_run(fixture: &str)` test helper in `tests/e2e.rs` that runs a fixture under `verb run` with `VERB_GC_DEBUG=1` and asserts a `verb_gc_live=0` line on stdout.
- Consumes: the pre-existing `std_map_basic.verb` / `std_map_basic.expected` fixtures and the `run_ok` helper.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` (near the other map tests; the `assert_no_leaks_under_run` helper goes next to `assert_no_leaks`):

```rust
/// Like `assert_no_leaks`, but exercises the JIT `run` path instead of AOT.
/// Runs the fixture under `verb run` with `VERB_GC_DEBUG=1` and asserts the
/// program reports `verb_gc_live=0` at exit — proving the host alloc/retain/
/// release thunks forward to the module's counter-touching implementations.
fn assert_no_leaks_under_run(fixture: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{fixture}.verb")])
        .env("VERB_GC_DEBUG", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{fixture}: run failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("{fixture}: no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "{fixture}: leaked under run:\n{stdout}");
}

#[test]
fn run_executes_std_map_import() {
    run_ok("std_map_basic");
    assert_no_leaks_under_run("std_map_basic");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e run_executes_std_map_import -- --nocapture`
Expected: FAIL — `run_ok` sees a non-zero exit; stderr contains `'verb run' does not support imports (std map)`.

- [ ] **Step 3: Replace the abort stubs with forwarding thunks**

In `src/main.rs`, add imports near the top:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
```

Replace the three `#[no_mangle] pub extern "C" fn verb_alloc/verb_retain_value/verb_release_value` stub definitions (the ones that `abort()`) with forwarding thunks plus their address slots:

```rust
// Under `verb run` the program module (src/codegen.rs) emits the real
// verb_alloc/verb_retain_value/verb_release_value bodies. The C++ runtime
// units linked into this binary (verb_map.cpp, verb_std_io.cpp) call these
// symbols, and those calls bind here at host link time — so we forward them
// to the module's JIT-compiled definitions, whose addresses are stored below
// at JIT init (see the `run` arm) before `main` is ever called. This keeps a
// single source of truth for the value runtime and keeps `verb_gc_live`
// consistent regardless of whether an alloc/release originates in module code
// or in host C++.
static VERB_ALLOC_ADDR: AtomicUsize = AtomicUsize::new(0);
static VERB_RETAIN_ADDR: AtomicUsize = AtomicUsize::new(0);
static VERB_RELEASE_ADDR: AtomicUsize = AtomicUsize::new(0);

fn thunk_target(slot: &AtomicUsize, name: &str) -> usize {
    let a = slot.load(Ordering::Relaxed);
    if a == 0 {
        eprintln!("internal error: host {name} thunk called before JIT init");
        std::process::abort();
    }
    a
}

#[no_mangle]
pub extern "C" fn verb_alloc(n: i64) -> *mut std::ffi::c_void {
    let f: extern "C" fn(i64) -> *mut std::ffi::c_void =
        unsafe { std::mem::transmute(thunk_target(&VERB_ALLOC_ADDR, "verb_alloc")) };
    f(n)
}
#[no_mangle]
pub extern "C" fn verb_retain_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(thunk_target(&VERB_RETAIN_ADDR, "verb_retain_value")) };
    f(v)
}
#[no_mangle]
pub extern "C" fn verb_release_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(thunk_target(&VERB_RELEASE_ADDR, "verb_release_value")) };
    f(v)
}
```

- [ ] **Step 4: Declare the map entry points and register them**

In `src/main.rs`, extend the `extern "C"` block (which currently declares only `verb_map_destroy_contents`) with the map functions, so their host addresses can be taken:

```rust
extern "C" {
    /// Defined in `runtime/verb_map.cpp`, compiled into this binary by build.rs.
    fn verb_map_destroy_contents(payload: *mut std::ffi::c_void);
    fn map_new() -> VerbValueAbi;
    fn map_set(m: VerbValueAbi, k: VerbValueAbi, v: VerbValueAbi) -> VerbValueAbi;
    fn map_get(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_has(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_remove(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_len(m: VerbValueAbi) -> VerbValueAbi;
}
```

Extend the `symbols` array in `register_jit_runtime_symbols` (the addresses are only registered when the module actually references the name, via the existing `get_function` guard):

```rust
    let symbols: [(&str, usize); 7] = [
        ("verb_map_destroy_contents", verb_map_destroy_contents as *const () as usize),
        ("map_new", map_new as *const () as usize),
        ("map_set", map_set as *const () as usize),
        ("map_get", map_get as *const () as usize),
        ("map_has", map_has as *const () as usize),
        ("map_remove", map_remove as *const () as usize),
        ("map_len", map_len as *const () as usize),
    ];
```

- [ ] **Step 5: Rewrite the `run` arm to wire thunks and drop the rejection**

In `src/main.rs`, replace the entire `"run" => { ... }` arm body. Remove the `if !imports.is_empty() || !std_imports.is_empty() { ... exit(1); }` rejection block. New arm:

```rust
        "run" => {
            let ee = cg
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| {
                    eprintln!("JIT error: {e}");
                    exit(1);
                });
            // Map/io entry points must be wired before the module is finalized
            // (finalization happens on the first get_function_address below).
            register_jit_runtime_symbols(&ee, cg.module());
            // Point the host verb_alloc/retain/release thunks at the module's
            // JIT-compiled definitions. Only read at runtime (during main), so
            // it is fine that get_function_address finalizes the module here.
            for (name, slot) in [
                ("verb_alloc", &VERB_ALLOC_ADDR),
                ("verb_retain_value", &VERB_RETAIN_ADDR),
                ("verb_release_value", &VERB_RELEASE_ADDR),
            ] {
                let addr = ee.get_function_address(name).unwrap_or_else(|e| {
                    eprintln!("JIT error: cannot resolve {name}: {e:?}");
                    exit(1);
                });
                slot.store(addr, Ordering::Relaxed);
            }
            unsafe {
                let main_fn = ee
                    .get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test e2e run_executes_std_map_import -- --nocapture`
Expected: PASS (`std_map_basic` output matches `.expected`, and `verb_gc_live=0` under run).

- [ ] **Step 7: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat(run): execute import std map via forwarding runtime thunks

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `import std io` runs under `verb run`

Compiles `verb_std_io.cpp` into the `verb` binary and registers the io entry points.

**Files:**
- Modify: `build.rs` (compile `verb_std_io.cpp` into the binary)
- Modify: `src/main.rs` (extern decls + registration for io functions; flip the stale reject test's subject in `tests/e2e.rs`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: the thunks and `register_jit_runtime_symbols` from Task 1.
- Produces: `register_jit_runtime_symbols` additionally maps `read_line`, `file_read`, `file_write`, `file_append`, `tcp_connect`, `tcp_listen`, `tcp_accept`, `send_line`, `recv_line`, `close_conn`.

- [ ] **Step 1: Write the failing test**

In `tests/e2e.rs`, replace `run_rejects_programs_with_std_io_import` (it asserted the old rejection) with:

```rust
#[test]
fn run_executes_std_io_file_roundtrip() {
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
    run_ok("std_io_file_roundtrip");
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e run_executes_std_io_file_roundtrip -- --nocapture`
Expected: FAIL — the JIT engine cannot resolve `file_write` (symbol not in the process; `verb_std_io.cpp` is not compiled into the binary yet).

- [ ] **Step 3: Compile `verb_std_io.cpp` into the binary**

In `build.rs`, add the std-io unit to the compiled runtime. Change the `cc::Build` invocation to include both files, and add its rerun trigger:

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

- [ ] **Step 4: Declare and register the io entry points**

In `src/main.rs`, add the io functions to the `extern "C"` block:

```rust
    fn read_line() -> VerbValueAbi;
    fn file_read(path: VerbValueAbi) -> VerbValueAbi;
    fn file_write(path: VerbValueAbi, contents: VerbValueAbi) -> VerbValueAbi;
    fn file_append(path: VerbValueAbi, contents: VerbValueAbi) -> VerbValueAbi;
    fn tcp_connect(host: VerbValueAbi, port: VerbValueAbi) -> VerbValueAbi;
    fn tcp_listen(port: VerbValueAbi) -> VerbValueAbi;
    fn tcp_accept(fd: VerbValueAbi) -> VerbValueAbi;
    fn send_line(fd: VerbValueAbi, s: VerbValueAbi) -> VerbValueAbi;
    fn recv_line(fd: VerbValueAbi) -> VerbValueAbi;
    fn close_conn(fd: VerbValueAbi) -> VerbValueAbi;
```

Extend the `symbols` array in `register_jit_runtime_symbols` (bump the length to `17`) by appending:

```rust
        ("read_line", read_line as *const () as usize),
        ("file_read", file_read as *const () as usize),
        ("file_write", file_write as *const () as usize),
        ("file_append", file_append as *const () as usize),
        ("tcp_connect", tcp_connect as *const () as usize),
        ("tcp_listen", tcp_listen as *const () as usize),
        ("tcp_accept", tcp_accept as *const () as usize),
        ("send_line", send_line as *const () as usize),
        ("recv_line", recv_line as *const () as usize),
        ("close_conn", close_conn as *const () as usize),
```

So the array declaration becomes `let symbols: [(&str, usize); 17] = [ ... ];`.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test e2e run_executes_std_io_file_roundtrip -- --nocapture`
Expected: PASS (`file_write`/`file_append`/`file_read` round-trip prints `hello world`).

- [ ] **Step 6: Verify the map path still passes (shared runtime unit changed)**

Run: `cargo test --test e2e run_executes_std_map_import -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add build.rs src/main.rs tests/e2e.rs
git commit -m "feat(run): execute import std io by compiling verb_std_io into the binary

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `import mod <lib>` runs under `verb run`

Resolves each `import mod <lib>` to a shared library, `dlopen`s it via LLVM, and makes its symbols visible to MCJIT.

**Files:**
- Modify: `src/main.rs` (mod resolver + dyn-load wiring in the `run` arm; flip the stale reject test)
- Test: `tests/e2e.rs` (unit test for the resolver; e2e for a real mod lib under `run`)

**Interfaces:**
- Consumes: the `run` arm from Task 1, `parsed.lib_dirs` (already parsed by `parse_cli`, entries carry the `-L` prefix, e.g. `"-L/opt/lib"`), and `parsed.files`/`imports`.
- Produces: `fn resolve_mod_lib(name: &str, lib_dirs: &[String]) -> Result<PathBuf, String>` — searches each `-L` dir (prefix stripped) then falls back to the bare filename (letting the OS loader search default paths), returning the first existing `lib<name>.dylib`/`lib<name>.so`, else an `Err` naming the library and the searched dirs.

- [ ] **Step 1: Write the failing resolver unit test**

In `src/main.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn resolve_mod_lib_finds_shared_lib_in_l_dir() {
        let dir = std::env::temp_dir().join(format!("verb_resolve_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let lib = dir.join(format!("libwidget.{ext}"));
        std::fs::write(&lib, b"").unwrap();

        let found = resolve_mod_lib("widget", &[format!("-L{}", dir.display())]).unwrap();
        assert_eq!(found, lib);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_mod_lib_missing_reports_name_and_dirs() {
        let err = resolve_mod_lib("nope", &["-L/does/not/exist".to_string()]).unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("/does/not/exist"), "{err}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin verb resolve_mod_lib -- --nocapture`
Expected: FAIL — `resolve_mod_lib` is not defined.

- [ ] **Step 3: Implement the resolver**

In `src/main.rs`, add near the other free functions:

```rust
/// Resolves an `import mod <name>` to a shared library file for the JIT to
/// dlopen. Searches each `-L<dir>` (prefix stripped) for `lib<name>.dylib`
/// (macOS) / `lib<name>.so` (Linux); if none exists on disk, returns the bare
/// filename so the OS loader can search its default paths. Static `.a`
/// archives are intentionally unsupported under `verb run` — use `verb build`.
fn resolve_mod_lib(name: &str, lib_dirs: &[String]) -> Result<PathBuf, String> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let filename = format!("lib{name}.{ext}");
    let dirs: Vec<&str> = lib_dirs.iter().map(|d| d.trim_start_matches("-L")).collect();
    for dir in &dirs {
        let candidate = PathBuf::from(dir).join(&filename);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    // Fall back to the bare name only if the loader can find it on its default
    // search path; otherwise report a clear error naming the searched dirs.
    let bare = PathBuf::from(&filename);
    if inkwell::support::load_library_permanently(&bare).is_ok() {
        return Ok(bare);
    }
    Err(format!(
        "cannot find shared library for 'import mod {name}' ({filename}); searched: [{}]. \
         'verb run' can only load shared libraries — use 'verb build' for static linking.",
        dirs.join(", ")
    ))
}
```

Note: `load_library_permanently` on the bare name doubles as the existence probe for the default-path fallback; it is idempotent, so loading it again in Step 4 is harmless.

- [ ] **Step 4: Load mod libraries in the `run` arm**

In `src/main.rs`, in the `"run"` arm, immediately after creating `ee` and before `register_jit_runtime_symbols`, insert:

```rust
            // Make the host process's own symbols searchable by MCJIT, then
            // dlopen each `import mod` shared library so its symbols resolve
            // during module finalization.
            inkwell::support::load_visible_symbols();
            for lib in &imports {
                let path = resolve_mod_lib(lib, &parsed.lib_dirs).unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    exit(1);
                });
                if inkwell::support::load_library_permanently(&path).is_err() {
                    eprintln!("error: failed to load shared library {}", path.display());
                    exit(1);
                }
            }
```

- [ ] **Step 5: Run the resolver unit tests to verify they pass**

Run: `cargo test --bin verb resolve_mod_lib -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Write the failing mod e2e test**

In `tests/e2e.rs`, replace `run_rejects_programs_with_imports` (asserted the old rejection) with a test that runs the mathlib fixture under `run`, reusing the existing `build_mathlib_fixture` helper:

```rust
#[test]
fn run_executes_mod_import() {
    let lib_dir = build_mathlib_fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/import_mathlib.verb",
            &format!("-L{}", lib_dir.display()),
        ])
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/import_mathlib.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn run_mod_import_missing_library_errors_clearly() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/import_mathlib.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mathlib"), "stderr: {stderr}");
    assert!(stderr.contains("verb build"), "stderr: {stderr}");
}
```

- [ ] **Step 7: Run the mod e2e tests to verify they pass**

Run: `cargo test --test e2e run_executes_mod_import run_mod_import_missing_library_errors_clearly -- --nocapture`
Expected: PASS — `import_mathlib` prints `3 / 5 / HI! / hello from cpp / true / false`; the missing-library case exits non-zero with a message naming `mathlib` and suggesting `verb build`.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat(run): execute import mod by dlopen-ing shared libraries

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Docs + full-suite verification

Updates user-facing docs that claim `run` cannot use imports, and verifies the whole suite.

**Files:**
- Modify: `README.md` and/or `docs/` (only where they state the old limitation)
- Test: full `cargo test`

- [ ] **Step 1: Find stale claims**

Run: `grep -rniE "run.*(does not|can.?t|cannot).*import|import.*use .?verb build|verb run.*support" README.md docs/ compiler_design_plan.md`
Expected: a list of lines asserting `verb run` rejects imports.

- [ ] **Step 2: Update the docs**

For each hit from Step 1, edit the prose to state that `verb run` now executes `import std io`, `import std map`, and `import mod <lib>` in-process (JIT), with the one caveat that `import mod` under `run` requires a **shared** library findable via `-L`/default loader paths (static `.a` archives remain `verb build`-only). Keep edits minimal and localized — do not restructure the docs. If Step 1 returned no hits, note that in the commit and skip to Step 3.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: PASS — all e2e, parser, formatter, and export-macro tests green, including the new `run_executes_std_map_import`, `run_executes_std_io_file_roundtrip`, `run_executes_mod_import`, `run_mod_import_missing_library_errors_clearly`, and both `resolve_mod_lib` unit tests. Confirm no test still references the removed `run_rejects_programs_with_imports` / `run_rejects_programs_with_std_io_import`.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/ compiler_design_plan.md
git commit -m "docs: verb run now supports imports (std io/map, mod shared libs)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
