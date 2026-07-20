# `import std io` — Design Spec (v1)

Date: 2026-07-20
Status: approved

## Purpose

Let Verb programs use a curated set of C++ standard library I/O
capabilities — reading stdin, whole-file read/write, and basic blocking
TCP sockets — without the user having to hand-write an `extern "C"`
wrapper library (as required by the existing generic [`import mod`
mechanism](2026-07-20-cpp-import-design.md)). Verb ships and auto-links
the wrapper itself.

This is deliberately narrower than "full C++ stdlib access." Verb has no
array/object/generic-type support yet (see README's "Known v1
limitations"), and true generic container/template access (`std::vector<T>`
for arbitrary `T`, arbitrary classes) would require a per-build C++
template-instantiation pipeline — out of scope for this spec, left for a
future one if pursued.

## Language surface

```
import std io;

assign line read_line();
check line eq nil begin
  print("no input");
end orelse begin
  print(line);
end
```

- New keyword `std`, alongside the existing `mod`.
- `import std <ident>;` — top-level statement, same positioning rules as
  `import mod`: must appear before any other top-level statement,
  repeatable, deduplicated at parse time. Can coexist with `import mod`
  statements in the same file.
- v1 recognizes exactly one module name: `io`. An unrecognized name
  after `std` is a compile error naming the bad identifier and listing
  valid module names — unlike `import mod` library names (arbitrary,
  unverifiable until link time), `std` module names are first-party and
  fully known ahead of time, so this is checked at parse/compile time.

## AST

```rust
pub struct Program {
    pub imports: Vec<String>,      // import mod: linker -l<name> targets
    pub std_imports: Vec<String>,  // import std: builtin module names, e.g. "io"
    pub body: Vec<Stmt>,
}
```

## Call resolution

Extends the existing resolution rule (`name(args)` where `name` is not a
local variable or a Verb `fn`) with one more tier, checked in order:

1. Local variable / Verb `fn` — existing behavior, always wins.
2. If `std_imports` contains `io` and `name` is one of `io`'s known
   functions (table below): bind to it. Arity is validated **at compile
   time** against the known table (a hard error on mismatch) — stronger
   than generic `mod` externs, whose arity is only checked against a
   prior call site of the same name, because these signatures are
   first-party and fully known in advance.
3. Else, existing generic `import mod` extern resolution (unchanged).
4. Else, existing "undefined variable" compile error (unchanged).

## Functions (`io` module)

All defined in `runtime/verb_std_io.cpp`, shipped with Verb, built against
`runtime/verb.h` (the existing `VerbValue` C-ABI struct — same ABI as
generic `import mod` externs, so codegen needs no new value-conversion
logic, only the new name-resolution tier above).

Uniform error convention: **failure returns `verb_nil()`**. No C++
exception ever crosses the FFI boundary. Callers check with the existing
`check x eq nil` construct.

| Verb name | Args | Returns | Notes |
|---|---|---|---|
| `read_line()` | — | string, or `nil` at EOF | reads one line from stdin, strips trailing `\n` |
| `file_read(path)` | string | string, or `nil` on failure | whole file contents |
| `file_write(path, contents)` | string, string | `true`, or `nil` on failure | overwrite/create |
| `file_append(path, contents)` | string, string | `true`, or `nil` on failure | create if missing |
| `tcp_connect(host, port)` | string, int | int fd, or `nil` | blocking connect |
| `tcp_listen(port)` | int | int fd, or `nil` | bind + listen, fixed backlog |
| `tcp_accept(fd)` | int | int fd, or `nil` | blocking accept |
| `send_line(fd, s)` | int, string | `true`, or `nil` | appends `\n`, writes to socket |
| `recv_line(fd)` | int | string, or `nil` at EOF/error | reads until `\n` or EOF |
| `close_conn(fd)` | int | `nil` always | closes any fd (socket or file-derived) |

File and socket handles reuse the existing `VERB_INT` tag (an fd is
already an integer) — no new `VerbValue` tag needed.

## Codegen

- `Codegen` gains `std_imports: Vec<String>` (copied from `Program`) and
  a static table `{ name -> (arity, param VerbValue count, returns
  VerbValue) }` for the `io` module's functions — all of them take and
  return plain `VerbValue`, so the table only needs to record arity.
- In `gen_call`, before falling through to the existing generic-extern
  path: if `callee` is `Expr::Var(name)`, `name` is not local/a Verb fn,
  `std_imports` contains `io`, and `name` is in the table — verify
  `args.len()` matches the table's arity (compile error on mismatch,
  citing the call site), declare (once, memoized like `externs`) an
  `add_function(name, value_ty.fn_type(&[value_ty; arity], false), None)`,
  and emit a direct `build_call`.
- Falls through to the unchanged generic `mod` extern path otherwise.

## Build integration (`verb build`)

- Whenever `std_imports` contains `io`: compile `runtime/verb_std_io.cpp`
  with `c++ -c` (the same compiler already forced by any import — generic
  `mod` or `std`) into a temp object file, and link that object alongside
  the LLVM-emitted object for the program. No extra `-l` flags required —
  stdio/file/socket calls resolve against libc/libSystem, always linked.
- A `c++` compile failure on `verb_std_io.cpp` itself (first-party,
  covered by CI) surfaces as a build error the same way any other link
  failure does today — not expected in normal operation.

## `verb run` (JIT)

Rejected the same way `import mod` is today: if `std_imports` is
non-empty, error before attempting to JIT — "std imports require 'verb
build' — JIT does not support std io calls in v1."

## Testing

- Parser unit tests: `import std io;` parsing, unknown-module-name
  compile error, ordering-must-precede-statements error, dedup of
  repeated `import std io;`, coexistence with `import mod`.
- Codegen unit test: arity mismatch against the known `io` table →
  compile error.
- E2e fixtures (extends the existing `verb build` + execute + diff-stdout
  harness):
  - JIT-rejection test: `verb run` on a program with `import std io;`
    errors out.
  - File roundtrip: `file_write` then `file_read` same path, diff
    contents.
  - Loopback TCP: single process does `tcp_listen` + `tcp_connect` +
    `tcp_accept` + `send_line`/`recv_line` + `close_conn`, diff stdout.

## Out of scope (future spec)

Additional `std` modules beyond `io`; generic containers/templates
(`std::vector<T>`, `std::string` as an object, `std::sort`, etc. for
arbitrary `T`); non-blocking/async sockets; UDP; streaming/handle-based
file I/O (only whole-file read/write in v1, matching Verb's scalar-only
value model); TLS.
