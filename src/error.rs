#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
        Self { msg: msg.into(), line, col }
    }
}
