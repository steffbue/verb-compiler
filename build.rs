//! Compiles the C++ runtime translation units that Verb's generated code
//! references *unconditionally* into the `verb` binary itself, so the JIT
//! (`verb run`) can resolve those symbols regardless of whether a given
//! program imports the corresponding `std` module.
//!
//! Why this exists: `src/codegen.rs` emits `verb_release_value` into every
//! module, and its map branch calls `verb_map_destroy_contents` (defined in
//! `runtime/verb_map.cpp`). That reference is always live — there is no
//! dead-code stripping in this pipeline — so MCJIT must be able to resolve
//! it even for a program that never touches maps. We compile the runtime
//! unit into this binary and register the symbol with the execution engine
//! via `add_global_mapping` in `src/main.rs`. Any future
//! always-referenced-but-conditionally-defined runtime symbol is added the
//! same way: list its `.cpp` here, and register it in `main.rs`.
//!
//! The AOT `build`/`compile` path does NOT use this object; it compiles and
//! links `runtime/verb_map.cpp` freshly per target (see `src/main.rs`).
//!
//! `verb_std_io.cpp` (unlike `verb_map.cpp`) has no Rust-side reference at
//! all — its symbols exist solely for the JIT to resolve dynamically via
//! `dlsym(RTLD_DEFAULT, ...)` at run time (see the later ffi-v2 tasks). Two
//! things stand between "compiled into the archive" and "dlsym finds it in
//! the final binary", and a dynamic-export linker flag alone addresses
//! neither:
//!
//! 1. A plain static-archive link only pulls in object files that resolve
//!    an undefined symbol, so `verb_std_io.o` is never even a link
//!    candidate — nothing references it.
//! 2. Once linked, `ld`'s `-dead_strip` pass (always on for Apple targets;
//!    part of every rustc invocation, not something this crate controls)
//!    removes any function unreachable from an entry point or another
//!    linker root, which every runtime function is, absent (1).
//!
//! `-Wl,-u,SYMBOL` (see `runtime_c_abi_symbols` below) solves both at once:
//! it tells the linker to treat `SYMBOL` as an unresolved reference that
//! must be satisfied (forcing archive extraction) *and* as a GC root
//! (surviving `-dead_strip`). We discover the exact symbol list by running
//! `nm` on the compiled archive rather than hand-maintaining one, so this
//! keeps working as `runtime/verb_std_io.cpp` (or a future
//! `runtime/verb_std_*.cpp`) gains functions.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("runtime");
    let map_cpp = runtime.join("verb_map.cpp");
    let std_io_cpp = runtime.join("verb_std_io.cpp");

    println!("cargo:rerun-if-changed={}", map_cpp.display());
    println!("cargo:rerun-if-changed={}", std_io_cpp.display());
    println!("cargo:rerun-if-changed={}", runtime.join("verb.h").display());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&runtime)
        .file(&map_cpp)
        .file(&std_io_cpp)
        .compile("verb_runtime");

    // cc::Build::compile's own `cargo:rustc-link-lib=static=verb_runtime` /
    // `cargo:rustc-link-lib=c++` output (see the `verb-*/output` build log)
    // is scoped by Cargo to *this package's library target* (`src/lib.rs`)
    // only — per Cargo's build-script docs, unqualified rustc-link-lib is
    // passed solely to the lib target, and other targets are expected to
    // reach native symbols "through the library target's public API". A
    // target that never uses the `verb` lib's Rust API at all (e.g.
    // tests/e2e.rs, which only shells out to the compiled `verb` binary)
    // gets neither the archive nor libc++ on its link line at all, so a
    // direct reference to a runtime symbol from such a target fails with
    // "symbol(s) not found" — not because dynamic export is broken, but
    // because the archive was never even a link candidate. Re-emit both
    // libs unscoped (rustc-link-arg's default scope: bins, examples, tests,
    // benches) so any such target can pull in just the translation units it
    // references, via ordinary reference-driven archive extraction.
    println!("cargo:rustc-link-arg=-lverb_runtime");
    println!("cargo:rustc-link-arg=-lc++");

    // Force every runtime translation unit's public, extern "C" symbols
    // into the `verb` binary specifically (scoped via rustc-link-arg-bin,
    // *not* the package-wide rustc-link-arg) and keep them past
    // `-dead_strip`. Scoping to the `verb` bin keeps this from also
    // touching `verb-lsp` (src/bin/verb-lsp.rs) or the other integration
    // test binaries — none of which define the
    // verb_alloc/verb_retain_value/verb_release_value host stubs that
    // verb_map.o/verb_std_io.o need resolved once forced in (src/main.rs
    // defines those solely for the `verb` bin). Cargo has no per-test-name
    // equivalent of rustc-link-arg-bin, so the analogous test
    // (tests/e2e.rs, `in_binary_std_symbols_are_dynamically_resolvable`)
    // instead pulls in just `verb_std_io.o` itself, the same way
    // `src/main.rs` already does for verb_map.o: a direct Rust-side
    // reference to `file_read`, plus its own local `verb_alloc` stub.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let archive = out_dir.join("libverb_runtime.a");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" || target_os == "linux" {
        for symbol in runtime_c_abi_symbols(&archive, &target_os) {
            println!("cargo:rustc-link-arg-bin=verb=-Wl,-u,{symbol}");
        }
    }
    // Other host OSes (e.g. Windows) aren't part of the dynamic-export
    // mechanism this build supports; leave archive extraction to the
    // linker's normal reference-driven behavior.
}

/// Runs `nm` on the compiled runtime archive and returns every globally
/// defined, non-name-mangled symbol — i.e. every `extern "C"` function in
/// `verb_map.cpp`/`verb_std_io.cpp` (`map_new`, `file_read`, and so on),
/// but none of the internal libc++ template instantiations pulled in
/// alongside them (nothing dlsyms those, and forcing them as linker roots
/// serves no purpose). `nm`'s `T` type code means "globally defined in the
/// text section", which is exactly the extern "C" API surface; mangled
/// C++ names are filtered out by their platform-specific prefix.
fn runtime_c_abi_symbols(archive: &Path, target_os: &str) -> Vec<String> {
    let output = Command::new("nm")
        .arg("-g")
        .arg(archive)
        .output()
        .expect("failed to run `nm` on the compiled runtime archive");
    assert!(
        output.status.success(),
        "`nm -g {}` failed: {}",
        archive.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);

    let is_plain_c_symbol = |name: &str| -> bool {
        if target_os == "macos" {
            // Mach-O gives every C symbol exactly one leading underscore;
            // Itanium-mangled C++ symbols (`_Z...`) appear here as `__Z...`
            // (two underscores) — exclude those, and clang-generated
            // helpers like `___clang_call_terminate` (three underscores).
            name.strip_prefix('_')
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic()))
        } else {
            // ELF/Itanium: mangled C++ symbols start with `_Z`; plain C
            // symbols carry no leading-underscore convention at all.
            !name.starts_with("_Z") && name.starts_with(|c: char| c.is_ascii_alphabetic())
        }
    };

    let mut symbols: Vec<String> = text
        .lines()
        .filter_map(|line| {
            // Typical line: `0000000000000230 T _file_read`. Archive `nm`
            // output also has header lines like `path.a(member.o):` with no
            // address/type/name triple — `?` on the missing fields skips
            // those safely.
            let mut fields = line.split_whitespace();
            let _address = fields.next()?;
            let kind = fields.next()?;
            let name = fields.next()?;
            (kind == "T" && is_plain_c_symbol(name)).then(|| name.to_string())
        })
        .collect();
    symbols.sort();
    symbols.dedup();
    symbols
}
