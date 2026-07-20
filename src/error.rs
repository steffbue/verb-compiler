#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
    pub hint: Option<String>,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
        Self { msg: msg.into(), line, col, hint: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}
