# Multi-file `.verb` linking — design spec

## Context

The compiler currently only accepts a single `.verb` file: `verb run <file.verb>` / `verb build <file.verb> -o <out>`. There is no import/module keyword in the language and no way to compile a program spread across multiple files. This spec adds support for passing multiple `.verb` files on the command line, which are linked (concatenated) into one program.

## Goals

- `verb run a.verb b.verb c.verb [--emit-llvm]`
- `verb build a.verb b.verb c.verb -o out [--emit-llvm]`
- Files and flags may interleave on the command line; `-o` still consumes its following argument as the output path; every other non-flag argument is treated as a source file.
- At least one file is required (existing usage-error behavior extends to "no files given").

## Non-goals

- No in-source `import`/`include` syntax. (Considered and explicitly rejected in favor of CLI-only linking — simpler, no cycle/dedup handling needed.)
- No enforcement of a "library file" convention. Any file, including non-entry files, may contain top-level executable statements.
- No new duplicate-symbol detection across files.
- No filename in runtime (JIT-generated) error output — see Accepted Limitation below.

## Semantics

### Linking = concatenation

Each file is read, lexed, and parsed independently using the existing `lexer::lex` and `parser::parse` functions — unchanged. This yields one `Vec<Stmt>` per file. These lists are concatenated, in the order files appear on the command line, into a single `Vec<Stmt>` representing the whole program. This concatenated list is passed to `codegen::compile_program` exactly as today's single-file `Vec<Stmt>` is.

Because `compile_program` already emits one `main` function and hoists `Stmt::Fn` definitions as it walks the top-level statement list in order (this is existing, unchanged behavior), concatenation alone is sufficient to "link" multiple files:

- Top-level executable statements from every file run, in file-argument order, as the body of `main`.
- Function definitions from every file are hoisted into the same shared function table (`Codegen::functions`), in the same order.
- Cross-file function calls follow the same source-order constraint that already applies within a single file today: a function must be defined (its `Stmt::Fn` processed) before a call to it is codegen'd. This is not a new limitation — it's the existing single-pass, no-forward-declaration behavior, now simply spanning file boundaries.
- Duplicate function names across files behave exactly as duplicate names within one file do today: the later definition silently shadows the earlier one in `Codegen::functions` and in scope. No new validation is added for this case.

### CLI argument parsing

`main.rs` currently does positional parsing: `args[1]` = command, `args[2]` = the one file. This becomes: walk `args[2..]`, classify each token —

- `--emit-llvm` → sets the existing flag
- `-o` → consumes the next argument as the output path (as today)
- anything else → appended to an ordered `files: Vec<String>` list

After the walk, `files` must be non-empty or `usage()` is invoked (mirrors today's `args.len() < 3` check, generalized).

### Error attribution (file:line:col)

`CompileError` (in `error.rs`) gains a new field:

```rust
pub file: Option<String>,
```

with a builder method `with_file(mut self, name: impl Into<String>) -> Self`, mirroring the existing `with_hint`.

**Lexer/parser errors**: `main.rs` already processes files one at a time when calling `lexer::lex` and `parser::parse` (this happens before concatenation). At that point the current filename is known directly, so the error is stamped immediately: `lexer::lex(&src).map_err(|e| e.with_file(file_name))`. No changes to `lexer.rs` or `parser.rs` internals are needed.

**Codegen errors**: these occur during the single `compile_program` call over the concatenated statement list, where the call site (`main.rs`) no longer knows which original file an error came from. To solve this without threading a `file` field through every AST node:

- `compile_program` receives a second parameter, a `stmt_files: &[String]` slice the same length as the concatenated top-level `Vec<Stmt>`, giving the originating filename for each top-level statement. `main.rs` builds this alongside the concatenation.
- `compile_program`'s existing top-level loop (today: `self.gen_stmts(stmts)`) becomes an explicit `for (i, s) in stmts.iter().enumerate()` loop. Before processing each top-level statement, `Codegen` records `self.cur_file = stmt_files[i].clone()`. On `Err(mut e)` bubbling up from `gen_stmt(s)`, if `e.file.is_none()`, it is stamped with `self.cur_file` before being returned.
- Nested statement processing (`gen_stmts` used for blocks/function bodies) is untouched — a nested block cannot span files, so `cur_file` set at the enclosing top-level statement is correct for everything nested inside it.

This means only the outermost loop in `compile_program` needs to know about file boundaries; no error-construction call site deep in `codegen.rs` needs to change.

**Error display**: `main.rs`'s `die()` gains the filename in its header line: `error [{file}:{line}:{col}]: {msg}` (previously `error [{line}:{col}]: {msg}`). Since `die()` is called once per file during the lex/parse phase (with that file's own source text, so line-indexed snippet lookup is unaffected) and once after codegen (using the now file-stamped error plus that file's source text, looked up by filename from a `Vec<(String, String)>` of (filename, source) pairs `main.rs` keeps around from the initial per-file read), the existing snippet-printing logic needs only this lookup added, not restructuring.

### Accepted limitation: runtime errors keep no filename

Runtime errors (division by zero, type mismatches, etc.) are baked into the generated LLVM IR as `printf` calls with a `runtime error [%d:%d]: ...` format string, filled with the statement's line/col at codegen time (`codegen.rs:103`). Giving these a filename too would require threading a filename (as a global string constant or file-id int) through every runtime-error call site in codegen and extending the format string — meaningfully more IR-generation work for a comparatively rare case (a runtime type/logic error in a multi-file program where the line number alone is ambiguous between files). This spec explicitly defers that; runtime errors continue to show bare `[line:col]`, documented here as a known gap for v1.

## Testing

Add fixtures under `tests/fixtures/` for a multi-file case, e.g. `multifile_a.verb` (defines a function) + `multifile_b.verb` (calls it, has the executable top-level code) with a `tests/e2e.rs` test that invokes `verb run` with both files and checks stdout against a `.expected` file — extending the existing `run_ok`-style harness to accept multiple file args. Add at least one error-path test confirming `file:line:col` attribution picks the correct file when the error originates in the second file.

## Summary of code changes

- `src/error.rs`: add `file: Option<String>` field + `with_file` builder.
- `src/main.rs`: multi-file CLI parsing; per-file read/lex/parse loop building `(files, sources, concatenated stmts, stmt_files)`; `die()` takes filename and looks up correct source text.
- `src/codegen.rs`: `compile_program` takes `stmt_files: &[String]`; tracks `cur_file`; stamps bubbled-up errors that lack a file.
- `tests/fixtures/` + `tests/e2e.rs`: new multi-file fixtures and test(s).
