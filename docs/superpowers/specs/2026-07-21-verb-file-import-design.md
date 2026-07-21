# Verb-File Import — Design Spec (v1)

Date: 2026-07-21
Status: approved

## Purpose

Let a `.verb` program pull in other `.verb` source files via `import mod`,
replacing today's mechanism of listing multiple files on the CLI
(`verb run a.verb b.verb`). One entry file per invocation; everything else
comes in through imports.

## Language surface

```
import mod utils.verb;

assign r helper(2, 3);
print(r);
```

- Reuses the existing `import mod <name>;` statement — no new keyword.
- Disambiguation: if `<name>` ends in the literal suffix `.verb`, it's a
  Verb-source import. Otherwise (bare identifier, no `.verb` suffix) it's
  today's C++ library import (`-l<name>` at link time, unchanged —
  see `docs/superpowers/specs/2026-07-20-cpp-import-design.md`).
- Same placement rule as existing imports: all `import mod`/`import std`
  statements must appear before any other top-level statement in a file.
  Applies per-file (an imported file's own imports must precede its own
  body), not just to the entry file.
- Path is a bare filename (no subdirectories, no `/` — out of scope for
  v1), resolved relative to the directory of the file containing the
  import statement, not the process's cwd.

## Lexer change

The lexer currently has no `.` token at all — `.` only ever appears inside
a float literal (`3.24`), handled inline in the digit-scanning branch.
Adding a general dot token/operator is unnecessary and risks colliding
with float lexing and any future member-access syntax.

Instead: while scanning an identifier (the existing
`a.is_ascii_alphabetic() || a == '_'` branch), after the identifier's
alphanumeric/underscore run ends, check whether the next 5 characters are
exactly `.verb` immediately followed by a non-identifier character (or
EOF) — i.e. a word boundary. If so, consume `.verb` as part of the same
token, producing a single `Ident("utils.verb")`. Otherwise, lexing is
unchanged (a lone `.` outside this pattern remains "unexpected character",
as it is not valid anywhere the lexer currently accepts it except inside a
numeric literal).

This keeps the change local to one branch, adds no new `TokenKind`, and
cannot affect any existing program: no valid Verb identifier today
contains `.verb`, since `.` was previously always a lex error outside
numbers.

## Parser change

`ImportStmt` (currently `Mod(String)` / `Std(String)`) gains a third
variant:

```rust
enum ImportStmt {
    Mod(String),       // C++ lib name, no '.verb' suffix
    Std(String),
    VerbFile(String),  // name ends in '.verb', as written in source
}
```

`import_stmt()`, after parsing the identifier following `mod`, branches on
`name.ends_with(".verb")` to produce `VerbFile` instead of `Mod`. No
filesystem access in the parser (unchanged principle — resolution happens
in `main.rs`, which already owns file I/O).

`Program` (`ast.rs`) gains `pub verb_imports: Vec<String>` alongside the
existing `imports`/`std_imports`, deduplicated at parse time the same way
(`dedup_push`).

`parse_recovering` gets the same three-way branch for LSP tooling parity.

## Resolution (`main.rs`)

Replace the current flat loop (`for file in parsed.files { ... }`) with a
recursive worklist starting from the single entry file:

1. CLI now takes exactly one entry `.verb` file (see CLI section below).
2. Maintain:
   - `done: HashSet<PathBuf>` — canonical (resolved, not necessarily
     `canonicalize()`'d if the file doesn't exist yet, but resolved
     relative to its importer) paths already fully spliced in.
   - `stack: Vec<PathBuf>` — paths currently being processed, for cycle
     detection.
3. `process(path)`:
   - If `path` is in `done`, return immediately (diamond dependency —
     include once).
   - If `path` is in `stack`, error: import cycle, reporting the chain
     (`stack` joined with `->`, plus `path` closing the loop) at the
     import statement that closed the cycle.
   - Push `path` onto `stack`. Read, lex, parse it (same `die()`-based
     error path as today, `sources` list grows to include every file
     touched so error messages can still show source snippets for
     imported files).
   - Splice the parsed `imports`/`std_imports`/`body` into the aggregate
     accumulators (same as today's per-file loop already does for the
     flat CLI case).
   - For each name in the parsed `verb_imports`, resolve it relative to
     `path`'s directory and recurse (`process`) *before* moving on to the
     next statement in `path` — so a diamond that's imported from two
     different points still only contributes its body once, at the point
     of its first (topologically earliest) import.
   - Pop `path` off `stack`, insert into `done`.
4. Start: `process(entry_file)`.

Duplicate top-level definitions across spliced files remain a compile
error exactly as they would today if two CLI-supplied files redefined the
same name — no new dedup/shadowing logic needed, since this is a flat
merge (see below).

## Flat merge, no namespacing

All statements from every transitively-imported `.verb` file are
concatenated into one global scope, in traversal order — identical to
today's CLI multi-file concatenation model. There is no qualified access
(`utils.helper()`); a function defined in an imported file is called the
same as a function defined in the entry file. This avoids adding a
name-resolution/scoping layer that doesn't exist anywhere else in the
compiler today.

## CLI change

`verb run <file.verb>` and `verb build <file.verb> -o <out>` accept
exactly one entry file, not a list. `ParsedArgs.files: Vec<String>`
collapses to `ParsedArgs.file: String`; `parse_cli` errors (falls through
to `usage()`) if more than one bare positional argument is given. Usage
strings drop the `...` after `<file.verb>`:

```
usage: verb run <file.verb> [--emit-llvm]
       verb build <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]
       verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]  (alias for build)
```

Existing e2e tests that currently pass multiple files on the CLI are
rewritten to use `import mod <other>.verb;` from a single entry file
instead.

## Codegen

No changes. `compile_program` already takes the fully-flattened
`stmts`/`stmt_files`/`imports`/`std_imports` produced by `main.rs`; it has
no awareness of which physical file contributed which statement beyond
`stmt_files` (already used for error attribution), which continues to
work unmodified since the recursive resolver still produces one flat
`stmt_files` vector in traversal order.

## Testing

- Lexer unit test: `utils.verb` lexes as a single `Ident("utils.verb")`;
  `mathlib` (no suffix) is unaffected; a bare `.` outside a number is
  still a lex error.
- Parser unit tests: `import mod utils.verb;` parses to
  `ImportStmt::VerbFile("utils.verb")`; mixed `import mod mathlib;
  import mod utils.verb; import std io;` in one prelude sorts into the
  right three buckets.
- `main.rs` / e2e tests:
  - Two-file program (entry imports one file, calls its function) —
    equivalent golden-output test to what today's multi-file CLI tests
    cover, ported to the new mechanism.
  - Diamond import (entry imports A and B, both import C) — C's top-level
    statements run exactly once (e.g. a top-level `print` in C appears
    once in output, not twice).
  - Cycle (A imports B, B imports A) — compile error naming the cycle,
    non-zero exit.
  - Import of a nonexistent `.verb` file — same "cannot read" error style
    as today's missing-entry-file case.
  - `verb run`/`verb build` invoked with two bare file arguments — usage
    error (CLI no longer accepts multiple entry files).

## Out of scope (v2+)

Subdirectory/path imports (`import mod lib/utils.verb;`), qualified/
namespaced access to imported symbols, `import std`-style module-name
imports for user `.verb` libraries, re-exporting, visibility/privacy
(everything imported is globally visible, same as everything in the
entry file), circular imports with partial/lazy resolution (a cycle is
always a hard error in v1).
