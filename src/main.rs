mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod value;

use std::process::exit;

use error::CompileError;

struct ParsedArgs {
    cmd: String,
    files: Vec<String>,
    out: Option<String>,
    emit_llvm: bool,
}

fn parse_cli(args: &[String]) -> Option<ParsedArgs> {
    if args.len() < 2 {
        return None;
    }
    let cmd = args[1].clone();
    let mut files = Vec::new();
    let mut out = None;
    let mut emit_llvm = false;
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
            f => {
                files.push(f.to_string());
                i += 1;
            }
        }
    }
    if files.is_empty() {
        return None;
    }
    Some(ParsedArgs { cmd, files, out, emit_llvm })
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
    eprintln!("       verb build <file.verb> -o <out> [--emit-llvm]");
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
        "build" => {
            let out = out.unwrap_or_else(|| usage());
            build_aot(&cg, &out); // implemented in Task 9; stub for now
        }
        _ => usage(),
    }
}

fn build_aot(_cg: &codegen::Codegen, _out: &str) {
    eprintln!("build: not implemented yet");
    exit(1);
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
