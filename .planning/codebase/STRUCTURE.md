# Codebase Structure

**Analysis Date:** 2026-07-21

## Directory Layout

```
compiler/
├── src/                           # Rust compiler source (lexer, parser, codegen, CLI)
│   ├── main.rs                    # CLI entry: run (JIT) and build (AOT)
│   ├── lib.rs                     # Library exports (modules)
│   ├── lexer.rs                   # Tokenization with comment preservation
│   ├── parser.rs                  # AST construction + error recovery (LSP)
│   ├── ast.rs                     # AST types: Program, Stmt, Expr, BinOp, UnOp
│   ├── codegen.rs                 # LLVM IR generation (~113KB, main compilation logic)
│   ├── targets.rs                 # Cross-compilation target definitions (6 OS×arch combos)
│   ├── value.rs                   # Runtime value model constants (TAG_* definitions)
│   ├── error.rs                   # CompileError struct + builder methods
│   ├── formatter.rs               # Code formatting (preserves comments)
│   └── bin/
│       └── verb-lsp.rs            # LSP server implementation (17KB)
│
├── runtime/                       # C++ runtime (maps, I/O, GC helpers)
│   ├── verb.h                     # VerbValue ABI, refcount header struct
│   ├── verb_map.cpp               # Hash map implementation
│   ├── verb_io.cpp                # File I/O and socket operations
│   └── [other runtime units]
│
├── build.rs                       # Compile C++ runtime units via cc crate
│
├── tests/                         # Test suite
│   └── fixtures/                  # .verb test programs
│
├── examples/                      # Sample Verb programs (.verb files)
│
├── docs/                          # Project documentation
│   └── superpowers/specs/         # Design documents (2026-07-* dated specs)
│
├── editors/                       # Editor integrations
│   ├── vscode-verb/               # VSCode extension
│   ├── nvim/                      # Neovim plugin
│   └── tree-sitter-verb/          # Tree-sitter grammar
│
├── .planning/                     # Planning and analysis documents
│   └── codebase/                  # Codebase maps (ARCHITECTURE.md, STRUCTURE.md, etc.)
│
├── .claude/                       # Claude Code configuration
│   ├── CLAUDE.md                  # User instructions
│   └── worktrees/                 # Git worktree state
│
├── Cargo.toml                     # Rust package manifest
├── Cargo.lock                     # Dependency lock file
├── .cargo/config.toml             # Cargo config (LLVM path wiring)
└── README.md                      # Main project README
```

## Directory Purposes

**`src/`:**
- Purpose: Main compiler implementation
- Contains: Lexer, parser, codegen, CLI entry point, LSP server
- Key files: `main.rs` (CLI), `codegen.rs` (core IR generation)

**`src/bin/`:**
- Purpose: Binary entry points (not lib.rs)
- Contains: LSP server implementation
- Key files: `verb-lsp.rs`

**`runtime/`:**
- Purpose: C++ runtime code compiled and linked into all binaries
- Contains: Standard library implementations (maps, I/O), GC helpers, memory management
- Key files: `verb.h` (value/header struct definitions), `verb_map.cpp`, `verb_io.cpp`
- Generated/Compiled: Yes (via `build.rs` → linkage via cc crate)
- Committed: Yes (source files)

**`tests/`:**
- Purpose: Test suite
- Contains: Test programs and fixtures (`.verb` files)
- Key files: Integration tests, `fixtures/` (example programs for testing)

**`examples/`:**
- Purpose: Sample Verb programs demonstrating language features
- Contains: `.verb` files (e.g., `hello.verb`, `uses_mathlib.verb`)

**`docs/`:**
- Purpose: Project documentation and design specs
- Contains: Design documents, architecture notes
- Key files: `superpowers/specs/` (dated design docs for features: C++ imports, I/O, maps)

**`editors/`:**
- Purpose: Editor plugin/extension implementations
- Contains:
  - `vscode-verb/`: VSCode extension
  - `nvim/`: Neovim plugin (Lua)
  - `tree-sitter-verb/`: Tree-sitter grammar for syntax highlighting
- Committed: Yes

## Key File Locations

**Entry Points:**
- `src/main.rs`: CLI binary entry (verb run / verb build)
- `src/bin/verb-lsp.rs`: LSP server entry

**Configuration:**
- `Cargo.toml`: Package manifest, dependencies (inkwell, serde_json)
- `build.rs`: Build script (compiles C++ runtime, links against LLVM)
- `.cargo/config.toml`: LLVM path configuration

**Core Logic:**
- `src/codegen.rs`: LLVM IR generation (113KB, largest file)
- `src/parser.rs`: AST construction + error recovery
- `src/lexer.rs`: Tokenization
- `src/ast.rs`: AST data structures

**Testing:**
- `tests/`: Test files
- `tests/fixtures/`: Example `.verb` programs used as test data

## Naming Conventions

**Files:**
- `src/*.rs`: Modules (lexer, parser, codegen, etc.) use descriptive lowercase names
- `src/bin/*.rs`: Binary entry points (verb-lsp.rs)
- `runtime/*.cpp/.h`: C++ runtime units
- `examples/*.verb`: Example programs
- `tests/*.verb` or `tests/fixtures/*.verb`: Test programs

**Directories:**
- `src/bin/`: Binary crates (non-library entry points)
- `tests/fixtures/`: Data files for tests
- `docs/superpowers/specs/`: Design specifications
- `.planning/codebase/`: Generated analysis documents
- `.claude/worktrees/`: Git worktree state

**Rust identifiers (src/*.rs):**
- Functions: snake_case (e.g., `compile_program`, `parse_cli`)
- Types: PascalCase (e.g., `Codegen`, `ParsedArgs`, `CompileError`)
- Constants: UPPER_SNAKE_CASE (e.g., `TAG_NIL`, `TAG_ARRAY`, `GC_STATIC_SENTINEL`)
- Modules: snake_case (e.g., `lexer`, `codegen`, `error`)

## Where to Add New Code

**New Feature (e.g., new operator, builtin function):**
- Primary code: 
  - Lexer: Add `TokenKind` variant in `src/lexer.rs` (scan + keyword handling)
  - Parser: Add parsing logic in `src/parser.rs` (expression or statement handling)
  - Codegen: Add IR generation in `src/codegen.rs` (statement/expression compilation)
- Tests: Add `.verb` test files in `tests/fixtures/` or inline in `tests/`

**New Builtin Function (e.g., get, set, push, pop for arrays):**
- Implementation: `src/codegen.rs` (build_*_fn methods, e.g., `build_array_get_fn()`)
- Call site: `src/codegen.rs` expr compilation (emit call to the builtin)
- Runtime support: If C++ is needed, add to `runtime/` and declare as extern in codegen

**New Standard Library Module (e.g., std foo):**
- Spec: Create design doc in `docs/superpowers/specs/`
- Frontend: Update parser to recognize `import std foo` in `src/parser.rs`
- Codegen: Add C++ unit to `runtime/` (e.g., `runtime/verb_foo.cpp`)
- Build: List `.cpp` file in `build.rs` → compile step
- Linker: Link into both JIT (via `register_jit_runtime_symbols`) and AOT binaries

**New Target/Platform (e.g., RISC-V):**
- Add to `src/targets.rs`:
  - `Arch` enum (e.g., `Riscv64`)
  - `ALL` array
  - `parse()` method case
  - `llvm_triple()` and `zig_triple()` methods
- Update CLI usage in `src/main.rs`
- Cross-compile support requires Zig (zig cc) for the new target; host builds use cc

**Utilities & Helpers:**
- Shared parsing helpers: `src/parser.rs` (private methods on Parser struct)
- Shared codegen helpers: `src/codegen.rs` (private methods on Codegen struct, e.g., `make_val()`, `tag_of()`)
- Shared value model: `src/value.rs` (TAG_* constants)
- Error utilities: `src/error.rs` (CompileError builder methods)

## Special Directories

**`.planning/codebase/`:**
- Purpose: Generated codebase analysis documents
- Contents: ARCHITECTURE.md, STRUCTURE.md, CONVENTIONS.md, TESTING.md, CONCERNS.md
- Generated: Yes (by gsd-map-codebase)
- Committed: Yes (output of analysis tools)

**`.claude/worktrees/`:**
- Purpose: Git worktree checkouts for parallel work
- Generated: Yes (by git worktree)
- Committed: No (working tree state, transient)

**`target/`:**
- Purpose: Build artifacts
- Generated: Yes (by cargo build)
- Committed: No (.gitignore)

**`docs/superpowers/specs/`:**
- Purpose: Design specifications for features
- Format: Markdown, dated (YYYY-MM-DD-feature-name.md)
- Committed: Yes (reference documentation)

## Project Type & Build System

**Language:** Rust (2021 edition)

**Build System:** Cargo + build.rs

**Runtime Dependencies:**
- `inkwell 0.9` (LLVM 20.1 binding)
- `serde_json 1.0` (JSON serialization, likely for LSP)

**Build-Time Dependencies:**
- `cc 1.0` (C++ compilation)

**External Tools Required:**
- Rust (2021 edition)
- LLVM 20.1 (path configured in `.cargo/config.toml`)
- C compiler (cc) for host AOT builds
- Zig (optional, for cross-compilation; required only for `--target` cross builds)

---

*Structure analysis: 2026-07-21*
