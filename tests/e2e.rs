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

/// Builds `fixture`, runs it with `VERB_GC_DEBUG=1`, and asserts the
/// binary reports `verb_gc_live=0` at exit -- i.e. every heap-owned value
/// (string, closure, array, map, cell) the program allocated was released
/// by the time it exits. Does not check the program's own stdout/output
/// beyond locating the `verb_gc_live=` line -- callers that also care
/// about output correctness should use `run_ok` separately or inline
/// their own check.
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

#[test]
fn literals() { run_ok("literals"); }

#[test]
fn array_literal_prints() { run_ok("arrays_literal"); }

#[test]
fn array_len() { run_ok("arrays_len"); }

#[test]
fn array_len_type_error_aborts() {
    run_err("err_array_len_type", "'len' needs an array, got int");
}

#[test]
fn array_get_set() { run_ok("arrays_get_set"); }

#[test]
fn array_get_type_error_aborts() {
    run_err("err_array_get_type", "'get' needs an array, got int");
}

#[test]
fn array_get_index_type_error_aborts() {
    run_err("err_array_get_index_type", "'get' needs an int index, got string");
}

#[test]
fn array_get_bounds_error_aborts() {
    run_err("err_array_get_bounds", "index 5 out of bounds for array of length 2");
}

#[test]
fn array_push_pop_grows() { run_ok("arrays_push_pop"); }

#[test]
fn array_push_type_error_aborts() {
    run_err("err_array_push_type", "'push' needs an array, got int");
}

#[test]
fn array_pop_empty_aborts() {
    run_err("err_array_pop_empty", "pop from empty array");
}

#[test]
fn array_of_arrays() { run_ok("arrays_of_arrays"); }

#[test]
fn array_of_closures() { run_ok("arrays_of_closures"); }

#[test]
fn nested_arrays_retain_and_release_correctly() { run_ok("gc_arrays_nested"); }

#[test]
fn arrays_of_closures_retain_and_release_correctly() { run_ok("gc_arrays_of_closures"); }

#[test]
fn array_push_regrowth_frees_old_buffers() { run_ok("gc_arrays_regrow"); }

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
fn control() { run_ok("control"); }

#[test]
fn functions() { run_ok("functions"); }

#[test]
fn call_non_function_aborts() { run_err("err_call_nonfn", "can only call functions, got int"); }

#[test]
fn nested_make_sees_own_scope_and_top_level_globals() { run_ok("closures"); }

#[test]
fn nested_make_cannot_capture_enclosing_local() {
    compile_err("err_closure_no_capture", &["undefined variable 'local'"]);
}

#[test]
fn nested_make_cannot_capture_enclosing_param() {
    compile_err("err_closure_no_capture_param", &["undefined variable 'a'"]);
}

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

#[test]
fn string_literals_carry_a_static_gc_sentinel_header() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("-9223372036854775808"), "no GC static sentinel in IR:\n{ir}");
    assert!(ir.contains("private unnamed_addr constant { i64,"),
        "string literal global isn't private/unnamed_addr:\n{ir}");
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
    assert!(ir.contains("declare void @verb_map_destroy_contents"),
        "verb_map_destroy_contents not declared:\n{ir}");
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

#[test]
fn verb_map_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_map_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_map.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_map.cpp");
    assert!(status.success(), "runtime/verb_map.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}

#[test]
fn verb_std_thread_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_std_thread_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-pthread", "-c",
            "runtime/verb_std_thread.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_std_thread.cpp");
    assert!(status.success(), "runtime/verb_std_thread.cpp failed to compile");
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
fn std_io_file_roundtrip_allocates_through_verb_alloc() {
    // Mirrors build_links_and_runs_a_program_using_std_io_files, but under
    // its own fixture + temp-file path (verb_e2e_gc_v2_roundtrip.tmp) so
    // this doesn't race the other std-io file-roundtrip test over the same
    // hardcoded path under cargo test's default parallelism. Exercises the
    // verb_alloc-backed file_read/file_write/file_append path so that any
    // retain/release GC touches this string without corrupting memory.
    let _ = std::fs::remove_file("verb_e2e_gc_v2_roundtrip.tmp");
    let out_path = std::env::temp_dir().join("verb_e2e_gc_std_io_file_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/gc_std_io_file_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/gc_std_io_file_roundtrip.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file("verb_e2e_gc_v2_roundtrip.tmp");
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

// ----- std map -----

#[test]
fn run_rejects_programs_with_std_map_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_map_basic.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std map"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_map() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_map_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_map_basic.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_map_basic.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn map_with_heap_valued_entries_retains_and_releases_correctly() {
    let out_path = std::env::temp_dir().join("verb_e2e_gc_map_heap_values_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/gc_map_heap_values.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/gc_map_heap_values.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn cross_build_links_a_program_using_std_map_for_a_non_host_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    // Unlike std io, std map has no Windows restriction (no POSIX-only
    // deps), so this also covers a Windows target rather than needing a
    // separate rejection test.
    let label = if cfg!(target_arch = "x86_64") { "linux-arm64" } else { "linux-x86_64" };
    let dir = std::env::temp_dir().join("verb_std_map_cross_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join(format!("std_map_basic_{label}"));
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_map_basic.verb",
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
fn reassign_and_short_circuit_release_correctly() { run_ok("gc_reassign_and_or"); }

#[test]
fn global_reassignment_releases_previous_value() { run_ok("gc_global_reassign"); }

#[test]
fn early_return_from_nested_loop_and_if_else_leaves_scopes_intact() { run_ok("gc_early_return_nested"); }

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

#[test]
fn gc_stress_all_kinds_leaks_nothing() { assert_no_leaks("gc_stress_all_kinds"); }

#[test]
fn gc_cyclic_array_leak_is_confined_not_corrupting() {
    // A self-referential array cannot be reclaimed by pure refcounting --
    // this is a known, accepted limitation (see the design spec's "cycle
    // limitation" section), resolved by a separate follow-up sub-project
    // (a backup cycle collector), not this one. This test's job is only
    // to prove the failure mode is a small, fixed, bounded leak -- the
    // cyclic array's own one block -- not unbounded growth, corruption,
    // or a crash.
    let out_path = std::env::temp_dir().join("verb_test_gc_v2_cyclic");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/gc_cyclic_array_leaks_confined.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).env("VERB_GC_DEBUG", "1").output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("3\n"), "unexpected program output:\n{stdout}");
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("no verb_gc_live line in stdout:\n{stdout}"));
    // Exactly the cyclic array's own header block leaks (its refcount
    // never reaches zero because it holds a reference to itself) -- a
    // small, fixed, non-zero number, not zero and not unbounded.
    assert_ne!(live_line, "verb_gc_live=0", "expected a confined leak, got none:\n{stdout}");
    let live_n: i64 = live_line.strip_prefix("verb_gc_live=").unwrap().parse().unwrap();
    assert!((1..=2).contains(&live_n), "expected a small, bounded leak count, got {live_n}:\n{stdout}");
    let _ = std::fs::remove_file(&out_path);
}

// ----- std thread -----

#[test]
fn run_rejects_programs_with_std_thread_import() {
	let out = Command::new(env!("CARGO_BIN_EXE_verb"))
		.args(["run", "tests/fixtures/std_thread_spawn_join.verb"])
		.output()
		.unwrap();
	assert!(!out.status.success());
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
	assert!(stderr.contains("std thread"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_spawn_join() {
	let out_path = std::env::temp_dir().join("verb_e2e_std_thread_spawn_join_bin");
	let build = Command::new(env!("CARGO_BIN_EXE_verb"))
		.args([
			"build", "tests/fixtures/std_thread_spawn_join.verb",
			"-o", out_path.to_str().unwrap(),
		])
		.output()
		.unwrap();
	assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

	let run = Command::new(&out_path).output().unwrap();
	assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
	let expected = std::fs::read_to_string("tests/fixtures/std_thread_spawn_join.expected").unwrap();
	assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

	let _ = std::fs::remove_file(&out_path);
}

#[test]
fn cross_build_rejects_std_thread_import_for_windows_target() {
	if !zig_available() {
		eprintln!("skipping: zig not on PATH");
		return;
	}
	let dir = std::env::temp_dir().join("verb_std_thread_windows_reject_test");
	std::fs::create_dir_all(&dir).unwrap();
	let bin = dir.join("std_thread_windows");
	let out = Command::new(env!("CARGO_BIN_EXE_verb"))
		.args([
			"build", "tests/fixtures/std_thread_spawn_join.verb",
			"-o", bin.to_str().unwrap(),
			"--target", "windows-x86_64",
		])
		.output()
		.unwrap();
	assert!(!out.status.success());
	let stderr = String::from_utf8_lossy(&out.stderr);
	assert!(stderr.contains("import std thread"), "stderr: {stderr}");
	assert!(stderr.contains("Windows"), "stderr: {stderr}");
}
