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

use std::cell::RefCell;
use std::ffi::{c_char, CStr};
use std::io::{self, BufRead, Write};

/// Mirrors `Codegen`'s `value_ty` LLVM struct `{ i8, i64 }` byte-for-byte
/// (both are the default, non-packed layout, so both insert the same
/// 7-byte padding after `tag` — see docs/superpowers/specs/2026-07-21-interactive-debugger-design.md).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawVerbValue {
    pub tag: u8,
    pub payload: i64,
}

/// Mirrors the array Codegen's `emit_checkpoint` builds: `{ name: ptr, cell: ptr }`.
#[repr(C)]
pub struct DebugVar {
    pub name: *const c_char,
    pub cell: *mut RawVerbValue,
}

thread_local! {
    static STATE: RefCell<DebuggerState> = RefCell::new(DebuggerState::new());
    /// Raw address of the JIT-compiled `verb_print_value(VerbValue)` function,
    /// resolved once via `ExecutionEngine::get_function_address` before the
    /// program starts running (Task 6). 0 means "not yet set" — `print`
    /// falls back to printing the raw tag/payload if so (should never
    /// happen once Task 6 wires it up correctly).
    static PRINT_VALUE_FN: RefCell<usize> = const { RefCell::new(0) };
}

/// Called once by the `debug` CLI command after creating the JIT execution
/// engine, before running `main`.
pub fn set_print_value_fn(addr: usize) {
    PRINT_VALUE_FN.with(|f| *f.borrow_mut() = addr);
}

fn call_print_value(v: RawVerbValue) {
    let addr = PRINT_VALUE_FN.with(|f| *f.borrow());
    if addr == 0 {
        println!("<tag={} payload={}>", v.tag, v.payload);
        return;
    }
    let f: extern "C" fn(RawVerbValue) = unsafe { std::mem::transmute(addr) };
    f(v);
    println!();
}

/// Runs the blocking console: prints a prompt, reads a line, dispatches.
/// `vars` is `None` before the first `run` (only break/delete/run/quit are
/// meaningful then); `Some(&[DebugVar])` once stopped at a checkpoint.
/// Returns when the user issues `continue`, `step`, or `run` (i.e. when
/// execution should proceed), or exits the process on `quit`.
fn run_console(vars: Option<&[DebugVar]>) {
    let stdin = io::stdin();
    loop {
        print!("(vdb) ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            std::process::exit(0); // stdin closed
        }
        if line.trim().is_empty() {
            continue;
        }
        let cmd = match parse_command(&line) {
            Ok(c) => c,
            Err(e) => {
                println!("{e}");
                continue;
            }
        };
        match cmd {
            Command::Break(n) => STATE.with(|s| s.borrow_mut().add_breakpoint(n)),
            Command::Delete(n) => STATE.with(|s| s.borrow_mut().remove_breakpoint(n)),
            Command::Run => {
                let already_started = STATE.with(|s| s.borrow().started());
                if already_started {
                    println!("program already running");
                    continue;
                }
                STATE.with(|s| s.borrow_mut().start());
                return;
            }
            Command::Continue => {
                let started = STATE.with(|s| s.borrow().started());
                if !started {
                    println!("program not running yet -- use 'run'");
                    continue;
                }
                STATE.with(|s| s.borrow_mut().set_stepping(false));
                return;
            }
            Command::Step => {
                let started = STATE.with(|s| s.borrow().started());
                if !started {
                    println!("program not running yet -- use 'run'");
                    continue;
                }
                STATE.with(|s| s.borrow_mut().set_stepping(true));
                return;
            }
            Command::Print(name) => match vars {
                None => println!("no running frame"),
                Some(vs) => match find_var(vs, &name) {
                    Some(v) => call_print_value(v),
                    None => println!("no such variable '{name}' in scope"),
                },
            },
            Command::Backtrace => {
                let frames = STATE.with(|s| s.borrow().frames().to_vec());
                if frames.is_empty() {
                    println!("(no active calls)");
                } else {
                    for f in frames.iter().rev() {
                        println!("{} (called at line {})", f.fn_name, f.call_line);
                    }
                }
            }
            Command::Quit => std::process::exit(0),
        }
    }
}

fn find_var(vars: &[DebugVar], name: &str) -> Option<RawVerbValue> {
    vars.iter().find(|v| {
        let n = unsafe { CStr::from_ptr(v.name) };
        n.to_str() == Ok(name)
    }).map(|v| unsafe { *v.cell })
}

/// The checkpoint hook: called by JIT'd code at the start of every
/// statement when `Codegen::enable_debug_hooks` was used. Blocks on the
/// console if stepping or a breakpoint is hit at `line`; otherwise
/// returns immediately.
///
/// # Safety
/// `vars`/`n_vars` must describe a valid `[DebugVar; n_vars]` built by
/// `Codegen::emit_checkpoint` -- true for every call site, since this
/// function is only ever reached via a JIT'd call instruction codegen
/// itself emitted.
pub unsafe extern "C" fn verb_debug_checkpoint(line: u32, vars: *const DebugVar, n_vars: usize) {
    STATE.with(|s| s.borrow_mut().set_current_line(line));
    let stop = STATE.with(|s| s.borrow().should_stop(line));
    if !stop {
        return;
    }
    println!("stopped at line {line}");
    let slice = if vars.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(vars, n_vars) } };
    run_console(Some(slice));
}

/// # Safety
/// `fn_name` must be a valid NUL-terminated C string -- true for every
/// call site, since codegen only ever passes a `self.cstr(name)` global.
pub unsafe extern "C" fn verb_debug_push_frame(fn_name: *const c_char) {
    let name = unsafe { CStr::from_ptr(fn_name) }.to_string_lossy().into_owned();
    STATE.with(|s| s.borrow_mut().push_frame(name));
}

pub extern "C" fn verb_debug_pop_frame() {
    STATE.with(|s| s.borrow_mut().pop_frame());
}

/// Runs the pre-execution console (only break/delete/run/quit are valid)
/// until the user issues `run`. Called by the `debug` CLI command before
/// invoking the JIT'd `main`.
pub fn run_pre_start_console() {
    println!("(vdb) verb debug -- type 'break <line>' then 'run', or 'quit'");
    run_console(None);
}
