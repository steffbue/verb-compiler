//! Interactive debugger console: command grammar, runtime state, and the
//! `extern "C"` hooks codegen calls into when `verb debug` compiles a
//! program with checkpoints enabled.

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `break <line>` (file defaults to the program's main/entry file --
    /// see `DebuggerState::main_file`) or `break <file>:<line>` to
    /// disambiguate a line number shared by two imported files (`import
    /// mod <name>.verb;`). Line numbers alone are not globally unique
    /// across a multi-file program, so `None` here is deliberately *not*
    /// "any file" -- it's shorthand for one specific, well-defined file.
    Break(Option<String>, u32),
    Delete(Option<String>, u32),
    Run,
    Continue,
    Step,
    Print(String),
    Backtrace,
    Quit,
}

/// Splits `break`/`delete`'s argument into an optional file qualifier and
/// a line number. `file:line` (file may itself contain `:` on some
/// platforms/paths, so we split on the *last* `:`) qualifies an explicit
/// file matching one of `stmt_files`' entries verbatim (the same display
/// string compile errors already print -- see `main.rs::die`); a bare
/// `line` leaves the file unspecified, resolved against the main file at
/// lookup time by `DebuggerState`.
fn parse_break_target(arg: &str) -> Result<(Option<String>, u32), String> {
    match arg.rsplit_once(':') {
        Some((file, line)) => {
            let n = line.parse::<u32>().map_err(|_| format!("not a line number: '{line}'"))?;
            Ok((Some(file.to_string()), n))
        }
        None => {
            let n = arg.parse::<u32>().map_err(|_| format!("not a line number: '{arg}'"))?;
            Ok((None, n))
        }
    }
}

pub fn parse_command(line: &str) -> Result<Command, String> {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    let cmd = parts.next().ok_or_else(|| "empty command".to_string())?;
    match cmd {
        "break" | "b" => {
            let n = parts.next().ok_or("usage: break <line> (or break <file.verb>:<line>)")?;
            let (file, line) = parse_break_target(n)?;
            Ok(Command::Break(file, line))
        }
        "delete" => {
            let n = parts.next().ok_or("usage: delete <line> (or delete <file.verb>:<line>)")?;
            let (file, line) = parse_break_target(n)?;
            Ok(Command::Delete(file, line))
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
    /// Keyed by `(file, line)` -- a bare line number is not unique across
    /// a multi-file program (`import mod <name>.verb;` can easily produce
    /// two files that both have a statement on, say, line 3). Every
    /// breakpoint is stored fully qualified; `add_breakpoint`/
    /// `remove_breakpoint` resolve an unqualified `break <line>` to
    /// `main_file` before touching this set.
    breakpoints: HashSet<(String, u32)>,
    stepping: bool,
    started: bool,
    current_line: u32,
    frames: Vec<Frame>,
    /// The entry file's display name (exactly as it appears in
    /// `stmt_files`/compile-error messages), used to resolve an
    /// unqualified `break <line>` -- see `breakpoints`. Empty until
    /// `set_main_file` is called (the CLI does this once, before the
    /// pre-start console runs).
    main_file: String,
}

impl DebuggerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_main_file(&mut self, file: String) {
        self.main_file = file;
    }

    /// Resolves an unqualified breakpoint file (`None`, from a bare
    /// `break <line>`) to the program's main file.
    fn resolve_file(&self, file: Option<String>) -> String {
        file.unwrap_or_else(|| self.main_file.clone())
    }

    pub fn add_breakpoint(&mut self, file: Option<String>, line: u32) {
        let file = self.resolve_file(file);
        self.breakpoints.insert((file, line));
    }

    pub fn remove_breakpoint(&mut self, file: Option<String>, line: u32) {
        let file = self.resolve_file(file);
        self.breakpoints.remove(&(file, line));
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

    /// `file` is the checkpoint's originating file (see
    /// `Codegen::emit_checkpoint`); only an exact `(file, line)` match --
    /// or an active `step` -- triggers a stop, so a `break 5` in one
    /// imported file never fires on an unrelated file's own line 5.
    pub fn should_stop(&self, file: &str, line: u32) -> bool {
        self.stepping || self.breakpoints.contains(&(file.to_string(), line))
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
        assert!(!s.should_stop("main.verb", 5));
        s.add_breakpoint(None, 5);
        // Unqualified breakpoint resolves against `main_file` (empty
        // string until set, matching a bare-entry-file debug session
        // where `stmt_files` is also just the plain filename).
        assert!(s.should_stop("", 5));
        assert!(!s.should_stop("", 6));
        s.remove_breakpoint(None, 5);
        assert!(!s.should_stop("", 5));
    }

    #[test]
    fn breakpoint_is_file_qualified() {
        // Reproduces the multi-file ambiguity: two imported files can
        // each have a statement on the same line number. A breakpoint
        // qualified to one file must not fire on the other file's
        // matching line.
        let mut s = DebuggerState::new();
        s.set_main_file("main.verb".to_string());
        s.add_breakpoint(Some("helper.verb".to_string()), 3);
        assert!(s.should_stop("helper.verb", 3));
        assert!(!s.should_stop("main.verb", 3));

        // An unqualified breakpoint resolves to the main file only.
        s.add_breakpoint(None, 7);
        assert!(s.should_stop("main.verb", 7));
        assert!(!s.should_stop("helper.verb", 7));
    }

    #[test]
    fn stepping_stops_at_any_line() {
        let mut s = DebuggerState::new();
        s.set_stepping(true);
        assert!(s.should_stop("main.verb", 1));
        assert!(s.should_stop("other.verb", 999));
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
        assert_eq!(parse_command("break 5"), Ok(Command::Break(None, 5)));
        assert_eq!(parse_command("b 5"), Ok(Command::Break(None, 5)));
    }

    #[test]
    fn parses_file_qualified_break() {
        assert_eq!(
            parse_command("break helper.verb:3"),
            Ok(Command::Break(Some("helper.verb".to_string()), 3))
        );
        assert_eq!(
            parse_command("b sub/helper.verb:12"),
            Ok(Command::Break(Some("sub/helper.verb".to_string()), 12))
        );
    }

    #[test]
    fn parses_delete() {
        assert_eq!(parse_command("delete 5"), Ok(Command::Delete(None, 5)));
        assert_eq!(
            parse_command("delete helper.verb:5"),
            Ok(Command::Delete(Some("helper.verb".to_string()), 5))
        );
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

use std::ffi::{c_char, CStr};
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

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

/// Process-wide, mutex-protected debugger state.
///
/// This used to be a `thread_local!`, which was correct as long as the
/// program being debugged only ever ran on the one OS thread that also
/// hosts the console (true when this module was written -- see
/// docs/superpowers/specs/2026-07-21-interactive-debugger-design.md,
/// which explicitly notes "thread-local is sufficient; the JIT runs
/// single-threaded"). `import std thread;`'s `thread_spawn` (added after
/// that design was written) breaks that assumption: it starts a real
/// `std::thread` that re-enters this module's `extern "C"` hooks
/// (`verb_debug_checkpoint` etc.) directly. A `thread_local!` would give
/// that spawned thread its own independent, empty `DebuggerState` --
/// breakpoints set at the console would silently never fire for code
/// running on it. Sharing one global, mutex-protected `DebuggerState`
/// instead makes breakpoints apply no matter which thread hits them.
///
/// (As of this writing `verb debug`/`verb run` refuse any program with
/// `import std ...` -- including `thread` -- because the JIT execution
/// engine never links the C++ runtime object that provides
/// `thread_spawn_raw`/etc., so this can't yet be exercised through the
/// `verb debug` CLI. It's still fixed proactively: the hooks below are
/// reachable from any thread the moment that JIT restriction is lifted,
/// and there's no benefit to leaving a footgun in the meantime. See the
/// `state_is_shared_across_os_threads` test below for direct evidence
/// this now behaves correctly.)
static STATE: LazyLock<Mutex<DebuggerState>> = LazyLock::new(|| Mutex::new(DebuggerState::new()));

/// Serializes interactive console sessions across threads. `run_console`
/// holds this for its entire duration, so if two threads both hit a
/// stopping checkpoint "at the same time", only one of them reads
/// stdin/writes stdout at a time -- the other blocks *before* touching
/// either fd, instead of racing on them and interleaving garbled prompts
/// and output.
static CONSOLE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Raw address of the JIT-compiled `verb_print_value(VerbValue)` function,
/// resolved once via `ExecutionEngine::get_function_address` before the
/// program starts running (Task 6). 0 means "not yet set" — `print`
/// falls back to printing the raw tag/payload if so (should never happen
/// once Task 6 wires it up correctly). A plain shared `AtomicUsize`
/// (rather than thread-local) for the same reason `STATE` is shared: a
/// `thread_spawn`ed thread's checkpoint needs to be able to resolve
/// `print` too.
static PRINT_VALUE_FN: AtomicUsize = AtomicUsize::new(0);

/// Locks `STATE`, recovering from poisoning instead of propagating a
/// panic. A poisoned lock here (some earlier hook call panicked mid-update)
/// shouldn't take the whole interactive console down with it -- printing
/// possibly-stale debugger state is far more useful than aborting the
/// session.
fn state() -> MutexGuard<'static, DebuggerState> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called once by the `debug` CLI command after creating the JIT execution
/// engine, before running `main`.
pub fn set_print_value_fn(addr: usize) {
    PRINT_VALUE_FN.store(addr, Ordering::SeqCst);
}

/// Called once by the `debug` CLI command before the pre-start console
/// runs, with the entry file's display name (the same string
/// `stmt_files` uses for its statements, e.g. the path passed on the
/// command line) -- lets an unqualified `break <line>` resolve to "the
/// main file" per `DebuggerState::add_breakpoint`.
pub fn set_main_file(file: String) {
    state().set_main_file(file);
}

extern "C" {
    fn fflush(stream: *mut std::ffi::c_void) -> i32;
}

/// Flushes libc's stdout buffer (a NULL argument flushes every open
/// stream). The JIT-compiled program prints through libc `printf`, a
/// completely separate buffer from Rust's `io::stdout()` -- when stdout
/// isn't a TTY (e.g. piped, as in tests), libc block-buffers instead of
/// line-buffering, so without an explicit flush at the right points the
/// two buffers interleave out of order (a debugger message can appear
/// before or after program output that actually ran earlier/later).
fn flush_libc_stdout() {
    unsafe {
        fflush(std::ptr::null_mut());
    }
}

fn call_print_value(v: RawVerbValue) {
    let addr = PRINT_VALUE_FN.load(Ordering::SeqCst);
    if addr == 0 {
        println!("<tag={} payload={}>", v.tag, v.payload);
        return;
    }
    let f: extern "C" fn(RawVerbValue) = unsafe { std::mem::transmute(addr) };
    f(v);
    flush_libc_stdout();
    println!();
}

/// Runs the blocking console: prints a prompt, reads a line, dispatches.
/// `vars` is `None` before the first `run` (only break/delete/run/quit are
/// meaningful then); `Some(&[DebugVar])` once stopped at a checkpoint.
/// Returns when the user issues `continue`, `step`, or `run` (i.e. when
/// execution should proceed), or exits the process on `quit`.
///
/// Holds `CONSOLE_LOCK` for its entire duration: if a second thread's
/// checkpoint also wants to prompt the user while this one is active, it
/// blocks trying to acquire the lock (before touching stdin/stdout at
/// all) until this session returns/exits -- see `CONSOLE_LOCK`.
fn run_console(vars: Option<&[DebugVar]>) {
    let _console_guard = CONSOLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            Command::Break(file, n) => state().add_breakpoint(file, n),
            Command::Delete(file, n) => state().remove_breakpoint(file, n),
            Command::Run => {
                let already_started = state().started();
                if already_started {
                    println!("program already running");
                    continue;
                }
                state().start();
                return;
            }
            Command::Continue => {
                let started = state().started();
                if !started {
                    println!("program not running yet -- use 'run'");
                    continue;
                }
                state().set_stepping(false);
                return;
            }
            Command::Step => {
                let started = state().started();
                if !started {
                    println!("program not running yet -- use 'run'");
                    continue;
                }
                state().set_stepping(true);
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
                let frames = state().frames().to_vec();
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
/// console if stepping or a breakpoint is hit at `file`:`line`; otherwise
/// returns immediately.
///
/// May be called concurrently from multiple OS threads (once `import std
/// thread;` can run under `verb debug` -- see the comment on `STATE`);
/// `state()`'s internal mutex and `run_console`'s `CONSOLE_LOCK` make that
/// safe.
///
/// # Safety
/// `file` must be a valid NUL-terminated C string, and `vars`/`n_vars`
/// must describe a valid `[DebugVar; n_vars]` -- both built by
/// `Codegen::emit_checkpoint` for every call site, since this function is
/// only ever reached via a JIT'd call instruction codegen itself emitted.
pub unsafe extern "C" fn verb_debug_checkpoint(
    file: *const c_char,
    line: u32,
    vars: *const DebugVar,
    n_vars: usize,
) {
    let file = unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned();
    state().set_current_line(line);
    let stop = state().should_stop(&file, line);
    if !stop {
        return;
    }
    // Flush any pending libc-buffered output from statements that already
    // ran (e.g. a `print(...)` just before this line) so it appears before
    // this message, not after -- see `flush_libc_stdout`.
    flush_libc_stdout();
    // Keeping the literal substring "stopped at line {line}" (rather than
    // e.g. "stopped at {file}:{line}") preserves the existing message
    // shape `tests/e2e.rs` already asserts on; the file is appended, not
    // interpolated into it.
    println!("stopped at line {line} ({file})");
    let slice = if vars.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(vars, n_vars) } };
    run_console(Some(slice));
}

/// # Safety
/// `fn_name` must be a valid NUL-terminated C string -- true for every
/// call site, since codegen only ever passes a `self.cstr(name)` global.
pub unsafe extern "C" fn verb_debug_push_frame(fn_name: *const c_char) {
    let name = unsafe { CStr::from_ptr(fn_name) }.to_string_lossy().into_owned();
    state().push_frame(name);
}

pub extern "C" fn verb_debug_pop_frame() {
    state().pop_frame();
}

/// Runs the pre-execution console (only break/delete/run/quit are valid)
/// until the user issues `run`. Called by the `debug` CLI command before
/// invoking the JIT'd `main`.
pub fn run_pre_start_console() {
    println!("(vdb) verb debug -- type 'break <line>' then 'run', or 'quit'");
    run_console(None);
}

/// Proves the two claims made in the design comment on `STATE`: (1) that
/// state is now genuinely shared across OS threads (unlike the old
/// `thread_local!`, where each thread saw its own, permanently-empty
/// `DebuggerState`), and (2) that `CONSOLE_LOCK` actually serializes
/// concurrent critical sections rather than letting them overlap. Can't
/// drive this end-to-end through `verb debug`'s own subprocess (the JIT
/// currently refuses any `import std ...`, including `thread`, so
/// `thread_spawn` can't reach these hooks via the CLI yet -- see the
/// comment on `STATE`) -- these exercise the same shared `STATE`/
/// `CONSOLE_LOCK` statics and public hook entry points directly instead,
/// using real `std::thread::spawn` OS threads.
///
/// These tests touch the same process-global `STATE`/`CONSOLE_LOCK` as
/// every other test in this file could in principle reach through the
/// public hooks; since nothing else in this crate's unit tests actually
/// calls `verb_debug_checkpoint`/etc., there's no cross-test
/// interference in practice, but each test still cleans up after itself
/// (removes any breakpoint it added) to keep that true going forward.
#[cfg(test)]
mod concurrency_tests {
    use super::*;

    #[test]
    fn state_is_shared_across_os_threads() {
        // A breakpoint added from a simulated "console thread" must be
        // visible to a simulated "spawned program thread" -- exactly the
        // guarantee `thread_spawn` needs from the debugger.
        std::thread::spawn(|| {
            state().add_breakpoint(Some("concurrency_test.verb".to_string()), 4242);
        })
        .join()
        .unwrap();

        let saw_it = std::thread::spawn(|| state().should_stop("concurrency_test.verb", 4242))
            .join()
            .unwrap();
        assert!(saw_it, "breakpoint set on one OS thread must be visible on another");

        state().remove_breakpoint(Some("concurrency_test.verb".to_string()), 4242);
    }

    #[test]
    fn console_lock_serializes_overlapping_critical_sections() {
        // Two threads both try to enter the region `run_console` would
        // guard with `CONSOLE_LOCK`. If the lock actually serializes
        // them, `active` is never seen at 2 by either thread.
        static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static MAX_SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        use std::sync::atomic::Ordering as O;

        fn enter_guarded_section() {
            let _g = CONSOLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let now = ACTIVE.fetch_add(1, O::SeqCst) + 1;
            MAX_SEEN.fetch_max(now, O::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20));
            ACTIVE.fetch_sub(1, O::SeqCst);
        }

        let t1 = std::thread::spawn(enter_guarded_section);
        let t2 = std::thread::spawn(enter_guarded_section);
        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(
            MAX_SEEN.load(O::SeqCst),
            1,
            "CONSOLE_LOCK must serialize console sessions, never let two run concurrently"
        );
    }
}
