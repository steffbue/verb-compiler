mod error;
mod lexer;

fn main() {
    // smoke: prove LLVM links
    let ctx = inkwell::context::Context::create();
    let module = ctx.create_module("smoke");
    println!("{}", module.get_name().to_str().unwrap());
}
