# Testing Patterns

**Analysis Date:** 2026-07-21

## Test Framework

**Runner:**
- Native Rust test framework via `#[test]` attribute
- Executed with `cargo test`
- Config: `Cargo.toml` defines test dependencies (none external)

**Assertion Library:**
- Standard Rust assertions: `assert!()`, `assert_eq!()`, `assert_ne!()`
- Assertions include custom messages: `assert!(condition, "message with context {}")`

**Run Commands:**
```bash
cargo test                    # Run all tests
cargo test -- --nocapture    # Show println!/eprintln! output
cargo test <test_name>        # Run specific test
cargo test <pattern>          # Run tests matching pattern
```

## Test File Organization

**Location:**
- Inline unit tests: `#[cfg(test)] mod tests { #[test] fn test_name() { } }` in source files
- Integration tests: Separate files in `tests/` directory
- Test fixtures: Reference files in `tests/fixtures/` directory

**Naming:**
- Test functions: `snake_case` with descriptive names
- Fixture files: `{name}.verb` (source) paired with `{name}.expected` (expected output)
- Test modules: `mod tests` for inline tests

**Structure:**
```
tests/
├── e2e.rs                          # End-to-end compilation and execution tests
├── parser_recovery_fuzz.rs         # Fuzz testing for parser robustness
├── formatter_roundtrip.rs          # Formatter idempotence tests
├── verb_export_macro.rs            # Macro export tests
└── fixtures/
    ├── literals.verb               # Test program source
    ├── literals.expected           # Expected stdout output
    ├── arrays_literal.verb
    ├── arrays_literal.expected
    ├── gc_arrays_regrow.verb
    ├── gc_arrays_regrow.expected
    └── ... (60+ fixture pairs)
```

## Test Structure

**Suite Organization:**
```rust
// src/error.rs - Inline unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_file_sets_file() {
        let e = CompileError::new("boom", 1, 2).with_file("a.verb");
        assert_eq!(e.file, Some("a.verb".to_string()));
    }

    #[test]
    fn new_leaves_file_unset() {
        let e = CompileError::new("boom", 1, 2);
        assert_eq!(e.file, None);
    }
}

// tests/formatter_roundtrip.rs - Integration test module
use verb::{formatter, lexer, parser};

const VALID_FIXTURES: &[&str] = &[
    "arith", "control", "declare", "functions", "literals", "strings", "vars",
];

fn assert_roundtrips(path: &str) {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    // ... test logic
}

#[test]
fn fixtures_roundtrip() {
    for name in VALID_FIXTURES {
        assert_roundtrips(&format!("tests/fixtures/{name}.verb"));
    }
}
```

**Patterns:**
- Unit tests in `#[cfg(test)] mod tests` blocks within source files
- Integration tests as separate `.rs` files in `tests/` directory
- Test helper functions extracted for reuse
- Descriptive test function names that explain what is being tested

## Mocking

**Framework:** No external mocking library used (no mock crate dependency)

**Patterns:**
- Direct function calls with test data
- Process mocking via `std::process::Command` for binary testing
- Test fixtures (`.verb` files) provide input data
- Expected output files (`.expected`) provide oracle results

**What to Mock:**
- External processes: Use `Command::new()` to invoke compiler binary with test arguments
- File I/O: Use `std::fs::read_to_string()` and `std::fs::create_dir_all()`
- Temporary files: Use `std::env::temp_dir()` for safe test isolation

**What NOT to Mock:**
- Compiler functions: Call actual parser, lexer, formatter directly
- AST construction: Use real AST nodes, not mocks
- Error types: Use actual `CompileError` from production code

## Fixtures and Factories

**Test Data:**
```rust
// tests/e2e.rs - Test fixtures are .verb files
// Example fixture: tests/fixtures/literals.verb
// Invoked via helper functions:
fn run_ok(name: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert!(out.status.success(), ...);
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn literals() { run_ok("literals"); }
```

**Location:**
- Fixture source files: `tests/fixtures/{name}.verb`
- Expected output files: `tests/fixtures/{name}.expected`
- C++ test libraries: `tests/fixtures/cpp/mathlib.cpp`

## Coverage

**Requirements:** No formal coverage target enforced

**View Coverage:**
```bash
cargo tarpaulin          # If tarpaulin is installed
cargo llvm-cov           # If llvm-cov is installed
```

**Gaps in Coverage:**
- Limited line-level coverage metrics
- Comprehensive functional coverage via E2E tests
- GC behavior verified by `assert_no_leaks()` helper across many fixtures

## Test Types

**Unit Tests:**
- Scope: Individual functions and small modules
- Approach: Inline tests in source files (e.g., `src/error.rs`)
- Example: Error builder pattern validation, token description generation
- Location: `#[cfg(test)] mod tests` blocks in source files

**Integration Tests:**
- Scope: Compiler subsystems and complete compilation pipeline
- Approach: Call public library functions directly or invoke binary via `Command`
- Examples:
  - `tests/formatter_roundtrip.rs` - Tests formatter idempotence
  - `tests/verb_export_macro.rs` - Tests macro expansion
  - `tests/parser_recovery_fuzz.rs` - Tests parser error recovery robustness
  - `tests/e2e.rs` - Tests end-to-end compilation, execution, and correctness

**E2E Tests:**
- Framework: `std::process::Command` to run compiled `verb` binary
- Scope: Full compilation pipeline from source to execution
- Testing approach:
  1. Build fixture with `verb build` command
  2. Execute resulting binary or run with `verb run`
  3. Compare output against expected results
  4. Verify error messages and exit codes

## Common Patterns

**Async Testing:**
- Not applicable (no async in this compiler)
- Threading used only for timeout testing in parser fuzz tests

**Error Testing:**
```rust
// tests/e2e.rs - Compile-time error testing
fn compile_err(name: &str, msgs: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    for msg in msgs {
        assert!(stderr.contains(msg), "missing {msg:?} in stderr:\n{stderr}");
    }
}

#[test]
fn syntax_error_shows_found_token_and_caret() {
    compile_err("err_syntax", &[
        "expected ')', found ';'",
        "print(x;",   // source line echoed
        "^",          // caret marker
    ]);
}

// Runtime error testing
fn run_err(name: &str, msg: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains(msg));
}
```

**GC/Memory Testing:**
```rust
// tests/e2e.rs - Custom leak detection helper
fn assert_no_leaks(fixture: &str) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_v2_{fixture}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", &format!("tests/fixtures/{fixture}.verb"), "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success());

    let run = Command::new(&out_path).env("VERB_GC_DEBUG", "1").output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("no verb_gc_live line"));
    assert_eq!(live_line, "verb_gc_live=0");
}

#[test]
fn gc_no_leaks_across_all_heap_kinds() {
    for fixture in [
        "strings", "closures", "arrays_literal", "arrays_get_set", "arrays_push_pop",
        "arrays_of_arrays", "arrays_of_closures", "std_map_basic",
        "gc_reassign_and_or", "gc_global_reassign", "gc_early_return_nested",
        "gc_arrays_nested", "gc_arrays_of_closures", "gc_arrays_regrow",
        "gc_map_heap_values", "gc_std_io_file_roundtrip",
    ] {
        assert_no_leaks(fixture);
    }
}
```

**Fuzz Testing:**
```rust
// tests/parser_recovery_fuzz.rs - Robustness testing with mutations
const DEADLINE: Duration = Duration::from_secs(2);

fn assert_terminates(src: &str, label: &str) {
    let src = src.to_string();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let toks = match lexer::lex(&src) {
            Ok(t) => t,
            Err(_) => return,
        };
        let _ = tx.send(parser::parse_recovering(toks));
    });
    match rx.recv_timeout(DEADLINE) {
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("parse_recovering did not terminate within {DEADLINE:?} on {label}");
        }
        _ => {}
    }
}

#[test]
fn recovering_parse_terminates_with_any_single_token_deleted() {
    for (name, src) in fixture_sources() {
        let spans = token_spans(&src);
        for (idx, &(start, end)) in spans.iter().enumerate() {
            let mut mutated = String::with_capacity(src.len());
            mutated.push_str(&src[..start]);
            mutated.push_str(&src[end..]);
            assert_terminates(&mutated, &format!("{name} (token #{idx} deleted)"));
        }
    }
}
```

**IR Generation Testing:**
```rust
// tests/e2e.rs - Verify generated LLVM IR
#[test]
fn array_literal_emits_malloc_and_store_in_ir() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/arrays_literal.verb", "--emit-llvm"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("call ptr @malloc"), "no malloc call in IR:\n{ir}");
    assert!(ir.contains("@verb_print_value"), "no verb_print_value in IR:\n{ir}");
}

#[test]
fn gc_retain_release_functions_are_emitted() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    for sym in ["@verb_retain_value", "@verb_release_value", "@verb_retain_cell", "@verb_release_cell"] {
        assert!(ir.contains(sym), "missing {sym} in IR:\n{ir}");
    }
}
```

**Cross-compilation Testing:**
```rust
// tests/e2e.rs - Test multiple target compilation
fn zig_available() -> bool {
    Command::new("zig").arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn aot_cross_build_produces_binary_for_each_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    for label in ["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64", "windows-x86_64", "windows-arm64"] {
        let bin = dir.join(format!("functions_{label}"));
        let out = Command::new(env!("CARGO_BIN_EXE_verb"))
            .args(["build", "tests/fixtures/functions.verb", "-o", bin.to_str().unwrap(), "--target", label])
            .output()
            .unwrap();
        assert!(out.status.success(), "target {label} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
}
```

## Test Statistics

**Test Count:** 80+ test functions across unit and integration tests

**Coverage Areas:**
- Lexer/Parser: Error recovery, syntax validation, keyword migrations
- Code generation: LLVM IR emission, symbol resolution
- Compiler CLI: Build modes, target selection, error handling
- Runtime: Array operations, closures, control flow, string operations
- Memory management: GC verification, leak detection across all heap types
- External imports: C++ FFI, std library integrations
- Cross-compilation: Multi-target binary generation

---

*Testing analysis: 2026-07-21*
