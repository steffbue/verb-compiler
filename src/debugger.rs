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
