# Cross-platform `verb compile` — design

## Context

`verb build` / `verb compile` (alias, added in `fdd0b3e`) currently call `build_aot`,
which is a stub: `eprintln!("build: not implemented yet"); exit(1);`. Task 9 of the
original implementation plan (`docs/superpowers/plans/2026-07-19-verb-compiler.md`)
specified a real AOT build (LLVM object emit + host `cc` link) but was never
implemented — later commits moved on to closures, the formatter, and the VSCode
extension instead.

This spec covers implementing real AOT build **and** extending it with
cross-platform target selection: Linux / macOS / Windows × x86_64 / arm64.

## Goals

- `verb build f.verb -o out` (no `--target`): produces a native binary for the host,
  using the host `cc` — matches the original Task 9 behavior, no new dependency.
- `verb build f.verb -o out --target <os>-<arch>`: cross-compiles to one of 6
  supported (os, arch) combos.
- `verb build f.verb -o out --target all`: builds all 6 combos in one invocation.

## Non-goals

- No target auto-detection beyond the host default (e.g. no reading a config file
  for a preferred target list).
- No packaging (installers, `.app` bundles, codesigning) — a raw executable/PE/Mach-O
  binary is the deliverable.
- No `verb targets` introspection command — invalid target values get a usage error
  listing the valid set inline.

## CLI

```
verb build <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]
verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]  (alias)
```

`<os>` ∈ `{linux, macos, windows}`. `<arch>` ∈ `{x86_64, arm64}` (`x86` accepted as an
alias for `x86_64`). Unknown/malformed `--target` value → usage-style error listing
the valid `os-arch` pairs, exit 2.

## Target mapping

| CLI `os-arch` | LLVM triple | zig triple |
|---|---|---|
| linux-x86_64 | x86_64-unknown-linux-gnu | x86_64-linux-gnu |
| linux-arm64 | aarch64-unknown-linux-gnu | aarch64-linux-gnu |
| macos-x86_64 | x86_64-apple-darwin | x86_64-macos-none |
| macos-arm64 | aarch64-apple-darwin | aarch64-macos-none |
| windows-x86_64 | x86_64-pc-windows-gnu | x86_64-windows-gnu |
| windows-arm64 | aarch64-pc-windows-gnu | aarch64-windows-gnu |

## Linking strategy

Two code paths in `build_aot`:

1. **No `--target`** (legacy/default): `Target::initialize_native`, host triple via
   `TargetMachine::get_default_triple()`, emit `<out>.o`, link with the host `cc`
   exactly as originally planned in Task 9. No new dependency for the common case.

2. **`--target <os>-<arch>` or `--target all`**: `Target::initialize_all` (cross
   targets require every backend registered, not just the host's), emit the object
   file for the mapped LLVM triple, then link with `zig cc -target <zig-triple> <obj>
   -o <out>` instead of `cc`. Chosen over (a) requiring per-target native toolchains
   — impractical, and cross-to-macOS is blocked by Apple SDK licensing anyway — or
   (b) emitting unlinked object files for non-host targets — punts the actual "compile
   for platform X" ask back onto the user. `zig cc` bundles clang plus libc sysroots
   for all 6 combos behind one dependency (the same approach `cargo zigbuild` uses).

   Before invoking `zig`, check it's on `PATH`. If missing, exit 1 with:
   `"zig not found on PATH. Cross-compiling requires zig
   (https://ziglang.org/download/) as the linker driver. Install it, or omit
   --target to build for this host with cc."`
   This check runs once per invocation (not once per target inside `--target all`).

## Output naming

- No `--target`, or a single explicit `--target <os>-<arch>`: output is written to
  exactly the `-o` path — except when the target os is `windows`, in which case
  `.exe` is auto-appended if the given path doesn't already end in `.exe`.
- `--target all`: each combo writes to `<out>-<os>-<arch>`, with `.exe` appended for
  the two windows combos. E.g. `-o hello --target all` produces `hello-linux-x86_64`,
  `hello-linux-arm64`, `hello-macos-x86_64`, `hello-macos-arm64`,
  `hello-windows-x86_64.exe`, `hello-windows-arm64.exe`.

## Error handling

- Single-target build (explicit or default): any failure (object emit, missing zig,
  link failure) prints an error and exits 1, same as the current stub's failure mode.
- `--target all`: best-effort — a failure on one combo is recorded and the loop
  continues to the rest. After all 6 attempts, print a summary table (`os-arch: ok`
  or `os-arch: FAILED — <reason>`). Exit 0 only if all 6 succeeded, else exit 1.

## Testing

- `aot_build_produces_working_binary` (from the original Task 9 plan): default
  no-`--target` build, executes the resulting binary, checks output. Always runs,
  no external dependency.
- New cross-target e2e tests: build for each of the 6 explicit targets and for `all`,
  asserting the expected output file(s) exist and are non-empty (can't execute a
  foreign-arch/OS binary on the CI host). Each test checks for `zig` on `PATH` first
  and skips with a printed note (not a failure) if it's absent, so CI doesn't hard-fail
  over a missing external tool it may not have installed.
- `README.md` gets a "Cross-compiling" section documenting the `--target` flag, the
  6 supported combos, and the `zig` dependency for cross-targets.

## Known limitations (document in README)

- Cross-compiled binaries are not executed or verified beyond "linked successfully" —
  no CI machine covers all 6 target/host pairs.
- `zig` is only required for `--target`; the default host build has no new dependency.
