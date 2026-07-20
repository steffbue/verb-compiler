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
