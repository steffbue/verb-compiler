mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod targets;
mod value;

use std::process::exit;

use error::CompileError;

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

fn die(e: CompileError, src: &str) -> ! {
    eprintln!("error [{}:{}]: {}", e.line, e.col, e.msg);
    if e.line > 0 {
        if let Some(text) = src.lines().nth(e.line as usize - 1) {
            let num = e.line.to_string();
            eprintln!(" {num} | {text}");
            let pad = " ".repeat(num.len());
            let offset = " ".repeat(e.col.saturating_sub(1) as usize);
            eprintln!(" {pad} | {offset}^");
        }
    }
    if let Some(hint) = &e.hint {
        eprintln!("   hint: {hint}");
    }
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage: verb run <file.verb> [--emit-llvm]");
    eprintln!("       verb build <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]");
    eprintln!("       verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]  (alias for build)");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 { usage(); }
    let cmd = args[1].as_str();
    let file = args[2].as_str();
    let emit_llvm = args.iter().any(|a| a == "--emit-llvm");
    let out = args.iter().position(|a| a == "-o").map(|i| {
        args.get(i + 1).cloned().unwrap_or_else(|| usage())
    });
    let target_arg = args.iter().position(|a| a == "--target").map(|i| {
        args.get(i + 1).cloned().unwrap_or_else(|| usage())
    });

    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("error: cannot read {file}: {e}"); exit(1); }
    };
    let toks = lexer::lex(&src).unwrap_or_else(|e| die(e, &src));
    let prog = parser::parse(toks).unwrap_or_else(|e| die(e, &src));

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&prog).unwrap_or_else(|e| die(e, &src));

    if emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match cmd {
        "run" => {
            let ee = cg.module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| { eprintln!("JIT error: {e}"); exit(1); });
            unsafe {
                let main_fn = ee.get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "build" | "compile" => {
            let out = out.unwrap_or_else(|| usage());
            match target_arg.as_deref() {
                None => build_aot_host(&cg, &out),
                Some("all") => build_aot_all(&cg, &out),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
                    check_zig_available();
                    if let Err(e) = build_aot_cross(&cg, &out, &target) {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            }
        }
        _ => usage(),
    }
}

fn build_aot_host(cg: &codegen::Codegen, out: &str) {
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

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let status = std::process::Command::new("cc")
        .args([obj.as_str(), "-o", out])
        .status()
        .unwrap_or_else(|e| { eprintln!("cc failed to start: {e}"); exit(1); });
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}

fn build_aot_cross(cg: &codegen::Codegen, out: &str, target: &targets::Target) -> Result<(), String> {
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

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    let status = std::process::Command::new("zig")
        .args(["cc", "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()])
        .status()
        .map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}

fn build_aot_all(_cg: &codegen::Codegen, _out: &str) {
    eprintln!("--target all: not implemented yet");
    exit(1);
}
