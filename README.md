# Verb

A tiny dynamically typed language compiled to LLVM IR. Educational project:
lexer → parser → AST → LLVM IR (inkwell) → JIT or native binary.

## Requirements

- Rust (2021)
- LLVM 20.1 (`brew install llvm`) — path wired via `.cargo/config.toml`
- A C compiler (`cc`) for linking host AOT builds
- [zig](https://ziglang.org/download/) for cross-platform builds (`--target`) — not required for the default host build

## Usage

    cargo run -- run examples/hello.verb          # JIT
    cargo run -- run examples/hello.verb --emit-llvm
    cargo run -- build examples/hello.verb -o hello   # native binary for this host

`compile` is an alias for `build` — `cargo run -- compile ...` behaves identically.

### Using the `verb` binary directly

The crate's package name is `verb`, so `cargo build`/`cargo install` produce a
binary named `verb` — you don't have to go through `cargo run -- ...` every
time.

Build once, then call the binary directly:

    cargo build --release
    ./target/release/verb run examples/hello.verb
    ./target/release/verb build examples/hello.verb -o hello

Or install it onto your `PATH` (`~/.cargo/bin`) so plain `verb` works
anywhere, like a normal CLI tool:

    cargo install --path .
    verb run examples/hello.verb
    verb build examples/hello.verb -o hello
    verb compile examples/hello.verb -o hello   # alias for build

Re-run `cargo install --path .` after pulling new changes to refresh the
installed binary.

## Cross-compiling

    cargo run -- build examples/hello.verb -o hello --target linux-x86_64
    cargo run -- build examples/hello.verb -o hello --target windows-arm64
    cargo run -- build examples/hello.verb -o hello --target all

Supported `<os>-<arch>` combos: `linux-x86_64`, `linux-arm64`, `macos-x86_64`,
`macos-arm64`, `windows-x86_64`, `windows-arm64` (`x86` is accepted as an alias
for `x86_64`). Cross-target builds link with `zig cc` instead of `cc` — install
zig first, or omit `--target` to build for the host with no extra dependency.

Windows targets get `.exe` appended to the output path automatically.
`--target all` writes one binary per combo, named `<out>-<os>-<arch>`
(`<out>-windows-x86_64.exe` etc.), and is best-effort: it builds every combo,
prints a pass/fail summary, and exits non-zero only if at least one failed.

Cross-compiled binaries aren't executed as part of the build (or by the test
suite) — there's no host that can run all six target/arch combinations, so
only "linked successfully" is verified.

## Importing C++ libraries

Verb programs can call `extern "C"` functions from a native library:

    import mod mathlib;

    assign r c_sqrt(2.0);
    print(r);

- `import mod <name>;` must appear before any other top-level statement,
  is repeatable, and maps to a linker `-l<name>` flag.
- Extern functions are declared with no signature — write the C++ side
  against Verb's tagged value struct (`runtime/verb.h`, `VerbValue`), e.g.
  `extern "C" VerbValue c_sqrt(VerbValue x) { ... }`.
- `verb build`/`compile` link with `c++` instead of `cc` whenever imports
  are present, pass `-l<name>` per import, and accept a repeatable
  `-L<dir>` flag for extra linker search paths:

      verb build examples/uses_mathlib.verb -o out -Lpath/to/libs

- `verb run` (JIT) executes `import mod` programs in-process too, but the
  library must be a **shared** library findable via `-L`/default loader
  paths — static `.a` archives still require `verb build`/`compile`.

See `docs/superpowers/specs/2026-07-20-cpp-import-design.md` for the full
design.

## Importing other Verb files

`import mod` also pulls in another Verb source file, not just a C++
library — the CLI itself now only ever takes a single entry file, so
multi-file programs are built entirely through this:

    %% utils.verb
    make double(x) begin
      return x times 2;
    end

    %% main.verb
    import mod utils.verb;

    print(double(21));

- Disambiguation is purely by name: `import mod <name>;` (no `.verb`
  suffix) is a C++ library; `import mod <name>.verb;` is a Verb source
  file.
- The path is a bare filename (no `/`, no subdirectories in v1), resolved
  relative to the directory of the file doing the importing — not the
  current working directory.
- Imports are recursive (an imported file can `import mod` further files)
  and deduplicated (the same file imported from two places is only
  included once). A file that imports itself, directly or transitively,
  is a compile error.
- Everything imported lands in one flat global scope, same as if it had
  all been written in one file — there's no `utils.helper()`-style
  qualified access in v1.

See `docs/superpowers/specs/2026-07-21-verb-file-import-design.md` for the
full design.

## Standard library I/O (`import std io`)

Unlike `import mod`, which requires writing your own `extern "C"`
wrapper, `import std io;` gives Verb programs a small set of built-in
functions for stdin, whole-file read/write, and blocking TCP sockets —
Verb compiles and links the C++ implementation itself.

    import std io;

    assign contents file_read("notes.txt");
    print(contents);

Available functions: `read_line()`, `file_read(path)`,
`file_write(path, contents)`, `file_append(path, contents)`,
`tcp_connect(host, port)`, `tcp_listen(port)`, `tcp_accept(fd)`,
`send_line(fd, s)`, `recv_line(fd)`, `close_conn(fd)`. Every function
returns `nil` on failure — check with `check x eq nil`.

- Only `io`, `map`, and `time` modules exist in v1 (`import std io;` /
  `import std map;` / `import std time;`); an unrecognized module name
  after `std` is a compile error.
- Like `import mod`, `import std io;` must appear before any other
  top-level statement. `verb run` (JIT) executes it in-process, same as
  `verb build`/`compile`.
- Cross-compiling to a Windows target (`--target windows-x86_64` /
  `windows-arm64`) with `import std io;` is not supported in v1 — the
  implementation uses POSIX socket APIs unavailable under the mingw
  cross toolchain.

See `docs/superpowers/specs/2026-07-20-std-io-import-design.md` for
the full design.

## Arrays

Growable arrays use a `list` literal and `get`/`set`/`push`/`pop`/`len`
built-in functions — no `[...]` bracket syntax:

    assign a list 10, 20, 30;
    print(get(a, 0));      %% 10
    set(a, 0, 99);
    push(a, 40);
    print(a);                %% [99, 20, 30, 40]
    print(len(a));           %% 4
    assign x pop(a);
    print(x);                 %% 40

`list e1, e2, ...` has no closing delimiter — it greedily consumes every
comma-separated expression that follows, so it can't be followed by a
sibling argument in a call, and a `list` literal nested as a non-final
element of another `list` gets swallowed by the inner one. Build nested
arrays via variables instead:

    assign inner list 1, 2;
    assign outer list inner, inner;   %% [[1, 2], [1, 2]]

Out-of-bounds `get`/`set`, a `pop` on an empty array, or a non-array
argument to any of these functions is a runtime error, same as other
type/bounds errors in Verb. `eqeq`/`neq` on two arrays compares them by
reference (same underlying array), not by contents.

## Standard library maps (`import std map`)

`import std map;` gives Verb programs a hash-map (dictionary) type,
compiled and linked in the same way `import std io;` is.

    import std map;

    assign m map_new();
    map_set(m, "name", "compiler");
    print(map_get(m, "name"));   %% compiler
    print(map_has(m, "missing")); %% false

Available functions: `map_new()`, `map_set(m, k, v)` (returns `m`),
`map_get(m, k)`, `map_has(m, k)`, `map_remove(m, k)`, `map_len(m)`. Map
values may be any Verb value. Map keys are restricted to nil, bool, int,
float, or string — int and float keys that are numerically equal (`1` and
`1.0`) refer to the same entry, matching the `equals` operator's own
cross-type numeric equality. Invalid usage (a non-map `m`, or an
unsupported key type) returns `nil`/`false`/`0` rather than aborting,
same as `std io`.

- Like `import std io;`, `import std map;` must appear before any other
  top-level statement. `verb run` (JIT) executes it in-process, same as
  `verb build`/`compile`.
- No `map_keys`/`map_values`/iteration in v1 — maps don't yet return their
  contents as arrays.

See `docs/superpowers/specs/2026-07-21-maps-design.md` for the full design.

## Structs (`shape`)

`shape` declares a struct type with a fixed, ordered set of named fields.
Instances are constructed by calling the type name with one positional
argument per field; fields are read and written with the `getf`/`setf`
built-ins:

    shape Point begin x, y end

    assign p Point(3, 4);
    print(getf(p, "x"));       %% 3
    setf(p, "y", 99);
    print(getf(p, "y"));       %% 99
    print(p);                   %% Point{x: 3, y: 99}

- Field lists are comma-separated bare identifiers between `begin`/`end`
  (a trailing comma is allowed); a struct may have zero fields. Duplicate
  field names are a compile error.
- `TypeName(a, b, ...)` takes exactly one argument per declared field, in
  declared order — a wrong count is a compile error. A local/global
  variable with the same name as a struct type shadows the constructor
  (the name is then treated as an ordinary call).
- `getf(s, "field")` returns the field's value; `setf(s, "field", v)`
  overwrites it in place and returns `v`. A missing field name, or a
  non-struct first argument, is a runtime error.
- Fields may hold any Verb value, including other structs and arrays;
  nested heap values are reference-counted and released with the struct.
- `equals`/`differs` compare structs by reference (same underlying
  instance), like arrays.
- Top-level `shape` decls may be used before their point of declaration
  (they're registered ahead of code generation); a `shape` nested inside
  a function/block is only visible from its declaration onward.

Unlike `import std map`, structs need no `import` — `shape` is a core
language construct and works under both `verb run` (JIT) and
`verb build`.

## Standard library time (`import std time`)

`import std time;` gives Verb programs wall-clock/monotonic millisecond
timestamps and a blocking sleep, compiled and linked in the same way
`import std io;`/`import std map;` are.

    import std time;

    assign start monotonic_ms();
    sleep_ms(250);
    print(monotonic_ms() sub start atleast 250);   %% true

Portable functions (every platform): `now_ms()` (milliseconds since the
Unix epoch, wall-clock — can jump backwards/forwards if the system clock
is adjusted), `monotonic_ms()` (milliseconds from a monotonic clock —
never goes backwards, only meaningful as a difference between two calls,
so prefer it over `now_ms()` for measuring elapsed time), `sleep_ms(ms)`
(blocks the calling thread for `ms` milliseconds; `ms <= 0` is a no-op),
`clock_ms()` (CPU time consumed by this process, in milliseconds — the
same quantity C's `clock()` reports; does *not* advance while blocked or
sleeping), `difftime_ms(later, earlier)` (equivalent to `later sub
earlier`, offered under C's familiar name).

Platform-specific functions — only defined (and only linkable) when
`verb build`/`compile` targets that platform, direct bindings to the
underlying OS API rather than the `<chrono>`/`<thread>` wrappers above:

- Linux: `linux_clock_gettime_ns(clock_id)` (nanoseconds from
  `clock_gettime`; `clock_id` is the raw Linux `clockid_t` value — `0`
  for `CLOCK_REALTIME`, `1` for `CLOCK_MONOTONIC`), `linux_nanosleep_ns(ns)`
  (`nanosleep`; `ns <= 0` is a no-op).
- Windows: `win_filetime_100ns()` (`GetSystemTimeAsFileTime`, raw FILETIME
  as 100ns intervals since 1601-01-01), `win_sleep_ms(ms)` (`Sleep`;
  `ms <= 0` is a no-op).

Calling a Linux function in a Windows build (or vice versa) is a link
error, not a compile error — same tradeoff generic `import mod` externs
already have for an unresolved name (see "C++ import" above).

- Like `import std io;`, `import std time;` must appear before any other
  top-level statement, and `verb run` (JIT) does not support it — use
  `verb build`/`compile`.
- No Windows restriction on the portable functions (`<chrono>`/`<thread>`
  are portable) — unlike `std io`, `std time` cross-compiles to every v1
  target.

See `docs/superpowers/specs/2026-07-21-time-design.md` for the full design.

## Language

See `docs/superpowers/specs/2026-07-19-verb-compiler-design.md` for the spec.

    %% comment
    assign x 41;
    x be x add 1;
    make add2(a, b) begin
      return a add b;
    end
    print(add2(x, 1));   %% 43

    %% loops: `repeat` (while) and `loop` (for), with `leave` (break)
    %% and `next` (continue)
    loop assign i 0; i trails 5; i be i add 1 begin
      check i equals 2 begin next; end   %% skip 2 (increment still runs)
      check i equals 4 begin leave; end  %% stop before 4
      print(i);                          %% 0 1 3
    end

## Known v1 limitations

- No GC — heap allocations are never freed
- No anonymous functions
- No closures — a nested `make` cannot reference any variable from its
  enclosing function's scope (not even ones declared before it); it can
  only see its own parameters/locals and top-level globals
- Shadowing the builtin `print` has no effect — calls named `print`
  always hit the builtin
