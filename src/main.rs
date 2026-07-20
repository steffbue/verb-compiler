use std::process::{exit, Command};

use verb::codegen;
use verb::error::CompileError;
use verb::lexer;
use verb::parser;

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
    eprintln!("       verb build <file.verb> -o <out> [-L<dir>]... [--emit-llvm]");
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
    let lib_dirs: Vec<String> = args.iter()
        .filter(|a| a.starts_with("-L") && a.as_str() != "-L")
        .cloned()
        .collect();

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
            if !prog.imports.is_empty() {
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    prog.imports.join(", ")
                );
                exit(1);
            }
            let ee = cg.module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| { eprintln!("JIT error: {e}"); exit(1); });
            unsafe {
                let main_fn = ee.get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "build" => {
            let out = out.unwrap_or_else(|| usage());
            build_aot(&cg, &out, &prog.imports, &lib_dirs);
        }
        _ => usage(),
    }
}

fn build_aot(cg: &codegen::Codegen, out: &str, imports: &[String], lib_dirs: &[String]) {
    use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("error: failed to initialize target: {e}"); exit(1); });

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("error: unsupported target: {e}"); exit(1); });
    let tm = target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            inkwell::OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect("failed to create target machine");

    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let obj_path = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, std::path::Path::new(&obj_path))
        .unwrap_or_else(|e| { eprintln!("error: failed to emit object file: {e}"); exit(1); });

    let linker = if imports.is_empty() { "cc" } else { "c++" };
    let mut cmd = Command::new(linker);
    cmd.arg(&obj_path).arg("-o").arg(out);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = match cmd.status() {
        Ok(status) => status,
        Err(e) => {
            let _ = std::fs::remove_file(&obj_path);
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj_path);
    if !status.success() {
        eprintln!("error: link failed");
        exit(1);
    }
}
