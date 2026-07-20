# Multi-file .verb Linking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `verb run`/`verb build` accept multiple `.verb` files on the command line, concatenated (linked) into one program, with compile-time errors correctly attributed to their originating file.

**Architecture:** Each file is lexed and parsed independently (unchanged `lexer::lex`/`parser::parse`). Their statement lists are concatenated in argument order into one `Vec<Stmt>` fed to the existing single-pass `compile_program`, which already hoists functions and builds `main` from top-level statements as it walks them — concatenation alone achieves linking. A parallel `Vec<String>` tracks which file each top-level statement came from, so codegen can stamp errors with the correct filename without touching every AST node.

**Tech Stack:** Rust, inkwell (LLVM bindings). Binary crate (no lib target) — module tests must live inline as `#[cfg(test)]` blocks in the module they test; `tests/e2e.rs` drives the compiled binary as a subprocess.

## Global Constraints

- No in-source `import`/`include` syntax — linking is CLI-only (spec: Non-goals).
- Any file, including non-entry files, may contain top-level executable statements — no "library file" restriction is enforced (spec: Semantics).
- No new duplicate-function-name detection across files — later definition shadows earlier, same as today's intra-file behavior (spec: Semantics).
- Runtime (JIT-generated, printf-baked) errors keep bare `[line:col]`, no filename — accepted limitation for v1, do not attempt to fix as part of this plan (spec: Accepted limitation).
- At least one file is required; zero files is a usage error (spec: Goals).
- `-o` still consumes its following argument as the output path regardless of position relative to file args (spec: Goals).

---

## Team Assignment

3 developer + 3 reviewer agent pairs. Dependency graph below — Wave 1 tasks are mutually independent (different files/logic, no shared state) and should be dispatched in parallel, one pair per task. Later waves depend on earlier ones landing first.

```
Wave 1 (parallel):  Task 1 (error.rs)         Task 2 (main.rs CLI parsing)
                            \                         |
Wave 2:                      \-> Task 3 (codegen.rs) /
                                        |
Wave 3:                          Task 4 (main.rs integration)
                                        |
Wave 4 (parallel):          Task 5 (e2e happy path)   Task 6 (e2e error + build path)
```

Suggested pairing: Pair A takes Task 1 → Task 5. Pair B takes Task 2 → Task 6. Pair C takes Task 3 → Task 4 (the integration task needs 1+2+3 done, so Pair C naturally starts it once all of Wave 1/2 lands — coordinate hand-off through the reviewer rather than blocking Pair C idle).

---

### Task 1: `CompileError` gains a `file` field

**Files:**
- Modify: `src/error.rs`

**Interfaces:**
- Produces: `CompileError.file: Option<String>` field; `CompileError::with_file(self, file: impl Into<String>) -> Self` builder. Task 3 and Task 4 both use this.

- [ ] **Step 1: Write the failing test**

Add to `src/error.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib error::tests -- --nocapture` (or `cargo test with_file_sets_file`)
Expected: FAIL to compile — `file` field / `with_file` method don't exist yet.

- [ ] **Step 3: Write minimal implementation**

Replace the full contents of `src/error.rs` (above the new test module) with:

```rust
#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
    pub hint: Option<String>,
    pub file: Option<String>,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
        Self { msg: msg.into(), line, col, hint: None, file: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib error::tests`
Expected: PASS (2 tests)

- [ ] **Step 5: Run full existing test suite to confirm no regressions**

Run: `cargo build 2>&1 | tail -30`
Expected: builds clean (adding a struct field with `Option<String>` doesn't break any existing `CompileError { .. }` construction since all existing construction goes through `::new`, which now sets `file: None`).

- [ ] **Step 6: Commit**

```bash
git add src/error.rs
git commit -m "feat: add file field to CompileError for multi-file error attribution"
```

---

### Task 2: `main.rs` CLI parsing for multiple files

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Produces: `struct ParsedArgs { cmd: String, files: Vec<String>, out: Option<String>, emit_llvm: bool }` and `fn parse_cli(args: &[String]) -> Option<ParsedArgs>`. Task 4 wires this into `main()`.
- This task does NOT modify `main()`'s existing body — it only adds the new struct/function/tests alongside the current code, so it can land independently of Task 3. (Expect an "unused" compiler warning until Task 4 wires it in — that's fine, not an error.)

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_multiple_files() {
        let p = parse_cli(&args(&["verb", "run", "a.verb", "b.verb"])).unwrap();
        assert_eq!(p.cmd, "run");
        assert_eq!(p.files, vec!["a.verb".to_string(), "b.verb".to_string()]);
        assert!(!p.emit_llvm);
        assert_eq!(p.out, None);
    }

    #[test]
    fn parses_flags_interleaved_with_files() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "b.verb", "--emit-llvm",
        ])).unwrap();
        assert_eq!(p.cmd, "build");
        assert_eq!(p.files, vec!["a.verb".to_string(), "b.verb".to_string()]);
        assert_eq!(p.out, Some("out".to_string()));
        assert!(p.emit_llvm);
    }

    #[test]
    fn rejects_no_files() {
        assert!(parse_cli(&args(&["verb", "run"])).is_none());
    }

    #[test]
    fn rejects_missing_o_value() {
        assert!(parse_cli(&args(&["verb", "build", "a.verb", "-o"])).is_none());
    }

    #[test]
    fn rejects_no_command() {
        assert!(parse_cli(&args(&["verb"])).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin verb tests::parses_multiple_files`
Expected: FAIL to compile — `ParsedArgs` / `parse_cli` don't exist yet.

- [ ] **Step 3: Write minimal implementation**

Add above `fn main()` in `src/main.rs` (after the existing `use error::CompileError;` line):

```rust
struct ParsedArgs {
    cmd: String,
    files: Vec<String>,
    out: Option<String>,
    emit_llvm: bool,
}

fn parse_cli(args: &[String]) -> Option<ParsedArgs> {
    if args.len() < 2 {
        return None;
    }
    let cmd = args[1].clone();
    let mut files = Vec::new();
    let mut out = None;
    let mut emit_llvm = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--emit-llvm" => {
                emit_llvm = true;
                i += 1;
            }
            "-o" => {
                i += 1;
                if i >= args.len() {
                    return None;
                }
                out = Some(args[i].clone());
                i += 1;
            }
            f => {
                files.push(f.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        return None;
    }
    Some(ParsedArgs { cmd, files, out, emit_llvm })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin verb tests::`
Expected: PASS (5 tests). A dead-code warning for `parse_cli`/`ParsedArgs` is expected and fine at this stage.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: add multi-file CLI argument parsing (not yet wired into main)"
```

---

### Task 3: `codegen.rs` — file-stamped error attribution

**Depends on:** Task 1 (`CompileError.file` / `with_file` must exist).

**Files:**
- Modify: `src/codegen.rs:14-25` (struct fields), `src/codegen.rs:35-38` (constructor), `src/codegen.rs:633-645` (`compile_program`)

**Interfaces:**
- Consumes: `CompileError.file: Option<String>` (Task 1).
- Produces: `Codegen::compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String]) -> Result<(), CompileError>` — signature change from the current `compile_program(&mut self, stmts: &[Stmt])`. Task 4 calls this new signature.

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/codegen.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use inkwell::context::Context;

    #[test]
    fn stamps_error_with_originating_file() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![
            Stmt::Assign { name: "x".to_string(), value: Expr::Int(1) },
            Stmt::ExprStmt(Expr::Var("undefined_name".to_string(), 3, 5)),
        ];
        let stmt_files = vec!["a.verb".to_string(), "b.verb".to_string()];

        let err = cg.compile_program(&stmts, &stmt_files).unwrap_err();

        assert_eq!(err.file, Some("b.verb".to_string()));
        assert_eq!(err.line, 3);
    }

    #[test]
    fn no_error_when_program_is_valid() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::Assign { name: "x".to_string(), value: Expr::Int(1) }];
        let stmt_files = vec!["a.verb".to_string()];

        assert!(cg.compile_program(&stmts, &stmt_files).is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin verb codegen::tests`
Expected: FAIL to compile — `compile_program` still takes one argument, not two.

- [ ] **Step 3: Write minimal implementation**

In the `Codegen` struct definition (`src/codegen.rs:14-25`), add a field:

```rust
pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    value_ty: StructType<'ctx>,
    closure_ty: StructType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    functions: HashMap<String, (FunctionValue<'ctx>, usize)>,
    fn_depth: u32,
    fn_counter: u32,
    cur_file: String,
}
```

In `Codegen::new` (`src/codegen.rs:35-38`), initialize it:

```rust
        let cg = Self {
            ctx, module, builder, value_ty, closure_ty, ptr_ty,
            scopes: Vec::new(), functions: HashMap::new(), fn_depth: 0, fn_counter: 0,
            cur_file: String::new(),
        };
```

Replace `compile_program` (`src/codegen.rs:633-645`) with:

```rust
    pub fn compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String]) -> Result<(), CompileError> {
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.scopes.push(HashMap::new());
        for (i, s) in stmts.iter().enumerate() {
            self.cur_file = stmt_files[i].clone();
            if let Err(mut e) = self.gen_stmt(s) {
                if e.file.is_none() {
                    e.file = Some(self.cur_file.clone());
                }
                return Err(e);
            }
            if !self.cur_block_open() {
                break; // dead code after return/abort
            }
        }
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --bin verb codegen::tests`
Expected: PASS (2 tests)

- [ ] **Step 5: Confirm the crate still builds** (main.rs still calls the old 1-arg signature at this point, so a build error here is expected and correct)

Run: `cargo build 2>&1 | grep "compile_program"`
Expected: an error pointing at the old call site in `main.rs` (`this function takes 2 arguments but 1 argument was supplied`) — confirms the signature actually changed. This will be fixed in Task 4; do not modify `main.rs` in this task.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs
git commit -m "feat: track originating file per top-level statement, stamp codegen errors"
```

---

### Task 4: `main.rs` integration — wire multi-file loop end to end

**Depends on:** Task 1, Task 2, Task 3 (all must be merged — this task makes the crate compile again after Task 3's intentional break).

**Files:**
- Modify: `src/main.rs` (full rewrite of `fn main()` and `fn die()`; `fn usage()` message text)

**Interfaces:**
- Consumes: `ParsedArgs`/`parse_cli` (Task 2), `CompileError.file`/`with_file` (Task 1), `Codegen::compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String])` (Task 3).

- [ ] **Step 1: Write the failing test**

Add to `tests/e2e.rs` (new test, appended at the end of the file):

```rust
#[test]
fn multi_file_error_reports_correct_filename() {
    // Written against fixtures added in Task 6; this test is expected to fail
    // to compile-and-run correctly until this task's main.rs changes land AND
    // Task 6 adds the fixture files. For this task, verify only the CLI-level
    // no-files-given usage error, which needs no new fixtures:
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e multi_file_error_reports_correct_filename`
Expected: FAIL — either a build error (Task 3 broke `main.rs`) or exit code mismatch, since `main()` still uses the old single-file `args[2]` path and old `compile_program` signature.

- [ ] **Step 3: Write minimal implementation**

Replace the entire contents of `src/main.rs` from `fn die(` through the end of the file (keep the `mod`/`use` lines and the `ParsedArgs`/`parse_cli` block from Task 2 as-is) with:

```rust
fn die(e: CompileError, sources: &[(String, String)]) -> ! {
    let file = e.file.as_deref().unwrap_or("<unknown>");
    eprintln!("error [{file}:{}:{}]: {}", e.line, e.col, e.msg);
    if e.line > 0 {
        if let Some((_, src)) = sources.iter().find(|(name, _)| name.as_str() == file) {
            if let Some(text) = src.lines().nth(e.line as usize - 1) {
                let num = e.line.to_string();
                eprintln!(" {num} | {text}");
                let pad = " ".repeat(num.len());
                let offset = " ".repeat(e.col.saturating_sub(1) as usize);
                eprintln!(" {pad} | {offset}^");
            }
        }
    }
    if let Some(hint) = &e.hint {
        eprintln!("   hint: {hint}");
    }
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage: verb run <file.verb>... [--emit-llvm]");
    eprintln!("       verb build <file.verb>... -o <out> [--emit-llvm]");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_cli(&args).unwrap_or_else(|| usage());

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut stmts = Vec::new();
    let mut stmt_files = Vec::new();

    for file in &parsed.files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {file}: {e}");
                exit(1);
            }
        };
        sources.push((file.clone(), src.clone()));

        let toks = lexer::lex(&src)
            .map_err(|e| e.with_file(file.clone()))
            .unwrap_or_else(|e| die(e, &sources));
        let file_stmts = parser::parse(toks)
            .map_err(|e| e.with_file(file.clone()))
            .unwrap_or_else(|e| die(e, &sources));

        stmt_files.extend(std::iter::repeat(file.clone()).take(file_stmts.len()));
        stmts.extend(file_stmts);
    }

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&stmts, &stmt_files).unwrap_or_else(|e| die(e, &sources));

    if parsed.emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match parsed.cmd.as_str() {
        "run" => {
            let ee = cg
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| {
                    eprintln!("JIT error: {e}");
                    exit(1);
                });
            unsafe {
                let main_fn = ee
                    .get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "build" => {
            let out = parsed.out.unwrap_or_else(|| usage());
            build_aot(&cg, &out);
        }
        _ => usage(),
    }
}

fn build_aot(_cg: &codegen::Codegen, _out: &str) {
    eprintln!("build: not implemented yet");
    exit(1);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e multi_file_error_reports_correct_filename`
Expected: PASS

- [ ] **Step 5: Run the FULL test suite to confirm no regressions across every existing single-file test**

Run: `cargo test 2>&1 | tail -60`
Expected: all existing `tests/e2e.rs` tests (literals, arith, strings, type_error_aborts, div_zero_aborts, join_type_error_aborts, neg_type_error_aborts, vars, declare_vars, control, functions, call_non_function_aborts, wrong_arity_aborts, syntax_error_shows_found_token_and_caret, undefined_var_suggests_closest_name, old_operator_keyword_gets_rename_hint, old_statement_keyword_gets_rename_hint, top_level_return_is_compile_error, undefined_var_is_compile_error, emits_llvm_ir) still PASS, plus all unit tests from Tasks 1-3, plus the new `multi_file_error_reports_correct_filename` test. Total 0 failures.

If any existing single-file `compile_err`/`run_err` test fails on message format, check that `die()`'s new `[{file}:{line}:{col}]` prefix didn't break a substring match those tests rely on — those tests use `.contains(msg)` on message fragments that don't include the bracket prefix, so they should be unaffected, but verify.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat: wire multi-file CLI parsing into main() end to end"
```

---

### Task 5: e2e happy-path test for multi-file linking

**Depends on:** Task 4.

**Files:**
- Create: `tests/fixtures/multifile_a.verb`
- Create: `tests/fixtures/multifile_b.verb`
- Create: `tests/fixtures/multifile.expected`
- Modify: `tests/e2e.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/multifile_a.verb`:

```
%% library file: helper function only, no top-level executable code
make double(x) begin
  return x times 2;
end
```

Create `tests/fixtures/multifile_b.verb`:

```
%% entry file: uses the function defined in multifile_a.verb
print(double(21));
assign total 0;
loop assign i 1; i atmost 3; i be i add 1 begin
  total be total add i;
end
print(total);
```

Create `tests/fixtures/multifile.expected`:

```
42
6
```

Add to `tests/e2e.rs` (a new helper for multi-file `run_ok`, plus the test):

```rust
fn run_ok_multi(names: &[&str], expected_name: &str) {
    let files: Vec<String> = names
        .iter()
        .map(|n| format!("tests/fixtures/{n}.verb"))
        .collect();
    let mut args = vec!["run".to_string()];
    args.extend(files);
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(format!("tests/fixtures/{expected_name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn multi_file_links_and_runs() {
    run_ok_multi(&["multifile_a", "multifile_b"], "multifile");
}

#[test]
fn multi_file_emits_single_merged_llvm_module() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/multifile_a.verb",
            "tests/fixtures/multifile_b.verb",
            "--emit-llvm",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e multi_file_links_and_runs`
Expected: FAIL — fixtures/test not present until this step's files are added (run this right after Step 1's file creation but before confirming correctness, i.e. treat first run as the check that the harness executes it; if it already passes because Task 4 is solid, that's fine too — but run it once before trusting the fixtures' expected output to catch typos in `multifile.expected`).

- [ ] **Step 3: No production code changes needed** — this task is fixtures + test only, exercising Task 4's implementation.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e multi_file`
Expected: PASS (both `multi_file_links_and_runs` and `multi_file_emits_single_merged_llvm_module`)

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/multifile_a.verb tests/fixtures/multifile_b.verb tests/fixtures/multifile.expected tests/e2e.rs
git commit -m "test: add multi-file happy-path e2e coverage"
```

---

### Task 6: e2e error-attribution + build-path tests

**Depends on:** Task 4. Independent of Task 5 (different fixtures, additive to the same `tests/e2e.rs` file — if run in parallel with Task 5, coordinate the merge since both append to `tests/e2e.rs`; take both additions, no logical overlap).

**Files:**
- Create: `tests/fixtures/multifile_err_a.verb`
- Create: `tests/fixtures/multifile_err_b.verb`
- Modify: `tests/e2e.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/fixtures/multifile_err_a.verb`:

```
%% first file: valid, no errors
assign x 1;
```

Create `tests/fixtures/multifile_err_b.verb`:

```
%% second file: line 2 references an undefined variable
assign y 2;
print(zz);
```

Add to `tests/e2e.rs`:

```rust
#[test]
fn multi_file_error_names_the_correct_file() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/multifile_err_a.verb",
            "tests/fixtures/multifile_err_b.verb",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("multifile_err_b.verb"),
        "expected error attributed to multifile_err_b.verb, got: {stderr}"
    );
    assert!(
        !stderr.contains("multifile_err_a.verb"),
        "error should not be attributed to multifile_err_a.verb, got: {stderr}"
    );
    assert!(stderr.contains("undefined variable 'zz'"), "stderr: {stderr}");
}

#[test]
fn multi_file_build_path_accepts_multiple_files() {
    // build_aot is still a stub (unimplemented), so this only verifies that
    // multiple files flow through CLI parsing + lex + parse + codegen + the
    // "build" dispatch arm without error before hitting the stub.
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/multifile_a.verb",
            "tests/fixtures/multifile_b.verb",
            "-o",
            "/tmp/verb_multifile_build_test_out",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("build: not implemented yet"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e multi_file_error_names_the_correct_file`
Expected: FAIL until fixtures exist (this confirms the fixtures are actually being read — run once right after creating them).

- [ ] **Step 3: No production code changes needed** — this task is fixtures + test only, exercising Task 4's implementation.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e multi_file`
Expected: PASS (all four `multi_file_*` tests across Task 5 and Task 6 combined, once both are merged)

- [ ] **Step 5: Run the complete test suite one final time**

Run: `cargo test 2>&1 | tail -60`
Expected: every test in the crate passes — unit tests from Tasks 1-3, all pre-existing e2e tests, and all new multi-file e2e tests from Tasks 4-6.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/multifile_err_a.verb tests/fixtures/multifile_err_b.verb tests/e2e.rs
git commit -m "test: add multi-file error-attribution and build-path e2e coverage"
```
