# Verb-File Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `import mod <name>.verb;` pull in another Verb source file (recursively, deduped, cycle-checked), and make that the *only* way to build a multi-file program — the CLI drops its current "list several `.verb` files" mode down to exactly one entry file.

**Architecture:** A new `.verb`-suffix check at the lexer level lets `import mod` disambiguate a Verb-source import from today's C++-library import with no new keyword or token. A new `src/resolve.rs` module owns all filesystem/recursion concerns (reading, deduping, cycle-detecting, splicing) that `main.rs` currently does inline in a flat one-level loop; `main.rs` shrinks to "parse CLI, call `resolve::resolve`, hand the flattened result to `Codegen` exactly as before." Codegen is untouched — it already only ever sees one flat `(stmts, stmt_files, imports, std_imports)` tuple, and doesn't care how it was assembled.

**Tech Stack:** Rust 2021, existing `verb` crate structure (`lexer` → `parser` → new `resolve` → `codegen`), no new dependencies.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-21-verb-file-import-design.md` — read it first; this plan implements it section by section.
- Disambiguation is purely syntactic: `<name>` ends in the literal suffix `.verb` → Verb-source import; otherwise → today's C++ `-l<name>` import. No filesystem probing at parse time.
- Import path is a bare filename only — no `/`, no subdirectories (v1).
- Path resolves relative to the directory of the file containing the `import mod x.verb;` statement, never the process cwd.
- Flat merge only: no namespacing/qualified access. Imported files' statements land in the same global scope as the entry file's, in traversal order, imported content spliced in before the importing file's own body (since `import` statements must precede a file's own body anyway).
- Diamond imports (same file imported from two places) include that file's content exactly once. A cycle is a hard compile-time error in v1 — no partial/lazy resolution.
- CLI (`verb run` / `verb build` / `verb compile`) now takes exactly one entry `.verb` file. Passing more than one bare file argument is a usage error (exit code 2), same as passing zero.
- No changes to `src/codegen.rs` or `src/bin/verb-lsp.rs` — out of scope per spec.

---

### Task 1: Lexer — fold a trailing `.verb` suffix into the identifier token

**Files:**
- Modify: `src/lexer.rs:152-173` (the identifier-scanning arm of `lex_with_comments`)
- Test: `src/lexer.rs` (`#[cfg(test)] mod tests` block, same file)

**Interfaces:**
- Consumes: nothing new — operates purely on the existing `chars: Vec<char>` scan already in `lex_with_comments`.
- Produces: `Ident("utils.verb")` as a *single* token for source text `utils.verb` immediately followed by a non-identifier character or EOF. No new `TokenKind` variant. Every other program lexes identically to today (a bare `.` anywhere it wasn't already valid — i.e. outside a float literal — remains a lex error).

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/lexer.rs`, right after `fn scans_std_import_keyword`:

```rust
    #[test]
    fn folds_dot_verb_suffix_into_identifier() {
        use TokenKind::*;
        assert_eq!(
            kinds("import mod utils.verb;"),
            vec![Import, Mod, Ident("utils.verb".into()), Semi, Eof]
        );
    }

    #[test]
    fn does_not_fold_dot_verb_when_followed_by_identifier_chars() {
        // `.verbose` is not the exact `.verb` suffix at a word boundary,
        // so no folding happens and the bare `.` after `utils` is still
        // a lex error, same as it was before this feature existed.
        assert!(lex("utils.verbose").is_err());
    }

    #[test]
    fn bare_dot_after_identifier_is_still_a_lex_error() {
        assert!(lex("mathlib.").is_err());
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib lexer::tests::folds_dot_verb_suffix_into_identifier`
Expected: FAIL — `assertion failed` (actual lex result is `[Import, Mod, Ident("utils"), ...]` then a lex error on the trailing `.`, i.e. `kinds()` panics inside `lex(src).unwrap()` because lexing `utils.verb` today errors on the `.`).

- [ ] **Step 3: Implement the suffix fold**

In `src/lexer.rs`, replace the identifier-scanning arm:

```rust
            a if a.is_ascii_alphabetic() || a == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1; col += 1;
                }
                let word: String = chars[start..i].iter().collect();
                use TokenKind::*;
                let kind = match word.as_str() {
```

with:

```rust
            a if a.is_ascii_alphabetic() || a == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1; col += 1;
                }
                // A `.verb` suffix directly after an identifier, at a word
                // boundary (not followed by another identifier/digit char),
                // folds into the same token so `import mod utils.verb;`
                // lexes as one name. `.` is otherwise never valid outside a
                // numeric literal, so this can't misfire on any program
                // that lexed successfully before — see
                // docs/superpowers/specs/2026-07-21-verb-file-import-design.md.
                const VERB_SUFFIX: &[char] = &['.', 'v', 'e', 'r', 'b'];
                if chars[i..].starts_with(VERB_SUFFIX) {
                    let after = i + VERB_SUFFIX.len();
                    let boundary = chars.get(after)
                        .map_or(true, |c| !(c.is_ascii_alphanumeric() || *c == '_'));
                    if boundary {
                        i += VERB_SUFFIX.len();
                        col += VERB_SUFFIX.len() as u32;
                    }
                }
                let word: String = chars[start..i].iter().collect();
                use TokenKind::*;
                let kind = match word.as_str() {
```

(The rest of the match arm — the keyword list and `_ => Ident(word)` fallback — is unchanged. No keyword string contains a `.`, so a folded `name.verb` word always falls through to `Ident`.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib lexer::tests`
Expected: PASS — all lexer tests, including the 3 new ones, and the pre-existing `scans_import_keywords` / `rejects_unknown_char` tests (unaffected).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): fold a .verb suffix into the identifier token"
```

---

### Task 2: AST + Parser — recognize `import mod <name>.verb;` as a Verb-file import

**Files:**
- Modify: `src/ast.rs:33-38` (`Program` struct)
- Modify: `src/parser.rs:5-16` (`parse`), `:25-69` (`parse_recovering`), `:71-74` (`ImportStmt`), `:130-163` (`imports`, `import_stmt`)
- Test: `src/parser.rs` (`#[cfg(test)] mod tests`), `src/formatter.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `TokenKind::Ident(String)` as already produced by Task 1's lexer for `.verb`-suffixed names.
- Produces: `ast::Program.verb_imports: Vec<String>` — the raw `.verb` filenames as written, in source order, deduplicated (same `dedup_push` used for the other two import kinds). This is what Task 3's `resolve` module consumes.

- [ ] **Step 1: Write the failing tests**

Add to `src/ast.rs` — no test file for this trivial struct change, it's exercised entirely through parser tests below.

Add to `src/parser.rs`'s `#[cfg(test)] mod tests`, right after `fn std_and_mod_imports_coexist`:

```rust
    #[test]
    fn parses_verb_file_import() {
        let p = parse(lex("import mod utils.verb;").unwrap()).unwrap();
        assert_eq!(p.verb_imports, vec!["utils.verb".to_string()]);
        assert!(p.imports.is_empty());
        assert!(p.body.is_empty());
    }

    #[test]
    fn dedups_repeated_verb_file_import() {
        let p = parse(lex("import mod utils.verb; import mod utils.verb;").unwrap()).unwrap();
        assert_eq!(p.verb_imports, vec!["utils.verb".to_string()]);
    }

    #[test]
    fn verb_file_and_cpp_lib_imports_coexist() {
        let p = parse(lex(
            "import mod mathlib; import mod utils.verb; import std io; print(1);"
        ).unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert_eq!(p.verb_imports, vec!["utils.verb".to_string()]);
        assert_eq!(p.std_imports, vec!["io".to_string()]);
        assert_eq!(p.body.len(), 1);
    }

    #[test]
    fn recovering_collects_verb_file_imports_too() {
        let src = "import mod utils.verb; print(1);";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(prog.verb_imports, vec!["utils.verb".to_string()]);
        assert_eq!(prog.body.len(), 1);
    }
```

Add to `src/formatter.rs`'s `#[cfg(test)] mod tests`, right after `fn formats_import_statements`:

```rust
    #[test]
    fn formats_verb_file_import_statement() {
        assert_eq!(
            fmt("import   mod   utils.verb  ;\nprint(1);"),
            "import mod utils.verb;\nprint(1);\n"
        );
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib parser::tests::parses_verb_file_import`
Expected: FAIL — `p.verb_imports` doesn't exist yet (compile error: `no field `verb_imports` on type `ast::Program``).

- [ ] **Step 3: Implement the AST and parser changes**

In `src/ast.rs`, change:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
    pub body: Vec<Stmt>,
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
    pub verb_imports: Vec<String>,
    pub body: Vec<Stmt>,
}
```

In `src/parser.rs`, change `pub fn parse`:

```rust
pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let (imports, std_imports) = p.imports()?;
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            return Err(p.err("'import' must appear before any other statement"));
        }
        body.push(p.statement()?);
    }
    Ok(Program { imports, std_imports, body })
}
```

to:

```rust
pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let (imports, std_imports, verb_imports) = p.imports()?;
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            return Err(p.err("'import' must appear before any other statement"));
        }
        body.push(p.statement()?);
    }
    Ok(Program { imports, std_imports, verb_imports, body })
}
```

Change `pub fn parse_recovering`:

```rust
pub fn parse_recovering(toks: Vec<Token>) -> (Program, Vec<CompileError>) {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let mut imports = Vec::new();
    let mut std_imports = Vec::new();
    let mut errors = Vec::new();
    while p.check(&TokenKind::Import) {
        match p.import_stmt() {
            Ok(ImportStmt::Mod(name)) => dedup_push(&mut imports, name),
            Ok(ImportStmt::Std(name)) => dedup_push(&mut std_imports, name),
            Err(e) => { errors.push(e); p.synchronize(); }
        }
    }
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            errors.push(p.err("'import' must appear before any other statement"));
            p.advance();
            continue;
        }
        let pos_before = p.pos;
        match p.statement() {
            Ok(s) => body.push(s),
            Err(e) => {
                errors.push(e);
                p.fn_depth = 0;
                p.synchronize();
                if p.pos == pos_before {
                    p.advance();
                }
            }
        }
    }
    (Program { imports, std_imports, body }, errors)
}
```

to:

```rust
pub fn parse_recovering(toks: Vec<Token>) -> (Program, Vec<CompileError>) {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let mut imports = Vec::new();
    let mut std_imports = Vec::new();
    let mut verb_imports = Vec::new();
    let mut errors = Vec::new();
    while p.check(&TokenKind::Import) {
        match p.import_stmt() {
            Ok(ImportStmt::Mod(name)) => dedup_push(&mut imports, name),
            Ok(ImportStmt::Std(name)) => dedup_push(&mut std_imports, name),
            Ok(ImportStmt::VerbFile(name)) => dedup_push(&mut verb_imports, name),
            Err(e) => { errors.push(e); p.synchronize(); }
        }
    }
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            errors.push(p.err("'import' must appear before any other statement"));
            p.advance();
            continue;
        }
        let pos_before = p.pos;
        match p.statement() {
            Ok(s) => body.push(s),
            Err(e) => {
                errors.push(e);
                p.fn_depth = 0;
                p.synchronize();
                if p.pos == pos_before {
                    p.advance();
                }
            }
        }
    }
    (Program { imports, std_imports, verb_imports, body }, errors)
}
```

Change the `ImportStmt` enum:

```rust
enum ImportStmt {
    Mod(String),
    Std(String),
}
```

to:

```rust
enum ImportStmt {
    Mod(String),
    Std(String),
    VerbFile(String),
}
```

Change `fn imports`:

```rust
    fn imports(&mut self) -> Result<(Vec<String>, Vec<String>), CompileError> {
        let mut imports = Vec::new();
        let mut std_imports = Vec::new();
        while self.check(&TokenKind::Import) {
            match self.import_stmt()? {
                ImportStmt::Mod(name) => dedup_push(&mut imports, name),
                ImportStmt::Std(name) => dedup_push(&mut std_imports, name),
            }
        }
        Ok((imports, std_imports))
    }
```

to:

```rust
    fn imports(&mut self) -> Result<(Vec<String>, Vec<String>, Vec<String>), CompileError> {
        let mut imports = Vec::new();
        let mut std_imports = Vec::new();
        let mut verb_imports = Vec::new();
        while self.check(&TokenKind::Import) {
            match self.import_stmt()? {
                ImportStmt::Mod(name) => dedup_push(&mut imports, name),
                ImportStmt::Std(name) => dedup_push(&mut std_imports, name),
                ImportStmt::VerbFile(name) => dedup_push(&mut verb_imports, name),
            }
        }
        Ok((imports, std_imports, verb_imports))
    }
```

Change `fn import_stmt`'s `mod` branch:

```rust
    fn import_stmt(&mut self) -> Result<ImportStmt, CompileError> {
        self.advance(); // 'import'
        if self.matches(&TokenKind::Mod) {
            let (name, ..) = self.expect_ident("library name after 'mod'")?;
            self.expect(&TokenKind::Semi, "';'")?;
            return Ok(ImportStmt::Mod(name));
        }
```

to:

```rust
    fn import_stmt(&mut self) -> Result<ImportStmt, CompileError> {
        self.advance(); // 'import'
        if self.matches(&TokenKind::Mod) {
            let (name, ..) = self.expect_ident("library name after 'mod'")?;
            self.expect(&TokenKind::Semi, "';'")?;
            if name.ends_with(".verb") {
                return Ok(ImportStmt::VerbFile(name));
            }
            return Ok(ImportStmt::Mod(name));
        }
```

(The `std` branch below is unchanged.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib parser::tests formatter::tests`
Expected: PASS — all parser and formatter tests, including the 4 new parser tests and 1 new formatter test.

- [ ] **Step 5: Run the full lib test suite to check for fallout**

Run: `cargo test --lib`
Expected: PASS — `Program` gained a field but every construction site (`parser.rs`, the only two) was updated in this step; every *read* site (`src/bin/verb-lsp.rs`, `src/main.rs`) only accesses `.imports`/`.std_imports`/`.body` by field name and isn't affected by the new field. `src/main.rs` won't yet compile against `resolve.rs` (that's Task 4) but this step only runs `--lib`, not the binary.

- [ ] **Step 6: Commit**

```bash
git add src/ast.rs src/parser.rs src/formatter.rs
git commit -m "feat(parser): parse import mod <name>.verb as a Verb-file import"
```

---

### Task 3: `src/resolve.rs` — recursive import resolution

**Files:**
- Create: `src/resolve.rs`
- Modify: `src/lib.rs:1-8` (register the new module)

**Interfaces:**
- Consumes: `ast::Program.verb_imports: Vec<String>` (Task 2), `lexer::lex`, `parser::parse`, `error::CompileError`.
- Produces (for Task 4 / `main.rs`):
  ```rust
  pub struct ResolvedProgram {
      pub sources: Vec<(String, String)>,
      pub stmts: Vec<ast::Stmt>,
      pub stmt_files: Vec<String>,
      pub imports: Vec<String>,
      pub std_imports: Vec<String>,
  }

  pub enum ResolveErrorKind {
      Compile(CompileError),
      Cycle(String),
      Io { path: String, message: String },
  }

  pub struct ResolveError {
      pub kind: ResolveErrorKind,
      pub sources: Vec<(String, String)>,
  }

  pub fn resolve(entry: &str) -> Result<ResolvedProgram, ResolveError>
  ```

- [ ] **Step 1: Write the failing tests**

Create `src/resolve.rs` with just the test module first (the real implementation comes in Step 3):

```rust
use std::collections::HashSet;
use std::path::PathBuf;

use crate::ast::Stmt;
use crate::error::CompileError;
use crate::lexer;
use crate::parser;

pub struct ResolvedProgram {
    pub sources: Vec<(String, String)>,
    pub stmts: Vec<Stmt>,
    pub stmt_files: Vec<String>,
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
}

pub enum ResolveErrorKind {
    Compile(CompileError),
    Cycle(String),
    Io { path: String, message: String },
}

pub struct ResolveError {
    pub kind: ResolveErrorKind,
    pub sources: Vec<(String, String)>,
}

pub fn resolve(_entry: &str) -> Result<ResolvedProgram, ResolveError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh temp dir per test, named after the test, under
    /// `std::env::temp_dir()` — same convention `tests/e2e.rs` already
    /// uses for filesystem-touching tests.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("verb_resolve_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, name: &str, src: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, src).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn resolves_a_single_file_with_no_imports() {
        let dir = test_dir("single_file");
        let entry = write(&dir, "entry.verb", "assign x 1;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 1);
        assert_eq!(r.stmt_files, vec![entry.clone()]);
        assert!(r.imports.is_empty());
        assert!(r.std_imports.is_empty());
        assert_eq!(r.sources.len(), 1);
    }

    #[test]
    fn splices_a_verb_file_import_before_the_importers_own_statements() {
        let dir = test_dir("splice");
        write(&dir, "lib.verb", "assign y 2;\n");
        let entry = write(&dir, "entry.verb", "import mod lib.verb;\nassign x 1;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 2);
        assert!(r.stmt_files[0].ends_with("lib.verb"), "stmt_files: {:?}", r.stmt_files);
        assert!(r.stmt_files[1].ends_with("entry.verb"), "stmt_files: {:?}", r.stmt_files);
    }

    #[test]
    fn dedups_a_diamond_import() {
        let dir = test_dir("diamond");
        write(&dir, "shared.verb", "assign s 1;\n");
        write(&dir, "a.verb", "import mod shared.verb;\nassign a 1;\n");
        write(&dir, "b.verb", "import mod shared.verb;\nassign b 1;\n");
        let entry = write(
            &dir, "entry.verb",
            "import mod a.verb;\nimport mod b.verb;\nassign e 1;\n",
        );

        let r = resolve(&entry).unwrap();
        assert_eq!(r.stmts.len(), 4, "stmt_files: {:?}", r.stmt_files);
        let shared_count = r.stmt_files.iter().filter(|f| f.ends_with("shared.verb")).count();
        assert_eq!(shared_count, 1, "stmt_files: {:?}", r.stmt_files);
    }

    #[test]
    fn detects_an_import_cycle() {
        let dir = test_dir("cycle");
        write(&dir, "a.verb", "import mod b.verb;\n");
        let entry = write(&dir, "b.verb", "import mod a.verb;\n");
        // b.verb imports a.verb, a.verb imports b.verb back -- resolving
        // either one hits the cycle.
        let _ = entry;
        let b_entry = dir.join("b.verb").to_str().unwrap().to_string();

        let err = resolve(&b_entry).unwrap_err();
        assert!(matches!(err.kind, ResolveErrorKind::Cycle(_)), "expected Cycle, file layout ok");
    }

    #[test]
    fn reports_a_missing_import_file() {
        let dir = test_dir("missing");
        let entry = write(&dir, "entry.verb", "import mod missing.verb;\n");

        let err = resolve(&entry).unwrap_err();
        match err.kind {
            ResolveErrorKind::Io { path, .. } => {
                assert!(path.ends_with("missing.verb"), "path: {path}");
            }
            _ => panic!("expected Io error"),
        }
    }

    #[test]
    fn merges_imports_and_std_imports_from_imported_files() {
        let dir = test_dir("merge_imports");
        write(&dir, "lib.verb", "import mod mathlib;\nimport std io;\n");
        let entry = write(&dir, "entry.verb", "import mod lib.verb;\n");

        let r = resolve(&entry).unwrap();
        assert_eq!(r.imports, vec!["mathlib".to_string()]);
        assert_eq!(r.std_imports, vec!["io".to_string()]);
    }
}
```

Add `pub mod resolve;` to `src/lib.rs` (any position, alphabetical to match the existing list — after `pub mod parser;`):

```rust
pub mod ast;
pub mod codegen;
pub mod error;
pub mod formatter;
pub mod lexer;
pub mod parser;
pub mod resolve;
pub mod targets;
pub mod value;
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib resolve::tests::resolves_a_single_file_with_no_imports`
Expected: FAIL — panics on `todo!()` (`not yet implemented`).

- [ ] **Step 3: Implement `resolve`**

Replace the `pub fn resolve(_entry: &str) -> Result<ResolvedProgram, ResolveError> { todo!() }` stub in `src/resolve.rs` with:

```rust
struct Resolver {
    sources: Vec<(String, String)>,
    stmts: Vec<Stmt>,
    stmt_files: Vec<String>,
    imports: Vec<String>,
    std_imports: Vec<String>,
    done: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

fn dedup_push(v: &mut Vec<String>, item: String) {
    if !v.contains(&item) { v.push(item); }
}

pub fn resolve(entry: &str) -> Result<ResolvedProgram, ResolveError> {
    let mut r = Resolver {
        sources: Vec::new(),
        stmts: Vec::new(),
        stmt_files: Vec::new(),
        imports: Vec::new(),
        std_imports: Vec::new(),
        done: HashSet::new(),
        stack: Vec::new(),
    };
    let entry_path = PathBuf::from(entry);
    match r.process(&entry_path, entry) {
        Ok(()) => Ok(ResolvedProgram {
            sources: r.sources,
            stmts: r.stmts,
            stmt_files: r.stmt_files,
            imports: r.imports,
            std_imports: r.std_imports,
        }),
        Err(kind) => Err(ResolveError { kind, sources: r.sources }),
    }
}

impl Resolver {
    /// `display` is the path text used for error messages and
    /// `stmt_files` (as written on the CLI, or as joined from an
    /// importer's directory) -- kept alongside `path` (used for
    /// cycle/dedup identity) so messages read naturally.
    fn process(&mut self, path: &PathBuf, display: &str) -> Result<(), ResolveErrorKind> {
        if self.done.contains(path) {
            return Ok(());
        }
        if self.stack.contains(path) {
            let mut chain: Vec<String> = self.stack.iter().map(|p| p.display().to_string()).collect();
            chain.push(display.to_string());
            return Err(ResolveErrorKind::Cycle(format!("import cycle: {}", chain.join(" -> "))));
        }

        let src = std::fs::read_to_string(path).map_err(|e| ResolveErrorKind::Io {
            path: display.to_string(),
            message: e.to_string(),
        })?;
        self.sources.push((display.to_string(), src.clone()));

        let toks = lexer::lex(&src)
            .map_err(|e| e.with_file(display.to_string()))
            .map_err(ResolveErrorKind::Compile)?;
        let prog = parser::parse(toks)
            .map_err(|e| e.with_file(display.to_string()))
            .map_err(ResolveErrorKind::Compile)?;

        self.stack.push(path.clone());

        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for name in &prog.verb_imports {
            let child_path = dir.join(name);
            let child_display = child_path.display().to_string();
            self.process(&child_path, &child_display)?;
        }

        self.stmt_files.extend(std::iter::repeat(display.to_string()).take(prog.body.len()));
        self.stmts.extend(prog.body);
        for lib in prog.imports { dedup_push(&mut self.imports, lib); }
        for m in prog.std_imports { dedup_push(&mut self.std_imports, m); }

        self.stack.pop();
        self.done.insert(path.clone());
        Ok(())
    }
}
```

(This goes after the `pub fn resolve` / struct / enum definitions already in the file from Step 1, before the `#[cfg(test)]` module.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib resolve::tests`
Expected: PASS — all 6 tests in `resolve::tests`.

- [ ] **Step 5: Run the full lib test suite**

Run: `cargo test --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/resolve.rs src/lib.rs
git commit -m "feat(resolve): recursive .verb import resolution with dedup and cycle detection"
```

---

### Task 4: `main.rs` — single entry file, wire in `resolve`

**Files:**
- Modify: `src/main.rs:24-80` (`ParsedArgs`, `parse_cli`), `:102-145` (`usage`, `main`)
- Test: `src/main.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `resolve::resolve(entry: &str) -> Result<ResolvedProgram, ResolveError>` (Task 3).
- Produces: `ParsedArgs.file: String` (was `files: Vec<String>`) — nothing outside `main.rs` reads `ParsedArgs`.

- [ ] **Step 1: Write the failing tests**

Replace the three CLI-parsing tests that assumed multiple files, in `src/main.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn parses_multiple_files() {
        let p = parse_cli(&args(&["verb", "run", "a.verb", "b.verb"])).unwrap();
        assert_eq!(p.cmd, "run");
        assert_eq!(p.files, vec!["a.verb".to_string(), "b.verb".to_string()]);
        assert!(!p.emit_llvm);
        assert_eq!(p.out, None);
    }

    #[test]
    fn parses_flags_interleaved_with_files() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "b.verb", "--emit-llvm",
        ])).unwrap();
        assert_eq!(p.cmd, "build");
        assert_eq!(p.files, vec!["a.verb".to_string(), "b.verb".to_string()]);
        assert_eq!(p.out, Some("out".to_string()));
        assert!(p.emit_llvm);
    }

    #[test]
    fn parses_lib_dirs() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "-L/opt/lib", "-L./libs",
        ])).unwrap();
        assert_eq!(p.files, vec!["a.verb".to_string()]);
        assert_eq!(p.lib_dirs, vec!["-L/opt/lib".to_string(), "-L./libs".to_string()]);
    }
```

with:

```rust
    #[test]
    fn parses_a_single_file() {
        let p = parse_cli(&args(&["verb", "run", "a.verb"])).unwrap();
        assert_eq!(p.cmd, "run");
        assert_eq!(p.file, "a.verb".to_string());
        assert!(!p.emit_llvm);
        assert_eq!(p.out, None);
    }

    #[test]
    fn rejects_multiple_files() {
        assert!(parse_cli(&args(&["verb", "run", "a.verb", "b.verb"])).is_none());
    }

    #[test]
    fn parses_flags_around_a_single_file() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "--emit-llvm",
        ])).unwrap();
        assert_eq!(p.cmd, "build");
        assert_eq!(p.file, "a.verb".to_string());
        assert_eq!(p.out, Some("out".to_string()));
        assert!(p.emit_llvm);
    }

    #[test]
    fn parses_lib_dirs() {
        let p = parse_cli(&args(&[
            "verb", "build", "a.verb", "-o", "out", "-L/opt/lib", "-L./libs",
        ])).unwrap();
        assert_eq!(p.file, "a.verb".to_string());
        assert_eq!(p.lib_dirs, vec!["-L/opt/lib".to_string(), "-L./libs".to_string()]);
    }
```

Leave `rejects_no_files`, `rejects_missing_o_value`, and `rejects_no_command` as-is (still valid — `rejects_no_files` still means "zero files given").

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --bin verb parses_a_single_file`
Expected: FAIL to compile — `ParsedArgs` has no field `file` yet (`p.file` doesn't exist).

- [ ] **Step 3: Implement the CLI and `main` changes**

In `src/main.rs`, change `ParsedArgs`:

```rust
struct ParsedArgs {
    cmd: String,
    files: Vec<String>,
    out: Option<String>,
    emit_llvm: bool,
    target: Option<String>,
    lib_dirs: Vec<String>,
}
```

to:

```rust
struct ParsedArgs {
    cmd: String,
    file: String,
    out: Option<String>,
    emit_llvm: bool,
    target: Option<String>,
    lib_dirs: Vec<String>,
}
```

Change `parse_cli`'s tail (everything from `if files.is_empty()` onward):

```rust
    if files.is_empty() {
        return None;
    }
    Some(ParsedArgs { cmd, files, out, emit_llvm, target, lib_dirs })
}
```

to:

```rust
    if files.len() != 1 {
        return None;
    }
    let file = files.remove(0);
    Some(ParsedArgs { cmd, file, out, emit_llvm, target, lib_dirs })
}
```

Change `usage`:

```rust
fn usage() -> ! {
    eprintln!("usage: verb run <file.verb>... [--emit-llvm]");
    eprintln!("       verb build <file.verb>... -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]");
    eprintln!("       verb compile <file.verb>... -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]  (alias for build)");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    exit(2)
}
```

to:

```rust
fn usage() -> ! {
    eprintln!("usage: verb run <file.verb> [--emit-llvm]");
    eprintln!("       verb build <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]");
    eprintln!("       verb compile <file.verb> -o <out> [--target <os>-<arch>|all] [-L<dir>]... [--emit-llvm]  (alias for build)");
    eprintln!("       targets: linux-x86_64 linux-arm64 macos-x86_64 macos-arm64 windows-x86_64 windows-arm64");
    eprintln!("       use 'import mod <name>.verb;' inside <file.verb> to pull in other Verb source files");
    exit(2)
}
```

Change the top of `fn main` (everything from the `let mut sources` block through the `cg.compile_program` call):

```rust
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut stmts = Vec::new();
    let mut stmt_files = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut std_imports: Vec<String> = Vec::new();

    for file in &parsed.files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {file}: {e}");
                exit(1);
            }
        };
        sources.push((file.clone(), src.clone()));

        let toks = lexer::lex(&src)
            .map_err(|e| e.with_file(file.clone()))
            .unwrap_or_else(|e| die(e, &sources));
        let prog = parser::parse(toks)
            .map_err(|e| e.with_file(file.clone()))
            .unwrap_or_else(|e| die(e, &sources));

        stmt_files.extend(std::iter::repeat(file.clone()).take(prog.body.len()));
        stmts.extend(prog.body);
        imports.extend(prog.imports);
        std_imports.extend(prog.std_imports);
    }

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports).unwrap_or_else(|e| die(e, &sources));
```

to:

```rust
    let resolved = resolve::resolve(&parsed.file).unwrap_or_else(|e| match e.kind {
        resolve::ResolveErrorKind::Compile(err) => die(err, &e.sources),
        resolve::ResolveErrorKind::Cycle(msg) => {
            eprintln!("error: {msg}");
            exit(1);
        }
        resolve::ResolveErrorKind::Io { path, message } => {
            eprintln!("error: cannot read {path}: {message}");
            exit(1);
        }
    });
    let sources = resolved.sources;
    let stmts = resolved.stmts;
    let stmt_files = resolved.stmt_files;
    let imports = resolved.imports;
    let std_imports = resolved.std_imports;

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports).unwrap_or_else(|e| die(e, &sources));
```

Update the `use` block at the top of `src/main.rs` — `lexer`/`parser` are no longer called directly in `main.rs` after this change (`resolve` calls them internally), so drop those two and add `resolve`:

```rust
use verb::codegen;
use verb::error::CompileError;
use verb::resolve;
use verb::targets;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --bin verb`
Expected: PASS — all `main.rs` unit tests.

- [ ] **Step 5: Full build + lib/bin test pass**

Run: `cargo build && cargo test --lib && cargo test --bin verb`
Expected: PASS, no errors. (`cargo test --test e2e` is expected to have failures at this point — Task 5 updates the e2e fixtures/tests for the new single-file CLI. Don't run it yet.)

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): single entry file; resolve .verb imports via the new resolve module"
```

---

### Task 5: e2e fixtures and tests

**Files:**
- Modify: `tests/fixtures/multifile_a.verb`, `tests/fixtures/multifile_b.verb`, `tests/fixtures/multifile_err_a.verb`, `tests/fixtures/multifile_err_b.verb`
- Create: `tests/fixtures/diamond_shared.verb`, `tests/fixtures/diamond_a.verb`, `tests/fixtures/diamond_b.verb`, `tests/fixtures/diamond_entry.verb`, `tests/fixtures/diamond.expected`, `tests/fixtures/cycle_a.verb`, `tests/fixtures/cycle_b.verb`
- Modify: `tests/e2e.rs:410-494` (`run_ok_multi` and the 4 multi-file tests), `tests/e2e.rs` test list (add new tests)

**Interfaces:**
- Consumes: the `verb` binary (`env!("CARGO_BIN_EXE_verb")`) built by Tasks 1-4.
- Produces: nothing consumed elsewhere — this is the outermost, user-facing test layer.

- [ ] **Step 1: Rewrite the multifile fixtures to use `import mod`**

`tests/fixtures/multifile_a.verb` (the library — unchanged, it already has no imports):

```
%% library file: helper function only, no top-level executable code
make double(x) begin
  return x times 2;
end
```

`tests/fixtures/multifile_b.verb` (the entry file — add the import):

```
%% entry file: uses the function defined in multifile_a.verb
import mod multifile_a.verb;

print(double(21));
assign total 0;
loop assign i 1; i atmost 3; i be i add 1 begin
  total be total add i;
end
print(total);
```

`tests/fixtures/multifile_err_a.verb` (the library — unchanged):

```
%% first file: valid, no errors
assign x 1;
```

`tests/fixtures/multifile_err_b.verb` (the entry file — add the import; the error stays in this file, which is exactly what the existing test needs to assert):

```
%% second file: imports a valid library, but this file itself
%% references an undefined variable
import mod multifile_err_a.verb;

assign y 2;
print(zz);
```

- [ ] **Step 2: Rewrite the 4 tests that used multi-file CLI args**

In `tests/e2e.rs`, replace `run_ok_multi` and the 4 tests that follow it (`multi_file_links_and_runs` through `multi_file_build_path_accepts_multiple_files`):

```rust
fn run_ok_multi(names: &[&str], expected_name: &str) {
    let files: Vec<String> = names
        .iter()
        .map(|n| format!("tests/fixtures/{n}.verb"))
        .collect();
    let mut args = vec!["run".to_string()];
    args.extend(files);
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(&args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(format!("tests/fixtures/{expected_name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn multi_file_links_and_runs() {
    run_ok_multi(&["multifile_a", "multifile_b"], "multifile");
}

#[test]
fn multi_file_emits_single_merged_llvm_module() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/multifile_a.verb",
            "tests/fixtures/multifile_b.verb",
            "--emit-llvm",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}

#[test]
fn multi_file_error_names_the_correct_file() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "run",
            "tests/fixtures/multifile_err_a.verb",
            "tests/fixtures/multifile_err_b.verb",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("multifile_err_b.verb"),
        "expected error attributed to multifile_err_b.verb, got: {stderr}"
    );
    assert!(
        !stderr.contains("multifile_err_a.verb"),
        "error should not be attributed to multifile_err_a.verb, got: {stderr}"
    );
    assert!(stderr.contains("undefined variable 'zz'"), "stderr: {stderr}");
}

#[test]
fn multi_file_build_path_accepts_multiple_files() {
    let dir = std::env::temp_dir().join("verb_multifile_build_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("multifile_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/multifile_a.verb",
            "tests/fixtures/multifile_b.verb",
            "-o",
            bin.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}
```

with:

```rust
#[test]
fn verb_file_import_links_and_runs() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_b.verb"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn verb_file_import_emits_a_single_merged_llvm_module() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_b.verb", "--emit-llvm"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}

#[test]
fn verb_file_import_error_names_the_importing_file_not_the_imported_one() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_err_b.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("multifile_err_b.verb"),
        "expected error attributed to multifile_err_b.verb, got: {stderr}"
    );
    assert!(
        !stderr.contains("multifile_err_a.verb"),
        "error should not be attributed to multifile_err_a.verb, got: {stderr}"
    );
    assert!(stderr.contains("undefined variable 'zz'"), "stderr: {stderr}");
}

#[test]
fn verb_file_import_build_path_links_and_runs() {
    let dir = std::env::temp_dir().join("verb_import_build_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("verb_file_import_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/multifile_b.verb", "-o", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/multifile.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

#[test]
fn cli_rejects_more_than_one_entry_file() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/multifile_a.verb", "tests/fixtures/multifile_b.verb"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage:"), "stderr: {stderr}");
}

#[test]
fn verb_file_import_dedups_a_diamond_dependency() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/diamond_entry.verb"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/diamond.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn verb_file_import_cycle_is_a_compile_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/cycle_a.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("import cycle"), "stderr: {stderr}");
}

#[test]
fn verb_file_import_missing_file_is_a_clear_error() {
    let dir = std::env::temp_dir().join("verb_missing_import_test");
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("entry.verb");
    std::fs::write(&entry, "import mod nope.verb;\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", entry.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope.verb"), "stderr: {stderr}");
}
```

- [ ] **Step 3: Add the diamond-import fixtures**

`tests/fixtures/diamond_shared.verb`:

```
print("shared");
```

`tests/fixtures/diamond_a.verb`:

```
import mod diamond_shared.verb;

print("a");
```

`tests/fixtures/diamond_b.verb`:

```
import mod diamond_shared.verb;

print("b");
```

`tests/fixtures/diamond_entry.verb`:

```
import mod diamond_a.verb;
import mod diamond_b.verb;

print("entry");
```

`tests/fixtures/diamond.expected` (traversal order: `diamond_a`'s import of `diamond_shared` resolves first and runs `diamond_shared`'s body, then `diamond_a`'s own body; `diamond_b`'s import of `diamond_shared` is a no-op since it's already `done`, then `diamond_b`'s own body; then `diamond_entry`'s own body):

```
shared
a
b
entry
```

- [ ] **Step 4: Add the cycle fixtures**

`tests/fixtures/cycle_a.verb`:

```
import mod cycle_b.verb;
```

`tests/fixtures/cycle_b.verb`:

```
import mod cycle_a.verb;
```

- [ ] **Step 5: Run the e2e suite**

Run: `cargo test --test e2e`
Expected: PASS — every test, including the 8 rewritten/new import tests. (Some pre-existing e2e tests are skip-if-no-`zig`; that's unrelated to this change and unaffected.)

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/multifile_a.verb tests/fixtures/multifile_b.verb \
        tests/fixtures/multifile_err_a.verb tests/fixtures/multifile_err_b.verb \
        tests/fixtures/diamond_shared.verb tests/fixtures/diamond_a.verb \
        tests/fixtures/diamond_b.verb tests/fixtures/diamond_entry.verb \
        tests/fixtures/diamond.expected tests/fixtures/cycle_a.verb \
        tests/fixtures/cycle_b.verb tests/e2e.rs
git commit -m "test(e2e): cover verb-file import — link/run, diamond dedup, cycle, missing file, single-entry CLI"
```

---

### Task 6: README

**Files:**
- Modify: `README.md:64-89` (append a new section after "Importing C++ libraries", before "Standard library I/O")

**Interfaces:** none — documentation only.

- [ ] **Step 1: Add the new section**

In `README.md`, right after this existing paragraph (ends the "Importing C++ libraries" section):

```
- `verb run` (JIT) does not support imports — programs using `import mod`
  must be built with `verb build`/`compile`, not run.

See `docs/superpowers/specs/2026-07-20-cpp-import-design.md` for the full
design.
```

insert a new section:

```markdown

## Importing other Verb files

`import mod` also pulls in another Verb source file, not just a C++
library — the CLI itself now only ever takes a single entry file, so
multi-file programs are built entirely through this:

    %% utils.verb
    make double(x) begin
      return x times 2;
    end

    %% main.verb
    import mod utils.verb;

    print(double(21));

- Disambiguation is purely by name: `import mod <name>;` (no `.verb`
  suffix) is a C++ library; `import mod <name>.verb;` is a Verb source
  file.
- The path is a bare filename (no `/`, no subdirectories in v1), resolved
  relative to the directory of the file doing the importing — not the
  current working directory.
- Imports are recursive (an imported file can `import mod` further files)
  and deduplicated (the same file imported from two places is only
  included once). A file that imports itself, directly or transitively,
  is a compile error.
- Everything imported lands in one flat global scope, same as if it had
  all been written in one file — there's no `utils.helper()`-style
  qualified access in v1.

See `docs/superpowers/specs/2026-07-21-verb-file-import-design.md` for the
full design.
```

- [ ] **Step 2: Sanity-check the doc example actually compiles**

Run:
```bash
mkdir -p /tmp/verb_readme_check && cd /tmp/verb_readme_check
cat > utils.verb <<'EOF'
make double(x) begin
  return x times 2;
end
EOF
cat > main.verb <<'EOF'
import mod utils.verb;

print(double(21));
EOF
```
then, from the repo root: `cargo run -- run /tmp/verb_readme_check/main.verb`
Expected output: `42`

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document import mod <name>.verb for multi-file Verb programs"
```

---

### Task 7: Full verification pass

**Files:** none — verification only.

- [ ] **Step 1: Run the complete test suite**

Run: `cargo test`
Expected: PASS — every lib, bin, and e2e test (cross-target tests skip cleanly if `zig` isn't on `PATH`, same as before this change).

- [ ] **Step 2: Release build sanity check**

Run: `cargo build --release`
Expected: builds cleanly, no warnings about unused imports left over from Task 4's `main.rs` changes.

- [ ] **Step 3: Confirm the example from the design spec works end-to-end via the installed-style binary**

Run:
```bash
./target/release/verb run /tmp/verb_readme_check/main.verb
```
Expected output: `42`

- [ ] **Step 4: Clean up the scratch files from Task 6/Task 7**

Run: `rm -rf /tmp/verb_readme_check`

No commit for this task — it's verification only, nothing changes in the tree.
