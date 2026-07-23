use std::io::Write;
use std::process::{Command, Stdio};

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
fn assert_no_leaks(fixture: &str) { assert_no_leaks_opt(fixture, 0); }

/// Like `assert_no_leaks`, but builds at optimization `level` -- used to
/// confirm the pass pipeline (esp. DCE) never removes GC release calls.
fn assert_no_leaks_opt(fixture: &str, level: u8) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_v2_{fixture}_O{level}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            &format!("tests/fixtures/{fixture}.verb"),
            "-o",
            out_path.to_str().unwrap(),
            &format!("-O{level}"),
        ])
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

// ----- structs (`shape`) -----

#[test]
fn struct_construct_get_set_print() { run_ok("structs"); }

#[test]
fn struct_leaks_nothing() { assert_no_leaks("structs"); }

#[test]
fn struct_heap_fields_and_nesting() { run_ok("structs_heap"); }

#[test]
fn struct_heap_fields_leak_nothing() { assert_no_leaks("structs_heap"); }

#[test]
fn struct_construct_arity_mismatch_is_compile_error() {
    compile_err("structs_arity", &["Point", "2", "1"]);
}

#[test]
fn struct_unknown_field_aborts() {
    run_err("structs_badfield", "unknown field 'z'");
}

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

// ----- optimizer / -O flags -----

/// Runs `fixture` at optimization `level`, asserting success and that stdout
/// matches the fixture's `.expected` -- i.e. `-O` preserves observable output.
fn run_ok_opt(name: &str, level: u8) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb"), &format!("-O{level}")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "-O{level} {name}: exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "-O{level} {name}: output changed");
}

/// Emits the module IR for `fixture` at `level` via `run --emit-llvm`.
fn emit_ir(name: &str, level: u8) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb"), "--emit-llvm", &format!("-O{level}")])
        .output()
        .unwrap();
    assert!(out.status.success(), "-O{level} {name}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn opt_o2_reduces_allocas_vs_o0() {
    // Function-local `assign`s lower to stack `alloca`s at -O0; the -O2
    // pipeline's mem2reg promotes them to SSA values, so the optimized IR
    // has strictly fewer `alloca`s. This proves the pass pipeline actually
    // ran (and differs from the unoptimized emit).
    let o0 = emit_ir("opt_locals", 0);
    let o2 = emit_ir("opt_locals", 2);
    let n0 = o0.matches("alloca").count();
    let n2 = o2.matches("alloca").count();
    assert!(n0 > 1, "expected several allocas at -O0, got {n0}");
    assert!(n2 < n0, "-O2 should reduce allocas ({n2}) below -O0 ({n0})");
    assert_ne!(o0, o2, "-O2 IR should differ from -O0 IR");
}

#[test]
fn opt_preserves_semantics_all_levels() {
    // Same program, every -O level, identical output: optimization is
    // semantics-preserving for a compute-heavy fixture.
    for level in 0..=3 {
        run_ok_opt("arith", level);
        run_ok_opt("opt_locals", level);
    }
}

#[test]
fn opt_o2_keeps_gc_release_calls() {
    // Aggressive DCE must not drop the GC's `verb_release_value` calls (they
    // have observable side effects). Build at -O2 and confirm the program
    // still frees every heap object it allocated.
    assert_no_leaks_opt("gc_structs", 2);
    assert_no_leaks_opt("gc_closures_capture", 2);
}

// ----- REPL -----

/// Pipes `input` into `verb repl` and returns (stdout, success). Prompts are
/// written to stderr, so stdout carries only the evaluated program output.
fn repl_run(input: &str) -> (String, bool) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_verb"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.success())
}

#[test]
fn repl_scripted_session_prints_expected() {
    let input = std::fs::read_to_string("tests/fixtures/repl_session.in").unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/repl_session.expected").unwrap();
    let (stdout, ok) = repl_run(&input);
    assert!(ok, "repl exited non-zero");
    // Definition (`assign x 3;`) replays silently; only `print(x add 4)` emits.
    assert_eq!(stdout, expected);
}

#[test]
fn repl_auto_prints_bare_expression() {
    // A bare expression (no `print`, no trailing `;`) is auto-printed.
    let (stdout, ok) = repl_run("5 add 2\n:quit\n");
    assert!(ok);
    assert_eq!(stdout, "7\n");
}

#[test]
fn repl_replays_multiline_function_definition() {
    // A `make` spanning several lines is buffered until `end`, retained in
    // history, and callable on a later turn.
    let input = "make dbl(n) begin\n  return n times 2;\nend\ndbl(21)\n:quit\n";
    let (stdout, ok) = repl_run(input);
    assert!(ok);
    assert_eq!(stdout, "42\n");
}

#[test]
fn repl_rejects_imports_but_keeps_running() {
    // Imports are rejected (JIT can't resolve them); the error must not be
    // fatal -- a subsequent valid line still evaluates.
    let (stdout, ok) = repl_run("import mod foo;\nprint(9);\n:quit\n");
    assert!(ok);
    assert_eq!(stdout, "9\n");
}

// ----- structs / records -----

#[test]
fn structs_basic_construct_get_set_print() { run_ok("structs_basic"); }

#[test]
fn structs_nested_hold_structs_and_arrays() { run_ok("structs_nested"); }

#[test]
fn structs_reassign_and_heap_fields_leak_nothing() {
    run_ok("gc_structs");
    assert_no_leaks("gc_structs");
}

#[test]
fn record_unknown_field_aborts() {
    run_err("err_struct_field_unknown", "unknown field 'z'");
}

#[test]
fn struct_wrong_arity_is_a_compile_error() {
    compile_err("err_struct_arity", &["record 'Point' takes 2 field(s), got 1"]);
}

#[test]
fn field_of_nonstruct_aborts() {
    run_err("err_field_of_nonstruct", "'field access' needs a record, got int");
}

// ----- enums (choice) / pattern matching (match) -----

#[test]
fn enums_basic_construct_match_print() { run_ok("enums_basic"); }

#[test]
fn enums_match_binds_fields_and_otherwise() { run_ok("enums_match_bind"); }

#[test]
fn enums_nested_hold_structs_and_arrays() { run_ok("enums_nested"); }

#[test]
fn enums_reassign_and_heap_fields_leak_nothing() {
    run_ok("gc_enums");
    assert_no_leaks("gc_enums");
}

// ----- result-style error handling (built-in Ok/Err + is_err/err_kind/err_msg) -----

#[test]
fn result_ok_err_construct_predicates_and_match() { run_ok("result_ok_err"); }

#[test]
fn match_no_matching_variant_aborts() {
    run_err("err_match_no_variant", "no matching variant");
}

#[test]
fn arith() { run_ok("arith"); }

#[test]
fn strings() { run_ok("strings"); }

#[test]
fn string_methods_len_slice_index_split() { run_ok("strings_methods"); }

#[test]
fn string_split_result_array_leaks_nothing() { assert_no_leaks("gc_string_split"); }

#[test]
fn string_slice_out_of_bounds_aborts() {
    run_err("err_str_slice_bounds", "str_slice out of bounds: start=0 end=5 len=2");
}

#[test]
fn type_error_aborts() {
    run_err("err_types", "runtime error [1:9]: 'add' needs numbers, got int and string");
}

#[test]
fn div_zero_aborts() { run_err("err_divzero", "runtime error [1:9]: division by zero"); }

#[test]
fn int_overflow_aborts() { run_err("err_int_overflow", "integer overflow"); }

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
fn break_and_continue() { run_ok("break_continue"); }

#[test]
fn break_and_continue_release_loop_scopes() { assert_no_leaks("gc_break_continue"); }

#[test]
fn break_outside_loop_is_compile_error() {
    compile_err("err_break_outside", &["'leave' outside loop"]);
}

#[test]
fn continue_outside_loop_is_compile_error() {
    compile_err("err_continue_outside", &["'next' outside loop"]);
}

#[test]
fn break_in_fn_does_not_see_enclosing_loop() {
    compile_err("err_break_in_fn", &["'leave' outside loop"]);
}

#[test]
fn break_word_gets_rename_hint() {
    compile_err("err_break_word", &["'break' was renamed to 'leave'"]);
}

#[test]
fn foreach_over_range_counts_half_open() {
    run_ok("foreach_range");
}

#[test]
fn foreach_over_empty_range_runs_zero_times() {
    run_ok("foreach_range_empty");
}

#[test]
fn foreach_over_array_visits_every_element() {
    run_ok("foreach_array");
}

#[test]
fn foreach_over_array_is_leak_free() {
    assert_no_leaks("foreach_array");
}

#[test]
fn foreach_early_return_is_leak_free() {
    assert_no_leaks("foreach_array_early_return");
}

#[test]
fn foreach_early_return_output() {
    run_ok("foreach_array_early_return");
}

#[test]
fn foreach_over_non_iterable_is_runtime_error() {
    run_err("err_foreach_not_iterable", "cannot iterate int");
}

#[test]
fn foreach_over_string_visits_each_char() {
    run_ok("foreach_string");
}

#[test]
fn foreach_over_string_is_leak_free() {
    assert_no_leaks("foreach_string");
}

#[test]
fn foreach_over_empty_string_runs_zero_times() {
    run_ok("foreach_empty_string");
}

#[test]
fn foreach_over_empty_string_is_leak_free() {
    assert_no_leaks("foreach_empty_string");
}

/// Regression: `map_len(m)` after an `each` over an array/string must not
/// declare a duplicate LLVM symbol. The `each`-over-iterable lowering emits a
/// tag-dispatching `fe.map` block that references `map_len`; before the fix,
/// `gen_std_io_call` deduplicated only via its own `externs` cache (missing
/// that module-level declaration) and added a second `map_len`, which LLVM
/// renamed to `map_len.N` -> JIT segfault. Exercises the `verb run` path.
#[test]
fn map_len_after_foreach_runs() {
    run_ok("map_len_after_foreach");
}

/// Same regression under AOT: the duplicate `map_len.N` symbol is provided by
/// no runtime unit, so `verb build` failed at link time. Build and run.
#[test]
fn map_len_after_foreach_builds_and_runs() {
    let out_path = std::env::temp_dir().join("verb_test_map_len_after_foreach");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/map_len_after_foreach.verb",
            "-o", out_path.to_str().unwrap(),
            "-O2",
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/map_len_after_foreach.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn functions() { run_ok("functions"); }

#[test]
fn call_non_function_aborts() { run_err("err_call_nonfn", "can only call functions, got int"); }

#[test]
fn nested_make_sees_own_scope_and_top_level_globals() { run_ok("closures"); }

#[test]
fn nested_make_captures_enclosing_local() { run_ok("closures_capture_local"); }

#[test]
fn nested_make_captures_enclosing_param() { run_ok("closures_capture_param"); }

// A returned closure keeps its captured value alive past the frame that
// created it, and each closure carries its own independent snapshot.
#[test]
fn returned_closure_captures_outlive_the_frame() { run_ok("closures_counter"); }

// Capture is by value: mutating the inner copy leaves the outer var unchanged.
#[test]
fn captures_are_by_value_snapshots() { run_ok("closures_by_value"); }

// Capturing heap values (string/array) and dropping the closures (reassign +
// program exit) leaks nothing.
#[test]
fn capturing_closures_release_their_env() { assert_no_leaks("gc_closures_capture"); }

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
fn overflow_checked_arith_intrinsics_are_emitted() {
    // The array-push and string-concat size computations, and the language
    // integer arithmetic helpers, are all built unconditionally into every
    // module, so any fixture's IR carries the checked-arith intrinsics.
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    // Unsigned size computations: array cap*2 / new_cap*16 (push) and la+lb /
    // sum+1 (concat).
    assert!(ir.contains("@llvm.umul.with.overflow.i64"),
        "no unsigned checked-mul (array push growth) in IR:\n{ir}");
    assert!(ir.contains("@llvm.uadd.with.overflow.i64"),
        "no unsigned checked-add (concat / verb_alloc header) in IR:\n{ir}");
    // Signed language arithmetic: add/sub/mul.
    for sym in ["@llvm.sadd.with.overflow.i64", "@llvm.ssub.with.overflow.i64",
                "@llvm.smul.with.overflow.i64"] {
        assert!(ir.contains(sym), "missing signed checked-arith {sym} in IR:\n{ir}");
    }
}

#[test]
fn verb_alloc_has_oom_null_check() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    // The malloc result is null-checked and, on failure, an OOM block prints
    // "out of memory" and exits. Split into two blocks: null -> abort, else
    // store/GEP/return.
    assert!(ir.contains("icmp eq ptr") && ir.contains("null"),
        "no null-check on malloc result in IR:\n{ir}");
    assert!(ir.contains("out of memory"), "no OOM message in IR:\n{ir}");
    // The header add is itself overflow-checked (n+8 can't wrap).
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
fn typed_extern_arity_mismatch_is_a_compile_error() {
    // A declared `exposing` signature is checked statically against the call:
    // one declared parameter, two supplied arguments -> compile error.
    compile_err("err_typed_extern_arity", &[
        "extern fn 'c_native_sqrt' declared with 1 parameter(s) but called with 2",
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
            // wrap<const char*> now references verb_alloc (defensive-copy fix,
            // runtime/verb.h). verb_alloc is defined in the *host* verb
            // executable's generated module, not this dylib, so allow it to be
            // resolved at load time rather than failing the dylib link.
            "-undefined", "dynamic_lookup",
            "-o", lib_path.to_str().unwrap(),
            "tests/fixtures/cpp/mathlib.cpp",
        ])
        .status()
        .expect("failed to invoke c++ to build the mathlib test fixture");
    assert!(status.success(), "failed to compile tests/fixtures/cpp/mathlib.cpp");
    dir
}

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

#[test]
fn run_mixes_mod_std_io_and_std_map_imports() {
    // Combined JIT-import path: one `verb run` that pulls in an `import mod`
    // C++ FFI library, `import std io`, and `import std map` at once, proving
    // the mod extern, std-io and std-map runtime symbols all resolve and the
    // refcount forwarders stay consistent across all three.
    let _ = std::fs::remove_file("verb_jit_all_imports.tmp");
    let lib_dir = build_mathlib_fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/jit_all_imports.verb",
            &format!("-L{}", lib_dir.display()),
        ])
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/jit_all_imports.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    let _ = std::fs::remove_file("verb_jit_all_imports.tmp");
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
fn typed_extern_signatures_marshal_native_scalars() {
    // `exposing c_native_sqrt(float) -> float, c_native_muladd(int, int) -> int`
    // targets raw native (non-VerbValue) C symbols: args are unboxed to
    // double/i64, the result reboxed. The third call passes an int (16) where
    // a float is declared, exercising the int->float coercion in the unbox.
    let lib_dir = build_mathlib_fixture();
    build_and_run_ok("import_typed_extern", &lib_dir);
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
fn verb_std_net_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_std_net_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_std_net.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_std_net.cpp");
    assert!(status.success(), "runtime/verb_std_net.cpp failed to compile");
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

#[test]
fn verb_time_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_time_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_time.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_time.cpp");
    assert!(status.success(), "runtime/verb_time.cpp failed to compile");
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

#[test]
fn verb_file_import_links_and_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_b.verb"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn verb_file_import_emits_a_single_merged_llvm_module() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_b.verb", "--emit-llvm"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}

#[test]
fn verb_file_import_error_names_the_importing_file_not_the_imported_one() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_err_b.verb"])
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
fn verb_file_import_build_path_links_and_runs() {
    let dir = std::env::temp_dir().join("verb_import_build_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("verb_file_import_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/multifile_b.verb", "-o", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

#[test]
fn cli_rejects_more_than_one_entry_file() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_a.verb", "tests/fixtures/multifile_b.verb"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}

#[test]
fn verb_file_import_dedups_a_diamond_dependency() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/diamond_entry.verb"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/diamond.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn verb_file_import_cycle_is_a_compile_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/cycle_a.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("import cycle"), "stderr: {stderr}");
}

#[test]
fn verb_file_import_missing_file_is_a_clear_error() {
    let dir = std::env::temp_dir().join("verb_missing_import_test");
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("entry.verb");
    std::fs::write(&entry, "import mod nope.verb;\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", entry.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope.verb"), "stderr: {stderr}");
}

// ----- std io -----

#[test]
fn run_executes_std_io_file_roundtrip() {
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
    run_ok("std_io_file_roundtrip");
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
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
fn std_io_failure_returns_err_not_nil() {
    // The std-io failure contract: a function that fails (here `file_read` on
    // a missing file) yields a built-in `Err`, so `is_err` is true and
    // `err_msg` carries a message -- instead of the old nil sentinel.
    let _ = std::fs::remove_file("verb_e2e_missing_file_should_not_exist.tmp");
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_err_file_missing_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_err_file_missing.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_err_file_missing.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
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
fn build_links_and_runs_a_program_using_std_net_udp_loopback() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_net_udp_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_net_udp_loopback.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let expected = std::fs::read_to_string("tests/fixtures/std_net_udp_loopback.expected").unwrap();
    // UDP loopback can drop a datagram under load; re-run once before failing.
    let mut run = Command::new(&out_path).output().unwrap();
    if !run.status.success() || String::from_utf8_lossy(&run.stdout) != expected {
        run = Command::new(&out_path).output().unwrap();
    }
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn windows_cross_target_rejects_std_net_import() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let out_path = std::env::temp_dir().join("verb_e2e_std_net_windows_reject");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_net_udp_loopback.verb",
            "-o", out_path.to_str().unwrap(),
            "--target", "windows-x86_64",
        ])
        .output()
        .unwrap();
    assert!(!build.status.success());
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("'import std net' is not supported"),
        "unexpected stderr: {stderr}"
    );
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
fn run_executes_std_map_import() {
    run_ok("std_map_basic");
    assert_no_leaks_under_run("std_map_basic");
}

#[test]
fn run_executes_std_map_import_with_nested_containers() {
    run_ok("gc_run_nested_map");
    assert_no_leaks_under_run("gc_run_nested_map");
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
fn build_runs_foreach_over_map_keys() {
    let out_path = std::env::temp_dir().join("verb_e2e_foreach_map_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/foreach_map.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/foreach_map.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn foreach_over_map_is_leak_free() {
    assert_no_leaks("foreach_map");
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
fn map_keys_and_values_iterate_into_arrays() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_map_iteration_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_map_iteration.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_map_iteration.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn map_keys_and_values_arrays_retain_and_release_correctly() {
    // The arrays map_keys/map_values return own a retained copy of every
    // key/value; releasing them must cascade back to zero live objects.
    assert_no_leaks("std_map_iteration");
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

fn run_debug_session(prog: &str, script: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // parallel test threads share this process's pid, so the pid alone isn't
    // a unique directory name -- an earlier version of this helper collided
    // across concurrently-running debug_* tests, each clobbering the others'
    // t.verb mid-run.
    let dir = std::env::temp_dir().join(format!("verb_dbg_test_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("t.verb");
    std::fs::write(&file, prog).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_verb"))
        .arg("debug")
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn debug_breakpoint_and_print_variable() {
    let prog = "assign x 1;\nassign y 2;\nprint(x add y);\n";
    let out = run_debug_session(prog, "break 2\nrun\nprint x\ncontinue\nquit\n");
    assert!(out.contains("stopped at line 2"), "{out}");
    // "(vdb) 1" is the prompt immediately followed by `print x`'s output --
    // a bare `out.contains('1')` would also pass on "line 1" and give no
    // real signal that `print x` printed anything at all.
    assert!(out.contains("(vdb) 1"), "{out}");
}

#[test]
fn debug_step_through_statements() {
    let prog = "assign x 1;\nassign y 2;\nprint(x add y);\n";
    // `run` alone doesn't pause execution -- an initial breakpoint is
    // needed to get the first stop, exactly like gdb requires a
    // breakpoint before `run` if you want to stop at the very start.
    let out = run_debug_session(prog, "break 1\nrun\nstep\nstep\nquit\n");
    assert!(out.contains("stopped at line 1"), "{out}");
    assert!(out.contains("stopped at line 2"), "{out}");
}

#[test]
fn debug_backtrace_across_nested_call() {
    let prog = "make inner()\nbegin\n  print(1);\nend\nmake outer()\nbegin\n  inner();\nend\nouter();\n";
    // line 3 is 'print(1);' inside inner()
    let out = run_debug_session(prog, "break 3\nrun\nbacktrace\ncontinue\nquit\n");
    assert!(out.contains("stopped at line 3"), "{out}");
    assert!(out.contains("inner"), "{out}");
    assert!(out.contains("outer"), "{out}");
}

#[test]
fn debug_quit_mid_session_exits_cleanly() {
    let prog = "assign x 1;\nprint(x);\n";
    let out = run_debug_session(prog, "break 1\nrun\nquit\n");
    assert!(out.contains("stopped at line 1"), "{out}");
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
        "gc_map_heap_values", "gc_std_io_file_roundtrip", "std_thread_spawn_join",
        "structs_basic", "structs_nested", "gc_structs",
        "enums_basic", "enums_nested", "gc_enums",
        "closures_capture_local", "closures_capture_param",
        "closures_counter", "closures_by_value", "gc_closures_capture",
        "strings_methods", "gc_string_split",
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

/// Builds and runs `examples/integration_all.verb` in place (D-02: no
/// tests/fixtures/ duplicate) and asserts both zero GC leaks and the
/// program's own deterministic summary line -- folding D-03's output
/// check and D-04's leak check into one standalone, cross-cutting test
/// rather than appending to gc_no_leaks_across_all_heap_kinds.
#[test]
fn integration_example_zero_leaks() {
    let lib_dir = build_mathlib_fixture();
    let out_path = std::env::temp_dir().join("verb_test_integration_all");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "examples/integration_all.verb",
            "-o", out_path.to_str().unwrap(),
            &format!("-L{}", lib_dir.display()),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path)
        .env("VERB_GC_DEBUG", "1")
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);

    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "leaked heap objects:\n{stdout}");

    assert!(
        stdout.contains("integration_summary: ok"),
        "missing expected deterministic summary line in stdout:\n{stdout}"
    );

    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file("verb_integration_demo.tmp");
}

/// Compiles tests/fixtures/cpp/mathlib.cpp for a specific cross-compile
/// target via `zig c++ -target <triple>` and archives the resulting object
/// into a static `libmathlib.a` in a per-target directory (for `-L<dir>`).
///
/// A host-built libmathlib (see `build_mathlib_fixture`, which produces a
/// host `.dylib`) cannot link into a foreign-target binary -- see the
/// comment on `build_aot_cross` in src/main.rs. Each target needs its own
/// libmathlib built with the same zig triple used for the program link.
/// Callers must guard with `zig_available()` before invoking this.
fn build_mathlib_for_target(label: &str, zig_triple: &str) -> std::path::PathBuf {
    // Unique per call so concurrently-running tests (each cross-building the
    // same set of labels) never share a lib dir and race on `mathlib.o`.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NONCE: AtomicUsize = AtomicUsize::new(0);
    let n = NONCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("verb_e2e_cross_libs_{label}_{n}"));
    std::fs::create_dir_all(&dir).unwrap();

    let obj_path = dir.join("mathlib.o");
    let compile = Command::new("zig")
        .args([
            "c++",
            "-target", zig_triple,
            "-std=c++17",
            "-Iruntime",
            "-c",
            "tests/fixtures/cpp/mathlib.cpp",
            "-o", obj_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to invoke zig c++ to cross-compile the mathlib test fixture");
    assert!(
        compile.status.success(),
        "zig c++ failed to compile mathlib.cpp for target {label} ({zig_triple}): {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let lib_path = dir.join("libmathlib.a");
    let archive = Command::new("zig")
        .args(["ar", "rcs", lib_path.to_str().unwrap(), obj_path.to_str().unwrap()])
        .output()
        .expect("failed to invoke zig ar to archive libmathlib.a");
    assert!(
        archive.status.success(),
        "zig ar failed to archive libmathlib.a for target {label}: {}",
        String::from_utf8_lossy(&archive.stderr)
    );

    let _ = std::fs::remove_file(&obj_path);
    dir
}

/// Cross-compiles the FFI-importing integration example (D-01) for all 6
/// supported OS/arch targets: `examples/integration_all.verb` (full, with
/// std io) for the 4 non-Windows targets, `examples/integration_all_windows.verb`
/// (std-io-less) for the 2 Windows targets (D-06). Each target links a
/// target-matched libmathlib built via `build_mathlib_for_target` so the
/// `import mod mathlib;` FFI import resolves at cross-link time -- this is
/// exactly the failure mode a host-built libmathlib would hit (T-08-06).
///
/// Build-only (D-05): asserts each target's output artifact exists and is
/// non-empty; never executes a foreign-target binary. Skips cleanly when
/// zig is unavailable, matching `aot_cross_build_produces_binary_for_each_target`.
#[test]
fn integration_example_cross_builds_all_targets() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_e2e_integration_cross_test");
    std::fs::create_dir_all(&dir).unwrap();

    // (label, zig_triple) pairs matching src/targets.rs::ALL and Target::zig_triple().
    let targets: [(&str, &str); 6] = [
        ("linux-x86_64", "x86_64-linux-gnu"),
        ("linux-arm64", "aarch64-linux-gnu"),
        ("macos-x86_64", "x86_64-macos-none"),
        ("macos-arm64", "aarch64-macos-none"),
        ("windows-x86_64", "x86_64-windows-gnu"),
        ("windows-arm64", "aarch64-windows-gnu"),
    ];

    for (label, zig_triple) in targets {
        let is_windows = label.starts_with("windows");
        let source = if is_windows {
            "examples/integration_all_windows.verb"
        } else {
            "examples/integration_all.verb"
        };

        let lib_dir = build_mathlib_for_target(label, zig_triple);
        let bin = dir.join(format!("integration_cross_{label}"));

        let out = Command::new(env!("CARGO_BIN_EXE_verb"))
            .args([
                "build",
                source,
                "-o", bin.to_str().unwrap(),
                "--target", label,
                &format!("-L{}", lib_dir.display()),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "target {label} failed to build {source}: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Build-only: never invoke the produced artifact -- it targets a
        // foreign OS/arch and cannot run on this host (D-05).
        let expected_path = if is_windows {
            dir.join(format!("integration_cross_{label}.exe"))
        } else {
            bin
        };
        let meta = std::fs::metadata(&expected_path)
            .unwrap_or_else(|e| panic!("missing output for {label} at {expected_path:?}: {e}"));
        assert!(meta.len() > 0, "empty output for {label}");
    }
}

// --- Tier 2 Task 2: FFI string ABI (defensive-copy of `const char*` returns) ---

/// Regression for the SIGTRAP-on-retain FFI string bug: `c_shout` returns a
/// bare malloc'd `const char*`; assigning it to a Verb variable retains it,
/// which (pre-fix) writes an 8-byte refcount at ptr-8 into malloc metadata and
/// crashes (exit 133). With the defensive copy in wrap<const char*> the string
/// lives in a verb_alloc'd block and the assignment/print works.
#[test]
fn ffi_string_return_can_be_retained() {
    let lib_dir = build_mathlib_fixture();
    build_and_run_ok("ffi_string_retain", &lib_dir);
}

/// The defensive copy must not leak on the Verb side: the copied buffer is a
/// verb_alloc block and must reach refcount 0 by exit (verb_gc_live=0). The
/// callee's *original* malloc'd buffer leaks but is invisible to this counter
/// (it only tracks verb_alloc blocks), so the count stays 0.
#[test]
fn ffi_string_return_retain_is_leak_free() {
    let lib_dir = build_mathlib_fixture();
    let out_path = std::env::temp_dir().join("verb_test_ffi_string_retain_leak");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/ffi_string_retain.verb",
            "-o", out_path.to_str().unwrap(),
            &format!("-L{}", lib_dir.display()),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path)
        .env("VERB_GC_DEBUG", "1")
        .env("DYLD_LIBRARY_PATH", &lib_dir)
        .output()
        .unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "leaked heap objects:\n{stdout}");
    let _ = std::fs::remove_file(&out_path);
}

// --- Tier 4 Task A: `verb targets` command ---

/// `verb targets` lists all six supported targets and marks exactly one host.
#[test]
fn targets_command_lists_all_targets_and_marks_host() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["targets"])
        .output()
        .unwrap();
    assert!(out.status.success(), "verb targets failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    for label in ["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64", "windows-x86_64", "windows-arm64"] {
        assert!(stdout.contains(label), "missing target {label} in:\n{stdout}");
    }
    let host_markers = stdout.matches("(host)").count();
    assert_eq!(host_markers, 1, "expected exactly one (host) marker, got {host_markers}:\n{stdout}");
}

// --- Tier 2 Task 1: per-target `-L` resolution for cross builds ---

/// Builds `tests/fixtures/cpp/mathlib.cpp` for a specific cross target into a
/// caller-chosen directory as a static `libmathlib.a` (for `-L<dir>`), used to
/// exercise per-target `-L` subdir resolution. Callers must guard with
/// `zig_available()`.
fn build_mathlib_into(dir: &std::path::Path, zig_triple: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let obj_path = dir.join("mathlib.o");
    let compile = Command::new("zig")
        .args([
            "c++", "-target", zig_triple, "-std=c++17", "-Iruntime", "-c",
            "tests/fixtures/cpp/mathlib.cpp", "-o", obj_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to invoke zig c++");
    assert!(compile.status.success(), "zig c++ failed: {}", String::from_utf8_lossy(&compile.stderr));
    let lib_path = dir.join("libmathlib.a");
    let archive = Command::new("zig")
        .args(["ar", "rcs", lib_path.to_str().unwrap(), obj_path.to_str().unwrap()])
        .output()
        .expect("failed to invoke zig ar");
    assert!(archive.status.success(), "zig ar failed: {}", String::from_utf8_lossy(&archive.stderr));
    let _ = std::fs::remove_file(&obj_path);
}

/// Maps this host to a (label, zig_triple) pair for cross-build-to-host tests,
/// or `None` when the host os/arch isn't one of the six supported targets.
fn host_label_and_zig_triple() -> Option<(String, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let label = format!("{os}-{arch}");
    let target = verb::targets::Target::parse(&label).ok()?;
    Some((label, target.zig_triple()))
}

/// With `--target <host-label>`, a `-L<dir>` where the matching library lives
/// only in the per-target subdir `<dir>/<label>` must still resolve — proving
/// `Target::resolve_lib_dirs` rewrites the token to the subdir. If it didn't,
/// `-lmathlib` would be unresolved and the link would fail.
#[test]
fn cross_build_resolves_per_target_lib_subdir() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let Some((label, zig_triple)) = host_label_and_zig_triple() else {
        eprintln!("skipping: host os/arch not a supported target");
        return;
    };
    let root = std::env::temp_dir().join("verb_e2e_per_target_L_subdir");
    let _ = std::fs::remove_dir_all(&root);
    let libs = root.join("libs");
    build_mathlib_into(&libs.join(&label), zig_triple);

    let bin = root.join("app");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/import_mathlib.verb",
            "-o", bin.to_str().unwrap(),
            "--target", &label,
            &format!("-L{}", libs.display()),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "build with per-target -L subdir failed (resolve_lib_dirs did not pick <dir>/{label}): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta = std::fs::metadata(&bin).expect("missing output binary");
    assert!(meta.len() > 0, "empty output binary");
    let _ = std::fs::remove_dir_all(&root);
}

/// Flat `-L<dir>` layout (library directly in `<dir>`, no per-target subdir)
/// keeps working: `resolve_lib_dirs` falls back to the bare token. Backward
/// compatibility guard for the per-target resolution change.
#[test]
fn cross_build_flat_lib_dir_still_works() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let Some((label, zig_triple)) = host_label_and_zig_triple() else {
        eprintln!("skipping: host os/arch not a supported target");
        return;
    };
    let root = std::env::temp_dir().join("verb_e2e_flat_L_dir");
    let _ = std::fs::remove_dir_all(&root);
    let libs = root.join("libs");
    build_mathlib_into(&libs, zig_triple); // flat: no per-target subdir

    let bin = root.join("app");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/import_mathlib.verb",
            "-o", bin.to_str().unwrap(),
            "--target", &label,
            &format!("-L{}", libs.display()),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "build with flat -L dir failed (fallback broken): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta = std::fs::metadata(&bin).expect("missing output binary");
    assert!(meta.len() > 0, "empty output binary");
    let _ = std::fs::remove_dir_all(&root);
}


/// Proves the literal `verb build --target all` invocation named in ROADMAP
/// Success Criterion 2 (INTEG-02): a SINGLE `--target all` run cross-links an
/// FFI-importing program for all 6 targets, each against its own arch-matched
/// libmathlib supplied via the `-L<target>=<dir>` per-target convention.
///
/// Uses `examples/integration_all_windows.verb` (no `std io`) so all 6 targets
/// are linkable in one program -- the full `integration_all.verb` carries
/// `import std io`, which by design cannot cross-compile to Windows, and that
/// pre-existing exception is covered by the per-target-loop test above. Here we
/// want a clean all-6-succeed summary that isolates the per-target-lib fix.
///
/// Before this fix, `--target all` broadcast one shared `-L` set to every
/// target, so at most one target's library was arch-compatible and the rest
/// failed with linker architecture-mismatch errors. Build-only (D-05).
#[test]
fn target_all_resolves_per_target_libs() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_e2e_target_all_per_lib");
    std::fs::create_dir_all(&dir).unwrap();

    // (label, zig_triple) matching src/targets.rs::ALL / Target::zig_triple().
    let targets: [(&str, &str); 6] = [
        ("linux-x86_64", "x86_64-linux-gnu"),
        ("linux-arm64", "aarch64-linux-gnu"),
        ("macos-x86_64", "x86_64-macos-none"),
        ("macos-arm64", "aarch64-macos-none"),
        ("windows-x86_64", "x86_64-windows-gnu"),
        ("windows-arm64", "aarch64-windows-gnu"),
    ];

    let out = dir.join("intg_all");

    // One arch-matched libmathlib per target, each scoped via `-L<label>=<dir>`.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_verb"));
    cmd.args([
        "build",
        "examples/integration_all_windows.verb",
        "-o",
        out.to_str().unwrap(),
        "--target",
        "all",
    ]);
    for (label, zig_triple) in targets {
        let lib_dir = build_mathlib_for_target(label, zig_triple);
        cmd.arg(format!("-L{label}={}", lib_dir.display()));
    }

    let res = cmd.output().unwrap();
    assert!(
        res.status.success(),
        "`--target all` with per-target libs failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&res.stdout),
        String::from_utf8_lossy(&res.stderr),
    );

    // Every target's labeled artifact exists and is non-empty (build_aot_all
    // writes `<out>-<label>`; windows gets `.exe` appended by adjust_output).
    for (label, _) in targets {
        let path = if label.starts_with("windows") {
            dir.join(format!("intg_all-{label}.exe"))
        } else {
            dir.join(format!("intg_all-{label}"))
        };
        let meta = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("missing --target all output for {label} at {path:?}: {e}"));
        assert!(meta.len() > 0, "empty --target all output for {label}");
    }
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
fn build_links_and_runs_a_program_using_std_thread_mutex() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_mutex_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_mutex.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_mutex.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_channel() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_channel_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_channel.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_channel.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn channel_send_rejects_a_non_primitive_value() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_channel_reject_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_channel_rejects_non_primitive.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_channel_rejects_non_primitive.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_sleep() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_sleep_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_sleep.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_sleep.expected").unwrap();
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

// ----- std time -----

#[test]
fn run_rejects_programs_with_std_time_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_time_basic.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std time"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_time() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_time_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_time_basic.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_time_basic.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn cross_build_links_a_program_using_std_time_for_a_non_host_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    // Like std map, std time has no Windows restriction (no POSIX-only
    // deps -- <chrono>/<thread> are portable), so this also covers a
    // Windows target rather than needing a separate rejection test.
    let label = if cfg!(target_arch = "x86_64") { "linux-arm64" } else { "linux-x86_64" };
    let dir = std::env::temp_dir().join("verb_std_time_cross_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join(format!("std_time_basic_{label}"));
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_time_basic.verb",
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

// `linux_*`/`win_*` are only defined in runtime/verb_time.cpp under
// __linux__/_WIN32 respectively (see TIME_FUNCS's doc comment in
// src/codegen.rs) -- these two tests cross-build (via zig, whose clang
// frontend sets the right predefined macros for -target regardless of
// host OS) to confirm each platform's functions actually compile and
// link for that platform. Not run, same as the other cross-build tests
// (foreign arch/OS binaries can't execute on this host).

#[test]
fn cross_build_links_a_program_using_linux_only_time_functions() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_std_time_linux_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("std_time_linux_x86_64");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_time_linux.verb",
            "-o", bin.to_str().unwrap(),
            "--target", "linux-x86_64",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "linux-x86_64 build failed: {}", String::from_utf8_lossy(&out.stderr));
    let meta = std::fs::metadata(&bin)
        .unwrap_or_else(|e| panic!("missing output at {bin:?}: {e}"));
    assert!(meta.len() > 0, "empty output for linux-x86_64");
}

#[test]
fn cross_build_links_a_program_using_windows_only_time_functions() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_std_time_windows_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("std_time_windows_x86_64");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_time_windows.verb",
            "-o", bin.to_str().unwrap(),
            "--target", "windows-x86_64",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "windows-x86_64 build failed: {}", String::from_utf8_lossy(&out.stderr));
    let expected_path = dir.join("std_time_windows_x86_64.exe");
    let meta = std::fs::metadata(&expected_path)
        .unwrap_or_else(|e| panic!("missing output at {expected_path:?}: {e}"));
    assert!(meta.len() > 0, "empty output for windows-x86_64");
}
