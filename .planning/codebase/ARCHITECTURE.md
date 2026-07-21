<!-- refreshed: 2026-07-21 -->
# Architecture

**Analysis Date:** 2026-07-21

## System Overview

```text
┌──────────────────────────────────────────────────────────────────────┐
│                    CLI & Entry Points                                 │
│  main.rs (run/build/compile) | verb-lsp.rs (language server)        │
└────────────────┬───────────────────────────────────────┬─────────────┘
                 │                                       │
                 ▼                                       ▼
┌──────────────────────────────────────┐   ┌───────────────────────────┐
│    Compilation Pipeline (CLI)        │   │   LSP Pipeline (LSP)      │
│                                      │   │                           │
│  1. Lexer → Tokens                   │   │  Error Recovery Parsing   │
│     `src/lexer.rs`                   │   │  `src/parser.rs:         │
│                                      │   │   parse_recovering()`     │
│  2. Parser → AST                     │   │                           │
│     `src/parser.rs`                  │   │  Incremental reparse      │
│                                      │   │  on file changes          │
│  3. Codegen → LLVM IR                │   │                           │
│     `src/codegen.rs` (~113KB)        │   │                           │
│                                      │   │                           │
│  4. Execution or Linking             │   │                           │
│     - JIT: MCJIT → main()            │   │                           │
│     - AOT: link + native binary      │   │                           │
└──────────────┬───────────────────────┘   └───────────────────────────┘
               │
               ▼
┌──────────────────────────────────────────────────────────────────────┐
│                   Code Generation Layer                               │
│  Codegen State: module, builder, scopes, globals, GC tracking       │
│  `src/codegen.rs`                                                    │
│                                                                      │
│  ├─ Type & Value Helpers (value_ty, closure_ty, array_ty)          │
│  ├─ Scope Management (scopes stack, globals HashMap)                │
│  ├─ GC Primitives (verb_retain_value, verb_release_value)          │
│  ├─ Builtin Functions (print, arithmetic, comparison, array ops)   │
│  └─ Statement & Expression Compilation                             │
└──────────────┬───────────────────────────────────────┬──────────────┘
               │                                       │
               ▼                                       ▼
┌──────────────────────────────────────┐   ┌──────────────────────────┐
│      Inkwell (LLVM Binding)          │   │  Runtime C++ Code        │
│  - Context, Module, Builder          │   │                          │
│  - Function value types              │   │  `runtime/verb.h`        │
│  - LLVM IR emission                  │   │  `runtime/verb_*.cpp`:   │
│                                      │   │  - verb_map              │
│                                      │   │  - verb_io               │
│                                      │   │  - Memory/GC helpers     │
└──────────────┬───────────────────────┘   └──────────────┬───────────┘
               │                                         │
               ▼                                         ▼
┌──────────────────────────────────────┐   ┌──────────────────────────┐
│        LLVM IR Generation             │   │  Compiler Toolchain      │
│  - LLVM 20.1 backend                 │   │                          │
│  - Target-specific IR                │   │  - cc (host AOT)         │
│  - Optimization (AOT only)           │   │  - zig cc (cross AOT)    │
└──────────────┬───────────────────────┘   └──────────────┬───────────┘
               │                                         │
               ├─ JIT Path (run)                         │
               │  ├─ MCJIT execution engine              │
               │  ├─ Runtime symbol resolution           │
               │  └─ main() → exit code                  │
               │                                         │
               └─ AOT Path (build/compile)               │
                  ├─ Single target (host or specified)   │
                  │  └─ cc: link host or cross binary    │
                  │                                       │
                  └─ All targets (--target all)          │
                     ├─ linux-x86_64 (zig cc)           │
                     ├─ linux-arm64 (zig cc)            │
                     ├─ macos-x86_64 (zig cc)           │
                     ├─ macos-arm64 (zig cc)            │
                     ├─ windows-x86_64 (zig cc)         │
                     └─ windows-arm64 (zig cc)          │
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Lexer | Tokenize source into Token stream | `src/lexer.rs` |
| Parser | Parse tokens into AST; error recovery for LSP | `src/parser.rs` |
| AST | Data structures (Program, Stmt, Expr, BinOp, UnOp) | `src/ast.rs` |
| Codegen | LLVM IR generation from AST; manages scopes, GC | `src/codegen.rs` |
| Targets | Cross-compilation target definitions & LLVM/Zig triples | `src/targets.rs` |
| Value | Runtime value model constants (tags 0–7) | `src/value.rs` |
| Error | CompileError struct with location & hints | `src/error.rs` |
| Formatter | Code formatting (preserves comments) | `src/formatter.rs` |
| Main | CLI entry: run (JIT), build/compile (AOT) | `src/main.rs` |
| LSP | Language server implementation | `src/bin/verb-lsp.rs` |

## Pattern Overview

**Overall:** Multi-stage compiler with separate JIT and AOT paths.

**Key Characteristics:**
- **Strict pipeline separation**: Each stage (lex, parse, codegen) is independent and composable
- **Error recovery**: Parser has both strict (`parse()`) and recovering (`parse_recovering()`) modes
- **Reference counting GC**: `verb_retain_value`/`verb_release_value` emitted into every LLVM module
- **Cross-compilation support**: 6 target combinations (3 OS × 2 arch) via `targets.rs`
- **Dual runtime paths**: JIT (MCJIT via Inkwell) for fast iteration; AOT (cc/zig cc) for deployment
- **Typed values as tagged unions**: Every value `{ i8 tag, i64 payload }` at runtime

## Layers

**Frontend (Lexical & Syntactic Analysis):**
- Purpose: Convert source text to structured AST
- Location: `src/lexer.rs`, `src/parser.rs`
- Contains: Tokenization, parsing, comment preservation for formatters
- Depends on: None (self-contained)
- Used by: Codegen, Formatter, LSP

**IR Generation (Code Generation):**
- Purpose: Translate AST into LLVM IR, manage scopes and GC
- Location: `src/codegen.rs`
- Contains: LLVM type/value construction, builtin function emission, statement/expression compilation
- Depends on: AST, Inkwell (LLVM binding), runtime value model (`src/value.rs`)
- Used by: Main CLI (run/build paths), LSP diagnostics

**Target Configuration:**
- Purpose: Define available cross-compilation targets, LLVM/Zig triples
- Location: `src/targets.rs`
- Contains: Target enum, parsing, triple generation, output path adjustment
- Depends on: None
- Used by: Main CLI for AOT compilation

**Runtime Support:**
- Purpose: Provide C++ implementations of built-in data structures and GC helpers
- Location: `runtime/` (C++ source files compiled by `build.rs`)
- Contains: Maps, I/O, memory management, refcount header manipulation
- Depends on: Standard C library, system APIs (POSIX/Windows)
- Used by: Generated LLVM IR (all compiled programs)

**Execution Engines:**
- Purpose: Execute compiled IR (JIT for immediate feedback, AOT for distribution)
- Components:
  - JIT: MCJIT (Inkwell), runtime symbol mapping, no optimizations
  - AOT: System linker (cc for host, zig cc for cross), optional LTO
- Used by: Main CLI

## Data Flow

### Primary JIT Path (run)

1. CLI argument parsing → validate single input file (`src/main.rs:177–200`)
2. Lexer tokenizes source → `Vec<Token>` (`src/lexer.rs:62`)
3. Parser builds AST → `Program { imports, std_imports, body }` (`src/parser.rs:5`)
4. Codegen emits LLVM module from AST (`src/codegen.rs:209–212`)
5. MCJIT creates execution engine (`src/main.rs:229–236`)
6. Runtime symbols registered (`src/main.rs:236`, `register_jit_runtime_symbols()`)
7. Unsafe call to `main()` → exit code (`src/main.rs:237–241`)

### Primary AOT Path (build/compile)

1. CLI parses arguments (files, `-o`, `--target`, `-L`) (`src/main.rs:100–147`)
2. Each input file: lex + parse (same as JIT) (`src/main.rs:197–208`)
3. All statements merged into single program context
4. Codegen emits single LLVM module for entire program (`src/main.rs:212`)
5. **Single target**: Link with cc (host) or zig cc (cross) → native binary
6. **All targets**: Loop over 6 targets, link each with zig cc → 6 binaries
7. Optional `--emit-llvm` prints IR to stdout before linking

### Error Handling

**Compilation errors** (syntax, type-related):
- Captured as `CompileError { msg, line, col, hint, file }`
- Strict mode (`parse()`): stops on first error
- Recovery mode (`parse_recovering()`): continues parsing, collects all errors (LSP use)
- Error display includes line/column, source excerpt, and optional hint (`src/main.rs:149–166`)

**Runtime errors**: Not caught—invalid operations (e.g., out-of-bounds array access) abort at runtime.

**State Management:**
- **Scopes**: Stack of `HashMap<String, PointerValue>` for variable lookup; pushed/popped for function entry/exit (`src/codegen.rs:22, 79`)
- **Globals**: Top-level variable storage (`src/codegen.rs:23`)
- **GC tracking**: `verb_gc_live` global counter; incremented on allocation, decremented on release
- **Imports**: Collected across all input files, deduplicated; routed to linker as `-l` flags

## Key Abstractions

**VerbValue (Tagged Union):**
- Purpose: Represent every runtime value uniformly
- Structure: `{ i8 tag, i64 payload }` in LLVM
- Tags: NIL(0), BOOL(1), INT(2), FLOAT(3), STR(4), CLOSURE(5), MAP(6), ARRAY(7)
- Implementation: `src/value.rs` (constants), `src/codegen.rs` (construction via `make_val()`)

**Program (AST Root):**
- Purpose: Represent entire compiled unit
- Structure: `Program { imports: Vec<String>, std_imports: Vec<String>, body: Vec<Stmt> }`
- Used by: Codegen to walk statements in order

**Codegen State Machine:**
- Purpose: Accumulate LLVM values as AST is walked
- Key fields: `scopes` (variable lookup), `globals`, `module` (target IR), `builder` (IR generation)
- Lifecycle: Created in `main()`, methods called for each statement/expression, module emitted

**Target Triplet:**
- Purpose: Encapsulate platform-specific compiler/linker configuration
- Fields: `os` (Linux/Macos/Windows), `arch` (X86_64/Arm64)
- Provides: LLVM triple (e.g., `x86_64-unknown-linux-gnu`), Zig triple, Windows `.exe` suffix logic

## Entry Points

**CLI Binary (run/build/compile):**
- Location: `src/main.rs:177`
- Triggers: `verb run <files...> [--emit-llvm]` or `verb build <files...> -o <out> [--target ...] [--emit-llvm]`
- Responsibilities:
  - Parse CLI arguments
  - Read and lex each input file
  - Parse and merge all ASTs
  - Codegen entire program
  - For `run`: MCJIT execute
  - For `build`: link and emit native binary (host or cross)

**LSP Binary:**
- Location: `src/bin/verb-lsp.rs`
- Triggers: Spawned by editor (VSCode/Neovim)
- Responsibilities: LSP protocol handling, incremental parsing (parse_recovering), diagnostics

## Architectural Constraints

- **Single-threaded execution**: No parallelism in JIT or codegen; Inkwell and MCJIT are not thread-safe for concurrent compilation
- **Global state (GC)**: `verb_gc_live` is a module-level global; JIT execution does not support concurrent programs
- **No intra-module optimization**: JIT codegen runs with `OptimizationLevel::None`; only AOT applies LTO
- **Layered imports**: `import` statements must appear before any executable code (checked in `parse()`)
- **No closures over enclosing scope**: Nested functions cannot reference outer variables (v1 limitation in spec)
- **Heap allocation never freed in v1**: GC is reference-counted but cycles are not collected; circular references leak memory
- **Array/map operations on non-target types return error codes** (not exceptions); e.g., `get()` on non-array returns `nil`

## Anti-Patterns

### Unbounded Codegen Output Size

**What happens:** A program with many functions or large expressions generates LLVM IR that grows without bound. No size limits, no lazy codegen.

**Why it's wrong:** Memory usage scales linearly with source size, even for programs that never fully execute (JIT) or are never linked (partial compilation).

**Do this instead:** For future optimization, consider streaming code generation or lazy module construction. For now, accept that v1 compiles eagerly and warn on large programs.

### Implicit Scope Leaks via Function Depth Tracking

**What happens:** `fn_depth` counter in parser tracks nesting but can become stale if error recovery leaves an unfinished block. Recovery resets `fn_depth = 0` to recover, but this may mask mismatched braces in some inputs (`src/parser.rs:50–54`).

**Why it's wrong:** A corrupted depth counter means subsequent errors may not be detected or may be misattributed (though the parser does force one token forward to prevent infinite loops).

**Do this instead:** Use a stack-based brace tracker instead of a counter; the parser already has a recovery mechanism that makes this low-priority.

### GC Stub Functions in Host Binary

**What happens:** `verb run` rejects imports (which require maps), but the host binary always links `runtime/verb_map.cpp`, which references `verb_alloc`, `verb_retain_value`, `verb_release_value`. These are stubbed in `src/main.rs:44–58` to abort loudly if called (`src/main.rs:45–58`).

**Why it's wrong:** The stubs are dead code under the current design (JIT rejects imports, so no map can exist); if a future optimization tries to eliminate the invariant check, these stubs hide the invariant.

**Do this instead:** Add a build-time toggle to omit map support entirely from JIT builds, or document the invariant in a test that verifies `verb run` rejects all imports, not just programmatically.

## Error Handling

**Strategy:** Parse errors are collected and reported to the user with source context; compilation errors in codegen abort immediately; runtime errors are not recoverable (programs that divide by zero, access out-of-bounds arrays, etc. abort at runtime).

**Patterns:**
- **CompileError**: Struct with message, line, column, optional hint, optional file. Constructed via builder methods (`.with_hint()`, `.with_file()`).
- **Result<T, CompileError>**: Used throughout (lex, parse, codegen); `main()` unwraps errors and calls `die()` to format and exit.
- **Parser recovery**: `parse_recovering()` used by LSP to continue after errors; synchronizes to next statement boundary and collects errors.

## Cross-Cutting Concerns

**Logging:** Console stderr for errors and progress (e.g., "zig not found on PATH"); no structured logging framework.

**Validation:** 
- Imports validated at parse time (must appear first)
- Types validated implicitly during execution (no type-checking pass)
- Targets validated at CLI parse time (split on `-`, check against enum)

**Multi-file handling:** All statements from all input files are merged into a single program; imports are deduplicated across files; each statement tracks its source file for error reporting.

---

*Architecture analysis: 2026-07-21*
