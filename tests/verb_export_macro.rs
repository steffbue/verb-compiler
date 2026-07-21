use std::io::Write;
use std::process::Command;

/// Compiles `snippet` (arbitrary top-level C++ code) as a standalone
/// translation unit against `runtime/verb.h`, which is `#include`d
/// automatically. Returns (compile succeeded, captured stderr).
fn compile_snippet(name: &str, snippet: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join("verb_export_macro_test");
    std::fs::create_dir_all(&dir).unwrap();
    let src_path = dir.join(format!("{name}.cpp"));
    let obj_path = dir.join(format!("{name}.o"));
    let mut f = std::fs::File::create(&src_path).unwrap();
    writeln!(f, "#include \"verb.h\"\n{snippet}").unwrap();
    let out = Command::new("c++")
        .args([
            "-std=c++17",
            "-Iruntime",
            "-c",
            "-o", obj_path.to_str().unwrap(),
            src_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to invoke c++");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&obj_path);
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn exports_a_cast_stdlib_function() {
    let (ok, stderr) = compile_snippet("export_sqrt", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 1, static_cast<double(*)(double)>(std::sqrt))
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_plain_two_arg_function() {
    let (ok, stderr) = compile_snippet("export_add", r#"
        int64_t add_int(int64_t a, int64_t b) { return a + b; }
        VERB_EXPORT(c_add_int, 2, add_int)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_zero_arity_void_function() {
    let (ok, stderr) = compile_snippet("export_void", r#"
        #include <cstdio>
        void say_hello() { std::printf("hi\n"); }
        VERB_EXPORT(c_hello, 0, say_hello)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_bool_returning_function() {
    let (ok, stderr) = compile_snippet("export_bool", r#"
        int is_positive(int64_t n) { return n > 0; }
        VERB_EXPORT(c_is_positive, 1, is_positive)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn arity_mismatch_fails_to_compile() {
    let (ok, stderr) = compile_snippet("bad_arity", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 2, static_cast<double(*)(double)>(std::sqrt))
    "#);
    assert!(!ok, "expected compile failure for arity mismatch");
    assert!(
        stderr.contains("VERB_EXPORT arity does not match callable's parameter count"),
        "stderr: {stderr}"
    );
}

#[test]
fn unsupported_parameter_type_fails_to_compile() {
    let (ok, stderr) = compile_snippet("bad_param_type", r#"
        float half(float x) { return x / 2; }
        VERB_EXPORT(c_half, 1, half)
    "#);
    assert!(!ok, "expected compile failure for unsupported type");
    assert!(
        stderr.contains("VERB_EXPORT: unsupported parameter type")
            || stderr.contains("VERB_EXPORT: unsupported return type"),
        "stderr: {stderr}"
    );
}

#[test]
fn bare_overloaded_callable_fails_to_compile() {
    let (ok, _stderr) = compile_snippet("bad_overload", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 1, std::sqrt)
    "#);
    assert!(!ok, "expected compile failure for bare overloaded name");
}

#[test]
fn lambda_callable_fails_to_compile() {
    let (ok, _stderr) = compile_snippet("bad_lambda", r#"
        auto lam = [](int64_t x) -> int64_t { return x; };
        VERB_EXPORT(c_lam, 1, lam)
    "#);
    assert!(!ok, "expected compile failure for lambda callable");
}
