mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod value;

use std::process::exit;

use error::CompileError;

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
    eprintln!("       verb build <file.verb> -o <out> [--emit-llvm]");
    eprintln!("       verb compile <file.verb> -o <out> [--emit-llvm]  (alias for build)");
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
            build_aot_host(&cg, &out);
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
