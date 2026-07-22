//! Minimal LSP server for the Verb language.
//!
//! Speaks LSP over stdio (Content-Length framed JSON-RPC) with no async
//! runtime — one request/notification at a time is plenty for a language
//! this small. Diagnostics reuse the real compiler pipeline
//! (`lexer::lex` -> `parser::parse` -> `codegen::Codegen::compile_program`),
//! so what the LSP flags is exactly what `verb run` would reject. Since
//! that pipeline stops at the first error, only one diagnostic is ever
//! reported per document at a time — matching the compiler's own
//! single-error-and-stop behavior.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::process::exit;

use inkwell::context::Context;
use serde_json::{json, Value};

use verb::ast::Stmt;
use verb::codegen::Codegen;
use verb::error::CompileError;
use verb::formatter;
use verb::lexer;
use verb::parser;

fn main() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    let mut docs: HashMap<String, String> = HashMap::new();
    let mut shutting_down = false;

    loop {
        let msg = match read_message(&mut reader) {
            Ok(Some(m)) => m,
            Ok(None) => break, // stdin closed
            Err(e) => {
                eprintln!("verb-lsp: framing error: {e}");
                break;
            }
        };

        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "initialize" => {
                if let Some(id) = id {
                    send_response(&mut writer, id, initialize_result());
                }
            }
            "shutdown" => {
                shutting_down = true;
                if let Some(id) = id {
                    send_response(&mut writer, id, Value::Null);
                }
            }
            "exit" => exit(if shutting_down { 0 } else { 1 }),
            "initialized" => {}

            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (param_str(&msg, &["textDocument", "uri"]), param_str(&msg, &["textDocument", "text"])) {
                    docs.insert(uri.clone(), text.clone());
                    publish_diagnostics(&mut writer, &uri, &text);
                }
            }
            "textDocument/didChange" => {
                let uri = param_str(&msg, &["textDocument", "uri"]);
                let text = msg
                    .get("params")
                    .and_then(|p| p.get("contentChanges"))
                    .and_then(Value::as_array)
                    .and_then(|changes| changes.last())
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let (Some(uri), Some(text)) = (uri, text) {
                    docs.insert(uri.clone(), text.clone());
                    publish_diagnostics(&mut writer, &uri, &text);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = param_str(&msg, &["textDocument", "uri"]) {
                    docs.remove(&uri);
                    send_notification(
                        &mut writer,
                        "textDocument/publishDiagnostics",
                        json!({ "uri": uri, "diagnostics": [] }),
                    );
                }
            }

            "textDocument/hover" => {
                let id = match id {
                    Some(id) => id,
                    None => continue,
                };
                let result = hover(&msg, &docs).unwrap_or(Value::Null);
                send_response(&mut writer, id, result);
            }
            "textDocument/completion" => {
                let id = match id {
                    Some(id) => id,
                    None => continue,
                };
                let src = param_str(&msg, &["textDocument", "uri"])
                    .and_then(|uri| docs.get(&uri).cloned())
                    .unwrap_or_default();
                send_response(&mut writer, id, completion_items(&src));
            }
            "textDocument/formatting" => {
                let id = match id {
                    Some(id) => id,
                    None => continue,
                };
                let src = param_str(&msg, &["textDocument", "uri"])
                    .and_then(|uri| docs.get(&uri).cloned());
                let result = src.and_then(|src| format_edits(&src)).unwrap_or(Value::Null);
                send_response(&mut writer, id, result);
            }

            "" => {} // malformed message; ignore rather than crash the server
            other => {
                if let Some(id) = id {
                    send_error(&mut writer, id, -32601, format!("method not found: {other}"));
                }
                // unknown notifications are silently ignored, per spec
            }
        }
    }
}

// ----- LSP stdio framing -----

fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None); // EOF between messages is a clean shutdown
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // blank line ends the header block
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().ok();
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let value = serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

fn write_message<W: Write>(writer: &mut W, value: &Value) {
    let body = serde_json::to_string(value).expect("LSP response is always valid JSON");
    let _ = write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body);
    let _ = writer.flush();
}

fn send_response<W: Write>(writer: &mut W, id: Value, result: Value) {
    write_message(writer, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn send_error<W: Write>(writer: &mut W, id: Value, code: i32, message: String) {
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    );
}

fn send_notification<W: Write>(writer: &mut W, method: &str, params: Value) {
    write_message(writer, &json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

/// Reads a nested string param, e.g. `param_str(&msg, &["textDocument", "uri"])`
/// for `params.textDocument.uri`. Never panics on missing/mismatched shape.
fn param_str(msg: &Value, path: &[&str]) -> Option<String> {
    let mut v = msg.get("params")?;
    for key in path {
        v = v.get(key)?;
    }
    v.as_str().map(str::to_string)
}

fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "textDocumentSync": 1, // Full
            "hoverProvider": true,
            "completionProvider": { "triggerCharacters": [] },
            "documentFormattingProvider": true,
        },
        "serverInfo": { "name": "verb-lsp", "version": env!("CARGO_PKG_VERSION") },
    })
}

// ----- diagnostics -----

fn publish_diagnostics<W: Write>(writer: &mut W, uri: &str, src: &str) {
    let diagnostics = compute_diagnostics(src);
    send_notification(
        writer,
        "textDocument/publishDiagnostics",
        json!({ "uri": uri, "diagnostics": diagnostics }),
    );
}

/// Runs the real compiler pipeline (lex -> parse -> typecheck via codegen)
/// and turns whatever `CompileError`s it hits into LSP Diagnostics.
///
/// Parsing uses `parse_recovering`, so every syntax error in the file is
/// reported at once. Codegen (semantic/type checking) has no such
/// recovery — it bails at the first error, same as the real compiler —
/// so once parsing succeeds, at most one semantic diagnostic shows per
/// file. Teaching codegen to recover would mean redesigning how it
/// tracks LLVM builder state and scopes across a broken statement, which
/// is a much bigger change than this fix; the one-syntax-error-at-a-time
/// limitation from before is gone, but semantic errors still surface one
/// at a time.
fn compute_diagnostics(src: &str) -> Vec<Value> {
    let toks = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return vec![diagnostic_from(&e)],
    };
    let (program, parse_errors) = parser::parse_recovering(toks);
    if !parse_errors.is_empty() {
        return parse_errors.iter().map(diagnostic_from).collect();
    }
    let ctx = Context::create();
    let mut cg = Codegen::new(&ctx);
    let stmt_files = vec![String::new(); program.body.len()];
    if let Err(e) = cg.compile_program(&program.body, &stmt_files, &program.imports, &program.std_imports) {
        return vec![diagnostic_from(&e)];
    }
    vec![]
}

fn diagnostic_from(e: &CompileError) -> Value {
    let line = e.line.saturating_sub(1);
    let character = e.col.saturating_sub(1);
    let message = match &e.hint {
        Some(hint) => format!("{}\nhint: {hint}", e.msg),
        None => e.msg.clone(),
    };
    json!({
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 },
        },
        "severity": 1, // Error
        "source": "verb",
        "message": message,
    })
}

// ----- hover -----

fn hover(msg: &Value, docs: &HashMap<String, String>) -> Option<Value> {
    let uri = param_str(msg, &["textDocument", "uri"])?;
    let src = docs.get(&uri)?;
    let line = msg.get("params")?.get("position")?.get("line")?.as_u64()? as u32;
    let character = msg.get("params")?.get("position")?.get("character")?.as_u64()? as u32;
    let word = word_at_position(src, line, character)?;

    let text = keyword_doc(&word)
        .map(str::to_string)
        .or_else(|| builtin_func_doc(&word).map(|(module, arity)| builtin_func_text(&word, module, arity)))
        .or_else(|| {
            let symbols = collect_symbols(src);
            symbols
                .functions
                .iter()
                .find(|(name, _)| name == &word)
                .map(|(name, arity)| {
                    let plural = if *arity == 1 { "" } else { "s" };
                    format!("function `{name}` ({arity} parameter{plural})")
                })
                .or_else(|| symbols.vars.iter().find(|v| *v == &word).map(|v| format!("variable `{v}`")))
        })?;

    Some(json!({ "contents": { "kind": "markdown", "value": text } }))
}

fn word_at_position(src: &str, line: u32, character: u32) -> Option<String> {
    let line_text = src.lines().nth(line as usize)?;
    let chars: Vec<char> = line_text.chars().collect();
    let idx = (character as usize).min(chars.len());
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';

    let mut start = idx;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    let mut end = idx;
    while end < chars.len() && is_ident(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some(chars[start..end].iter().collect())
}

/// One-line docs for every Verb keyword/operator, grounded in
/// `src/lexer.rs`'s `TokenKind`/`renamed_keyword` and `examples/demo.verb`.
fn keyword_doc(word: &str) -> Option<&'static str> {
    Some(match word {
        "assign" => "`assign name expr;` — declare a new variable initialized to `expr`.",
        "declare" => "`declare name;` — declare a new variable with no value yet (starts as `nil`).",
        "be" => "`name be expr;` — reassign an existing variable.",
        "make" => "`make name(params) begin ... end` — define a function.",
        "return" => "`return [expr];` — return from a function (error outside one).",
        "check" => "`check cond begin ... end [orelse ...]` — if-statement.",
        "orelse" => "`orelse` — else / else-if branch attached to a `check`.",
        "repeat" => "`repeat cond begin ... end` — while-loop.",
        "loop" => "`loop init; cond; update begin ... end` — C-style for-loop (desugars to a `repeat`).",
        "begin" => "opens a block (Verb has no `{ }`).",
        "end" => "closes a block opened by `begin`.",
        "import" => "`import mod <lib>;` / `import std <module>;` — must appear before any other top-level statement.",
        "std" => "`import std <module>;` — built-in stdlib module, no user-written C++ shim needed. Four modules exist: `io` (file/stdin/TCP I/O), `map` (hash maps), `thread` (OS threads/mutex/channel), `time` (clocks/sleep).",
        "true" => "boolean literal `true`.",
        "false" => "boolean literal `false`.",
        "nil" => "the null / not-yet-initialized value.",
        "add" => "`a add b` — addition (`+`).",
        "sub" => "`a sub b` — subtraction (`-`).",
        "neg" => "`neg a` — unary negation (`-a`).",
        "times" => "`a times b` — multiplication (`*`).",
        "div" => "`a div b` — division (`/`).",
        "mod" => "`a mod b` — modulo (`%`). Also the `import mod <lib>;` keyword for a C++ extern library.",
        "equals" => "`a equals b` — equality (`==`).",
        "differs" => "`a differs b` — inequality (`!=`).",
        "trails" => "`a trails b` — less than (`<`).",
        "beats" => "`a beats b` — greater than (`>`).",
        "atmost" => "`a atmost b` — less than or equal (`<=`).",
        "atleast" => "`a atleast b` — greater than or equal (`>=`).",
        "and" => "`a and b` — logical AND.",
        "or" => "`a or b` — logical OR.",
        "not" => "`not a` — logical negation.",
        "join" => "`a join b` — string/value concatenation.",
        "list" => "`list e1, e2, ...` — array literal (no closing delimiter; see `get`/`set`/`push`/`pop`/`len`).",
        _ => return None,
    })
}

/// Fixed name -> (stdlib module, arity) table for every built-in stdlib
/// function, mirroring the authoritative tables in `src/codegen.rs`
/// (`IO_FUNCS`, `MAP_FUNCS`, `THREAD_FUNCS`, `TIME_FUNCS`) plus
/// `thread_spawn`, which codegen dispatches through bespoke codegen
/// (`gen_thread_spawn`) rather than a table lookup, since its closure
/// argument can't cross the C++ boundary as a plain `VerbValue`. These
/// are a distinct hover/completion category from both keywords
/// (`keyword_doc`) and user-defined functions (`collect_symbols`):
/// they're only meaningful once the matching `import std <module>;`
/// is present, but are still worth surfacing unconditionally so a user
/// typing `thread_join(...)` gets its arity and origin module on hover
/// without having to go look at the runtime source.
const BUILTIN_FUNCS: &[(&str, &str, usize)] = &[
    // std io (see IO_FUNCS in src/codegen.rs)
    ("read_line", "io", 0),
    ("file_read", "io", 1),
    ("file_write", "io", 2),
    ("file_append", "io", 2),
    ("tcp_connect", "io", 2),
    ("tcp_listen", "io", 1),
    ("tcp_accept", "io", 1),
    ("send_line", "io", 2),
    ("recv_line", "io", 1),
    ("close_conn", "io", 1),
    // std map (see MAP_FUNCS in src/codegen.rs)
    ("map_new", "map", 0),
    ("map_set", "map", 3),
    ("map_get", "map", 2),
    ("map_has", "map", 2),
    ("map_remove", "map", 2),
    ("map_len", "map", 1),
    // std thread (see THREAD_FUNCS in src/codegen.rs; thread_spawn has no
    // table entry there, but does have one here -- see doc comment above)
    ("thread_spawn", "thread", 1),
    ("thread_join", "thread", 1),
    ("thread_sleep_ms", "thread", 1),
    ("mutex_new", "thread", 0),
    ("mutex_lock", "thread", 1),
    ("mutex_unlock", "thread", 1),
    ("channel_new", "thread", 0),
    ("channel_send", "thread", 2),
    ("channel_recv", "thread", 1),
    // std time (see TIME_FUNCS in src/codegen.rs)
    ("now_ms", "time", 0),
    ("monotonic_ms", "time", 0),
    ("sleep_ms", "time", 1),
    ("clock_ms", "time", 0),
    ("difftime_ms", "time", 2),
    ("linux_clock_gettime_ns", "time", 1),
    ("linux_nanosleep_ns", "time", 1),
    ("win_filetime_100ns", "time", 0),
    ("win_sleep_ms", "time", 1),
];

/// Looks up a stdlib builtin's `(module, arity)` by name, or `None` if
/// `word` isn't one. See `BUILTIN_FUNCS`.
fn builtin_func_doc(word: &str) -> Option<(&'static str, usize)> {
    BUILTIN_FUNCS.iter().find(|(n, _, _)| *n == word).map(|(_, module, arity)| (*module, *arity))
}

/// Renders the shared hover/completion-detail text for a builtin, e.g.
/// "built-in `std thread` function `thread_join` (1 parameter)."
fn builtin_func_text(name: &str, module: &str, arity: usize) -> String {
    let plural = if arity == 1 { "" } else { "s" };
    format!("built-in `std {module}` function `{name}` ({arity} parameter{plural}).")
}

// ----- completion -----

fn completion_items(src: &str) -> Value {
    const KEYWORD_KIND: u64 = 14; // CompletionItemKind::Keyword
    const FUNCTION_KIND: u64 = 3; // CompletionItemKind::Function
    const VARIABLE_KIND: u64 = 6; // CompletionItemKind::Variable

    let mut items: Vec<Value> = Vec::new();

    for word in [
        "assign", "declare", "be", "make", "return", "check", "orelse", "repeat", "loop", "begin",
        "end", "import", "std", "true", "false", "nil", "add", "sub", "neg", "times", "div", "mod",
        "equals", "differs", "trails", "beats", "atmost", "atleast", "and", "or", "not", "join", "list",
    ] {
        items.push(json!({
            "label": word,
            "kind": KEYWORD_KIND,
            "detail": keyword_doc(word).unwrap_or_default(),
        }));
    }

    for (name, module, arity) in BUILTIN_FUNCS {
        items.push(json!({
            "label": name,
            "kind": FUNCTION_KIND,
            "detail": builtin_func_text(name, module, *arity),
        }));
    }

    let symbols = collect_symbols(src);
    for (name, arity) in &symbols.functions {
        items.push(json!({
            "label": name,
            "kind": FUNCTION_KIND,
            "detail": format!("function ({arity} params)"),
        }));
    }
    for name in &symbols.vars {
        items.push(json!({ "label": name, "kind": VARIABLE_KIND, "detail": "variable" }));
    }

    Value::Array(items)
}

// ----- formatting -----

/// Whole-document `TextEdit` from `formatter::format`, or `None` if the
/// document doesn't currently parse (mirrors `compute_diagnostics`
/// already showing the syntax error — formatting just no-ops instead of
/// erroring the LSP request).
fn format_edits(src: &str) -> Option<Value> {
    let formatted = formatter::format(src).ok()?;
    if formatted == src {
        return Some(Value::Array(vec![]));
    }
    let lines: Vec<&str> = src.split('\n').collect();
    let end_line = (lines.len().max(1) - 1) as u64;
    let end_character = lines.last().map(|l| l.chars().count()).unwrap_or(0) as u64;
    Some(json!([{
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": end_line, "character": end_character },
        },
        "newText": formatted,
    }]))
}

// ----- best-effort symbol collection (for hover/completion, not diagnostics) -----

#[derive(Default)]
struct Symbols {
    functions: Vec<(String, usize)>,
    vars: Vec<String>,
}

fn collect_symbols(src: &str) -> Symbols {
    let mut symbols = Symbols::default();
    if let Ok(toks) = lexer::lex(src) {
        // best-effort: use whatever parsed even if part of the file has
        // a syntax error elsewhere, so hover/completion still work
        let (program, _errors) = parser::parse_recovering(toks);
        collect_from_stmts(&program.body, &mut symbols);
    }
    symbols
}

fn collect_from_stmts(stmts: &[Stmt], out: &mut Symbols) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { name, .. } | Stmt::Declare { name, .. } => {
                if !out.vars.contains(name) {
                    out.vars.push(name.clone());
                }
            }
            Stmt::Fn { name, params, body, .. } => {
                out.functions.push((name.clone(), params.len()));
                collect_from_stmts(body, out);
            }
            Stmt::If { then_body, else_body, .. } => {
                collect_from_stmts(then_body, out);
                if let Some(else_body) = else_body {
                    collect_from_stmts(else_body, out);
                }
            }
            Stmt::While { body, .. } => collect_from_stmts(body, out),
            Stmt::Block(inner, ..) => collect_from_stmts(inner, out),
            // A `shape` type name is callable as its positional constructor.
            Stmt::Shape { name, fields, .. } => out.functions.push((name.clone(), fields.len())),
            Stmt::Reassign { .. } | Stmt::Return { .. } | Stmt::ExprStmt(..) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover_at(src: &str, line: u32, character: u32) -> Option<Value> {
        let mut docs = HashMap::new();
        docs.insert("test://doc".to_string(), src.to_string());
        let msg = json!({
            "params": {
                "textDocument": { "uri": "test://doc" },
                "position": { "line": line, "character": character },
            },
        });
        hover(&msg, &docs)
    }

    /// Finds the 0-indexed (line, character) of the first occurrence of
    /// `needle` in `src`, so tests don't hardcode brittle column offsets.
    fn locate(src: &str, needle: &str) -> (u32, u32) {
        for (i, line) in src.lines().enumerate() {
            if let Some(col) = line.find(needle) {
                return (i as u32, col as u32);
            }
        }
        panic!("{needle:?} not found in fixture source");
    }

    #[test]
    fn keyword_doc_std_mentions_all_four_modules() {
        let text = keyword_doc("std").expect("`std` should have hover text");
        for module in ["io", "map", "thread", "time"] {
            assert!(text.contains(module), "expected \"{module}\" in std doc: {text}");
        }
    }

    #[test]
    fn hover_on_thread_join_reports_module_and_arity() {
        let src = "import std thread;\nmake main() begin\n  thread_join(1);\nend\n";
        let (line, col) = locate(src, "thread_join");
        let result = hover_at(src, line, col + 1).expect("hover result for thread_join");
        let text = result["contents"]["value"].as_str().expect("hover value is a string");
        assert!(text.contains("thread"), "expected module name \"thread\" in: {text}");
        assert!(text.contains("1 parameter"), "expected arity 1 in: {text}");
        assert!(!text.contains("1 parameters"), "singular \"parameter\" expected in: {text}");
    }

    #[test]
    fn hover_on_thread_spawn_reports_module_and_arity() {
        // thread_spawn has bespoke codegen (gen_thread_spawn) and no entry
        // in codegen.rs's THREAD_FUNCS table, but should still hover.
        let src = "import std thread;\nmake worker() begin\nend\nmake main() begin\n  thread_spawn(worker);\nend\n";
        let (line, col) = locate(src, "thread_spawn");
        let result = hover_at(src, line, col + 1).expect("hover result for thread_spawn");
        let text = result["contents"]["value"].as_str().expect("hover value is a string");
        assert!(text.contains("thread"), "expected module name \"thread\" in: {text}");
        assert!(text.contains("1 parameter"), "expected arity 1 in: {text}");
    }

    #[test]
    fn hover_on_map_get_reports_module_and_arity() {
        let src = "import std map;\nmake main() begin\n  map_get(m, k);\nend\n";
        let (line, col) = locate(src, "map_get");
        let result = hover_at(src, line, col + 1).expect("hover result for map_get");
        let text = result["contents"]["value"].as_str().expect("hover value is a string");
        assert!(text.contains("map"), "expected module name \"map\" in: {text}");
        assert!(text.contains("2 parameters"), "expected arity 2 in: {text}");
    }

    #[test]
    fn completion_includes_builtin_stdlib_functions_from_all_modules() {
        let items = completion_items("");
        let arr = items.as_array().expect("completion result is an array");
        let label = |name: &str| {
            arr.iter()
                .find(|i| i["label"] == name)
                .unwrap_or_else(|| panic!("completion missing builtin `{name}`"))
        };

        let thread_join = label("thread_join");
        assert!(thread_join["detail"].as_str().unwrap().contains("thread"));

        let thread_spawn = label("thread_spawn");
        assert!(thread_spawn["detail"].as_str().unwrap().contains("thread"));

        let now_ms = label("now_ms");
        assert!(now_ms["detail"].as_str().unwrap().contains("time"));

        let map_new = label("map_new");
        assert!(map_new["detail"].as_str().unwrap().contains("map"));

        let read_line = label("read_line");
        assert!(read_line["detail"].as_str().unwrap().contains("io"));
    }

    #[test]
    fn builtin_func_doc_matches_all_codegen_arities() {
        // Sanity check against src/codegen.rs's authoritative tables
        // (IO_FUNCS, MAP_FUNCS, THREAD_FUNCS, TIME_FUNCS) plus thread_spawn.
        let expected: &[(&str, &str, usize)] = &[
            ("read_line", "io", 0),
            ("file_read", "io", 1),
            ("file_write", "io", 2),
            ("file_append", "io", 2),
            ("tcp_connect", "io", 2),
            ("tcp_listen", "io", 1),
            ("tcp_accept", "io", 1),
            ("send_line", "io", 2),
            ("recv_line", "io", 1),
            ("close_conn", "io", 1),
            ("map_new", "map", 0),
            ("map_set", "map", 3),
            ("map_get", "map", 2),
            ("map_has", "map", 2),
            ("map_remove", "map", 2),
            ("map_len", "map", 1),
            ("thread_spawn", "thread", 1),
            ("thread_join", "thread", 1),
            ("thread_sleep_ms", "thread", 1),
            ("mutex_new", "thread", 0),
            ("mutex_lock", "thread", 1),
            ("mutex_unlock", "thread", 1),
            ("channel_new", "thread", 0),
            ("channel_send", "thread", 2),
            ("channel_recv", "thread", 1),
            ("now_ms", "time", 0),
            ("monotonic_ms", "time", 0),
            ("sleep_ms", "time", 1),
            ("clock_ms", "time", 0),
            ("difftime_ms", "time", 2),
            ("linux_clock_gettime_ns", "time", 1),
            ("linux_nanosleep_ns", "time", 1),
            ("win_filetime_100ns", "time", 0),
            ("win_sleep_ms", "time", 1),
        ];
        for (name, module, arity) in expected {
            assert_eq!(builtin_func_doc(name), Some((*module, *arity)), "mismatch for {name}");
        }
    }
}
