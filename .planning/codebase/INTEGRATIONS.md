# External Integrations

**Analysis Date:** 2026-07-21

## APIs & External Services

**Compiler Infrastructure:**
- LLVM 20.1 - JIT and AOT code generation
  - Accessed via inkwell bindings (`src/codegen.rs`)
  - Installation: Homebrew (`brew install llvm@20`)
  - Environment: `LLVM_SYS_201_PREFIX=/opt/homebrew/opt/llvm@20`

## Data Storage

**Databases:**
- None - This is a compiler, not an application with persistent state

**File Storage:**
- Local filesystem only - Programs compiled with `import std io;` can read/write files via:
  - `file_read(path)` - Read entire file as string
  - `file_write(path, contents)` - Write string to file
  - `file_append(path, contents)` - Append string to file
  - Implementation: `runtime/verb_std_io.cpp`

**Caching:**
- None - No caching layer

## Authentication & Identity

**Auth Provider:**
- None - This is a compiler with no user authentication

## Monitoring & Observability

**Error Tracking:**
- None - Errors are printed to stderr and captured by CLI caller

**Logs:**
- stderr - Compilation errors, diagnostics printed to stderr
- stdout - Program output and LLVM IR dumps (via `--emit-llvm` flag)

## CI/CD & Deployment

**Hosting:**
- None - Binary is self-contained and runs locally

**CI Pipeline:**
- None detected - Tests run via `cargo test` locally

## External Library Integration (C/C++ FFI)

**User-Defined Imports:**
- `import mod <name>;` - Links against external native libraries
  - Maps to linker `-l<name>` flag
  - Requires `extern "C" VerbValue` functions using `VerbValue` struct from `runtime/verb.h`
  - CLI: `verb build program.verb -o out -L/path/to/libs`
  - Not supported in JIT mode (`verb run`)
  - Implementation: `src/main.rs` lines for `build_aot_host()`, cross-platform linking

**Standard Library Modules (Built-in C++ Imports):**

**I/O Module (`import std io;`):**
- Functions compiled into generated binaries automatically:
  - `read_line()` - Read from stdin
  - `file_read(path)` - Read whole file
  - `file_write(path, contents)` - Write to file
  - `file_append(path, contents)` - Append to file
  - `tcp_connect(host, port)` - Connect to TCP socket
  - `tcp_listen(port)` - Listen on TCP port
  - `tcp_accept(fd)` - Accept connection
  - `send_line(fd, s)` - Send data on socket
  - `recv_line(fd)` - Receive data from socket
  - `close_conn(fd)` - Close connection
- Implementation: `runtime/verb_std_io.cpp`
- Availability:
  - Works with `verb build` (all targets except Windows cross-compile)
  - Not supported with `verb run` (JIT)
  - Windows cross-compile targets (windows-x86_64, windows-arm64) not supported in v1

**Map Module (`import std map;`):**
- Hash-map (dictionary) data structure functions:
  - `map_new()` - Create new map
  - `map_set(m, k, v)` - Set key-value pair
  - `map_get(m, k)` - Get value by key
  - `map_has(m, k)` - Check key existence
  - `map_remove(m, k)` - Remove key
  - `map_len(m)` - Get map size
- Implementation: `runtime/verb_map.cpp`, `runtime/verb.h`
- Key types: nil, bool, int, float, string (numeric equality cross-type: 1 == 1.0)
- Compiled into generated binaries automatically
- Not supported with `verb run` (JIT)

## Build-Time Integrations

**C/C++ Compiler Integration:**
- Host builds: Use system `cc` compiler for linking
- Cross-platform builds: Use `zig cc` toolchain (requires zig installation)
- Supported cross-targets: linux-x86_64, linux-arm64, macos-x86_64, macos-arm64, windows-x86_64, windows-arm64
- Runtime symbol linking:
  - All generated LLVM modules reference `verb_alloc`, `verb_retain_value`, `verb_release_value`
  - These symbols compiled into binary at build time via `build.rs`
  - Runtime symbols defined in `src/main.rs`

## Webhooks & Callbacks

**Incoming:**
- None

**Outgoing:**
- None

## Language Server Protocol (LSP)

**LSP Server Binary:**
- `verb-lsp` - Minimal LSP server for editor integration
  - Location: `src/bin/verb-lsp.rs`
- Communication: LSP over stdio (Content-Length framed JSON-RPC)
- Features:
  - Initialization and shutdown
  - Document diagnostics (reuse compiler pipeline)
  - No async runtime (processes one request at a time)
  - Single-error-stop behavior (matches compiler behavior)
  - Diagnostics include parse/compile errors only

## Environment Configuration

**Required env vars:**
- `LLVM_SYS_201_PREFIX` - Path to LLVM 20.1 installation (set in `.cargo/config.toml`)

**Optional:**
- `ZIG` - Path to zig binary (if not on PATH) for cross-platform builds
- `CC` / `CXX` - Compiler overrides (standard Rust/cc behavior)

**Secrets location:**
- N/A - No secrets required

---

*Integration audit: 2026-07-21*
