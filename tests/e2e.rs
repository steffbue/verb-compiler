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
    // build_aot isn't implemented until Task 6, so this still exits non-zero —
    // --emit-llvm prints IR to stdout before build_aot ever runs, so the IR
    // shape is already checkable here.
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
