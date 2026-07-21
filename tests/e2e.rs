use std::process::Command;

fn run_ok(name: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

/// Compile-time error: exit != 0, all `msgs` appear on stderr.
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

#[allow(dead_code)]
fn run_err(name: &str, msg: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(msg),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn literals() { run_ok("literals"); }

#[test]
fn arith() { run_ok("arith"); }

#[test]
fn strings() { run_ok("strings"); }

#[test]
fn type_error_aborts() {
    run_err("err_types", "runtime error [1:9]: 'add' needs numbers, got int and string");
}

#[test]
fn div_zero_aborts() { run_err("err_divzero", "runtime error [1:9]: division by zero"); }

#[test]
fn join_type_error_aborts() {
    run_err("err_join", "'join' needs strings, got string and nil");
}

#[test]
fn neg_type_error_aborts() { run_err("err_neg", "'neg' needs a number, got string"); }

#[test]
fn vars() { run_ok("vars"); }

#[test]
fn declare_vars() { run_ok("declare"); }

#[test]
fn reassign_releases_previous_string_value() { run_ok("reassign_strings"); }

#[test]
fn control() { run_ok("control"); }

#[test]
fn functions() { run_ok("functions"); }

#[test]
fn call_non_function_aborts() { run_err("err_call_nonfn", "can only call functions, got int"); }

#[test]
fn wrong_arity_aborts() {
    run_err("err_arity", "wrong number of arguments: expected 1, got 2");
}

#[test]
fn syntax_error_shows_found_token_and_caret() {
    compile_err("err_syntax", &[
        "expected ')', found ';'",
        "print(x;",   // source line echoed
        "^",          // caret marker
    ]);
}

#[test]
fn undefined_var_suggests_closest_name() {
    compile_err("err_typo", &[
        "undefined variable 'contuer'",
        "did you mean 'counter'?",
    ]);
}

#[test]
fn old_operator_keyword_gets_rename_hint() {
    compile_err("err_oldkw", &["'plus' was renamed to 'add'"]);
}

#[test]
fn old_statement_keyword_gets_rename_hint() {
    compile_err("err_oldstmt", &["'if' was renamed to 'check'"]);
}

#[test]
fn top_level_return_is_compile_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/err_return_top.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("'return' outside function"), "stderr: {stderr}");
}

#[test]
fn undefined_var_is_compile_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/err_undef.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("undefined variable"), "stderr: {stderr}");
}

#[test]
fn emits_llvm_ir() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/literals.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}

#[test]
fn verb_alloc_is_emitted() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define ptr @verb_alloc"), "no verb_alloc in IR:\n{ir}");
}

// ----- C++ import / extern (from cpp-import) -----

#[test]
fn extern_call_compiles_to_a_direct_call_instruction() {
    let tmp = std::env::temp_dir().join("verb_test_extern_ir_out");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/import_extern_call.verb",
            "-o", tmp.to_str().unwrap(),
            "--emit-llvm",
        ])
        .output()
        .unwrap();
    // --emit-llvm prints IR to stdout before the link step runs, so the IR
    // shape is already checkable here. This still exits non-zero because the
    // link step fails: `-lmathlib` isn't an actual library built/linked in
    // this test — only the emitted IR shape is under test, not a successful
    // link.
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("@c_sqrt"), "no call to c_sqrt in IR:\n{ir}");
}

#[test]
fn extern_arity_mismatch_across_call_sites_is_a_compile_error() {
    compile_err("err_extern_arity", &[
        "extern fn 'c_sqrt' called with 2 argument(s), previously called with 1",
    ]);
}

#[test]
fn build_produces_a_runnable_binary_for_import_free_programs() {
    let out_path = std::env::temp_dir().join("verb_test_build_literals");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/literals.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success());
    let expected = std::fs::read_to_string("tests/fixtures/literals.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn build_with_l_flag_forwards_it_without_breaking_the_build() {
    let out_path = std::env::temp_dir().join("verb_test_build_with_lflag");
    // Any real, existing directory is enough to prove -L<dir> is parsed and
    // forwarded to the linker without breaking an otherwise-normal, import-free
    // build. A stronger test that proves the linker actually *resolves* a
    // symbol via -L would duplicate the full C++ library import e2e coverage
    // Task 7 is already adding.
    let lib_dir = std::env::temp_dir();
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/literals.verb",
            "-o", out_path.to_str().unwrap(),
            &format!("-L{}", lib_dir.to_str().unwrap()),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success());
    let expected = std::fs::read_to_string("tests/fixtures/literals.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn run_rejects_programs_with_imports() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/import_extern_call.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("mathlib"), "stderr: {stderr}");
}

/// Compiles tests/fixtures/cpp/mathlib.cpp into a shared library once per
/// test run and returns the directory it landed in (for `-L`).
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

#[test]
fn verb_std_io_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_std_io_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_std_io.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_std_io.cpp");
    assert!(status.success(), "runtime/verb_std_io.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}

// ----- AOT host / cross build + multi-file (from main) -----

#[test]
fn aot_build_produces_working_binary() {
    let dir = std::env::temp_dir().join("verb_aot_host_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("functions_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/functions.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

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

#[test]
fn aot_build_invalid_target_is_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", "/tmp/whatever", "--target", "solaris-x86_64"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --target"), "stderr: {stderr}");
}

#[test]
fn aot_build_target_all_produces_six_binaries_with_summary() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_aot_all_test");
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("functions_all");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", base.to_str().unwrap(), "--target", "all"])
        .output()
        .unwrap();
    assert!(out.status.success(), "build --target all failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("linux-x86_64: ok"), "summary missing linux-x86_64: {stdout}");
    assert!(stdout.contains("windows-arm64: ok"), "summary missing windows-arm64: {stdout}");

    for suffix in [
        "linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64",
    ] {
        let path = dir.join(format!("functions_all-{suffix}"));
        assert!(path.exists(), "missing {path:?}");
    }
    for suffix in ["windows-x86_64", "windows-arm64"] {
        let path = dir.join(format!("functions_all-{suffix}.exe"));
        assert!(path.exists(), "missing {path:?}");
    }
}

#[test]
fn no_files_given_shows_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}

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
    let dir = std::env::temp_dir().join("verb_multifile_build_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("multifile_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/multifile_a.verb",
            "tests/fixtures/multifile_b.verb",
            "-o",
            bin.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

// ----- std io -----

#[test]
fn run_rejects_programs_with_std_io_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_io_file_roundtrip.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std io"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_io_files() {
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_file_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_file_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_file_roundtrip.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
}

#[test]
fn cross_build_links_a_program_using_std_io_for_a_non_host_non_windows_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    // Non-Windows, non-host target: this exercises build_aot_cross's
    // std-io object compile + link path (zig c++, not zig cc) without
    // relying on the guard that rejects Windows targets. Not executed —
    // it's cross-compiled for a foreign arch/OS, only checked for a
    // successful, non-empty link, same scope as
    // aot_cross_build_produces_binary_for_each_target.
    let label = if cfg!(target_arch = "x86_64") { "linux-arm64" } else { "linux-x86_64" };
    let dir = std::env::temp_dir().join("verb_std_io_cross_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join(format!("std_io_roundtrip_{label}"));
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_file_roundtrip.verb",
            "-o", bin.to_str().unwrap(),
            "--target", label,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "target {label} failed: {}", String::from_utf8_lossy(&out.stderr));
    let meta = std::fs::metadata(&bin)
        .unwrap_or_else(|e| panic!("missing output for {label} at {bin:?}: {e}"));
    assert!(meta.len() > 0, "empty output for {label}");
}

#[test]
fn build_links_and_runs_a_program_using_std_io_tcp_loopback() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_tcp_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_tcp_loopback.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_tcp_loopback.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn windows_cross_target_rejects_std_io_import() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_windows_reject");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_file_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
            "--target", "windows-x86_64",
        ])
        .output()
        .unwrap();
    assert!(!build.status.success());
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("not supported when cross-compiling to a Windows target"),
        "stderr: {stderr}"
    );
}

#[test]
fn string_literals_carry_a_static_gc_sentinel_header() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("-9223372036854775808"), "no GC static sentinel in IR:\n{ir}");
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

#[test]
fn gc_retain_release_calls_are_wired_into_expr_codegen() {
    // NOTE: deviates from the task brief, which named tests/fixtures/strings.verb
    // here. strings.verb has no variable reads, so it can never produce a
    // verb_retain_value call site (Step 1 only retains on Expr::Var). vars.verb
    // is an existing, already-valid fixture (see the `vars` test above) that
    // reads variables, so it actually exercises both the retain (Step 1) and
    // release (Steps 2/5) call sites this test is meant to check for.
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/vars.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("call void @verb_retain_value"), "no retain call site in IR:\n{ir}");
    assert!(ir.contains("call void @verb_release_value"), "no release call site in IR:\n{ir}");
}

#[test]
fn gc_releases_block_scope_cells() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/control.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("call void @verb_release_cell"), "no cell release in IR:\n{ir}");
}

#[test]
fn early_return_from_nested_block_releases_open_scopes() { run_ok("early_return_releases"); }

#[test]
fn early_return_in_if_then_leaves_scopes_intact_for_else_branch() { run_ok("early_return_if_else_outer_var"); }

fn assert_no_leaks(fixture: &str) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_{fixture}"));
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

#[test]
fn gc_stress_loop_leaks_nothing() { assert_no_leaks("gc_stress"); }

#[test]
fn gc_no_leaks_across_representative_programs() {
    for fixture in ["strings", "functions", "control", "reassign_strings", "early_return_releases", "early_return_if_else_outer_var", "gc_and_or_strings", "gc_redeclare"] {
        assert_no_leaks(fixture);
    }
}

#[test]
fn gc_short_circuit_and_or_discards_unchosen_operand_without_leaking() {
    run_ok("gc_and_or_strings");
    assert_no_leaks("gc_and_or_strings");
}

#[test]
fn gc_redeclaring_a_name_in_the_same_scope_releases_the_orphaned_cell() {
    run_ok("gc_redeclare");
    assert_no_leaks("gc_redeclare");
}

#[test]
fn gc_no_leaks_with_std_io_file_roundtrip() {
    // Uses its own fixture (std_io_file_roundtrip_gc) writing to its own temp
    // path (verb_e2e_gc_leak_check.tmp) rather than reusing
    // std_io_file_roundtrip.verb — that fixture and its hardcoded temp path
    // are also used by build_links_and_runs_a_program_using_std_io_files,
    // which asserts on the file's actual content. Since cargo test runs
    // tests concurrently by default, sharing the same path would let the two
    // tests race on the same file.
    let _ = std::fs::remove_file("verb_e2e_gc_leak_check.tmp");
    assert_no_leaks("std_io_file_roundtrip_gc");
    let _ = std::fs::remove_file("verb_e2e_gc_leak_check.tmp");
}
