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

use std::path::PathBuf;

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
}
