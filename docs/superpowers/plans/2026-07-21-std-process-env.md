# std env / std process Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `import std env` (getenv/setenv/unsetenv), `import std process` (cwd/exe_path/spawn/wait), and three core builtins (`exit`, `abort`, `get_pid`) to the Verb compiler, per `docs/superpowers/specs/2026-07-21-std-process-env-design.md`.

**Architecture:** Two new build-only stdlib modules follow the exact `std io`/`std map` recipe (parser accepts the module name → codegen resolves calls via a static arity table → a new `runtime/*.cpp` unit is compiled and linked only when imported). The three core builtins follow the `verb_map.cpp` "always referenced" recipe instead (new `runtime/verb_builtins.cpp`, registered in both `build.rs`, for JIT, and unconditionally in every AOT link, since any program can call them without an import).

**Tech Stack:** Rust 2021 + inkwell/LLVM (codegen), C++17 runtime units built via `cc`/`zig c++`.

## Global Constraints

- Failure convention: every new runtime function returns `verb_nil()` (or `verb_bool(0)`) on failure — no C++ exception ever crosses the `extern "C"` boundary. Copied from the spec's "sentinel return, caller checks" decision (D-08).
- `spawn(cmd, args)`: `args` is a Verb array of strings, never a shell string — no shell is invoked (spec: spawn/wait API shape).
- `wait(pid)` returns exit code only, no captured stdout/stderr (spec: D-07).
- `exit`/`abort` skip GC/refcount cleanup entirely — matches C `exit`/`abort` semantics exactly (spec: D-09/D-10).
- `import std env;` / `import std process;` are build-only: `verb run` must reject them (this already happens for free — `src/main.rs`'s existing `!std_imports.is_empty()` check covers any new module name with no code change).
- `exit`/`abort`/`get_pid` are core builtins: never gated behind `std_imports`, and must work under `verb run` (JIT) exactly like `print` does (spec: D-02/D-03).

---

### Task 1: Parser accepts `import std env;` / `import std process;`

**Files:**
- Modify: `src/parser.rs:149-163` (the `import_stmt` method's known-module check)
- Test: `src/parser.rs` (inline `#[cfg(test)] mod tests` block, alongside the existing `parses_std_io_import`/`parses_std_map_import`/`unknown_std_module_is_a_compile_error` tests)

**Interfaces:**
- Consumes: nothing new — `ImportStmt::Std(String)` already exists.
- Produces: `Parser::import_stmt` accepts `"env"` and `"process"` as valid names after `std`, in addition to the existing `"io"` and `"map"`. `Program.std_imports` can now contain `"env"`/`"process"` — Task 2/3 consume this.

- [ ] **Step 1: Write the failing tests**

Add to `src/parser.rs`'s `#[cfg(test)] mod tests` block (near `parses_std_map_import`):

```rust
    #[test]
    fn parses_std_env_import() {
        let p = parse(lex("import std env;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["env".to_string()]);
        assert!(p.imports.is_empty());
    }

    #[test]
    fn parses_std_process_import() {
        let p = parse(lex("import std process;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["process".to_string()]);
        assert!(p.imports.is_empty());
    }
```

And update the existing `unknown_std_module_is_a_compile_error` test's assertions to match the grown module list:

```rust
    #[test]
    fn unknown_std_module_is_a_compile_error() {
        let err = parse(lex("import std vector;").unwrap()).unwrap_err();
        assert!(err.msg.contains("unknown std module 'vector'"), "{}", err.msg);
        assert!(err.msg.contains("io"), "{}", err.msg);
        assert!(err.msg.contains("map"), "{}", err.msg);
        assert!(err.msg.contains("env"), "{}", err.msg);
        assert!(err.msg.contains("process"), "{}", err.msg);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib parses_std_env_import parses_std_process_import unknown_std_module_is_a_compile_error`
Expected: `parses_std_env_import` and `parses_std_process_import` FAIL (`unknown std module 'env'`/`'process'`); `unknown_std_module_is_a_compile_error` FAILS (message doesn't contain "env"/"process" yet).

- [ ] **Step 3: Implement**

In `src/parser.rs`, replace:

```rust
        if name != "io" && name != "map" {
            return Err(CompileError::new(
                format!("unknown std module '{name}' (known std modules: io, map)"),
                l, c,
            ));
        }
```

with:

```rust
        if name != "io" && name != "map" && name != "env" && name != "process" {
            return Err(CompileError::new(
                format!(
                    "unknown std module '{name}' (known std modules: io, map, env, process)"
                ),
                l, c,
            ));
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib parses_std_env_import parses_std_process_import unknown_std_module_is_a_compile_error parses_std_io_import parses_std_map_import`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/parser.rs
git commit -m "feat(parser): accept 'import std env;' and 'import std process;'"
```

---

### Task 2: Codegen resolves `std env`/`std process` calls by arity

**Files:**
- Modify: `src/codegen.rs:2114-2144` (add two tables next to `IO_FUNCS`/`MAP_FUNCS`), and `src/codegen.rs:1966-1980` (`gen_call`'s `std_imports`-gated tiers)
- Test: `src/codegen.rs` (find or add a `#[cfg(test)] mod tests` block in this file — if none exists yet, add one at the bottom of the file, matching `parser.rs`'s style)

**Interfaces:**
- Consumes: `Codegen.std_imports: Vec<String>` (already exists), `Codegen::gen_std_io_call` (existing method, generic despite its name — declares an extern by arity once, calls it, releases args).
- Produces: `ENV_FUNCS`/`PROCESS_FUNCS: &[(&str, usize)]` and `env_func_arity`/`process_func_arity(name: &str) -> Option<usize>` — Task 5/9/10's runtime `.cpp` files must define exactly these names with exactly these arities: `getenv`(1), `setenv`(2), `unsetenv`(1), `cwd`(0), `exe_path`(0), `spawn`(2), `wait`(1).

- [ ] **Step 1: Write the failing test**

Check whether `src/codegen.rs` already has a `#[cfg(test)] mod tests` block:

Run: `grep -n "mod tests" src/codegen.rs`

If it exists, add the test below inside it; if not, add this whole block at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;
    use crate::parser::parse;

    fn compile_err_msg(src: &str) -> String {
        let prog = parse(lex(src).unwrap()).unwrap();
        let ctx = inkwell::context::Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmt_files = vec!["t.verb".to_string(); prog.body.len()];
        cg.compile_program(&prog.body, &stmt_files, &prog.imports, &prog.std_imports)
            .unwrap_err()
            .msg
    }

    #[test]
    fn std_env_arity_mismatch_is_a_compile_error() {
        let msg = compile_err_msg("import std env; print(getenv(\"A\", \"B\"));");
        assert!(msg.contains("takes 1 argument"), "{msg}");
    }

    #[test]
    fn std_process_arity_mismatch_is_a_compile_error() {
        let msg = compile_err_msg("import std process; print(wait());");
        assert!(msg.contains("takes 1 argument"), "{msg}");
    }
}
```

(If `mod tests` already exists with a different helper for compiling-and-expecting-an-error, reuse that helper instead of adding `compile_err_msg` — check for one calling `compile_program` and asserting on `.unwrap_err()` before adding a duplicate.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib std_env_arity_mismatch_is_a_compile_error std_process_arity_mismatch_is_a_compile_error`
Expected: FAIL — `getenv`/`wait` are undefined (no `std env`/`std process` resolution tier exists yet), so the error message will be an "undefined variable" error instead, not an arity error.

- [ ] **Step 3: Implement — arity tables**

In `src/codegen.rs`, right after `MAP_FUNCS`/`map_func_arity` (i.e. after line 2144 in the pre-change file), add:

```rust
/// Fixed name -> arity table for the `env` module's built-in functions
/// (see runtime/verb_env.cpp and the design spec). See `IO_FUNCS`.
const ENV_FUNCS: &[(&str, usize)] = &[
    ("getenv", 1),
    ("setenv", 2),
    ("unsetenv", 1),
];

fn env_func_arity(name: &str) -> Option<usize> {
    ENV_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// Fixed name -> arity table for the `process` module's built-in functions
/// (see runtime/verb_process.cpp and the design spec). See `IO_FUNCS`.
const PROCESS_FUNCS: &[(&str, usize)] = &[
    ("cwd", 0),
    ("exe_path", 0),
    ("spawn", 2),
    ("wait", 1),
];

fn process_func_arity(name: &str) -> Option<usize> {
    PROCESS_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}
```

- [ ] **Step 4: Implement — wire into `gen_call`**

In `src/codegen.rs`'s `gen_call`, immediately after the existing `map` tier (right after this block, which ends around line 1976):

```rust
            if !is_bound && self.std_imports.iter().any(|m| m == "map") {
                if let Some(arity) = map_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, line, col);
                }
            }
```

insert:

```rust
            if !is_bound && self.std_imports.iter().any(|m| m == "env") {
                if let Some(arity) = env_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, line, col);
                }
            }
            if !is_bound && self.std_imports.iter().any(|m| m == "process") {
                if let Some(arity) = process_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, line, col);
                }
            }
```

(This must come before the `if !is_bound && !self.imports.is_empty()` generic-extern fallback that follows.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib std_env_arity_mismatch_is_a_compile_error std_process_arity_mismatch_is_a_compile_error`
Expected: PASS.

- [ ] **Step 6: Run the full test suite to check nothing broke**

Run: `cargo test --lib`
Expected: all existing tests still PASS.

- [ ] **Step 7: Commit**

```bash
git add src/codegen.rs
git commit -m "feat(codegen): resolve 'std env'/'std process' calls by arity"
```

---

### Task 3: Codegen resolves `exit`/`abort`/`get_pid` as core builtins

**Files:**
- Modify: `src/codegen.rs:1891-1980` (`gen_call`, the hardcoded-name tier alongside `print`/`len`/`get`/`set`/`push`/`pop`)
- Test: same `mod tests` block as Task 2

**Interfaces:**
- Consumes: `Codegen::call_named` (existing helper).
- Produces: three externs, lazily declared in `self.externs` exactly like `gen_std_io_call` does, named `builtin_exit` (1 `VerbValue` arg, returns `VerbValue`), `builtin_abort` (0 args, returns `VerbValue`), `builtin_get_pid` (0 args, returns `VerbValue`). Task 6 must define exactly these three `extern "C"` names in `runtime/verb_builtins.cpp`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block from Task 2:

```rust
    #[test]
    fn exit_abort_get_pid_need_no_import() {
        // No `import std ...` at all -- must still compile.
        let prog = parse(lex("exit(0);").unwrap()).unwrap();
        let ctx = inkwell::context::Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmt_files = vec!["t.verb".to_string(); prog.body.len()];
        assert!(cg.compile_program(&prog.body, &stmt_files, &prog.imports, &prog.std_imports).is_ok());
    }

    #[test]
    fn get_pid_and_abort_compile_with_no_import() {
        let src = "print(get_pid()); check false begin abort(); end";
        let prog = parse(lex(src).unwrap()).unwrap();
        let ctx = inkwell::context::Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmt_files = vec!["t.verb".to_string(); prog.body.len()];
        assert!(cg.compile_program(&prog.body, &stmt_files, &prog.imports, &prog.std_imports).is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib exit_abort_get_pid_need_no_import get_pid_and_abort_compile_with_no_import`
Expected: FAIL — `exit`/`abort`/`get_pid` are undefined variables today.

- [ ] **Step 3: Implement**

In `src/codegen.rs`'s `gen_call`, add three new hardcoded-name blocks right after the existing `pop` block (before the `let is_bound = self.lookup(name).is_some();` line):

```rust
            if name == "exit" {
                if args.len() != 1 {
                    return Err(CompileError::new("exit takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                let fnv = match self.externs.get("builtin_exit").copied() {
                    Some(fnv) => fnv,
                    None => {
                        let fnty = self.value_ty.fn_type(&[self.value_ty.into()], false);
                        let fnv = self.module.add_function("builtin_exit", fnty, None);
                        self.externs.insert("builtin_exit".to_string(), fnv);
                        fnv
                    }
                };
                let rv = self.builder.build_call(fnv, &[v.into()], "exit_call")
                    .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(rv);
            }
            if name == "abort" {
                if !args.is_empty() {
                    return Err(CompileError::new("abort takes no arguments", line, col));
                }
                let fnv = match self.externs.get("builtin_abort").copied() {
                    Some(fnv) => fnv,
                    None => {
                        let fnty = self.value_ty.fn_type(&[], false);
                        let fnv = self.module.add_function("builtin_abort", fnty, None);
                        self.externs.insert("builtin_abort".to_string(), fnv);
                        fnv
                    }
                };
                let rv = self.builder.build_call(fnv, &[], "abort_call")
                    .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
                return Ok(rv);
            }
            if name == "get_pid" {
                if !args.is_empty() {
                    return Err(CompileError::new("get_pid takes no arguments", line, col));
                }
                let fnv = match self.externs.get("builtin_get_pid").copied() {
                    Some(fnv) => fnv,
                    None => {
                        let fnty = self.value_ty.fn_type(&[], false);
                        let fnv = self.module.add_function("builtin_get_pid", fnty, None);
                        self.externs.insert("builtin_get_pid".to_string(), fnv);
                        fnv
                    }
                };
                let rv = self.builder.build_call(fnv, &[], "get_pid_call")
                    .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
                return Ok(rv);
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib exit_abort_get_pid_need_no_import get_pid_and_abort_compile_with_no_import`
Expected: PASS.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --lib`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs
git commit -m "feat(codegen): resolve exit/abort/get_pid as core builtins"
```

---

### Task 4: `runtime/verb_env.cpp`

**Files:**
- Create: `runtime/verb_env.cpp`
- Test: `tests/e2e.rs` (a `verb_env_cpp_compiles_standalone` test, modeled on `verb_std_io_cpp_compiles_standalone` at `tests/e2e.rs:425-438`)

**Interfaces:**
- Consumes: `runtime/verb.h`'s `VerbValue`/`verb_nil`/`verb_bool`/`verb_string`/`verb_as_string`, and `verb_alloc` (extern, defined by codegen).
- Produces: `extern "C" VerbValue getenv(VerbValue name)`, `extern "C" VerbValue setenv(VerbValue name, VerbValue value)`, `extern "C" VerbValue unsetenv(VerbValue name)` — must match `ENV_FUNCS`'s arities from Task 2 exactly.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` (near `verb_std_io_cpp_compiles_standalone`):

```rust
#[test]
fn verb_env_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_env_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_env.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_env.cpp");
    assert!(status.success(), "runtime/verb_env.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e verb_env_cpp_compiles_standalone`
Expected: FAIL — `runtime/verb_env.cpp` doesn't exist yet.

- [ ] **Step 3: Implement**

Create `runtime/verb_env.cpp`:

```cpp
// Built-in bindings for `import std env;` -- getenv/setenv/unsetenv.
// Compiled and linked in automatically by `verb build`/`compile` whenever
// a program uses `import std env;`. Mirrors runtime/verb_std_io.cpp's
// shape: build-only, never linked into `verb run` (JIT).
#include "verb.h"

#include <cstdlib>
#include <cstring>
#include <string>

#ifdef _WIN32
#include <stdlib.h> // _putenv_s / _dupenv_s
#endif

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue getenv(VerbValue name) {
    if (name.tag != VERB_STRING) return verb_nil();
#ifdef _WIN32
    char* buf = nullptr;
    size_t len = 0;
    if (_dupenv_s(&buf, &len, verb_as_string(name)) != 0 || !buf) return verb_nil();
    VerbValue v = verb_string_from(std::string(buf, len > 0 ? len - 1 : 0));
    free(buf);
    return v;
#else
    const char* v = std::getenv(verb_as_string(name));
    if (!v) return verb_nil();
    return verb_string_from(v);
#endif
}

extern "C" VerbValue setenv(VerbValue name, VerbValue value) {
    if (name.tag != VERB_STRING || value.tag != VERB_STRING) return verb_bool(0);
#ifdef _WIN32
    bool ok = _putenv_s(verb_as_string(name), verb_as_string(value)) == 0;
#else
    bool ok = ::setenv(verb_as_string(name), verb_as_string(value), 1) == 0;
#endif
    return verb_bool(ok);
}

extern "C" VerbValue unsetenv(VerbValue name) {
    if (name.tag != VERB_STRING) return verb_bool(0);
#ifdef _WIN32
    bool ok = _putenv_s(verb_as_string(name), "") == 0;
#else
    bool ok = ::unsetenv(verb_as_string(name)) == 0;
#endif
    return verb_bool(ok);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e verb_env_cpp_compiles_standalone`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add runtime/verb_env.cpp tests/e2e.rs
git commit -m "feat(runtime): add verb_env.cpp (getenv/setenv/unsetenv)"
```

---

### Task 5: `runtime/verb_builtins.cpp` (exit/abort/get_pid)

**Files:**
- Create: `runtime/verb_builtins.cpp`
- Test: `tests/e2e.rs` (a `verb_builtins_cpp_compiles_standalone` test)

**Interfaces:**
- Consumes: `runtime/verb.h`'s `VerbValue`/`verb_int`/`verb_as_int`.
- Produces: `extern "C" VerbValue builtin_exit(VerbValue code)`, `extern "C" VerbValue builtin_abort()`, `extern "C" VerbValue builtin_get_pid()` — must match the extern names Task 3's codegen changes declare exactly.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs`:

```rust
#[test]
fn verb_builtins_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_builtins_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_builtins.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_builtins.cpp");
    assert!(status.success(), "runtime/verb_builtins.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e verb_builtins_cpp_compiles_standalone`
Expected: FAIL — file doesn't exist.

- [ ] **Step 3: Implement**

Create `runtime/verb_builtins.cpp`:

```cpp
// Core builtins that need no `import`: exit, abort, get_pid. Unlike
// verb_std_io.cpp/verb_env.cpp/verb_process.cpp (build-only, linked only
// when their `std` module is imported), this unit is *always* linked --
// see build.rs and src/main.rs's build_aot_host/build_aot_cross, which
// treat it the same way they already treat verb_map.cpp -- and always
// compiled into the `verb` binary itself so `verb run` (JIT) can resolve
// these symbols too, since exit/abort/get_pid must work without any
// import, exactly like `print` does.
#include "verb.h"

#include <cstdlib>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

extern "C" VerbValue builtin_exit(VerbValue code) {
    // Deliberately skips GC/refcount cleanup -- matches C's exit()
    // semantics exactly (see design spec, D-09). Never returns.
    std::exit(static_cast<int>(verb_as_int(code)));
}

extern "C" VerbValue builtin_abort() {
    // Hard SIGABRT-style crash, not a "friendly" Verb-level panic
    // (design spec, D-10). Never returns.
    std::abort();
}

extern "C" VerbValue builtin_get_pid() {
#ifdef _WIN32
    return verb_int(static_cast<int64_t>(GetCurrentProcessId()));
#else
    return verb_int(static_cast<int64_t>(getpid()));
#endif
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e verb_builtins_cpp_compiles_standalone`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add runtime/verb_builtins.cpp tests/e2e.rs
git commit -m "feat(runtime): add verb_builtins.cpp (exit/abort/get_pid)"
```

---

### Task 6: Wire `verb_builtins.cpp` into `build.rs` + JIT symbol registration

**Files:**
- Modify: `build.rs`
- Modify: `src/main.rs:23-75` (the `VerbValueAbi`/`extern "C"` block and `register_jit_runtime_symbols`)
- Test: `tests/e2e.rs` (JIT works for exit/abort/get_pid — this test is written now but will only pass once Task 8 also makes `build_aot_host` link `verb_builtins.cpp`; see note in Step 4)

**Interfaces:**
- Consumes: `runtime/verb_builtins.cpp`'s three `extern "C"` functions from Task 5.
- Produces: `verb_builtins.cpp` compiled into the `verb` binary itself (`cc::Build` in `build.rs`), and its three symbols registered with `add_global_mapping` so MCJIT can resolve calls to them from a JIT'd module.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` (near the top-level tests, e.g. after `literals`):

```rust
#[test]
fn get_pid_works_under_jit_with_no_import() {
    run_ok("core_builtins_get_pid");
}
```

Create `tests/fixtures/core_builtins_get_pid.verb`:

```
assign pid get_pid();
check pid beats 0 begin
  print("pid ok");
end orelse begin
  print("pid bad");
end
```

Create `tests/fixtures/core_builtins_get_pid.expected`:

```
pid ok
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e get_pid_works_under_jit_with_no_import`
Expected: FAIL (link/JIT error resolving `builtin_get_pid`) — `verb_builtins.cpp` isn't compiled into the `verb` binary yet.

- [ ] **Step 3: Implement — `build.rs`**

In `build.rs`, change:

```rust
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("runtime");
    let map_cpp = runtime.join("verb_map.cpp");

    println!("cargo:rerun-if-changed={}", map_cpp.display());
    println!("cargo:rerun-if-changed={}", runtime.join("verb.h").display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&runtime)
        .file(&map_cpp)
        .compile("verb_runtime");
```

to:

```rust
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("runtime");
    let map_cpp = runtime.join("verb_map.cpp");
    let builtins_cpp = runtime.join("verb_builtins.cpp");

    println!("cargo:rerun-if-changed={}", map_cpp.display());
    println!("cargo:rerun-if-changed={}", builtins_cpp.display());
    println!("cargo:rerun-if-changed={}", runtime.join("verb.h").display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&runtime)
        .file(&map_cpp)
        .file(&builtins_cpp)
        .compile("verb_runtime");
```

- [ ] **Step 4: Implement — `src/main.rs`**

In `src/main.rs`, change the `extern "C"` block:

```rust
extern "C" {
    /// Defined in `runtime/verb_map.cpp`, compiled into this binary by build.rs.
    fn verb_map_destroy_contents(payload: *mut std::ffi::c_void);
}
```

to:

```rust
extern "C" {
    /// Defined in `runtime/verb_map.cpp`, compiled into this binary by build.rs.
    fn verb_map_destroy_contents(payload: *mut std::ffi::c_void);
    /// Defined in `runtime/verb_builtins.cpp`, compiled into this binary by build.rs.
    fn builtin_exit(code: VerbValueAbi) -> VerbValueAbi;
    fn builtin_abort() -> VerbValueAbi;
    fn builtin_get_pid() -> VerbValueAbi;
}
```

`VerbValueAbi` needs `#[derive(Clone, Copy)]` for this (a function pointer cast below needs it to be `Copy`) — change:

```rust
#[repr(C)]
pub struct VerbValueAbi {
    pub tag: i8,
    pub payload: i64,
}
```

to:

```rust
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VerbValueAbi {
    pub tag: i8,
    pub payload: i64,
}
```

Then change `register_jit_runtime_symbols`:

```rust
fn register_jit_runtime_symbols<'ctx>(
    ee: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &inkwell::module::Module<'ctx>,
) {
    let symbols: [(&str, usize); 1] =
        [("verb_map_destroy_contents", verb_map_destroy_contents as *const () as usize)];
    for (name, addr) in symbols {
        if let Some(f) = module.get_function(name) {
            ee.add_global_mapping(&f, addr);
        }
    }
}
```

to:

```rust
fn register_jit_runtime_symbols<'ctx>(
    ee: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &inkwell::module::Module<'ctx>,
) {
    let symbols: [(&str, usize); 4] = [
        ("verb_map_destroy_contents", verb_map_destroy_contents as *const () as usize),
        ("builtin_exit", builtin_exit as *const () as usize),
        ("builtin_abort", builtin_abort as *const () as usize),
        ("builtin_get_pid", builtin_get_pid as *const () as usize),
    ];
    for (name, addr) in symbols {
        if let Some(f) = module.get_function(name) {
            ee.add_global_mapping(&f, addr);
        }
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test e2e get_pid_works_under_jit_with_no_import`
Expected: PASS. (If it still fails with a link error for the `verb` binary itself — not the test — rerun `cargo build` first to force `build.rs` to pick up the new file; `cargo test` should trigger this automatically via the `rerun-if-changed` lines.)

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add build.rs src/main.rs tests/e2e.rs tests/fixtures/core_builtins_get_pid.verb tests/fixtures/core_builtins_get_pid.expected
git commit -m "feat(runtime): compile verb_builtins.cpp into verb binary, register JIT symbols"
```

---

### Task 7: AOT build links `verb_env.cpp` / `verb_process.cpp` (conditionally) and `verb_builtins.cpp` (always)

**Files:**
- Modify: `src/main.rs:270-456` (`RUNTIME_DIR`/`STD_IO_CPP`/`MAP_CPP` consts, `compile_std_io_obj`/`compile_map_obj`, `build_aot_host`, `build_aot_cross`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `runtime/verb_env.cpp`, `runtime/verb_process.cpp` (stub — Task 8 fills in `cwd`/`exe_path`; this task only needs the file to exist and compile, so create a minimal stub here if Task 8 hasn't run yet — but per plan ordering, do this task *after* Task 8/9/10 land the real file; see Task ordering note below), `runtime/verb_builtins.cpp` (Task 5).
- Produces: AOT binaries that link successfully when a program imports `std env`/`std process`, and link `verb_builtins.cpp` into every AOT binary unconditionally (mirrors `verb_map.cpp`'s existing "always linked" treatment).

**Task ordering note:** Do Task 7 *after* Task 9 (POSIX `spawn`/`wait`) so `runtime/verb_process.cpp` already exists with real content when this task's tests link it. If executing tasks strictly in file order, skip ahead to Task 9 first, then return here.

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` fixtures + test:

Create `tests/fixtures/std_env_roundtrip.verb`:

```
import std env;

setenv("VERB_E2E_TEST_VAR", "hello");
print(getenv("VERB_E2E_TEST_VAR"));
unsetenv("VERB_E2E_TEST_VAR");
assign gone getenv("VERB_E2E_TEST_VAR");
check gone eq nil begin
  print("gone");
end
```

Create `tests/fixtures/std_env_roundtrip.expected`:

```
hello
gone
```

Add to `tests/e2e.rs` (near `build_links_and_runs_a_program_using_std_io_files`):

```rust
#[test]
fn build_links_and_runs_a_program_using_std_env() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_env_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_env_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_env_roundtrip.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn run_rejects_programs_with_std_env_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_env_roundtrip.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std env"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e build_links_and_runs_a_program_using_std_env run_rejects_programs_with_std_env_import`
Expected: `run_rejects_programs_with_std_env_import` PASSES already (generic rejection needs no new code — confirms the "no new work needed" claim from the spec). `build_links_and_runs_a_program_using_std_env` FAILS (link error — `verb_env.cpp` never gets compiled/linked by `verb build`).

- [ ] **Step 3: Implement**

In `src/main.rs`, change the consts:

```rust
const RUNTIME_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime");
const STD_IO_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_io.cpp");
const MAP_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_map.cpp");
```

to:

```rust
const RUNTIME_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime");
const STD_IO_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_io.cpp");
const MAP_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_map.cpp");
const ENV_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_env.cpp");
const PROCESS_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_process.cpp");
const BUILTINS_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_builtins.cpp");
```

Add two new compile-step functions right after `compile_map_obj`:

```rust
/// Compiles the bundled `runtime/verb_env.cpp` into an object file. See
/// `compile_std_io_obj`.
fn compile_env_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_env_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", ENV_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {ENV_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {ENV_CPP}"));
    }
    Ok(obj)
}

/// Compiles the bundled `runtime/verb_process.cpp` into an object file. See
/// `compile_std_io_obj`.
fn compile_process_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_process_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", PROCESS_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {PROCESS_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {PROCESS_CPP}"));
    }
    Ok(obj)
}

/// Compiles the bundled `runtime/verb_builtins.cpp` into an object file. See
/// `compile_std_io_obj`. Unlike that function, this is called unconditionally
/// (see `build_aot_host`) since exit/abort/get_pid need no import.
fn compile_builtins_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_builtins_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", BUILTINS_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {BUILTINS_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {BUILTINS_CPP}"));
    }
    Ok(obj)
}
```

In `build_aot_host`, change:

```rust
    let wants_std_io = std_imports.iter().any(|m| m == "io");
    // `runtime/verb_map.cpp` is now linked into every build, not just ones that
    // `import std map`: codegen's `verb_release_value` references
    // `verb_map_destroy_contents` unconditionally and nothing strips it. Since a
    // C++ translation unit's symbol is now always present, the link must always
    // go through the C++ driver — the old "cc when no imports" fast path is gone.
    let linker = "c++";

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let map_obj = compile_map_obj(linker, &[]).unwrap_or_else(|e| {
        let _ = std::fs::remove_file(&obj);
        if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
        eprintln!("error: {e}");
        exit(1);
    });

    let mut cmd = Command::new(linker);
    cmd.arg(&obj).arg("-o").arg(out);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    cmd.arg(&map_obj);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = match cmd.status() {
        Ok(status) => status,
        Err(e) => {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            let _ = std::fs::remove_file(&map_obj);
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&map_obj);
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
```

to:

```rust
    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_env = std_imports.iter().any(|m| m == "env");
    let wants_process = std_imports.iter().any(|m| m == "process");
    // `runtime/verb_map.cpp` and `runtime/verb_builtins.cpp` are linked into
    // every build, not just ones that import the corresponding `std` module:
    // codegen's `verb_release_value` references `verb_map_destroy_contents`
    // unconditionally, and `exit`/`abort`/`get_pid` need no import at all.
    // Since C++ translation units are always present, the link must always
    // go through the C++ driver — the old "cc when no imports" fast path is gone.
    let linker = "c++";

    let cleanup = |paths: &[&std::path::Path]| {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    };

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj(linker, &[]).unwrap_or_else(|e| {
            cleanup(&[obj.as_ref()]);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let env_obj = if wants_env {
        Some(compile_env_obj(linker, &[]).unwrap_or_else(|e| {
            let mut paths: Vec<&std::path::Path> = vec![obj.as_ref()];
            if let Some(p) = &std_io_obj { paths.push(p); }
            cleanup(&paths);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let process_obj = if wants_process {
        Some(compile_process_obj(linker, &[]).unwrap_or_else(|e| {
            let mut paths: Vec<&std::path::Path> = vec![obj.as_ref()];
            if let Some(p) = &std_io_obj { paths.push(p); }
            if let Some(p) = &env_obj { paths.push(p); }
            cleanup(&paths);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let map_obj = compile_map_obj(linker, &[]).unwrap_or_else(|e| {
        let mut paths: Vec<&std::path::Path> = vec![obj.as_ref()];
        if let Some(p) = &std_io_obj { paths.push(p); }
        if let Some(p) = &env_obj { paths.push(p); }
        if let Some(p) = &process_obj { paths.push(p); }
        cleanup(&paths);
        eprintln!("error: {e}");
        exit(1);
    });
    let builtins_obj = compile_builtins_obj(linker, &[]).unwrap_or_else(|e| {
        let mut paths: Vec<&std::path::Path> = vec![obj.as_ref(), map_obj.as_ref()];
        if let Some(p) = &std_io_obj { paths.push(p); }
        if let Some(p) = &env_obj { paths.push(p); }
        if let Some(p) = &process_obj { paths.push(p); }
        cleanup(&paths);
        eprintln!("error: {e}");
        exit(1);
    });

    let mut cmd = Command::new(linker);
    cmd.arg(&obj).arg("-o").arg(out);
    for p in [&std_io_obj, &env_obj, &process_obj] {
        if let Some(p) = p {
            cmd.arg(p);
        }
    }
    cmd.arg(&map_obj);
    cmd.arg(&builtins_obj);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let all_objs = || -> Vec<&std::path::Path> {
        let mut v: Vec<&std::path::Path> = vec![obj.as_ref(), map_obj.as_ref(), builtins_obj.as_ref()];
        if let Some(p) = &std_io_obj { v.push(p); }
        if let Some(p) = &env_obj { v.push(p); }
        if let Some(p) = &process_obj { v.push(p); }
        v
    };
    let status = match cmd.status() {
        Ok(status) => status,
        Err(e) => {
            cleanup(&all_objs());
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    cleanup(&all_objs());
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
```

In `build_aot_cross`, apply the analogous change — replace:

```rust
    let wants_std_io = std_imports.iter().any(|m| m == "io");
    if wants_std_io && target.is_windows() {
        return Err(
            "'import std io' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std io'".to_string(),
        );
    }
```

keep as-is (no change — this restriction stays `io`-specific), but change the object-compiling/linking section below it:

```rust
    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };
    // Always linked now — see build_aot_host for why verb_map.cpp is unconditional.
    let map_obj = compile_map_obj("zig", &["c++", "-target", target.zig_triple()])?;

    // Imports/lib_dirs are forwarded to zig c++ so cross-linking works when the imported
    // C++ libraries are available for the chosen target via -L<dir>. Host-built .o/.a
    // fixtures won't link for a foreign target — that requires target-built libraries.
    // The link always goes through `zig c++` now that a C++ unit (verb_map.cpp) is
    // always present; the old "cc when no imports" fast path is gone.
    let linker_subcmd = "c++";
    let mut cmd = Command::new("zig");
    cmd.args([linker_subcmd, "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()]);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    cmd.arg(&map_obj);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&map_obj);
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
```

to:

```rust
    let wants_env = std_imports.iter().any(|m| m == "env");
    let wants_process = std_imports.iter().any(|m| m == "process");
    let zig_args = ["c++", "-target", target.zig_triple()];

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &zig_args)?)
    } else {
        None
    };
    let env_obj = if wants_env {
        Some(compile_env_obj("zig", &zig_args)?)
    } else {
        None
    };
    let process_obj = if wants_process {
        Some(compile_process_obj("zig", &zig_args)?)
    } else {
        None
    };
    // Always linked now — see build_aot_host for why verb_map.cpp/verb_builtins.cpp
    // are unconditional.
    let map_obj = compile_map_obj("zig", &zig_args)?;
    let builtins_obj = compile_builtins_obj("zig", &zig_args)?;

    // Imports/lib_dirs are forwarded to zig c++ so cross-linking works when the imported
    // C++ libraries are available for the chosen target via -L<dir>. Host-built .o/.a
    // fixtures won't link for a foreign target — that requires target-built libraries.
    // The link always goes through `zig c++` now that a C++ unit (verb_map.cpp) is
    // always present; the old "cc when no imports" fast path is gone.
    let linker_subcmd = "c++";
    let mut cmd = Command::new("zig");
    cmd.args([linker_subcmd, "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()]);
    for p in [&std_io_obj, &env_obj, &process_obj] {
        if let Some(p) = p {
            cmd.arg(p);
        }
    }
    cmd.arg(&map_obj);
    cmd.arg(&builtins_obj);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    for p in [&std_io_obj, &env_obj, &process_obj] {
        if let Some(p) = p { let _ = std::fs::remove_file(p); }
    }
    let _ = std::fs::remove_file(&map_obj);
    let _ = std::fs::remove_file(&builtins_obj);
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e build_links_and_runs_a_program_using_std_env run_rejects_programs_with_std_env_import get_pid_works_under_jit_with_no_import`
Expected: all PASS.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: all PASS, including `aot_build_produces_working_binary`, `aot_cross_build_produces_binary_for_each_target` (skips if `zig` isn't on `PATH`), and the existing `std io` tests.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/e2e.rs tests/fixtures/std_env_roundtrip.verb tests/fixtures/std_env_roundtrip.expected
git commit -m "feat(build): link verb_env.cpp/verb_process.cpp (conditional) and verb_builtins.cpp (always) into AOT binaries"
```

---

### Task 8: `runtime/verb_process.cpp` — `cwd()` / `exe_path()`

**Files:**
- Create: `runtime/verb_process.cpp`
- Test: `tests/e2e.rs` + `tests/fixtures/std_process_cwd_exe.verb`

**Interfaces:**
- Consumes: `runtime/verb.h`.
- Produces: `extern "C" VerbValue cwd()`, `extern "C" VerbValue exe_path()` (arity 0 each, matching `PROCESS_FUNCS` from Task 2). `spawn`/`wait` are added in Tasks 9-10, appended to this same file.

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/std_process_cwd_exe.verb`:

```
import std process;

assign d cwd();
assign e exe_path();
check d differs nil begin
  check e differs nil begin
    print("both ok");
  end
end
```

Create `tests/fixtures/std_process_cwd_exe.expected`:

```
both ok
```

Add to `tests/e2e.rs`:

```rust
#[test]
fn std_process_cwd_and_exe_path_are_non_nil() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_process_cwd_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_process_cwd_exe.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_process_cwd_exe.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}
```

Also add the standalone-compiles test (same pattern as Task 4/5):

```rust
#[test]
fn verb_process_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_process_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_process.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_process.cpp");
    assert!(status.success(), "runtime/verb_process.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e std_process_cwd_and_exe_path_are_non_nil verb_process_cpp_compiles_standalone`
Expected: FAIL (`runtime/verb_process.cpp` doesn't exist; `build` also fails at the linker step from Task 7 since it references `PROCESS_CPP`/`compile_process_obj` — this is expected, resolved in this task by finally creating the file, so run Task 7 and Task 8 together if working strictly in file order, per Task 7's ordering note).

- [ ] **Step 3: Implement**

Create `runtime/verb_process.cpp`:

```cpp
// Built-in bindings for `import std process;` -- cwd/exe_path/spawn/wait.
// Compiled and linked in automatically by `verb build`/`compile` whenever
// a program uses `import std process;`. Mirrors runtime/verb_std_io.cpp's
// shape: build-only, never linked into `verb run` (JIT).
#include "verb.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#include <sys/wait.h>
#include <climits>
#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif
#endif

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue cwd() {
    char buf[4096];
#ifdef _WIN32
    DWORD n = GetCurrentDirectoryA(sizeof(buf), buf);
    if (n == 0 || n >= sizeof(buf)) return verb_nil();
    return verb_string_from(std::string(buf, n));
#else
    if (!getcwd(buf, sizeof(buf))) return verb_nil();
    return verb_string_from(buf);
#endif
}

extern "C" VerbValue exe_path() {
#ifdef _WIN32
    char buf[MAX_PATH];
    DWORD n = GetModuleFileNameA(nullptr, buf, MAX_PATH);
    if (n == 0) return verb_nil();
    return verb_string_from(std::string(buf, n));
#elif defined(__APPLE__)
    char buf[4096];
    uint32_t size = sizeof(buf);
    if (_NSGetExecutablePath(buf, &size) != 0) return verb_nil();
    return verb_string_from(buf);
#else
    char buf[4096];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    if (n <= 0) return verb_nil();
    buf[n] = '\0';
    return verb_string_from(buf);
#endif
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e std_process_cwd_and_exe_path_are_non_nil verb_process_cpp_compiles_standalone`
Expected: PASS (this requires Task 7 already landed so `verb build` actually links `verb_process.cpp` — if Task 7 hasn't been done yet, do it now before checking this step).

- [ ] **Step 5: Commit**

```bash
git add runtime/verb_process.cpp tests/e2e.rs tests/fixtures/std_process_cwd_exe.verb tests/fixtures/std_process_cwd_exe.expected
git commit -m "feat(runtime): add verb_process.cpp with cwd()/exe_path()"
```

---

### Task 9: `runtime/verb_process.cpp` — `spawn`/`wait` on POSIX

**Files:**
- Modify: `runtime/verb_process.cpp` (append to the file from Task 8)
- Modify: `runtime/verb.h` (add `VERB_ARRAY = 7` to the tag enum)
- Test: `tests/e2e.rs` + two new fixtures

**Interfaces:**
- Consumes: `src/value.rs`'s `TAG_ARRAY = 7` and the array memory layout `src/codegen.rs`'s `Expr::ArrayLit` case builds (`{ int64_t len, cap; VerbValue* elems }`, elements 16 bytes each — `int8_t tag` padded to 8 bytes, `int64_t payload` — matching `VerbValue`'s own layout in `verb.h`).
- Produces: `extern "C" VerbValue spawn(VerbValue cmd, VerbValue args)`, `extern "C" VerbValue wait(VerbValue pid)` (arities 2 and 1, matching `PROCESS_FUNCS`).

- [ ] **Step 1: Write the failing tests**

Create `tests/fixtures/std_process_spawn_wait.verb`:

```
import std process;

assign pid spawn("sh", list "-c", "exit 7");
check pid eq nil begin
  print("spawn failed");
end orelse begin
  print(wait(pid));
end
```

Create `tests/fixtures/std_process_spawn_wait.expected`:

```
7
```

Create `tests/fixtures/std_process_spawn_missing_binary.verb`:

```
import std process;

assign pid spawn("verb-e2e-definitely-not-a-real-binary", list "x");
check pid eq nil begin
  print("nil as expected");
end
```

Create `tests/fixtures/std_process_spawn_missing_binary.expected`:

```
nil as expected
```

Add to `tests/e2e.rs`:

```rust
#[test]
fn std_process_spawn_and_wait_roundtrip_exit_code() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_process_spawn_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_process_spawn_wait.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_process_spawn_wait.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn std_process_spawn_missing_binary_returns_nil() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_process_spawn_missing_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_process_spawn_missing_binary.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_process_spawn_missing_binary.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e std_process_spawn_and_wait_roundtrip_exit_code std_process_spawn_missing_binary_returns_nil`
Expected: FAIL — `spawn`/`wait` are undefined in `verb_process.cpp` (link error).

- [ ] **Step 3: Implement — `verb.h`**

In `runtime/verb.h`, change:

```cpp
enum {
    VERB_NIL = 0,
    VERB_BOOL = 1,
    VERB_INT = 2,
    VERB_FLOAT = 3,
    VERB_STRING = 4,
    VERB_MAP = 6,
};
```

to:

```cpp
enum {
    VERB_NIL = 0,
    VERB_BOOL = 1,
    VERB_INT = 2,
    VERB_FLOAT = 3,
    VERB_STRING = 4,
    VERB_MAP = 6,
    // Matches src/value.rs's TAG_ARRAY. Payload points at
    // { int64_t len, cap; VerbValue* elems } (src/codegen.rs's
    // Expr::ArrayLit codegen) -- runtime/verb_process.cpp's spawn() is
    // the first C++ unit that needs to recognize this tag, to unpack a
    // Verb array of argv strings.
    VERB_ARRAY = 7,
};
```

- [ ] **Step 4: Implement — `verb_process.cpp`**

Append to `runtime/verb_process.cpp` (after `exe_path()`, before any `#ifdef _WIN32` spawn/wait split — the two platforms' implementations both live in this same file, guarded by `#ifdef _WIN32`):

```cpp
namespace {

// Mirrors the array header src/codegen.rs's Expr::ArrayLit builds:
// 24-byte header { i64 len, i64 cap, ptr elems }, each element a
// 16-byte VerbValue (matches verb.h's VerbValue layout exactly).
struct VerbArrayLayout {
    int64_t len;
    int64_t cap;
    VerbValue* elems;
};

// Builds argv[0] = cmd, argv[1..] = each string element of args (in
// order), argv[N] = nullptr. Returns false (leaving argv untouched) if
// cmd isn't a string or args isn't an array of strings.
bool build_argv(VerbValue cmd, VerbValue args, std::vector<std::string>& storage,
                 std::vector<char*>& argv) {
    if (cmd.tag != VERB_STRING) return false;
    storage.push_back(verb_as_string(cmd));
    if (args.tag != VERB_ARRAY) return false;
    auto* arr = reinterpret_cast<VerbArrayLayout*>(verb_as_map(args));
    for (int64_t i = 0; i < arr->len; ++i) {
        VerbValue elem = arr->elems[i];
        if (elem.tag != VERB_STRING) return false;
        storage.push_back(verb_as_string(elem));
    }
    argv.reserve(storage.size() + 1);
    for (auto& s : storage) argv.push_back(const_cast<char*>(s.c_str()));
    argv.push_back(nullptr);
    return true;
}

} // namespace

#ifdef _WIN32

#include <unordered_map>

namespace {
std::unordered_map<int64_t, HANDLE>& spawned_handles() {
    static std::unordered_map<int64_t, HANDLE> handles;
    return handles;
}
} // namespace

extern "C" VerbValue spawn(VerbValue cmd, VerbValue args) {
    std::vector<std::string> storage;
    std::vector<char*> argv;
    if (!build_argv(cmd, args, storage, argv)) return verb_nil();

    std::string cmdline;
    for (size_t i = 0; i + 1 < argv.size(); ++i) {
        if (i > 0) cmdline.push_back(' ');
        cmdline.push_back('"');
        cmdline += argv[i];
        cmdline.push_back('"');
    }

    STARTUPINFOA si{};
    si.cb = sizeof(si);
    PROCESS_INFORMATION pi{};
    BOOL ok = CreateProcessA(
        nullptr, cmdline.data(), nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi);
    if (!ok) return verb_nil();
    CloseHandle(pi.hThread);
    spawned_handles()[static_cast<int64_t>(pi.dwProcessId)] = pi.hProcess;
    return verb_int(static_cast<int64_t>(pi.dwProcessId));
}

extern "C" VerbValue wait(VerbValue pid) {
    auto& handles = spawned_handles();
    auto it = handles.find(verb_as_int(pid));
    if (it == handles.end()) return verb_nil();
    HANDLE h = it->second;
    handles.erase(it);
    if (WaitForSingleObject(h, INFINITE) != WAIT_OBJECT_0) {
        CloseHandle(h);
        return verb_nil();
    }
    DWORD code = 0;
    BOOL ok = GetExitCodeProcess(h, &code);
    CloseHandle(h);
    if (!ok) return verb_nil();
    return verb_int(static_cast<int64_t>(code));
}

#else

extern "C" VerbValue spawn(VerbValue cmd, VerbValue args) {
    std::vector<std::string> storage;
    std::vector<char*> argv;
    if (!build_argv(cmd, args, storage, argv)) return verb_nil();

    pid_t pid = fork();
    if (pid < 0) return verb_nil();
    if (pid == 0) {
        execvp(argv[0], argv.data());
        _exit(127); // execvp only returns on failure
    }
    return verb_int(static_cast<int64_t>(pid));
}

extern "C" VerbValue wait(VerbValue pid) {
    int status = 0;
    pid_t result = waitpid(static_cast<pid_t>(verb_as_int(pid)), &status, 0);
    if (result < 0) return verb_nil();
    if (!WIFEXITED(status)) return verb_nil();
    return verb_int(static_cast<int64_t>(WEXITSTATUS(status)));
}

#endif
```

Note: `spawn` uses `execvp` (searches `PATH`), not `execve` literally — the design spec's D-05 gives explicit discretion here ("fork+execve vs. posix_spawn/vfork ... whichever is simplest/safest ... as long as spawn()/wait() behave correctly"); `execvp` is `execve` plus `PATH` search, needed so `spawn("sh", ...)` resolves without a hardcoded absolute path, and every other design constraint (no shell, argv-array based, sentinel-on-failure) still holds exactly.

`verb_as_map(args)` is reused here to extract the raw pointer payload from a non-`VERB_MAP` tagged value — this works because `verb_as_map`/`verb_map` are just untyped `void*` pointer accessors regardless of what `VerbValue.tag` says (see `verb.h`); it is not asserting `args` is actually a map.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test e2e std_process_spawn_and_wait_roundtrip_exit_code std_process_spawn_missing_binary_returns_nil`
Expected: PASS.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add runtime/verb.h runtime/verb_process.cpp tests/e2e.rs tests/fixtures/std_process_spawn_wait.verb tests/fixtures/std_process_spawn_wait.expected tests/fixtures/std_process_spawn_missing_binary.verb tests/fixtures/std_process_spawn_missing_binary.expected
git commit -m "feat(runtime): add POSIX spawn()/wait() to verb_process.cpp"
```

---

### Task 10: Verify `verb_process.cpp`'s Windows branch cross-compiles

**Files:**
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `runtime/verb_process.cpp`'s `#ifdef _WIN32` branch from Task 9 (already written — this task only verifies it actually compiles, since the dev machine can't execute it).

- [ ] **Step 1: Write the test**

Add to `tests/e2e.rs` (near `aot_cross_build_produces_binary_for_each_target`):

```rust
#[test]
fn verb_process_cpp_cross_compiles_for_windows() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let obj = std::env::temp_dir().join("verb_process_windows_syntax_check.o");
    let status = Command::new("zig")
        .args([
            "c++", "-target", "x86_64-windows-gnu",
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_process.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke zig c++ to cross-compile runtime/verb_process.cpp");
    assert!(status.success(), "runtime/verb_process.cpp failed to cross-compile for Windows");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test e2e verb_process_cpp_cross_compiles_for_windows`
Expected: PASS if `zig` is on `PATH` (skips with a message otherwise). If it fails, read the compiler error — likely a missing `#include`, a `windows.h` macro clash (e.g. `min`/`max`), or a typo in a `Win32` API name from Task 9's `#ifdef _WIN32` block — fix `runtime/verb_process.cpp` directly and rerun.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e.rs
git commit -m "test: verify verb_process.cpp's Windows branch cross-compiles"
```

---

### Task 11: `exit`/`abort` E2E behavior (skip GC cleanup) + JIT rejection for `std process`

**Files:**
- Test: `tests/e2e.rs` + fixtures

**Interfaces:**
- Consumes: everything from Tasks 1-9.

- [ ] **Step 1: Write the tests**

Create `tests/fixtures/core_builtins_exit_skips_trailing_code.verb`:

```
print("before");
exit(3);
print("after");
```

Create `tests/fixtures/core_builtins_exit_skips_trailing_code.expected`:

```
before
```

Add to `tests/e2e.rs`:

```rust
#[test]
fn exit_stops_execution_and_sets_exit_code() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/core_builtins_exit_skips_trailing_code.verb"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let expected = std::fs::read_to_string(
        "tests/fixtures/core_builtins_exit_skips_trailing_code.expected",
    ).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn run_rejects_programs_with_std_process_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_process_cwd_exe.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std process"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run tests to verify they pass (or fail correctly)**

Run: `cargo test --test e2e exit_stops_execution_and_sets_exit_code run_rejects_programs_with_std_process_import`
Expected: both PASS given Tasks 1-9 are complete (this task adds no new implementation, only tests that lock in already-built behavior — if either fails, it means an earlier task's implementation has a bug; go fix that task, not this one).

- [ ] **Step 3: Run the full test suite one more time**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/e2e.rs tests/fixtures/core_builtins_exit_skips_trailing_code.verb tests/fixtures/core_builtins_exit_skips_trailing_code.expected
git commit -m "test: lock in exit() skip-cleanup semantics and std process JIT rejection"
```

---

### Task 12: `.planning/PROJECT.md` documentation

**Files:**
- Modify: `.planning/PROJECT.md`

**Interfaces:**
- Consumes: nothing code-facing — pure documentation, per the spec's explicit requirement (D-11) that this exception be written down, not silently assumed.

- [ ] **Step 1: Implement**

In `.planning/PROJECT.md`, in the `## Constraints` section, change:

```
- **Tech stack**: C++17 runtime (`runtime/verb.h`, `verb_map.cpp`,
  `verb_std_io.cpp`) compiled via `build.rs`/the `cc` crate and linked into
  every generated binary
```

to:

```
- **Tech stack**: C++17 runtime (`runtime/verb.h`, `verb_map.cpp`,
  `verb_std_io.cpp`, `verb_env.cpp`, `verb_process.cpp`, `verb_builtins.cpp`)
  compiled via `build.rs`/the `cc` crate and linked into every generated
  binary
```

and add a new bullet right after the existing:

```
- **Compatibility**: Windows cross-compile targets don't support `import std io`
  (POSIX socket dependency in `verb_std_io.cpp`)
```

bullet:

```
- **Memory model**: `exit()`/`abort()` (core builtins, `runtime/verb_builtins.cpp`)
  call libc `exit`/`abort` directly and skip refcount cleanup entirely — matches
  C's own `exit`/`abort` semantics exactly, and is excluded from the zero-leak
  (GC) guarantee below by design, not by omission
```

(Insert this as its own bullet; do not merge it into the existing "Memory model" bullet about the cycle collector below it, since these are two independent documented exceptions.)

- [ ] **Step 2: Verify**

Run: `grep -n "verb_builtins.cpp\|exit()/abort()" .planning/PROJECT.md`
Expected: both new mentions appear.

- [ ] **Step 3: Commit**

```bash
git add .planning/PROJECT.md
git commit -m "docs(project): document exit/abort GC-exclusion constraint and new runtime units"
```

---

## Final check

- [ ] Run `cargo test` one more time from a clean state and confirm everything passes.
- [ ] Run `cargo build --release` once to confirm no warnings were introduced (`cargo build 2>&1 | grep -i warning` should show nothing new compared to `git stash`'s baseline).
