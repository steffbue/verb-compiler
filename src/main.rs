use std::path::PathBuf;
use std::process::{exit, Command};

use verb::codegen;
use verb::debugger;
use verb::error::CompileError;
use verb::lexer;
use verb::parser;
use verb::targets;

// --- JIT runtime symbol resolution -----------------------------------------
//
// `src/codegen.rs` emits `verb_release_value` into *every* module, and its
// map branch calls `verb_map_destroy_contents` (defined in
// `runtime/verb_map.cpp`). There is no dead-code stripping anywhere in this
// pipeline, so that reference is always live — even in a program that never
// touches maps. For AOT builds we always link `runtime/verb_map.cpp` (below);
// for the JIT `run` path we compile that unit into this binary (see build.rs)
// and hand its address to MCJIT via `add_global_mapping` in
// `register_jit_runtime_symbols`. Add future
// always-referenced-but-conditionally-defined runtime symbols the same way:
// list the `.cpp` in build.rs, and add a tuple to `register_jit_runtime_symbols`.

// C++ ABI mirror of `VerbValue` (`{ i8, i64 }`) — see runtime/verb.h. Only
// used to give the host stubs below a matching signature; never populated.
#[repr(C)]
pub struct VerbValueAbi {
    pub tag: i8,
    pub payload: i64,
}

extern "C" {
    /// Defined in `runtime/verb_map.cpp`, compiled into this binary by build.rs.
    fn verb_map_destroy_contents(payload: *mut std::ffi::c_void);
}

// `runtime/verb_map.cpp` (linked into this binary) references these three
// symbols, but they are emitted into each *JIT module* by codegen, not the
// host — so the host linker has nothing to resolve them against. These
// definitions satisfy the link. They are never reached at runtime under
// `verb run`, which rejects imports (so no map value can exist, so
// `verb_map_destroy_contents` — the only host code that would call them —
// never runs). They abort loudly rather than silently corrupt state if that
// invariant is ever broken.
#[no_mangle]
pub extern "C" fn verb_alloc(_n: i64) -> *mut std::ffi::c_void {
    eprintln!("internal error: host verb_alloc stub called (verb run cannot use maps)");
    std::process::abort();
}
#[no_mangle]
pub extern "C" fn verb_retain_value(_v: VerbValueAbi) {
    eprintln!("internal error: host verb_retain_value stub called (verb run cannot use maps)");
    std::process::abort();
}
#[no_mangle]
pub extern "C" fn verb_release_value(_v: VerbValueAbi) {
    eprintln!("internal error: host verb_release_value stub called (verb run cannot use maps)");
    std::process::abort();
}

/// Registers runtime symbols that codegen references unconditionally but whose
/// definitions live in C++ runtime units compiled into this binary, so MCJIT
/// can resolve the reference for every `run` — including programs that never
/// exercise the symbol. Extend the array to add more such symbols.
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
    eprintln!("       verb debug <file.verb>");
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
    if parsed.cmd == "debug" {
        cg.enable_debug_hooks();
    }
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports).unwrap_or_else(|e| die(e, &sources));

    if parsed.emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match parsed.cmd.as_str() {
        "run" => {
            if !imports.is_empty() || !std_imports.is_empty() {
                let mut names = imports.clone();
                names.extend(std_imports.iter().map(|m| format!("std {m}")));
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    names.join(", ")
                );
                exit(1);
            }
            let ee = cg
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| {
                    eprintln!("JIT error: {e}");
                    exit(1);
                });
            register_jit_runtime_symbols(&ee, cg.module());
            unsafe {
                let main_fn = ee
                    .get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "debug" => {
            if !imports.is_empty() || !std_imports.is_empty() {
                let mut names = imports.clone();
                names.extend(std_imports.iter().map(|m| format!("std {m}")));
                eprintln!(
                    "error: 'verb debug' does not support imports ({}); use 'verb build' instead",
                    names.join(", ")
                );
                exit(1);
            }
            let ee = cg
                .module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| {
                    eprintln!("JIT error: {e}");
                    exit(1);
                });
            ee.add_global_mapping(
                &cg.module().get_function("verb_debug_checkpoint").unwrap(),
                debugger::verb_debug_checkpoint as *const () as usize,
            );
            ee.add_global_mapping(
                &cg.module().get_function("verb_debug_push_frame").unwrap(),
                debugger::verb_debug_push_frame as *const () as usize,
            );
            ee.add_global_mapping(
                &cg.module().get_function("verb_debug_pop_frame").unwrap(),
                debugger::verb_debug_pop_frame as *const () as usize,
            );
            let print_value_addr = ee.get_function_address("verb_print_value").unwrap_or_else(|e| {
                eprintln!("JIT error resolving verb_print_value: {e}");
                exit(1);
            });
            debugger::set_print_value_fn(print_value_addr);
            debugger::run_pre_start_console();
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
                None => build_aot_host(&cg, &out, &imports, &std_imports, &parsed.lib_dirs),
                Some("all") => build_aot_all(&cg, &out, &imports, &std_imports, &parsed.lib_dirs),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
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

/// Absolute paths into this crate's bundled `runtime/` dir, embedded at
/// compile time — `verb build` must find these regardless of the caller's
/// current directory, unlike a relative `runtime/verb_std_io.cpp`, which
/// only works when run from the repo root.
const RUNTIME_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime");
const STD_IO_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_io.cpp");
const MAP_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_map.cpp");
const STD_THREAD_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_thread.cpp");

/// Compiles the bundled `runtime/verb_std_io.cpp` into an object file with
/// `compiler` (`"cc"`/`"c++"` for the host, `"zig"` for cross targets),
/// prepending `extra_args` (e.g. `["c++", "-target", triple]` for zig).
/// Returns the object file's path on success.
fn compile_std_io_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_std_io_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", STD_IO_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {STD_IO_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {STD_IO_CPP}"));
    }
    Ok(obj)
}

/// Compiles the bundled `runtime/verb_map.cpp` into an object file. See
/// `compile_std_io_obj`.
fn compile_map_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_map_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", MAP_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {MAP_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {MAP_CPP}"));
    }
    Ok(obj)
}

/// Compiles the bundled `runtime/verb_std_thread.cpp` into an object
/// file. See `compile_std_io_obj`. `-pthread` is required by
/// `std::thread`/`std::mutex`/`std::condition_variable` on Linux
/// (glibc splits pthread symbols into a separate archive there); macOS's
/// libc++ links threading support unconditionally, so the flag is a
/// harmless no-op there, and is applied unconditionally rather than
/// gated on host OS to keep this function symmetric with its zig-cross
/// caller in `build_aot_cross`, which cannot check the *host*'s OS
/// (only the *target*'s).
fn compile_std_thread_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_std_thread_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-pthread", "-c", STD_THREAD_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {STD_THREAD_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {STD_THREAD_CPP}"));
    }
    Ok(obj)
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

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
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
    let std_thread_obj = if wants_std_thread {
        let extra_link_args: &[&str] = if cfg!(target_os = "linux") { &["-pthread"] } else { &[] };
        Some(compile_std_thread_obj(linker, extra_link_args).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            let _ = std::fs::remove_file(&map_obj);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };

    let mut cmd = Command::new(linker);
    cmd.arg(&obj).arg("-o").arg(out);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    cmd.arg(&map_obj);
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && cfg!(target_os = "linux") {
        cmd.arg("-pthread");
    }
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
            if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&map_obj);
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
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

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
    if wants_std_io && target.is_windows() {
        return Err(
            "'import std io' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std io'".to_string(),
        );
    }
    if wants_std_thread && target.is_windows() {
        return Err(
            "'import std thread' is not supported when cross-compiling to a Windows target in v1 \
             (std::thread isn't available under the mingw cross toolchain used here) -- build \
             natively on Windows instead, or drop 'import std thread'".to_string(),
        );
    }

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

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };
    // Always linked now — see build_aot_host for why verb_map.cpp is unconditional.
    let map_obj = compile_map_obj("zig", &["c++", "-target", target.zig_triple()])?;
    let std_thread_obj = if wants_std_thread {
        let extra: Vec<&str> = if target.os == targets::Os::Linux {
            vec!["c++", "-target", target.zig_triple(), "-pthread"]
        } else {
            vec!["c++", "-target", target.zig_triple()]
        };
        Some(compile_std_thread_obj("zig", &extra)?)
    } else {
        None
    };

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
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && target.os == targets::Os::Linux {
        cmd.arg("-pthread");
    }
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
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}

fn build_aot_all(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String]) {
    check_zig_available();
    let mut failures = 0;
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for target in targets::ALL {
        let labeled_out = format!("{out}-{}", target.label());
        let res = build_aot_cross(cg, &labeled_out, &target, imports, std_imports, lib_dirs);
        if res.is_err() {
            failures += 1;
        }
        results.push((target.label(), res));
    }
    println!("build --target all summary:");
    for (label, res) in &results {
        match res {
            Ok(()) => println!("  {label}: ok"),
            Err(e) => println!("  {label}: FAILED — {e}"),
        }
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

    #[test]
    fn parses_debug_command() {
        let p = parse_cli(&args(&["verb", "debug", "a.verb"])).unwrap();
        assert_eq!(p.cmd, "debug");
        assert_eq!(p.files, vec!["a.verb".to_string()]);
    }
}
