#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
    pub hint: Option<String>,
    pub file: Option<String>,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
        Self { msg: msg.into(), line, col, hint: None, file: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_file_sets_file() {
        let e = CompileError::new("boom", 1, 2).with_file("a.verb");
        assert_eq!(e.file, Some("a.verb".to_string()));
    }

    #[test]
    fn new_leaves_file_unset() {
        let e = CompileError::new("boom", 1, 2);
        assert_eq!(e.file, None);
    }
}
