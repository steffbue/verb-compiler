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

- `verb run` (JIT) does not support imports — programs using `import mod`
  must be built with `verb build`/`compile`, not run.

See `docs/superpowers/specs/2026-07-20-cpp-import-design.md` for the full
design.

## Language

See `docs/superpowers/specs/2026-07-19-verb-compiler-design.md` for the spec.

    %% comment
    assign x 41;
    x be x add 1;
    make add2(a, b) begin
      return a add b;
    end
    print(add2(x, 1));   %% 43

## Known v1 limitations

- No GC — heap allocations are never freed
- No arrays/maps, no `break`/`continue`, no anonymous functions
- No closures — a nested `make` cannot reference any variable from its
  enclosing function's scope (not even ones declared before it); it can
  only see its own parameters/locals and top-level globals
- Shadowing the builtin `print` has no effect — calls named `print`
  always hit the builtin
