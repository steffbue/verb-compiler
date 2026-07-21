# Interactive debugger for verb — design spec

Date: 2026-07-21

## Goal

Add `verb debug <file>` — a source-level, statement-granularity interactive
debugger (breakpoints, step, print variable, backtrace) for the JIT
execution path, in the style of a minimal gdb/lldb console.

## Non-goals (v1)

- No DWARF/native debug info, no lldb/gdb integration.
- No support for `verb build` (AOT) — JIT only.
- No arbitrary expression evaluation in `print` (variable names only).
- No step-over (a single `step` command always steps to the next executed
  statement, including into called functions — no separate "next").
- No watchpoints.
- No multi-file breakpoint targeting (`break <line>` assumes the main file;
  multi-file programs are out of scope for v1 breakpoint addressing).
- No post-mortem inspection on uncaught runtime errors — errors print and
  exit exactly like `verb run` does today.

## Architecture

`verb debug foo.verb` compiles the program with a debug-hooks flag enabled
on `Codegen`, then JIT-executes it like `verb run`, except the compiled IR
additionally contains:

- A call to `verb_debug_checkpoint(line, vars_ptr, vars_len)` at the start
  of every statement.
- A call to `verb_debug_push_frame(fn_name_ptr, call_line)` at the top of
  every function body, and `verb_debug_pop_frame()` at every `Return` and
  at implicit fall-through return.

These hooks are plain Rust `extern "C" fn`s bound into the JIT module via
`ExecutionEngine::add_global_mapping` — no C++ runtime file changes, no new
build step. They read/write a global `DebuggerState` (thread-local is
sufficient; the JIT runs single-threaded):

```rust
struct DebuggerState {
    breakpoints: HashSet<u32>,   // source line numbers
    stepping: bool,              // true = stop at the very next checkpoint
    started: bool,               // false until the user issues `run`
    frames: Vec<Frame>,          // call stack, top = current
}
struct Frame { fn_name: String, call_line: u32 }
```

`verb_debug_checkpoint` logic: if `stepping` or `line` is in `breakpoints`,
print source context and enter the console loop (blocking on stdin) until
the user issues `continue` or `step`; otherwise return immediately. This
keeps per-statement overhead to a hashset lookup in the common (not
stopped) case.

Before the first `run` command, execution has not started — `verb_debug_checkpoint`
is not yet being called, so `break`/`delete`/`run`/`quit` are the only
valid commands.

## AST changes

Today only `Reassign`, `Fn`, and expression nodes (`Var`, `Binary`, `Unary`,
`Call`) carry `line`/`col`. Statement-granularity breakpoints require it on
every `Stmt` variant, so add `line: u32, col: u32` to `Assign`, `Declare`,
`ExprStmt`, `Block`, `If`, `While`.

This is mechanical (the parser already has the starting token's position
available at every statement construction site) but touches:

- `src/ast.rs` — enum field additions.
- `src/parser.rs` — every `Stmt` construction site threads through the
  position of the statement's leading token.
- `src/codegen.rs::gen_stmt` — every match arm destructures the new fields
  (only used to drive the checkpoint call; no behavioral change to codegen
  otherwise).
- `src/formatter.rs` — existing match patterns need `..` added since they
  don't use the new fields.
- Existing tests that construct or `assert_eq!` compare `Stmt` values
  directly (parser unit tests, formatter round-trip tests) will need
  updated line/col literals, since `Stmt` derives `PartialEq`. This is a
  known, called-out fixup pass — not a design risk, just churn.

## Variable table per checkpoint

Variable names exist only at compile time today (`Codegen.scopes: Vec<HashMap<String,
PointerValue>>`); at runtime there are only anonymous heap cells allocated
via the refcounting-GC-v2 `verb_alloc` header-carrying allocator. So each
checkpoint call site bakes a small array reflecting the scope visible at
that point in the source:

```rust
#[repr(C)]
struct DebugVar { name: *const c_char, cell: *mut VerbValue }
```

Codegen emits an `alloca [DebugVar; N]` at each checkpoint site, N being
the number of variables in scope there, populates it (name = a compile-time
constant global string per variable; cell = the already-known
`PointerValue` for that variable in `self.scopes`), and passes `ptr, len`
to `verb_debug_checkpoint`.

`print <name>` in the console does a linear scan of the *current* (topmost
stopped) checkpoint's array, loads the `VerbValue` from the matched cell,
and renders it via the same LLVM-IR-generated value-formatting function
`print()` already uses (`build_print_value_fn`) — reused, not reimplemented.
This performs no retain/release on the inspected value — `print` only
reads it for display, never stores or drops a reference, so it's safe
under the refcounting scheme without extra bookkeeping.

Closures do not capture enclosing scope today (confirmed: nested `make`
resets `self.scopes`, `env_ptr` is always null), so each checkpoint's
variable table is self-contained — no upvalue chain to walk for v1.

## Backtrace

`frames: Vec<Frame>` is maintained purely via the push/pop hooks described
above. `backtrace`/`bt` prints it top-to-bottom (current frame first),
each line as `<fn_name> (called at line <call_line>)`.

## CLI

New `"debug"` arm in `main.rs`, alongside `"run"`/`"build"`. `compile_program`
currently runs once, before branching on `parsed.cmd` — the debug-hooks
flag must be known before that call, so the cmd is checked first and the
flag threaded into `Codegen::new(...)` (or set via a small builder method)
before compiling. `run`/`build` compile with hooks off (zero overhead,
unchanged codegen output).

On `debug`, after compiling: create the JIT execution engine exactly as
`run` does, `add_global_mapping` the three hook symbols, print the initial
`(vdb)` prompt, and enter the pre-run console loop.

## REPL grammar

Prompt: `(vdb) `

| Command | Alias | Valid | Effect |
|---|---|---|---|
| `break <line>` | `b` | always | add breakpoint |
| `delete <line>` | | always | remove breakpoint |
| `run` | `r` | before start | begin execution |
| `continue` | `c` | while stopped | resume until next breakpoint/step |
| `step` | `s` | while stopped | execute one statement, stop |
| `print <name>` | `p` | while stopped | show variable value |
| `backtrace` | `bt` | while stopped | show call stack |
| `quit` | `q` | always | abort program |

Program completion (normal exit or uncaught runtime error) prints exactly
what `verb run` would print, then the console session ends.

## Testing

- `tests/e2e.rs` already spawns `CARGO_BIN_EXE_verb` and asserts on stdout.
  Debug tests add `Stdio::piped()` on stdin, write a scripted command
  sequence (e.g. `break 3\ncontinue\nprint x\ncontinue\n`), and assert
  stdout contains the expected prompt/value/backtrace lines.
- Cover: breakpoint hit, step through a loop, print a variable of each
  VerbValue type (int/float/bool/string/nil/array/map), backtrace across
  at least two nested calls, breakpoint on a line inside a called function
  (step-into semantics), quit mid-session.
- Fixup pass: update existing parser/formatter unit tests for the new
  `Stmt` fields so `cargo test` passes before adding new debugger tests.
