# Coding Conventions

**Analysis Date:** 2026-07-21

## Naming Patterns

**Files:**
- Rust modules: `snake_case.rs` (e.g., `lexer.rs`, `parser.rs`, `codegen.rs`)
- Binary entry: `src/bin/verb-lsp.rs` for tool binaries
- Tests: separate files in `tests/` directory, integration tests use descriptive names like `e2e.rs`, `parser_recovery_fuzz.rs`

**Functions:**
- `snake_case` for all function definitions
- Descriptive action-oriented names: `parse()`, `lex()`, `format()`, `assert_terminates()`
- Public library functions exported from `src/lib.rs`
- Helper functions are private with descriptive names: `register_jit_runtime_symbols()`, `build_mathlib_fixture()`

**Variables:**
- `snake_case` for all variable and field names
- Meaningful names: `fn_depth`, `prev_kind`, `scopes`, `externs`, `cur_file`
- Loop counters use descriptive names: `pos` (position), `cpos` (comment position), not single letters

**Types:**
- Enums: `PascalCase` (e.g., `BinOp`, `UnOp`, `Expr`, `Stmt`, `TokenKind`, `ImportStmt`)
- Structs: `PascalCase` (e.g., `Parser`, `Printer`, `Program`, `CompileError`, `Token`, `Codegen`)
- Lifetimes: `'ctx` for context lifetimes (see `Codegen<'ctx>`)

**Constants:**
- `UPPER_CASE` for constant values: `DEADLINE`, `VALID_FIXTURES`
- Used in test configuration and compile-time settings

## Code Style

**Formatting:**
- No rustfmt.toml or clippy.toml found — uses Rust defaults
- Standard indentation: 4 spaces
- Line breaks at logical statement boundaries
- Multi-line constructs: opening brace on same line, closing on own line

**Linting:**
- No explicit linting rules configured
- Code follows idiomatic Rust patterns
- Standard cargo/rustc warnings apply

## Import Organization

**Order:**
1. Standard library imports (`use std::...`)
2. External crates (`use inkwell::...`, `use serde_json::...`)
3. Internal crate modules (`use crate::ast::*`, `use crate::error::CompileError`)
4. Specific items imported last

**Examples from codebase:**
```rust
// src/parser.rs
use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{renamed_keyword, Token, TokenKind};

// src/codegen.rs
use std::collections::HashMap;
use inkwell::builder::Builder;
use crate::ast::*;
use crate::error::CompileError;
use crate::value::*;
```

**Path Aliases:**
- Wildcard imports (`use crate::ast::*`) common for AST modules where many types are used
- Specific imports for error types and utilities
- No path aliases defined in codebase (no crate::path settings)

## Error Handling

**Patterns:**
- Custom `CompileError` struct defined in `src/error.rs` with fields: `msg`, `line`, `col`, `hint`, `file`
- Builder pattern: methods return `Self` for chaining (`with_hint()`, `with_file()`)
- Return type: `Result<T, CompileError>` for fallible operations
- Error creation: `CompileError::new(msg, line, col)` constructor

**Examples:**
```rust
// src/error.rs - Error construction and building
pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
    Self { msg: msg.into(), line, col, hint: None, file: None }
}

pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
    self.hint = Some(hint.into());
    self
}

// src/parser.rs - Error usage
fn err(&self, msg: impl Into<String>) -> CompileError {
    let (l, c) = self.here();
    CompileError::new(msg, l, c)
}

fn err_found(&self, msg: impl Into<String>) -> CompileError {
    let found = self.peek();
    let e = self.err(format!("{}, found {}", msg.into(), found.describe()));
    if let Some(new) = renamed_keyword(n) {
        return e.with_hint(format!("'{n}' was renamed to '{new}'"));
    }
    e
}
```

**Error Recovery:**
- Parser supports two modes: strict (`parse()`) and recovering (`parse_recovering()`)
- Recovering parser collects errors and continues parsing after each error
- Used for editor tools (LSP) that need all errors in a file at once

## Logging

**Framework:** `eprintln!()` for runtime errors and warnings

**Patterns:**
- Error messages use `eprintln!()` to stderr
- Diagnostic output for development: "internal error: ..." messages
- JIT stub functions abort with diagnostic messages

**Examples:**
```rust
// src/main.rs - Diagnostic aborts
eprintln!("internal error: host verb_alloc stub called (verb run cannot use maps)");
std::process::abort();
```

## Comments

**When to Comment:**
- Module-level documentation using `//!` for explaining design decisions
- Item documentation using `///` for public API
- Inline comments `//` before complex logic blocks
- Comments explain "why", not "what" (code should be self-explanatory for "what")

**Documentation Comment Patterns:**
- `//!` at module start explains purpose and design rationale
- `///` before public functions and types
- Multi-line comments for complex invariants and limitations

**Examples:**
```rust
// build.rs - Module-level doc
//! Compiles the C++ runtime translation units that Verb's generated code
//! references *unconditionally* into the `verb` binary itself...

// src/lexer.rs - Item documentation
/// Human-readable form for error messages.
pub fn describe(&self) -> String { ... }

/// Pre-verb-sweep keyword -> current keyword, for migration hints.
pub fn renamed_keyword(word: &str) -> Option<&'static str> { ... }

// src/parser.rs - Inline explanation comments
// An error deep inside an unfinished `make` body leaves
// fn_depth incremented (the decrement after `block()?`
// never runs); recovery always resumes at top level...
```

## Function Design

**Size:** 
- Functions range from 5-50 lines typically
- Larger functions (100+ lines) break down complex parsing logic into named steps
- Helper functions extract repeated patterns

**Parameters:**
- Use descriptive parameter names
- Generic `impl Into<String>` for flexible string parameters (see `CompileError::new()`)
- Reference parameters `&self` and `&mut self` for methods

**Return Values:**
- Always use `Result<T, CompileError>` for fallible operations
- Return owned values or references as appropriate
- Builder pattern methods return `Self` for chaining

**Examples:**
```rust
// Small utility function - src/parser.rs
fn check(&self, k: &TokenKind) -> bool {
    self.peek() == k
}

// Builder pattern - src/error.rs
pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
    self.hint = Some(hint.into());
    self
}

// Fallible operation - src/lexer.rs
pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    lex_with_comments(src).map(|(toks, _)| toks)
}
```

## Module Design

**Exports:**
- `pub mod` declarations in `src/lib.rs` for library modules
- All public functions/types have `pub` visibility
- Internal implementation details remain private

**Module Structure:**
- `src/lib.rs`: Library module declarations (ast, codegen, error, formatter, lexer, parser, targets, value)
- `src/main.rs`: Binary entry point for compiler CLI
- `src/bin/`: Separate binaries (e.g., verb-lsp)
- No barrel files (no `mod.rs` re-exports)

**Crate Dependencies:**
- `inkwell` (v0.9, features: llvm20-1) for LLVM code generation
- `serde_json` (v1.0) for JSON handling
- `cc` (v1.0, build-only) for C++ compilation

---

*Convention analysis: 2026-07-21*
