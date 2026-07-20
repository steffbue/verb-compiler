use std::process::{exit, Command};

use verb::codegen;
use verb::error::CompileError;
use verb::lexer;
use verb::parser;
use verb::targets;

fn check_zig_available() {
    let ok = std::process::Command::new("zig")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!(
            "zig not found on PATH. Cross-compiling requires zig (https://ziglang.org/download/) as the linker driver. Install it, or omit --target to build for this host with cc."
        );
        exit(1);
    }
}

struct ParsedArgs {
    cmd: String,
    files: Vec<String>,
    out: Option<String>,
    emit_llvm: bool,
    target: Option<String>,
    lib_dirs: Vec<String>,
}

fn parse_cli(args: &[String]) -> Option<ParsedArgs> {
    if args.len() < 2 {
        return None;
    }
    let cmd = args[1].clone();
    let mut files = Vec::new();
    let mut out = None;
    let mut emit_llvm = false;
    let mut target = None;
    let mut lib_dirs = Vec::new();
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
            "--target" => {
                i += 1;
                if i >= args.len() {
                    return None;
                }
                target = Some(args[i].clone());
                i += 1;
            }
            a if a.starts_with("-L") && a.len() > 2 => {
                lib_dirs.push(a.to_string());
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
    Some(ParsedArgs { cmd, files, out, emit_llvm, target, lib_dirs })
}

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
    eprintln!("       verb build <file.verb>... -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]");
    eprintln!("       verb compile <file.verb>... -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]  (alias for build)");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_cli(&args).unwrap_or_else(|| usage());

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut stmts = Vec::new();
    let mut stmt_files = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut std_imports: Vec<String> = Vec::new();

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
        let prog = parser::parse(toks)
            .map_err(|e| e.with_file(file.clone()))
            .unwrap_or_else(|e| die(e, &sources));

        stmt_files.extend(std::iter::repeat(file.clone()).take(prog.body.len()));
        stmts.extend(prog.body);
        imports.extend(prog.imports);
        std_imports.extend(prog.std_imports);
    }

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports).unwrap_or_else(|e| die(e, &sources));

    if parsed.emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match parsed.cmd.as_str() {
        "run" => {
            if !imports.is_empty() {
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    imports.join(", ")
                );
                exit(1);
            }
            if !std_imports.is_empty() {
                eprintln!("error: std imports require 'verb build' — JIT does not support std io calls in v1");
                exit(1);
            }
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
        "build" | "compile" => {
            let out = parsed.out.unwrap_or_else(|| usage());
            match parsed.target.as_deref() {
                None => {
                    if !std_imports.is_empty() && cfg!(target_os = "windows") {
                        reject_std_io_on_windows();
                    }
                    build_aot_host(&cg, &out, &imports, &std_imports, &parsed.lib_dirs)
                }
                Some("all") => build_aot_all(&cg, &out, &imports, &std_imports, &parsed.lib_dirs),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
                    if !std_imports.is_empty() && target.is_windows() {
                        reject_std_io_on_windows();
                    }
                    check_zig_available();
                    if let Err(e) = build_aot_cross(&cg, &out, &target, &imports, &std_imports, &parsed.lib_dirs) {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            }
        }
        _ => usage(),
    }
}

/// `runtime/verb_std_io.cpp` is implemented against POSIX sockets, not
/// Winsock, so it can't be compiled for a Windows target in v1 — reject
/// with a clear message before invoking a C++ compiler that would only
/// fail later with a confusing header error.
fn reject_std_io_on_windows() -> ! {
    eprintln!("error: 'import std io' is not supported when targeting Windows in v1");
    eprintln!("reason: runtime/verb_std_io.cpp uses POSIX sockets, not Winsock");
    exit(1);
}

/// Compiles `runtime/verb_std_io.cpp` into an object file at `obj_path`.
/// `compiler` is `"c++"` for host builds or `"zig"` for cross builds, with
/// `extra_args` supplying whatever leading args that compiler needs (e.g.
/// `["cc", "-target", triple]` for zig).
fn compile_std_io_obj(compiler: &str, extra_args: &[&str], obj_path: &str) -> Result<(), String> {
    let status = Command::new(compiler)
        .args(extra_args)
        .args(["-std=c++17", "-Iruntime", "-c", "runtime/verb_std_io.cpp", "-o", obj_path])
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile runtime/verb_std_io.cpp: {e}"))?;
    if !status.success() {
        return Err("failed to compile runtime/verb_std_io.cpp".to_string());
    }
    Ok(())
}

fn build_aot_host(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String]) {
    use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("target init error: {e}"); exit(1); });
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("target error: {e}"); exit(1); });
    let tm = target
        .create_target_machine(&triple, "generic", "",
            inkwell::OptimizationLevel::Default, RelocMode::PIC, CodeModel::Default)
        .unwrap_or_else(|| { eprintln!("cannot create target machine"); exit(1); });
    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let uses_std_io = !std_imports.is_empty();
    let std_io_obj = format!("{out}.std_io.o");
    let mut obj_files = vec![obj.clone()];
    if uses_std_io {
        if let Err(e) = compile_std_io_obj("c++", &[], &std_io_obj) {
            let _ = std::fs::remove_file(&obj);
            eprintln!("error: {e}");
            exit(1);
        }
        obj_files.push(std_io_obj.clone());
    }

    let linker = if imports.is_empty() && !uses_std_io { "cc" } else { "c++" };
    let mut cmd = Command::new(linker);
    for obj_file in &obj_files {
        cmd.arg(obj_file);
    }
    cmd.arg("-o").arg(out);
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
            if uses_std_io { let _ = std::fs::remove_file(&std_io_obj); }
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if uses_std_io { let _ = std::fs::remove_file(&std_io_obj); }
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}

fn build_aot_cross(
    cg: &codegen::Codegen,
    out: &str,
    target: &targets::Target,
    imports: &[String],
    std_imports: &[String],
    lib_dirs: &[String],
) -> Result<(), String> {
    use inkwell::targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetTriple,
    };

    LlvmTarget::initialize_all(&InitializationConfig::default());
    let triple = TargetTriple::create(target.llvm_triple());
    let llvm_target = LlvmTarget::from_triple(&triple).map_err(|e| format!("target error: {e}"))?;
    let tm = llvm_target
        .create_target_machine(
            &triple, "generic", "",
            inkwell::OptimizationLevel::Default, RelocMode::PIC, CodeModel::Default,
        )
        .ok_or_else(|| "cannot create target machine".to_string())?;
    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    let uses_std_io = !std_imports.is_empty();
    let std_io_obj = format!("{out}.std_io.o");
    let mut obj_files = vec![obj.clone()];
    if uses_std_io {
        if let Err(e) = compile_std_io_obj("zig", &["cc", "-target", target.zig_triple()], &std_io_obj) {
            let _ = std::fs::remove_file(&obj);
            return Err(e);
        }
        obj_files.push(std_io_obj.clone());
    }

    // Imports/lib_dirs are forwarded to zig cc so cross-linking works when the imported
    // C++ libraries are available for the chosen target via -L<dir>. Host-built .o/.a
    // fixtures won't link for a foreign target — that requires target-built libraries.
    let mut cmd = Command::new("zig");
    cmd.args(&["cc", "-target", target.zig_triple(), "-o", out.as_str()]);
    for obj_file in &obj_files {
        cmd.arg(obj_file);
    }
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if uses_std_io { let _ = std::fs::remove_file(&std_io_obj); }
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}

fn build_aot_all(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String]) {
    check_zig_available();
    let uses_std_io = !std_imports.is_empty();
    let mut failures = 0;
    let mut results: Vec<(String, String)> = Vec::new();
    for target in targets::ALL {
        if uses_std_io && target.is_windows() {
            results.push((
                target.label(),
                "skipped — 'import std io' not supported on Windows in v1".to_string(),
            ));
            continue;
        }
        let labeled_out = format!("{out}-{}", target.label());
        match build_aot_cross(cg, &labeled_out, &target, imports, std_imports, lib_dirs) {
            Ok(()) => results.push((target.label(), "ok".to_string())),
            Err(e) => {
                failures += 1;
                results.push((target.label(), format!("FAILED — {e}")));
            }
        }
    }
    println!("build --target all summary:");
    for (label, msg) in &results {
        println!("  {label}: {msg}");
    }
    if failures > 0 {
        exit(1);
    }
}

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
    fn parses_lib_dirs() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "-L/opt/lib", "-L./libs",
        ])).unwrap();
        assert_eq!(p.files, vec!["a.verb".to_string()]);
        assert_eq!(p.lib_dirs, vec!["-L/opt/lib".to_string(), "-L./libs".to_string()]);
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
