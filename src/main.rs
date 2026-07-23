use std::path::PathBuf;
use std::process::{exit, Command};
use std::sync::atomic::{AtomicUsize, Ordering};

use verb::codegen;
use verb::debugger;
use verb::error::CompileError;
use verb::lexer;
use verb::parser;
use verb::resolve;
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
    fn map_new() -> VerbValueAbi;
    fn map_set(m: VerbValueAbi, k: VerbValueAbi, v: VerbValueAbi) -> VerbValueAbi;
    fn map_get(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_has(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_remove(m: VerbValueAbi, k: VerbValueAbi) -> VerbValueAbi;
    fn map_len(m: VerbValueAbi) -> VerbValueAbi;
    fn read_line() -> VerbValueAbi;
    fn file_read(path: VerbValueAbi) -> VerbValueAbi;
    fn file_write(path: VerbValueAbi, contents: VerbValueAbi) -> VerbValueAbi;
    fn file_append(path: VerbValueAbi, contents: VerbValueAbi) -> VerbValueAbi;
    fn tcp_connect(host: VerbValueAbi, port: VerbValueAbi) -> VerbValueAbi;
    fn tcp_listen(port: VerbValueAbi) -> VerbValueAbi;
    fn tcp_accept(fd: VerbValueAbi) -> VerbValueAbi;
    fn send_line(fd: VerbValueAbi, s: VerbValueAbi) -> VerbValueAbi;
    fn recv_line(fd: VerbValueAbi) -> VerbValueAbi;
    fn close_conn(fd: VerbValueAbi) -> VerbValueAbi;
}

// Under `verb run` the program module (src/codegen.rs) emits the real
// verb_alloc/verb_retain_value/verb_release_value bodies. The C++ runtime
// units linked into this binary (verb_map.cpp, verb_std_io.cpp) call these
// symbols, and those calls bind here at host link time — so we forward them
// to the module's JIT-compiled definitions, whose addresses are stored below
// at JIT init (see the `run` arm) before `main` is ever called. This keeps a
// single source of truth for the value runtime and keeps `verb_gc_live`
// consistent regardless of whether an alloc/release originates in module code
// or in host C++.
static VERB_ALLOC_ADDR: AtomicUsize = AtomicUsize::new(0);
static VERB_RETAIN_ADDR: AtomicUsize = AtomicUsize::new(0);
static VERB_RELEASE_ADDR: AtomicUsize = AtomicUsize::new(0);

fn thunk_target(slot: &AtomicUsize, name: &str) -> usize {
    let a = slot.load(Ordering::Relaxed);
    if a == 0 {
        eprintln!("internal error: host {name} thunk called before JIT init");
        std::process::abort();
    }
    a
}

#[no_mangle]
pub extern "C" fn verb_alloc(n: i64) -> *mut std::ffi::c_void {
    let f: extern "C" fn(i64) -> *mut std::ffi::c_void =
        unsafe { std::mem::transmute(thunk_target(&VERB_ALLOC_ADDR, "verb_alloc")) };
    f(n)
}
#[no_mangle]
pub extern "C" fn verb_retain_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(thunk_target(&VERB_RETAIN_ADDR, "verb_retain_value")) };
    f(v)
}
#[no_mangle]
pub extern "C" fn verb_release_value(v: VerbValueAbi) {
    let f: extern "C" fn(VerbValueAbi) =
        unsafe { std::mem::transmute(thunk_target(&VERB_RELEASE_ADDR, "verb_release_value")) };
    f(v)
}

/// Registers runtime symbols that codegen references unconditionally but whose
/// definitions live in C++ runtime units compiled into this binary, so MCJIT
/// can resolve the reference for every `run` — including programs that never
/// exercise the symbol. Extend the array to add more such symbols.
fn register_jit_runtime_symbols<'ctx>(
    ee: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &inkwell::module::Module<'ctx>,
) {
    let symbols: [(&str, usize); 17] = [
        ("verb_map_destroy_contents", verb_map_destroy_contents as *const () as usize),
        ("map_new", map_new as *const () as usize),
        ("map_set", map_set as *const () as usize),
        ("map_get", map_get as *const () as usize),
        ("map_has", map_has as *const () as usize),
        ("map_remove", map_remove as *const () as usize),
        ("map_len", map_len as *const () as usize),
        ("read_line", read_line as *const () as usize),
        ("file_read", file_read as *const () as usize),
        ("file_write", file_write as *const () as usize),
        ("file_append", file_append as *const () as usize),
        ("tcp_connect", tcp_connect as *const () as usize),
        ("tcp_listen", tcp_listen as *const () as usize),
        ("tcp_accept", tcp_accept as *const () as usize),
        ("send_line", send_line as *const () as usize),
        ("recv_line", recv_line as *const () as usize),
        ("close_conn", close_conn as *const () as usize),
    ];
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
    file: String,
    out: Option<String>,
    emit_llvm: bool,
    target: Option<String>,
    /// Global library search dirs (`-L<dir>`), applied to every target.
    lib_dirs: Vec<String>,
    opt: u8,
    /// Per-target library search dirs (`-L<label>=<dir>`), each applied only
    /// to the matching `--target` label — lets a single `--target all` run
    /// supply a different, arch-matched library per target (INTEG-02).
    target_lib_dirs: Vec<(String, String)>,
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
    let mut opt = 0u8;
    let mut target_lib_dirs = Vec::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--emit-llvm" => {
                emit_llvm = true;
                i += 1;
            }
            "-O0" => { opt = 0; i += 1; }
            "-O1" => { opt = 1; i += 1; }
            "-O2" => { opt = 2; i += 1; }
            "-O3" => { opt = 3; i += 1; }
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
                // `-L<label>=<dir>` scopes a search dir to one `--target` label;
                // a plain `-L<dir>` (or any `=`-form whose prefix isn't a valid
                // target) stays global and applies to every target.
                match a[2..].split_once('=') {
                    Some((label, dir))
                        if !dir.is_empty() && targets::Target::parse(label).is_ok() =>
                    {
                        let canon = targets::Target::parse(label).unwrap().label();
                        target_lib_dirs.push((canon, format!("-L{dir}")));
                    }
                    _ => lib_dirs.push(a.to_string()),
                }
                i += 1;
            }
            f => {
                files.push(f.to_string());
                i += 1;
            }
        }
    }
    if files.len() != 1 {
        return None;
    }
    let file = files.remove(0);
    Some(ParsedArgs { cmd, file, out, emit_llvm, target, lib_dirs, opt, target_lib_dirs })
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
    eprintln!("usage: verb run <file.verb> [-L<dir>]... [-O0|-O1|-O2|-O3] [--emit-llvm]");
    eprintln!("       verb debug <file.verb>");
    eprintln!("       verb build <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [-L<target>=<dir>]... [-O0|-O1|-O2|-O3] [--emit-llvm]");
    eprintln!("       verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [-L<target>=<dir>]... [-O0|-O1|-O2|-O3] [--emit-llvm]  (alias for build)");
    eprintln!("       verb repl   (interactive read-eval-print loop; JIT, no imports)");
    eprintln!("       verb targets   (list supported cross-compile targets, marking the host)");
    eprintln!("       -O0..-O3 select the LLVM optimization level (default -O0)");
    eprintln!("       -L<dir>            library search dir applied to every target");
    eprintln!("       -L<target>=<dir>   library search dir applied only to that --target (e.g. -Llinux-arm64=./libs/arm); use with --target all to supply arch-matched libs per target");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    eprintln!("       use 'import mod <name>.verb;' inside <file.verb> to pull in other Verb source files");
    exit(2)
}

/// Prints the six supported cross-compile targets (label + LLVM triple),
/// marking the one matching this host with `(host)`. Dispatched before
/// `parse_cli` so it needs no `<file.verb>` argument.
fn print_targets() {
    use inkwell::targets::TargetMachine;
    let host_triple = TargetMachine::get_default_triple();
    let host_str = host_triple.as_str().to_string_lossy().into_owned();
    let host = targets::Target::from_host_triple(&host_str);
    for t in targets::ALL {
        let marker = if Some(t) == host { "  (host)" } else { "" };
        println!("{:<16} {}{}", t.label(), t.llvm_triple(), marker);
    }
}

/// Maps a `-O` level (0..=3) to the corresponding inkwell/LLVM codegen
/// optimization level. Used at both the JIT execution-engine creation and the
/// AOT `create_target_machine` sites so `run` and `build` agree on `-O`.
fn opt_level(o: u8) -> inkwell::OptimizationLevel {
    use inkwell::OptimizationLevel::{Aggressive, Default, Less, None};
    match o {
        0 => None,
        1 => Less,
        2 => Default,
        _ => Aggressive,
    }
}

/// Builds a `TargetMachine` for the host, used to run the module pass pipeline
/// on the JIT (`run`/`repl`) path, which otherwise has no `TargetMachine`.
/// `level` feeds the machine's own backend opt level. Mirrors the setup in
/// `build_aot_host`.
fn host_target_machine(level: u8) -> inkwell::targets::TargetMachine {
    use inkwell::targets::{CodeModel, InitializationConfig, RelocMode, Target, TargetMachine};
    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("target init error: {e}"); exit(1); });
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("target error: {e}"); exit(1); });
    target
        .create_target_machine(&triple, "generic", "",
            opt_level(level), RelocMode::PIC, CodeModel::Default)
        .unwrap_or_else(|| { eprintln!("cannot create target machine"); exit(1); })
}

/// Resolves an `import mod <name>` to a shared library file for the JIT to
/// dlopen. Searches each `-L<dir>` (prefix stripped) for `lib<name>.dylib`
/// (macOS) / `lib<name>.so` (Linux); if none exists on disk, returns the bare
/// filename so the OS loader can search its default paths. Static `.a`
/// archives are intentionally unsupported under `verb run` — use `verb build`.
///
/// Side effect: in the fallback branch, "resolving" the bare filename probes
/// for its existence by calling `load_library_permanently` on it, which — if
/// the library is found — permanently loads it into this process for the
/// remainder of its lifetime. This is idempotent (loading an already-loaded
/// library again is a no-op), and the `run` arm loads the resolved path again
/// for real afterward, so the extra load here is harmless, just a side effect
/// worth knowing about when reasoning about process state.
fn resolve_mod_lib(name: &str, lib_dirs: &[String]) -> Result<PathBuf, String> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let filename = format!("lib{name}.{ext}");
    let dirs: Vec<&str> = lib_dirs.iter().map(|d| d.trim_start_matches("-L")).collect();
    for dir in &dirs {
        let candidate = PathBuf::from(dir).join(&filename);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    // Fall back to the bare name only if the loader can find it on its default
    // search path; otherwise report a clear error naming the searched dirs.
    let bare = PathBuf::from(&filename);
    if inkwell::support::load_library_permanently(&bare).is_ok() {
        return Ok(bare);
    }
    Err(format!(
        "cannot find shared library for 'import mod {name}' ({filename}); searched: [{}]. \
         'verb run' can only load shared libraries — use 'verb build' for static linking.",
        dirs.join(", ")
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // `verb targets` takes no file argument, so it must be handled before
    // `parse_cli`, which returns `None` (→ usage) whenever `files` is empty.
    if args.get(1).map(String::as_str) == Some("targets") {
        print_targets();
        return;
    }

    // `verb repl` reads from stdin rather than a file, so it must be handled
    // before `parse_cli` (which requires at least one `<file.verb>`).
    if args.get(1).map(String::as_str) == Some("repl") {
        run_repl();
        return;
    }

    let parsed = parse_cli(&args).unwrap_or_else(|| usage());

    let resolved = resolve::resolve(&parsed.file).unwrap_or_else(|e| match e.kind {
        resolve::ResolveErrorKind::Compile(err) => die(err, &e.sources),
        resolve::ResolveErrorKind::Cycle(msg) => {
            eprintln!("error: {msg}");
            exit(1);
        }
        resolve::ResolveErrorKind::Io { path, message } => {
            eprintln!("error: cannot read {path}: {message}");
            exit(1);
        }
    });
    let sources = resolved.sources;
    let stmts = resolved.stmts;
    let stmt_files = resolved.stmt_files;
    let imports = resolved.imports;
    let std_imports = resolved.std_imports;
    let extern_sigs = resolved.extern_sigs;

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    if parsed.cmd == "debug" {
        cg.enable_debug_hooks();
    }
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports, &extern_sigs).unwrap_or_else(|e| die(e, &sources));

    match parsed.cmd.as_str() {
        "run" => {
            // `verb run` executes via JIT: it can wire `std io`/`std map`
            // runtime thunks (register_jit_runtime_symbols) and dlopen
            // `import mod` shared libraries, but it cannot link the C++
            // runtimes required by other std modules (e.g. `std thread`,
            // `std time`). Reject those with a clear message instead of
            // failing later during JIT symbol resolution.
            let unsupported: Vec<String> = std_imports
                .iter()
                .filter(|m| *m != "io" && *m != "map")
                .map(|m| format!("std {m}"))
                .collect();
            if !unsupported.is_empty() {
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    unsupported.join(", ")
                );
                exit(1);
            }
            // The JIT has no TargetMachine of its own, so `-O` runs the pass
            // pipeline against a host machine before building the engine (and
            // before printing IR, so `--emit-llvm` reflects the optimization).
            if parsed.opt > 0 {
                let tm = host_target_machine(parsed.opt);
                cg.optimize(&tm, parsed.opt).unwrap_or_else(|e| {
                    eprintln!("optimizer error: {e}");
                    exit(1);
                });
            }
            if parsed.emit_llvm {
                println!("{}", cg.module().print_to_string().to_string());
            }
            let ee = cg
                .module()
                .create_jit_execution_engine(opt_level(parsed.opt))
                .unwrap_or_else(|e| {
                    eprintln!("JIT error: {e}");
                    exit(1);
                });
            // Make the host process's own symbols searchable by MCJIT, then
            // dlopen each `import mod` shared library so its symbols resolve
            // during module finalization.
            inkwell::support::load_visible_symbols();
            for lib in &imports {
                let path = resolve_mod_lib(lib, &parsed.lib_dirs).unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    exit(1);
                });
                if inkwell::support::load_library_permanently(&path).is_err() {
                    eprintln!("error: failed to load shared library {}", path.display());
                    exit(1);
                }
            }
            // Map/io entry points must be wired before the module is finalized
            // (finalization happens on the first get_function_address below).
            register_jit_runtime_symbols(&ee, cg.module());
            // Point the host verb_alloc/retain/release thunks at the module's
            // JIT-compiled definitions. Only read at runtime (during main), so
            // it is fine that get_function_address finalizes the module here.
            //
            // Load-bearing invariant: codegen (src/codegen.rs build_alloc_fn /
            // build_retain_value_fn / build_release_value_fn) MUST emit a real
            // *body* for each of these three functions into every module, not
            // just a declaration. If codegen ever emitted only a declaration,
            // get_function_address would resolve the symbol back to this very
            // host thunk (since the thunk itself is exported under the same
            // name for AOT builds), and the thunk would call itself forever —
            // infinite recursion, stack overflow, with no diagnostic pointing
            // at the real cause.
            for (name, slot) in [
                ("verb_alloc", &VERB_ALLOC_ADDR),
                ("verb_retain_value", &VERB_RETAIN_ADDR),
                ("verb_release_value", &VERB_RELEASE_ADDR),
            ] {
                let addr = ee.get_function_address(name).unwrap_or_else(|e| {
                    eprintln!("JIT error: cannot resolve {name}: {e:?}");
                    exit(1);
                });
                slot.store(addr, Ordering::Relaxed);
            }
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
            // The JIT has no TargetMachine of its own, so `-O` runs the pass
            // pipeline against a host machine before building the engine (and
            // before printing IR, so `--emit-llvm` reflects the optimization).
            if parsed.opt > 0 {
                let tm = host_target_machine(parsed.opt);
                cg.optimize(&tm, parsed.opt).unwrap_or_else(|e| {
                    eprintln!("optimizer error: {e}");
                    exit(1);
                });
            }
            if parsed.emit_llvm {
                println!("{}", cg.module().print_to_string().to_string());
            }
            let ee = cg
                .module()
                .create_jit_execution_engine(opt_level(parsed.opt))
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
            // Matches `stmt_files`' entry for every top-level statement in
            // the entry file verbatim (see `resolve::resolve`), so an
            // unqualified `break <line>` resolves to this file -- see
            // `DebuggerState::add_breakpoint`.
            debugger::set_main_file(parsed.file.clone());
            debugger::run_pre_start_console();
            unsafe {
                let main_fn = ee
                    .get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "build" | "compile" => {
            // AOT paths run the pass pipeline against their own (possibly
            // cross) TargetMachine after the data layout is set, so IR emitted
            // here is pre-optimization.
            if parsed.emit_llvm {
                println!("{}", cg.module().print_to_string().to_string());
            }
            let out = parsed.out.unwrap_or_else(|| usage());
            match parsed.target.as_deref() {
                None => build_aot_host(&cg, &out, &imports, &std_imports, &parsed.lib_dirs, parsed.opt),
                Some("all") => build_aot_all(&cg, &out, &imports, &std_imports, &parsed.lib_dirs, &parsed.target_lib_dirs, parsed.opt),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
                    check_zig_available();
                    let lib_dirs = resolve_lib_dirs(&parsed.lib_dirs, &parsed.target_lib_dirs, &target.label());
                    if let Err(e) = build_aot_cross(&cg, &out, &target, &imports, &std_imports, &lib_dirs, parsed.opt) {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            }
        }
        _ => usage(),
    }
}

// --- REPL ------------------------------------------------------------------
//
// Strategy: "declaration replay, fresh module per turn" (the lowest-codegen-
// risk option in the Tier-4 plan). We keep a history of the input lines that
// were *pure definitions* (assign/declare/reassign/make/record/choice) and
// produced no observable output. Each turn we compile
// `history + "\n" + new_line` from scratch into a fresh Context+Codegen, JIT
// it like `verb run`, and call `main` WITHOUT exiting the process. Because the
// replayed history never prints, only the new line's output appears, and all
// program state (globals, function defs) is rebuilt deterministically every
// turn. Bare expressions are auto-printed by wrapping them as `print(<expr>)`.
// Imports are rejected (the JIT can't resolve `-l` libraries). Session values
// with side effects in their initializer (e.g. `assign x read_line();`) would
// re-run each turn -- documented as a v1 limitation.

/// True for statement kinds that define/mutate state without producing output,
/// and so are safe to replay verbatim on every REPL turn.
fn is_definition_stmt(s: &verb::ast::Stmt) -> bool {
    use verb::ast::Stmt::*;
    matches!(
        s,
        Assign { .. } | Declare { .. } | Reassign { .. } | Fn { .. } | Record { .. } | Choice { .. }
    )
}

/// True when `e` is already a `print(...)` call, so we don't double-wrap it.
fn is_print_call(e: &verb::ast::Expr) -> bool {
    if let verb::ast::Expr::Call { callee, .. } = e {
        if let verb::ast::Expr::Var(name, ..) = callee.as_ref() {
            return name == "print";
        }
    }
    false
}

/// Wraps a bare-expression entry so its value is printed: `x add 4` -> the
/// source `print(x add 4);`. A trailing `;` on the input is tolerated.
fn wrap_print(line: &str) -> String {
    format!("print({});", line.trim().trim_end_matches(';').trim())
}

/// Classifies a single REPL input line. Returns the source text to append to
/// the program this turn plus whether the line is a *pure definition* (and so
/// should be added to history on success). Rejects imports.
fn classify_repl_line(line: &str) -> Result<(String, bool), String> {
    use verb::ast::Stmt;
    let toks = lexer::lex(line).map_err(|e| e.msg)?;
    match parser::parse(toks) {
        Ok(prog) => {
            if !prog.imports.is_empty() || !prog.std_imports.is_empty() {
                return Err(
                    "imports are not supported in the REPL (v1); compile with `verb build` instead"
                        .to_string(),
                );
            }
            // A single bare expression that isn't already `print(...)` gets
            // auto-printed and is never retained in history.
            if prog.body.len() == 1 {
                if let Stmt::ExprStmt(e, ..) = &prog.body[0] {
                    if !is_print_call(e) {
                        return Ok((wrap_print(line), false));
                    }
                }
            }
            let pure = !prog.body.is_empty() && prog.body.iter().all(is_definition_stmt);
            Ok((line.to_string(), pure))
        }
        Err(e1) => {
            // A bare expression is often not a valid statement on its own
            // (needs a trailing `;`). Retry it wrapped in `print(...)`; if that
            // parses, treat it as an auto-printed expression.
            let wrapped = wrap_print(line);
            match lexer::lex(&wrapped).and_then(parser::parse) {
                Ok(_) => Ok((wrapped, false)),
                Err(_) => Err(e1.msg),
            }
        }
    }
}

/// Compiles `source` into a fresh module and JITs it, calling `main` once
/// (without exiting the process). Mirrors the `verb run` JIT path.
fn compile_and_jit_run(source: &str) -> Result<(), String> {
    let toks = lexer::lex(source).map_err(|e| e.msg)?;
    let prog = parser::parse(toks).map_err(|e| e.msg)?;
    let stmt_files = vec!["<repl>".to_string(); prog.body.len()];

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&prog.body, &stmt_files, &prog.imports, &prog.std_imports, &prog.extern_sigs)
        .map_err(|e| e.msg)?;

    let ee = cg
        .module()
        .create_jit_execution_engine(inkwell::OptimizationLevel::None)
        .map_err(|e| e.to_string())?;
    register_jit_runtime_symbols(&ee, cg.module());
    unsafe {
        let main_fn = ee
            .get_function::<unsafe extern "C" fn() -> i32>("main")
            .map_err(|e| e.to_string())?;
        main_fn.call();
    }
    // Flush so JIT'd C stdio output lands before the next prompt (interactive)
    // and in order (piped).
    use std::io::Write;
    let _ = std::io::stdout().flush();
    Ok(())
}

/// True when `src` has more `begin` than `end` tokens, i.e. an open block --
/// the REPL keeps reading continuation lines until a multi-line `make`/
/// `record`/`match`/`check`/`repeat` is complete. On a lex error (e.g. an
/// unterminated string mid-entry) we report the block closed and let the
/// parser surface the error rather than looping forever.
fn block_is_open(src: &str) -> bool {
    use verb::lexer::TokenKind::{Begin, End};
    match lexer::lex(src) {
        Ok(toks) => {
            let begins = toks.iter().filter(|t| t.kind == Begin).count();
            let ends = toks.iter().filter(|t| t.kind == End).count();
            begins > ends
        }
        Err(_) => false,
    }
}

/// Interactive read-eval-print loop. Prompts on stderr (so piped stdout holds
/// only program output), reads lines from stdin, and evaluates each entry with
/// declaration replay. Multi-line entries (`begin`..`end`) are buffered until
/// balanced. `:quit` / `:q` or EOF ends the session.
fn run_repl() {
    use std::io::{BufRead, Write};

    let prompt = |s: &str| {
        eprint!("{s}");
        let _ = std::io::stderr().flush();
    };

    let stdin = std::io::stdin();
    let mut history: Vec<String> = Vec::new();
    let mut buffer = String::new();

    prompt("verb> ");
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        // Top-level directives / blanks only apply when not mid-entry.
        if buffer.is_empty() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                prompt("verb> ");
                continue;
            }
            if trimmed == ":quit" || trimmed == ":q" {
                break;
            }
        }

        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(&line);

        // Keep reading continuation lines while a block is open.
        if block_is_open(&buffer) {
            prompt("  ... ");
            continue;
        }

        let entry = std::mem::take(&mut buffer);
        let entry = entry.trim().to_string();
        if entry.is_empty() {
            prompt("verb> ");
            continue;
        }

        match classify_repl_line(&entry) {
            Ok((tail, is_def)) => {
                let program = if history.is_empty() {
                    tail.clone()
                } else {
                    format!("{}\n{}", history.join("\n"), tail)
                };
                match compile_and_jit_run(&program) {
                    Ok(()) => {
                        // Retain only successful, pure definitions so the
                        // replayed prefix stays output-free and deterministic.
                        if is_def {
                            history.push(entry.clone());
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            }
            Err(e) => eprintln!("error: {e}"),
        }
        prompt("verb> ");
    }
    eprintln!();
}

/// Absolute paths into this crate's bundled `runtime/` dir, embedded at
/// compile time — `verb build` must find these regardless of the caller's
/// current directory, unlike a relative `runtime/verb_std_io.cpp`, which
/// only works when run from the repo root.
const RUNTIME_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime");
const STD_IO_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_io.cpp");
const STD_NET_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_net.cpp");
const MAP_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_map.cpp");
const STD_THREAD_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_thread.cpp");
const TIME_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_time.cpp");

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

/// Compiles the bundled `runtime/verb_std_net.cpp` into an object file.
/// See `compile_std_io_obj`.
fn compile_net_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_std_net_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", STD_NET_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {STD_NET_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {STD_NET_CPP}"));
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

/// Compiles the bundled `runtime/verb_time.cpp` into an object file. See
/// `compile_std_io_obj`.
fn compile_time_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_time_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-c", TIME_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {TIME_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {TIME_CPP}"));
    }
    Ok(obj)
}

fn build_aot_host(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String], opt: u8) {
    use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("target init error: {e}"); exit(1); });
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("target error: {e}"); exit(1); });
    let tm = target
        .create_target_machine(&triple, "generic", "",
            opt_level(opt), RelocMode::PIC, CodeModel::Default)
        .unwrap_or_else(|| { eprintln!("cannot create target machine"); exit(1); });
    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());
    cg.optimize(&tm, opt).unwrap_or_else(|e| { eprintln!("optimizer error: {e}"); exit(1); });

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_net = std_imports.iter().any(|m| m == "net");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
    let wants_time = std_imports.iter().any(|m| m == "time");
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
    let net_obj = if wants_net {
        Some(compile_net_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let map_obj = compile_map_obj(linker, &[]).unwrap_or_else(|e| {
        let _ = std::fs::remove_file(&obj);
        if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
        if let Some(p) = &net_obj { let _ = std::fs::remove_file(p); }
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
    let time_obj = if wants_time {
        Some(compile_time_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            let _ = std::fs::remove_file(&map_obj);
            if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
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
    if let Some(p) = &net_obj {
        cmd.arg(p);
    }
    cmd.arg(&map_obj);
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && cfg!(target_os = "linux") {
        cmd.arg("-pthread");
    }
    if let Some(p) = &time_obj {
        cmd.arg(p);
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
            if let Some(p) = &net_obj { let _ = std::fs::remove_file(p); }
            let _ = std::fs::remove_file(&map_obj);
            if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
            if let Some(p) = &time_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &net_obj { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&map_obj);
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &time_obj { let _ = std::fs::remove_file(p); }
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
    opt: u8,
) -> Result<(), String> {
    use inkwell::targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetTriple,
    };

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_net = std_imports.iter().any(|m| m == "net");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
    let wants_time = std_imports.iter().any(|m| m == "time");
    if wants_std_io && target.is_windows() {
        return Err(
            "'import std io' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std io'".to_string(),
        );
    }
    if wants_net && target.is_windows() {
        return Err(
            "'import std net' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std net'".to_string(),
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
            opt_level(opt), RelocMode::PIC, CodeModel::Default,
        )
        .ok_or_else(|| "cannot create target machine".to_string())?;
    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());
    cg.optimize(&tm, opt)?;

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };
    let net_obj = if wants_net {
        Some(compile_net_obj("zig", &["c++", "-target", target.zig_triple()])?)
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
    let time_obj = if wants_time {
        Some(compile_time_obj("zig", &["c++", "-target", target.zig_triple()])?)
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
    if let Some(p) = &net_obj {
        cmd.arg(p);
    }
    cmd.arg(&map_obj);
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && target.os == targets::Os::Linux {
        cmd.arg("-pthread");
    }
    if let Some(p) = &time_obj {
        cmd.arg(p);
    }
    // Per-target `-L` resolution: for `--target all` (and any single cross
    // build) each `-L<dir>` prefers a `<dir>/<label>` subdir holding that
    // target's single-arch libraries, falling back to the bare `<dir>` when
    // absent. Fixes arch-mismatch link failures when one flat `-L` served all
    // six targets. See Target::resolve_lib_dirs.
    let resolved = target.resolve_lib_dirs(lib_dirs);
    for dir in &resolved {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &net_obj { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&map_obj);
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &time_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}

/// Effective library search dirs for one target: the global `-L<dir>` set plus
/// any `-L<label>=<dir>` scoped to this target's label. Lets `--target all`
/// cross-link an FFI import against a different, arch-matched library per target.
fn resolve_lib_dirs(global: &[String], scoped: &[(String, String)], label: &str) -> Vec<String> {
    let mut dirs = global.to_vec();
    for (l, dir) in scoped {
        if l == label {
            dirs.push(dir.clone());
        }
    }
    dirs
}

fn build_aot_all(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String], target_lib_dirs: &[(String, String)], opt: u8) {
    check_zig_available();
    let mut failures = 0;
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for target in targets::ALL {
        let labeled_out = format!("{out}-{}", target.label());
        let effective = resolve_lib_dirs(lib_dirs, target_lib_dirs, &target.label());
        let res = build_aot_cross(cg, &labeled_out, &target, imports, std_imports, &effective, opt);
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
    fn parses_a_single_file() {
        let p = parse_cli(&args(&["verb", "run", "a.verb"])).unwrap();
        assert_eq!(p.cmd, "run");
        assert_eq!(p.file, "a.verb".to_string());
        assert!(!p.emit_llvm);
        assert_eq!(p.out, None);
    }

    #[test]
    fn rejects_multiple_files() {
        assert!(parse_cli(&args(&["verb", "run", "a.verb", "b.verb"])).is_none());
    }

    #[test]
    fn parses_flags_around_a_single_file() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "--emit-llvm",
        ])).unwrap();
        assert_eq!(p.cmd, "build");
        assert_eq!(p.file, "a.verb".to_string());
        assert_eq!(p.out, Some("out".to_string()));
        assert!(p.emit_llvm);
    }

    #[test]
    fn parses_lib_dirs() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "-L/opt/lib", "-L./libs",
        ])).unwrap();
        assert_eq!(p.file, "a.verb".to_string());
        assert_eq!(p.lib_dirs, vec!["-L/opt/lib".to_string(), "-L./libs".to_string()]);
        assert!(p.target_lib_dirs.is_empty());
    }

    #[test]
    fn parses_per_target_lib_dirs() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out",
            "-L/opt/lib",                      // global
            "-Llinux-arm64=./libs/arm",        // scoped
            "-Lmacos-x86_64=/opt/mac",         // scoped
            "-Llinux-x86=./libs/x86",          // scoped, x86 alias -> canonical label
        ])).unwrap();
        // Global stays in lib_dirs.
        assert_eq!(p.lib_dirs, vec!["-L/opt/lib".to_string()]);
        // Scoped land in target_lib_dirs, labels canonicalized (x86 -> x86_64).
        assert_eq!(
            p.target_lib_dirs,
            vec![
                ("linux-arm64".to_string(), "-L./libs/arm".to_string()),
                ("macos-x86_64".to_string(), "-L/opt/mac".to_string()),
                ("linux-x86_64".to_string(), "-L./libs/x86".to_string()),
            ]
        );
    }

    #[test]
    fn non_target_prefix_before_equals_stays_global() {
        // A dir path that happens to contain '=' but whose prefix isn't a valid
        // target label must remain a global search dir, verbatim.
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "-L/weird=path/lib",
        ])).unwrap();
        assert_eq!(p.lib_dirs, vec!["-L/weird=path/lib".to_string()]);
        assert!(p.target_lib_dirs.is_empty());
    }

    #[test]
    fn resolve_lib_dirs_merges_global_and_matching_scoped() {
        let global = vec!["-L/opt/lib".to_string()];
        let scoped = vec![
            ("linux-arm64".to_string(), "-L./arm".to_string()),
            ("macos-arm64".to_string(), "-L./mac".to_string()),
        ];
        assert_eq!(
            resolve_lib_dirs(&global, &scoped, "linux-arm64"),
            vec!["-L/opt/lib".to_string(), "-L./arm".to_string()]
        );
        // No scoped match -> global only.
        assert_eq!(
            resolve_lib_dirs(&global, &scoped, "linux-x86_64"),
            vec!["-L/opt/lib".to_string()]
        );
    }

    #[test]
    fn opt_defaults_to_zero() {
        let p = parse_cli(&args(&["verb", "run", "a.verb"])).unwrap();
        assert_eq!(p.opt, 0);
    }

    #[test]
    fn parses_opt_levels() {
        for (flag, want) in [("-O0", 0u8), ("-O1", 1), ("-O2", 2), ("-O3", 3)] {
            let p = parse_cli(&args(&["verb", "run", "a.verb", flag])).unwrap();
            assert_eq!(p.opt, want, "flag {flag}");
        }
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
    fn resolve_mod_lib_finds_shared_lib_in_l_dir() {
        let dir = std::env::temp_dir().join(format!("verb_resolve_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let lib = dir.join(format!("libwidget.{ext}"));
        std::fs::write(&lib, b"").unwrap();

        let found = resolve_mod_lib("widget", &[format!("-L{}", dir.display())]).unwrap();
        assert_eq!(found, lib);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_mod_lib_missing_reports_name_and_dirs() {
        let err = resolve_mod_lib("nope", &["-L/does/not/exist".to_string()]).unwrap_err();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("/does/not/exist"), "{err}");
    }

    #[test]
    fn parses_debug_command() {
        let p = parse_cli(&args(&["verb", "debug", "a.verb"])).unwrap();
        assert_eq!(p.cmd, "debug");
        assert_eq!(p.file, "a.verb".to_string());
    }
}
