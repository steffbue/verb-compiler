//! Interactive debugger console: command grammar, runtime state, and the
//! `extern "C"` hooks codegen calls into when `verb debug` compiles a
//! program with checkpoints enabled.

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Break(u32),
    Delete(u32),
    Run,
    Continue,
    Step,
    Print(String),
    Backtrace,
    Quit,
}

pub fn parse_command(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    let cmd = parts.next().ok_or_else(|| "empty command".to_string())?;
    match cmd {
        "break" | "b" => {
            let n = parts.next().ok_or("usage: break <line>")?;
            n.parse::<u32>().map(Command::Break).map_err(|_| format!("not a line number: '{n}'"))
        }
        "delete" => {
            let n = parts.next().ok_or("usage: delete <line>")?;
            n.parse::<u32>().map(Command::Delete).map_err(|_| format!("not a line number: '{n}'"))
        }
        "run" | "r" => Ok(Command::Run),
        "continue" | "c" => Ok(Command::Continue),
        "step" | "s" => Ok(Command::Step),
        "print" | "p" => {
            let name = parts.next().ok_or("usage: print <name>")?;
            Ok(Command::Print(name.to_string()))
        }
        "backtrace" | "bt" => Ok(Command::Backtrace),
        "quit" | "q" => Ok(Command::Quit),
        other => Err(format!("unknown command '{other}' (try: break, delete, run, continue, step, print, backtrace, quit)")),
    }
}

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fn_name: String,
    pub call_line: u32,
}

#[derive(Debug, Default)]
pub struct DebuggerState {
    breakpoints: HashSet<u32>,
    stepping: bool,
    started: bool,
    current_line: u32,
    frames: Vec<Frame>,
}

impl DebuggerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_breakpoint(&mut self, line: u32) {
        self.breakpoints.insert(line);
    }

    pub fn remove_breakpoint(&mut self, line: u32) {
        self.breakpoints.remove(&line);
    }

    pub fn start(&mut self) {
        self.started = true;
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn set_stepping(&mut self, on: bool) {
        self.stepping = on;
    }

    /// Called by the checkpoint hook before deciding whether to stop.
    pub fn set_current_line(&mut self, line: u32) {
        self.current_line = line;
    }

    pub fn current_line(&self) -> u32 {
        self.current_line
    }

    pub fn should_stop(&self, line: u32) -> bool {
        self.stepping || self.breakpoints.contains(&line)
    }

    pub fn push_frame(&mut self, fn_name: String) {
        let call_line = self.current_line;
        self.frames.push(Frame { fn_name, call_line });
    }

    pub fn pop_frame(&mut self) {
        self.frames.pop();
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn breakpoint_add_remove() {
        let mut s = DebuggerState::new();
        assert!(!s.should_stop(5));
        s.add_breakpoint(5);
        assert!(s.should_stop(5));
        assert!(!s.should_stop(6));
        s.remove_breakpoint(5);
        assert!(!s.should_stop(5));
    }

    #[test]
    fn stepping_stops_at_any_line() {
        let mut s = DebuggerState::new();
        s.set_stepping(true);
        assert!(s.should_stop(1));
        assert!(s.should_stop(999));
    }

    #[test]
    fn frame_push_pop_captures_current_line() {
        let mut s = DebuggerState::new();
        s.set_current_line(10);
        s.push_frame("foo".to_string());
        assert_eq!(s.frames(), &[Frame { fn_name: "foo".to_string(), call_line: 10 }]);
        s.set_current_line(20);
        s.push_frame("bar".to_string());
        assert_eq!(
            s.frames(),
            &[
                Frame { fn_name: "foo".to_string(), call_line: 10 },
                Frame { fn_name: "bar".to_string(), call_line: 20 },
            ]
        );
        s.pop_frame();
        assert_eq!(s.frames(), &[Frame { fn_name: "foo".to_string(), call_line: 10 }]);
    }

    #[test]
    fn not_started_until_run() {
        let s = DebuggerState::new();
        assert!(!s.started());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_break_and_alias() {
        assert_eq!(parse_command("break 5"), Ok(Command::Break(5)));
        assert_eq!(parse_command("b 5"), Ok(Command::Break(5)));
    }

    #[test]
    fn parses_delete() {
        assert_eq!(parse_command("delete 5"), Ok(Command::Delete(5)));
    }

    #[test]
    fn parses_run_continue_step_aliases() {
        assert_eq!(parse_command("run"), Ok(Command::Run));
        assert_eq!(parse_command("r"), Ok(Command::Run));
        assert_eq!(parse_command("continue"), Ok(Command::Continue));
        assert_eq!(parse_command("c"), Ok(Command::Continue));
        assert_eq!(parse_command("step"), Ok(Command::Step));
        assert_eq!(parse_command("s"), Ok(Command::Step));
    }

    #[test]
    fn parses_print_and_backtrace_and_quit() {
        assert_eq!(parse_command("print x"), Ok(Command::Print("x".to_string())));
        assert_eq!(parse_command("p x"), Ok(Command::Print("x".to_string())));
        assert_eq!(parse_command("backtrace"), Ok(Command::Backtrace));
        assert_eq!(parse_command("bt"), Ok(Command::Backtrace));
        assert_eq!(parse_command("quit"), Ok(Command::Quit));
        assert_eq!(parse_command("q"), Ok(Command::Quit));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_command("frobnicate").is_err());
        assert!(parse_command("break notanumber").is_err());
        assert!(parse_command("break").is_err());
        assert!(parse_command("print").is_err());
    }
}
