# Phase 8: Integration Validation & Release Readiness - Pattern Map

**Mapped:** 2026-07-21
**Files analyzed:** 6 (2 new .verb examples, 1 modified test file with ~3 new test fns, 2 doc edits, 0 new Rust/C++ source)
**Analogs found:** 6 / 6

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|-----------------|----------------|
| `examples/integration_all.verb` | example program (.verb source) | CRUD (FFI call + file I/O + map + array) | `tests/fixtures/gc_stress_all_kinds.verb` (structure) + `examples/files.verb` (banner/build-comment style) + `tests/fixtures/import_mathlib.verb` (FFI call style) | role-match (composite) |
| `examples/integration_all_windows.verb` (or similar name, Claude's discretion) | example program (.verb source) | CRUD (FFI call + map + array, no file I/O) | same as above, minus `import std io` | role-match (composite) |
| `tests/e2e.rs` — new fn `integration_example_zero_leaks()` | test | request-response (build+run, assert stdout marker) | `assert_no_leaks()` helper + its existing callers (e.g. any single-line `#[test] fn X() { assert_no_leaks("Y"); }`) at `tests/e2e.rs:52-67` | exact |
| `tests/e2e.rs` — new fn for cross-compile FFI build (build-only, 6 targets) | test | batch / build-only | `aot_cross_build_produces_binary_for_each_target()` at `tests/e2e.rs:477-500`, combined with `build_mathlib_fixture()` at `tests/e2e.rs:378-394` | exact |
| `tests/e2e.rs` — new fn asserting `examples/integration_all.verb` output correctness | test | request-response | `build_and_run_ok()` at `tests/e2e.rs:396-417` (build+run+compare-stdout pattern), adapted to build from `examples/` instead of `tests/fixtures/` | exact (needs path-source adaptation, see below) |
| `docs/superpowers/specs/2026-07-21-arrays-design.md` (doc edit) | config/doc | transform (text substitution) | itself — in-place edit, no analog needed | n/a |
| `docs/superpowers/plans/2026-07-21-arrays.md` (doc edit) | config/doc | transform (text substitution) | itself — in-place edit, no analog needed | n/a |

## Pattern Assignments

### `examples/integration_all.verb` (example program)

**Analog 1 — banner/build-instructions comment style:** `examples/files.verb:1-9`
```verb
%% files.verb — whole-file read/write/append via `import std io;`
!?!
  Build (imports need `verb build`, not `verb run` — see README.md)
  and run:

    verb build examples/files.verb -o files_demo
    ./files_demo
!?!
```
Use this exact `%% <name>.verb — <one-line purpose>` + `!?!...!?!` banner block at the top of `integration_all.verb`, updating the build command to reference `integration_all`.

**Analog 2 — FFI import + calls:** `tests/fixtures/import_mathlib.verb:1-6`
```verb
import mod mathlib;

print(c_sqrt(9.0));
print(c_add_int(2, 3));
print(c_shout("hi"));
c_hello();
print(c_is_positive(5));
print(c_is_positive(neg 5));
```
The exported symbols available for reuse (no new C++ needed) are declared in `tests/fixtures/cpp/mathlib.cpp:25-29`: `c_sqrt`, `c_add_int`, `c_shout`, `c_hello`, `c_is_positive`. Pick 1-2 (e.g. `c_add_int`) to keep the combo minimal per D-01.

**Analog 3 — map + array + loop combo:** `tests/fixtures/gc_stress_all_kinds.verb` (full file, 14 lines)
```verb
import std map;

assign total 0;
loop assign i 0; i trails 500; i be i add 1 begin
  declare s;
  s be "iter" join "-done";
  assign arr list s, i;
  assign m map_new();
  map_set(m, "k", get(arr, 0));
  check map_get(m, "k") equals "iter-done" begin
    total be total add 1;
  end
end
print(total);
```
Reuse `map_new()`, `map_set(m, k, v)`, `map_get(m, k)`, `assign arr list ...`, `get(arr, i)` idioms directly; drop the 500-iteration stress loop (D-01 wants ~15 lines total, not a stress test) — a single pass tallying one call count is enough.

**Analog 4 — file write via `std io`:** `examples/files.verb:11-19` (import + `file_write`/`file_append`/`file_read` + nil-check via `check ... equals nil begin ... end orelse begin ... end`)
```verb
import std io;

assign path "verb_files_demo.tmp";
file_write(path, "first line");
```
Use `import std io;` + `file_write(path, result)` for D-01's "write a result to a file" requirement.

**Composition guidance:** combine analogs 1-4: banner comment, then `import mod mathlib;` + `import std io;` + `import std map;`, call one FFI function, write its result via `file_write`, tally a call count in a map, collect a couple of results in an array via `assign arr list ...`, `print()` a final summary line. Keep to ~15 lines per D-01.

---

### `examples/integration_all_windows.verb` (Windows-variant, no `std io`)

**Analog:** same as above minus the `import std io;` / `file_write` section (D-06). Otherwise identical FFI + map + array structure. No filesystem write, no `check ... equals nil` on file result — replace with a `print()` of the tallied map value directly.

---

### `tests/e2e.rs` — `integration_example_zero_leaks()` (test, build-and-check-marker)

**Analog:** `assert_no_leaks()` helper, `tests/e2e.rs:44-67`
```rust
fn assert_no_leaks(fixture: &str) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_v2_{fixture}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", &format!("tests/fixtures/{fixture}.verb"), "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "{fixture}: build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).env("VERB_GC_DEBUG", "1").output().unwrap();
    assert!(run.status.success(), "{fixture}: run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("{fixture}: no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "{fixture}: leaked heap objects:\n{stdout}");
    let _ = std::fs::remove_file(&out_path);
}
```
**IMPORTANT deviation required (per D-02/D-03):** `assert_no_leaks` hardcodes the `tests/fixtures/{fixture}.verb` path. Since `integration_all.verb` lives in `examples/` (not duplicated into fixtures per D-02), the new test **cannot** call `assert_no_leaks("integration_all")` unmodified — it must either (a) inline a variant of the helper's build+run+check-marker logic pointed at `examples/integration_all.verb`, or (b) add a small path-parameterized overload. Recommend inlining, since this is described as a "standalone test function" (D-04) and the helper's simplicity makes a full duplicate cheap:
```rust
#[test]
fn integration_example_zero_leaks() {
    // build from examples/integration_all.verb instead of tests/fixtures/
    // reuse the exact build -> run with VERB_GC_DEBUG=1 -> verb_gc_live=0 assertion body from assert_no_leaks()
}
```
Since the example also needs `-L<mathlib dir>` for the FFI import to link, combine with `build_mathlib_fixture()` (see below) for the build args.

**Naming convention analog:** single-purpose `#[test] fn <name>() { ... }` functions immediately following `assert_no_leaks`'s definition, e.g. lines 69-100 (`literals`, `array_literal_prints`, etc.) — one test, one assertion concern, short body.

---

### `tests/e2e.rs` — cross-compile build-only test for the FFI/mod-import combo (6 targets)

**Analog:** `aot_cross_build_produces_binary_for_each_target()`, `tests/e2e.rs:472-500`
```rust
fn zig_available() -> bool {
    Command::new("zig").arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn aot_cross_build_produces_binary_for_each_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_aot_cross_test");
    std::fs::create_dir_all(&dir).unwrap();
    for label in ["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64", "windows-x86_64", "windows-arm64"] {
        let bin = dir.join(format!("functions_{label}"));
        let out = Command::new(env!("CARGO_BIN_EXE_verb"))
            .args(["build", "tests/fixtures/functions.verb", "-o", bin.to_str().unwrap(), "--target", label])
            .output()
            .unwrap();
        assert!(out.status.success(), "target {label} failed: {}", String::from_utf8_lossy(&out.stderr));
        let expected_path = if label.starts_with("windows") {
            dir.join(format!("functions_{label}.exe"))
        } else {
            bin
        };
        let meta = std::fs::metadata(&expected_path)
            .unwrap_or_else(|e| panic!("missing output for {label} at {expected_path:?}: {e}"));
        assert!(meta.len() > 0, "empty output for {label}");
    }
}
```
Copy this loop structure exactly, but: (1) guard with `zig_available()` same as original, (2) call `build_mathlib_fixture()` first to get `lib_dir`, (3) build `examples/integration_all.verb` for `linux-*`/`macos-*` targets and `examples/integration_all_windows.verb` for `windows-*` targets (per D-06, since std io isn't supported cross-compiled to Windows), (4) pass `-L{lib_dir}` on every build invocation so the `import mod mathlib;` FFI import resolves, (5) assert build success only — never execute non-host binaries (D-05).

**`build_mathlib_fixture()` analog (reuse verbatim, no changes needed):** `tests/e2e.rs:376-394`
```rust
fn build_mathlib_fixture() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("verb_e2e_cpp_libs");
    std::fs::create_dir_all(&dir).unwrap();
    let lib_path = dir.join("libmathlib.dylib");
    let status = Command::new("c++")
        .args([
            "-std=c++17",
            "-Iruntime",
            "-dynamiclib",
            "-o", lib_path.to_str().unwrap(),
            "tests/fixtures/cpp/mathlib.cpp",
        ])
        .status()
        .expect("failed to invoke c++ to build the mathlib test fixture");
    assert!(status.success(), "failed to compile tests/fixtures/cpp/mathlib.cpp");
    dir
}
```
Already builds a `.dylib` from `mathlib.cpp` once per test run and returns its dir for `-L`. Call this directly; no modification needed since the integration example reuses `mathlib.cpp` unchanged.

---

### `tests/e2e.rs` — output-correctness test for `examples/integration_all.verb` (host build + run)

**Analog:** `build_and_run_ok()` + its caller `imports_cpp_library_and_calls_extern_functions()`, `tests/e2e.rs:396-423`
```rust
fn build_and_run_ok(name: &str, lib_dir: &std::path::Path) {
    let out_path = std::env::temp_dir().join(format!("verb_e2e_build_{name}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            &format!("tests/fixtures/{name}.verb"),
            "-o", out_path.to_str().unwrap(),
            &format!("-L{}", lib_dir.display()),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path)
        .env("DYLD_LIBRARY_PATH", lib_dir)
        .output()
        .unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn imports_cpp_library_and_calls_extern_functions() {
    let lib_dir = build_mathlib_fixture();
    build_and_run_ok("import_mathlib", &lib_dir);
}
```
Same deviation as `assert_no_leaks`: `build_and_run_ok` hardcodes `tests/fixtures/{name}.verb` and `tests/fixtures/{name}.expected` — since there is no `.expected` file for an `examples/` program (D-02 explicitly avoids fixture duplication), this test should either (a) inline the build+run body pointed at `examples/integration_all.verb` and assert on stdout content directly in the test body (no separate `.expected` file), or (b) fold this assertion into the same `integration_example_zero_leaks()` test rather than writing a second nearly-identical test — D-03 only requires checking `verb_gc_live=0` **plus** expected output, suggesting one combined test is sufficient and matches D-04's "standalone test function" (singular) framing.

---

### Doc corrections (HOUSEKEEP-01) — no code pattern, direct text edits

**File 1:** `docs/superpowers/specs/2026-07-21-arrays-design.md:77`
```
Add `TAG_ARRAY: u64 = 6` to `src/value.rs`.
```
Change `6` → `7`. (Line 82, 111, 115, 117, 119, 142, 170, 179 reference `TAG_ARRAY` by name only, not the numeric value — leave unchanged; only numeric-literal occurrences need correction. Verify no other numeric `= 6` reference to `TAG_ARRAY` exists elsewhere in the file with a full-file grep before editing.)

**File 2:** `docs/superpowers/plans/2026-07-21-arrays.md:7,16`
```
**Architecture:** New value tag `TAG_ARRAY = 6` whose payload is a heap point...
...
- `TAG_ARRAY = 6`, appended after the existing tags 0–5 in `src/value.rs`.
```
Change both `TAG_ARRAY = 6` → `TAG_ARRAY = 7`; the "tags 0–5" range on line 16 should also be reviewed — if array is now tag 7, confirm whether the preceding tag range text needs adjusting to "0–6" (check current `src/value.rs` tag enumeration for ground truth before editing, since `docs/superpowers/specs/2026-07-21-maps-design.md` competingly claims `TAG_MAP = 6` — read for context per canonical_refs, do not modify).

---

## Shared Patterns

### Build-then-run CLI invocation
**Source:** `tests/e2e.rs:54-61` (`assert_no_leaks`) and `tests/e2e.rs:398-413` (`build_and_run_ok`)
**Apply to:** all new `tests/e2e.rs` test functions in this phase
```rust
Command::new(env!("CARGO_BIN_EXE_verb"))
    .args(["build", "<path>.verb", "-o", out_path.to_str().unwrap() /* , "-L<lib_dir>" , "--target", label */])
    .output()
    .unwrap();
```
All new tests should use `env!("CARGO_BIN_EXE_verb")` as the binary under test, never a hardcoded path.

### Zig availability guard for cross-compile tests
**Source:** `tests/e2e.rs:472-474`
**Apply to:** the new cross-compile FFI test
```rust
fn zig_available() -> bool {
    Command::new("zig").arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}
```
Reuse this existing helper directly (already defined once in the file) rather than redefining it.

### GC leak marker convention
**Source:** `tests/e2e.rs:62-65`
**Apply to:** `integration_example_zero_leaks()`
```rust
let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
    .unwrap_or_else(|| panic!("{fixture}: no verb_gc_live line in stdout:\n{stdout}"));
assert_eq!(live_line, "verb_gc_live=0", "{fixture}: leaked heap objects:\n{stdout}");
```

### `.verb` example file header/banner convention
**Source:** `examples/files.verb:1-9`, `examples/sockets.verb:1-9`
**Apply to:** both new example files
```verb
%% <filename> — <one-line purpose>
!?!
  Build (imports need `verb build`, not `verb run` — see README.md)
  and run:

    verb build examples/<name>.verb -o <name>_demo
    ./<name>_demo
!?!
```

## No Analog Found

None — every file in scope has at least a role-match analog. The two doc edits are direct text corrections with no code-pattern equivalent (not applicable, not missing).

## Metadata

**Analog search scope:** `tests/e2e.rs`, `tests/fixtures/`, `tests/fixtures/cpp/`, `examples/`, `docs/superpowers/specs/`, `docs/superpowers/plans/`
**Files scanned:** `tests/e2e.rs` (912 lines, targeted reads), `tests/fixtures/gc_stress_all_kinds.verb` (14 lines), `tests/fixtures/cpp/mathlib.cpp` (29 lines), `tests/fixtures/import_mathlib.verb` (7 lines), `examples/files.verb`, `examples/sockets.verb`, `docs/superpowers/specs/2026-07-21-arrays-design.md` (grep only), `docs/superpowers/plans/2026-07-21-arrays.md` (grep only)
**Pattern extraction date:** 2026-07-21
</content>
