# Cross-platform Compile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the still-stubbed `build_aot` for real (host-default AOT build), then extend `verb build`/`verb compile` with a `--target <os>-<arch>|all` flag that cross-compiles to Linux/macOS/Windows × x86_64/arm64 by linking with `zig cc`.

**Architecture:** `src/targets.rs` holds a small `Target` type (os × arch) with LLVM-triple/zig-triple/output-path logic and no I/O. `src/main.rs` gets three build functions: `build_aot_host` (existing Task 9 design — host triple, `cc`), `build_aot_cross` (any single explicit target — all LLVM backends, `zig cc`), and `build_aot_all` (loops all 6 `Target::ALL`, best-effort, prints a summary). CLI parsing picks between them based on whether `--target` is present and what it's set to.

**Tech Stack:** Rust 2021, inkwell 0.9 (LLVM 20.1) targets API, `zig cc` (external, cross-target only), `cc` (external, host-default only, matches current toolchain requirement).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-20-cross-platform-compile-design.md`
- Default `verb build`/`compile` (no `--target`) has **no new dependency** — uses host `cc`, exactly the original Task 9 design.
- `--target <os>-<arch>` and `--target all` require `zig` on `PATH`; missing `zig` fails fast with an install hint, not a raw spawn error.
- `<os>` ∈ `{linux, macos, windows}`, `<arch>` ∈ `{x86_64, arm64}`, `x86` accepted as an alias for `x86_64`.
- Windows targets get `.exe` auto-appended to the output path if not already present.
- `--target all` output naming: `<out>-<os>-<arch>` (plus `.exe` for windows), e.g. `hello-windows-x86_64.exe`.
- `--target all` is best-effort: one failing combo doesn't stop the rest; a summary prints at the end; exit 0 only if all 6 succeeded.
- No datalayout-dependent codegen exists in `src/codegen.rs` today (confirmed: no `target_data`/`ptr_sized`/`get_abi_size` usage) — safe to re-target-machine the same module across triples in a loop.

---

### Task 1: Real AOT build for the host (replace the stub)

**Files:**
- Modify: `src/main.rs:80-83` (replace the `build_aot` stub)
- Modify: `tests/e2e.rs` (new test)

**Interfaces:**
- Consumes: `codegen::Codegen::module() -> &Module<'ctx>` (existing, `src/codegen.rs:52`)
- Produces: `fn build_aot_host(cg: &codegen::Codegen, out: &str)` — used by Task 2's CLI dispatch

- [ ] **Step 1: Write the failing test** — append to `tests/e2e.rs`:

```rust
#[test]
fn aot_build_produces_working_binary() {
    let dir = std::env::temp_dir().join("verb_aot_host_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("functions_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/functions.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e aot_build_produces_working_binary -v`
Expected: FAIL — stderr contains `build: not implemented yet`

- [ ] **Step 3: Replace the stub in `src/main.rs`**

Replace lines 80-83 (`fn build_aot(_cg: &codegen::Codegen, _out: &str) { ... }`) with:

```rust
fn build_aot_host(cg: &codegen::Codegen, out: &str) {
    use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("target init error: {e}"); exit(1); });
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("target error: {e}"); exit(1); });
    let tm = target
        .create_target_machine(&triple, "generic", "",
            inkwell::OptimizationLevel::Default, RelocMode::PIC, CodeModel::Default)
        .unwrap_or_else(|| { eprintln!("cannot create target machine"); exit(1); });
    cg.module().set_triple(&triple);

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let status = std::process::Command::new("cc")
        .args([obj.as_str(), "-o", out])
        .status()
        .unwrap_or_else(|e| { eprintln!("cc failed to start: {e}"); exit(1); });
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}
```

Update the callsite at `src/main.rs:74` from `build_aot(&cg, &out);` to `build_aot_host(&cg, &out);` (Task 2 will replace this callsite again with full `--target` dispatch — this step just gets the host path working end to end first).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e aot_build_produces_working_binary -v`
Expected: PASS

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all existing tests still pass

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat: implement real AOT build (host default)"
```

---

### Task 2: `Target` type — os/arch parsing, triple mapping, output naming

**Files:**
- Create: `src/targets.rs`
- Modify: `src/main.rs:1` (add `mod targets;`)
- Test: `src/targets.rs` (inline unit tests, `#[cfg(test)]`)

**Interfaces:**
- Produces (used by Task 3 and Task 4):
  - `pub enum Os { Linux, Macos, Windows }` (derives `Clone, Copy, PartialEq, Eq`)
  - `pub enum Arch { X86_64, Arm64 }` (derives `Clone, Copy, PartialEq, Eq`)
  - `pub struct Target { pub os: Os, pub arch: Arch }` (derives `Clone, Copy, PartialEq, Eq`)
  - `pub const ALL: [Target; 6]`
  - `impl Target { pub fn parse(s: &str) -> Result<Target, String>; pub fn llvm_triple(&self) -> &'static str; pub fn zig_triple(&self) -> &'static str; pub fn is_windows(&self) -> bool; pub fn label(&self) -> String; pub fn adjust_output(&self, out: &str) -> String; }`

- [ ] **Step 1: Write the failing tests** — create `src/targets.rs`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86_64,
    Arm64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

pub const ALL: [Target; 6] = [
    Target { os: Os::Linux, arch: Arch::X86_64 },
    Target { os: Os::Linux, arch: Arch::Arm64 },
    Target { os: Os::Macos, arch: Arch::X86_64 },
    Target { os: Os::Macos, arch: Arch::Arm64 },
    Target { os: Os::Windows, arch: Arch::X86_64 },
    Target { os: Os::Windows, arch: Arch::Arm64 },
];

fn invalid(s: &str) -> String {
    format!(
        "invalid --target {s:?}; expected <os>-<arch> or \"all\"\n  os: linux, macos, windows\n  arch: x86_64 (or x86), arm64"
    )
}

impl Target {
    pub fn parse(s: &str) -> Result<Target, String> {
        let (os_s, arch_s) = s.split_once('-').ok_or_else(|| invalid(s))?;
        let os = match os_s {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            _ => return Err(invalid(s)),
        };
        let arch = match arch_s {
            "x86_64" | "x86" => Arch::X86_64,
            "arm64" => Arch::Arm64,
            _ => return Err(invalid(s)),
        };
        Ok(Target { os, arch })
    }

    pub fn llvm_triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-gnu",
            (Os::Linux, Arch::Arm64) => "aarch64-unknown-linux-gnu",
            (Os::Macos, Arch::X86_64) => "x86_64-apple-darwin",
            (Os::Macos, Arch::Arm64) => "aarch64-apple-darwin",
            (Os::Windows, Arch::X86_64) => "x86_64-pc-windows-gnu",
            (Os::Windows, Arch::Arm64) => "aarch64-pc-windows-gnu",
        }
    }

    pub fn zig_triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-linux-gnu",
            (Os::Linux, Arch::Arm64) => "aarch64-linux-gnu",
            (Os::Macos, Arch::X86_64) => "x86_64-macos-none",
            (Os::Macos, Arch::Arm64) => "aarch64-macos-none",
            (Os::Windows, Arch::X86_64) => "x86_64-windows-gnu",
            (Os::Windows, Arch::Arm64) => "aarch64-windows-gnu",
        }
    }

    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// `os-arch` label used for `--target all` output suffixes, e.g. "linux-x86_64".
    pub fn label(&self) -> String {
        let os = match self.os {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        };
        let arch = match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        };
        format!("{os}-{arch}")
    }

    /// Appends `.exe` for windows targets if the given path doesn't already have it.
    pub fn adjust_output(&self, out: &str) -> String {
        if self.is_windows() && !out.ends_with(".exe") {
            format!("{out}.exe")
        } else {
            out.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_six_combos() {
        assert_eq!(Target::parse("linux-x86_64").unwrap(), Target { os: Os::Linux, arch: Arch::X86_64 });
        assert_eq!(Target::parse("linux-arm64").unwrap(), Target { os: Os::Linux, arch: Arch::Arm64 });
        assert_eq!(Target::parse("macos-x86_64").unwrap(), Target { os: Os::Macos, arch: Arch::X86_64 });
        assert_eq!(Target::parse("macos-arm64").unwrap(), Target { os: Os::Macos, arch: Arch::Arm64 });
        assert_eq!(Target::parse("windows-x86_64").unwrap(), Target { os: Os::Windows, arch: Arch::X86_64 });
        assert_eq!(Target::parse("windows-arm64").unwrap(), Target { os: Os::Windows, arch: Arch::Arm64 });
    }

    #[test]
    fn x86_is_alias_for_x86_64() {
        assert_eq!(Target::parse("linux-x86").unwrap(), Target { os: Os::Linux, arch: Arch::X86_64 });
    }

    #[test]
    fn rejects_unknown_os_or_arch() {
        assert!(Target::parse("solaris-x86_64").is_err());
        assert!(Target::parse("linux-sparc").is_err());
        assert!(Target::parse("garbage").is_err());
    }

    #[test]
    fn llvm_and_zig_triples_are_distinct_per_target() {
        for t in ALL {
            assert!(!t.llvm_triple().is_empty());
            assert!(!t.zig_triple().is_empty());
        }
    }

    #[test]
    fn windows_output_gets_exe_appended_once() {
        let win = Target { os: Os::Windows, arch: Arch::X86_64 };
        assert_eq!(win.adjust_output("hello"), "hello.exe");
        assert_eq!(win.adjust_output("hello.exe"), "hello.exe");
        let linux = Target { os: Os::Linux, arch: Arch::X86_64 };
        assert_eq!(linux.adjust_output("hello"), "hello");
    }

    #[test]
    fn label_matches_cli_syntax() {
        assert_eq!(Target { os: Os::Macos, arch: Arch::Arm64 }.label(), "macos-arm64");
    }
}
```

Add `mod targets;` to `src/main.rs` line 1 (alongside the existing `mod ast; mod codegen; ...` lines).

- [ ] **Step 2: Confirm the tests actually exercise the code (red check)**

In `src/targets.rs`, temporarily replace each `impl Target` method body with `unimplemented!()`, e.g. `pub fn parse(s: &str) -> Result<Target, String> { unimplemented!() }` for all five methods.

Run: `cargo test --lib targets::`
Expected: FAIL — panics with `not implemented` in `parses_all_six_combos` and the other tests

- [ ] **Step 3: Restore the real implementation**

Undo Step 2 — put back the method bodies exactly as written in Step 1 (`parse`, `llvm_triple`, `zig_triple`, `is_windows`, `label`, `adjust_output`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib targets::`
Expected: PASS, 6 tests

- [ ] **Step 5: Commit**

```bash
git add src/targets.rs src/main.rs
git commit -m "feat: add Target type for os/arch parsing and triple mapping"
```

---

### Task 3: Single explicit `--target` cross-build via `zig cc`

**Files:**
- Modify: `src/main.rs` (add `build_aot_cross`, `check_zig_available`, wire `--target` parsing into `main()`)
- Modify: `tests/e2e.rs` (new tests)

**Interfaces:**
- Consumes: `targets::Target` (Task 2), `codegen::Codegen::module()` (existing)
- Produces: `fn build_aot_cross(cg: &codegen::Codegen, out: &str, target: &targets::Target) -> Result<(), String>`, `fn check_zig_available()` — both reused by Task 4

- [ ] **Step 1: Write the failing tests** — append to `tests/e2e.rs`:

```rust
fn zig_available() -> bool {
    Command::new("zig").arg("version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn aot_cross_build_produces_binary_for_each_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_aot_cross_test");
    std::fs::create_dir_all(&dir).unwrap();
    for label in ["linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64", "windows-x86_64", "windows-arm64"] {
        let bin = dir.join(format!("functions_{label}"));
        let out = Command::new(env!("CARGO_BIN_EXE_verb"))
            .args(["build", "tests/fixtures/functions.verb", "-o", bin.to_str().unwrap(), "--target", label])
            .output()
            .unwrap();
        assert!(out.status.success(), "target {label} failed: {}", String::from_utf8_lossy(&out.stderr));
        let expected_path = if label.starts_with("windows") {
            dir.join(format!("functions_{label}.exe"))
        } else {
            bin
        };
        let meta = std::fs::metadata(&expected_path)
            .unwrap_or_else(|e| panic!("missing output for {label} at {expected_path:?}: {e}"));
        assert!(meta.len() > 0, "empty output for {label}");
    }
}

#[test]
fn aot_build_invalid_target_is_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", "/tmp/whatever", "--target", "solaris-x86_64"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --target"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e aot_cross_build aot_build_invalid_target -v`
Expected: FAIL — `--target` flag isn't recognized yet (usage error / unused arg), `aot_cross_build_produces_binary_for_each_target` fails or the invalid-target test doesn't find the "invalid --target" message

- [ ] **Step 3: Implement in `src/main.rs`**

Add near the top of `src/main.rs` (after the `use error::CompileError;` line):

```rust
fn check_zig_available() {
    let ok = std::process::Command::new("zig")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!(
            "zig not found on PATH. Cross-compiling requires zig (https://ziglang.org/download/) as the linker driver. Install it, or omit --target to build for this host with cc."
        );
        exit(1);
    }
}
```

Add `build_aot_cross` next to `build_aot_host`:

```rust
fn build_aot_cross(cg: &codegen::Codegen, out: &str, target: &targets::Target) -> Result<(), String> {
    use inkwell::targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetMachine, TargetTriple,
    };

    LlvmTarget::initialize_all(&InitializationConfig::default());
    let triple = TargetTriple::create(target.llvm_triple());
    let llvm_target = LlvmTarget::from_triple(&triple).map_err(|e| format!("target error: {e}"))?;
    let tm = llvm_target
        .create_target_machine(
            &triple, "generic", "",
            inkwell::OptimizationLevel::Default, RelocMode::PIC, CodeModel::Default,
        )
        .ok_or_else(|| "cannot create target machine".to_string())?;
    cg.module().set_triple(&triple);

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    let status = std::process::Command::new("zig")
        .args(["cc", "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()])
        .status()
        .map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}
```

Update `usage()` (`src/main.rs:29-34`) to:

```rust
fn usage() -> ! {
    eprintln!("usage: verb run <file.verb> [--emit-llvm]");
    eprintln!("       verb build <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]");
    eprintln!("       verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [--emit-llvm]  (alias for build)");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    exit(2)
}
```

In `main()`, after the existing `out` parsing block (`src/main.rs:42-44`), add target parsing:

```rust
let target_arg = args.iter().position(|a| a == "--target").map(|i| {
    args.get(i + 1).cloned().unwrap_or_else(|| usage())
});
```

Replace the `"build" | "compile" => { ... }` match arm (`src/main.rs:72-75`) with:

```rust
"build" | "compile" => {
    let out = out.unwrap_or_else(|| usage());
    match target_arg.as_deref() {
        None => build_aot_host(&cg, &out),
        Some("all") => build_aot_all(&cg, &out),
        Some(t) => {
            let target = targets::Target::parse(t).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                exit(2);
            });
            check_zig_available();
            if let Err(e) = build_aot_cross(&cg, &out, &target) {
                eprintln!("error: {e}");
                exit(1);
            }
        }
    }
}
```

(`build_aot_all` doesn't exist yet — Task 4 adds it. Add a temporary stub so this compiles: `fn build_aot_all(_cg: &codegen::Codegen, _out: &str) { eprintln!("--target all: not implemented yet"); exit(1); }`. Task 4 replaces this stub.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e aot_cross_build aot_build_invalid_target -v`
Expected: PASS (the cross-build test prints "skipping: zig not on PATH" and passes trivially if `zig` isn't installed on this machine — that's expected, not a failure)

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat: add --target cross-compilation via zig cc"
```

---

### Task 4: `--target all`

**Files:**
- Modify: `src/main.rs` (replace the `build_aot_all` stub from Task 3 with the real implementation)
- Modify: `tests/e2e.rs` (new test)

**Interfaces:**
- Consumes: `targets::ALL` (Task 2), `build_aot_cross` (Task 3), `check_zig_available` (Task 3)
- Produces: `fn build_aot_all(cg: &codegen::Codegen, out: &str)` (real implementation, replacing Task 3's stub)

- [ ] **Step 1: Write the failing test** — append to `tests/e2e.rs`:

```rust
#[test]
fn aot_build_target_all_produces_six_binaries_with_summary() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_aot_all_test");
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("functions_all");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/functions.verb", "-o", base.to_str().unwrap(), "--target", "all"])
        .output()
        .unwrap();
    assert!(out.status.success(), "build --target all failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("linux-x86_64: ok"), "summary missing linux-x86_64: {stdout}");
    assert!(stdout.contains("windows-arm64: ok"), "summary missing windows-arm64: {stdout}");

    for suffix in [
        "linux-x86_64", "linux-arm64", "macos-x86_64", "macos-arm64",
    ] {
        let path = dir.join(format!("functions_all-{suffix}"));
        assert!(path.exists(), "missing {path:?}");
    }
    for suffix in ["windows-x86_64", "windows-arm64"] {
        let path = dir.join(format!("functions_all-{suffix}.exe"));
        assert!(path.exists(), "missing {path:?}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e aot_build_target_all -v`
Expected: FAIL — stdout contains `--target all: not implemented yet` (the Task 3 stub) instead of a summary

- [ ] **Step 3: Implement `build_aot_all` in `src/main.rs`**, replacing the Task 3 stub:

```rust
fn build_aot_all(cg: &codegen::Codegen, out: &str) {
    check_zig_available();
    let mut failures = 0;
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for target in targets::ALL {
        let labeled_out = format!("{out}-{}", target.label());
        let res = build_aot_cross(cg, &labeled_out, &target);
        if res.is_err() {
            failures += 1;
        }
        results.push((target.label(), res));
    }
    println!("build --target all summary:");
    for (label, res) in &results {
        match res {
            Ok(()) => println!("  {label}: ok"),
            Err(e) => println!("  {label}: FAILED — {e}"),
        }
    }
    if failures > 0 {
        exit(1);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e aot_build_target_all -v`
Expected: PASS

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat: add --target all for building every platform combo"
```

---

### Task 5: README — document cross-compiling

**Files:**
- Modify: `README.md` (create if it doesn't exist yet — check first; if Task 9 from the original plan was never done, this file may not exist)

**Interfaces:**
- None (documentation only)

- [ ] **Step 1: Check whether `README.md` exists**

Run: `test -f README.md && echo exists || echo missing`

- [ ] **Step 2a: If missing**, create `README.md` with the full content below. **If it exists**, read it first and merge the "Usage" and new "Cross-compiling" sections into its existing structure instead of overwriting unrelated content.

```markdown
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
    make make_counter() begin
      assign n 0;
      make inc() begin n be n add 1; return n; end
      return inc;
    end
    assign counter make_counter();
    print(counter());   %% 1

## Known v1 limitations

- No GC — heap allocations are never freed
- No arrays/maps, no `break`/`continue`, no anonymous functions
- Captured variables must be declared before the `make` statement
  (no mutual recursion)
- Shadowing the builtin `print` has no effect — calls named `print`
  always hit the builtin
```

- [ ] **Step 3: Verify examples/hello.verb exists** (referenced by the README)

Run: `test -f examples/hello.verb && cat examples/hello.verb || echo missing`

If missing, create it:

```
print("hello from verb");
```

- [ ] **Step 4: Sanity check the documented commands actually work**

Run: `cargo run -- run examples/hello.verb && cargo run -- build examples/hello.verb -o /tmp/verb_hello_check && /tmp/verb_hello_check`
Expected: prints `hello from verb` twice (once via JIT, once via the AOT binary)

- [ ] **Step 5: Commit**

```bash
git add README.md examples/hello.verb
git commit -m "docs: document AOT build and cross-compiling in README"
```

---

## Final verification

- [ ] Run: `cargo test` — full suite passes
- [ ] Run: `cargo run -- build examples/hello.verb -o /tmp/verb_check` then `/tmp/verb_check` — host build works
- [ ] If `zig` is installed: `cargo run -- build examples/hello.verb -o /tmp/verb_check_all --target all` — prints a 6-line `ok` summary and produces 6 files in `/tmp`
