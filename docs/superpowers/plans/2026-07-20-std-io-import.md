# `import std io` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Verb programs read stdin, read/write whole files, and use blocking TCP sockets via `import std io;`, with Verb shipping and auto-linking the C++ wrapper itself (no user-authored `extern "C"` shim required, unlike the existing generic `import mod` mechanism).

**Architecture:** New `std` keyword parsed into a second `Program.std_imports: Vec<String>` list (parallel to the existing `imports: Vec<String>` used by `import mod`). Codegen recognizes a fixed table of `io`-module function names with known arities and declares/calls them the same way generic externs are called (same `VerbValue` C-ABI), but with compile-time arity checking against the known table instead of only against a prior call site. `verb build` compiles a new bundled file, `runtime/verb_std_io.cpp`, into an object and links it in whenever `import std io;` is used; `verb run` (JIT) rejects it the same way it already rejects `import mod`.

**Tech Stack:** Rust (existing `verb` crate: lexer/parser/codegen using `inkwell`/LLVM), C++17 (`runtime/verb_std_io.cpp`, POSIX sockets/stdio), same `c++`/`zig c++` toolchain the existing `import mod` feature already uses for linking.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-20-std-io-import-design.md` — follow it exactly; do not add modules beyond `io`, do not add streaming/handle-based file I/O, do not add non-blocking sockets or UDP (all explicitly out of scope for v1).
- Error convention: every `io` function returns `verb_nil()` on failure, never a C++ exception across the FFI boundary.
- File/socket handles reuse the existing `VERB_INT` tag — no new `VerbValue` tag.
- `import std io;` must be usable together with `import mod <lib>;` in the same file without interference.
- Every new Rust test must actually run as part of `cargo test` (no `#[ignore]`) except where it depends on `zig` being installed, matching the existing pattern in `tests/e2e.rs` (`zig_available()` guard + early return with an `eprintln!` skip notice).

---

### Task 1: `std` keyword in lexer + formatter

**Files:**
- Modify: `src/lexer.rs` (keyword enum ~line 7-8, keyword match ~line 164, tests module ~line 257-263)
- Modify: `src/formatter.rs` (token-to-string match ~line 200-201)
- Test: `src/lexer.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `TokenKind::Std` variant, usable by Task 2's parser.

- [ ] **Step 1: Write the failing test**

In `src/lexer.rs`, inside `mod tests`, add next to `scans_import_keywords`:

```rust
    #[test]
    fn scans_std_import_keyword() {
        use TokenKind::*;
        assert_eq!(
            kinds("import std io;"),
            vec![Import, Std, Ident("io".into()), Semi, Eof]
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib scans_std_import_keyword`
Expected: FAIL to compile — `error[E0599]: no variant or associated item named `Std` found for enum `TokenKind``

- [ ] **Step 3: Add the `Std` token**

In `src/lexer.rs`, change line 7-8:

```rust
    Import, Mod,
```

to:

```rust
    Import, Mod, Std,
```

Then in the keyword match (~line 164), change:

```rust
                    "import" => Import, "mod" => Mod,
```

to:

```rust
                    "import" => Import, "mod" => Mod, "std" => Std,
```

In `src/formatter.rs`, change (~line 200-201):

```rust
        Import => "import".into(),
        Mod => "mod".into(),
```

to:

```rust
        Import => "import".into(),
        Mod => "mod".into(),
        Std => "std".into(),
```

(This match is exhaustive over `TokenKind` — the compiler will refuse to build until this arm exists, so skipping it isn't possible.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib scans_std_import_keyword`
Expected: PASS

- [ ] **Step 5: Run the full lexer/formatter test suites to check nothing else broke**

Run: `cargo test --lib lexer:: formatter::`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add src/lexer.rs src/formatter.rs
git commit -m "$(cat <<'EOF'
feat: add 'std' keyword token for import std io

First piece of import std io — reserves the keyword in the lexer and
keeps the formatter's exhaustive token match in sync.
EOF
)"
```

---

### Task 2: Parser grammar for `import std <ident>;`

**Files:**
- Modify: `src/ast.rs` (`Program` struct, ~line 34-37)
- Modify: `src/parser.rs` (`parse`, `parse_recovering`, `Parser::imports`, `Parser::import_stmt`, ~lines 1-134)
- Modify: `src/codegen.rs` (test call sites that construct `compile_program(...)` with a bare `&[]` for imports — **only** to keep the crate compiling after `Program` gains a field; the 4th `std_imports` param itself is Task 3's job, so for now just make sure nothing here needs touching — verify in Step 2, don't blind-edit)
- Modify: `src/bin/verb-lsp.rs` (`compute_diagnostics`, ~line 239 — uses `program.imports`, needs no change yet since `Program.std_imports` merely adds a field; verify in Step 2)
- Test: `src/parser.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `Program.std_imports: Vec<String>` (deduplicated, source order). `Parser::import_stmt` returns a new private enum `ImportStmt { Mod(String), Std(String) }` instead of `String`.
- Consumes: `TokenKind::Std` from Task 1.

- [ ] **Step 1: Write the failing tests**

In `src/parser.rs`, inside `mod tests`, add next to the existing import tests (~after `program_with_no_imports_has_empty_imports_vec`):

```rust
    #[test]
    fn parses_std_io_import() {
        let p = parse(lex("import std io;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["io".to_string()]);
        assert!(p.imports.is_empty());
    }

    #[test]
    fn dedups_repeated_std_import() {
        let p = parse(lex("import std io; import std io;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["io".to_string()]);
    }

    #[test]
    fn std_and_mod_imports_coexist() {
        let p = parse(lex("import mod mathlib; import std io; print(1);").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert_eq!(p.std_imports, vec!["io".to_string()]);
        assert_eq!(p.body.len(), 1);
    }

    #[test]
    fn unknown_std_module_is_a_compile_error() {
        let err = parse(lex("import std vector;").unwrap()).unwrap_err();
        assert!(err.msg.contains("unknown std module 'vector'"), "{}", err.msg);
        assert!(err.msg.contains("io"), "{}", err.msg);
    }

    #[test]
    fn std_import_after_a_statement_is_a_compile_error() {
        let err = parse(lex("print(1); import std io;").unwrap()).unwrap_err();
        assert!(err.msg.contains("must appear before"), "{}", err.msg);
    }

    #[test]
    fn recovering_collects_std_imports_too() {
        let src = "import std io; print(1);";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(prog.std_imports, vec!["io".to_string()]);
        assert_eq!(prog.body.len(), 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib parser::`
Expected: FAIL to compile — `error[E0609]: no field `std_imports` on type `Program``

Also confirm (read-only, no edit) that `src/codegen.rs`'s two `Program`-unrelated `compile_program(&stmts, &stmt_files, &[])` calls and `src/bin/verb-lsp.rs`'s `cg.compile_program(&program.body, &stmt_files, &program.imports)` still reference only `.imports`/positional args that exist today — they do, so this task doesn't need to touch either file. (Task 3 will change `compile_program`'s signature and fix all four call sites at once.)

- [ ] **Step 3: Add `std_imports` to the AST**

In `src/ast.rs`, change:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub body: Vec<Stmt>,
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
    pub body: Vec<Stmt>,
}
```

- [ ] **Step 4: Rewrite import parsing in `src/parser.rs`**

Replace the `imports`/`import_stmt` methods (current lines ~121-135):

```rust
    fn imports(&mut self) -> Result<Vec<String>, CompileError> {
        let mut imports = Vec::new();
        while self.check(&TokenKind::Import) {
            let name = self.import_stmt()?;
            if !imports.contains(&name) { imports.push(name); }
        }
        Ok(imports)
    }

    fn import_stmt(&mut self) -> Result<String, CompileError> {
        self.advance(); // 'import'
        self.expect(&TokenKind::Mod, "'mod'")?;
        let (name, ..) = self.expect_ident("library name after 'mod'")?;
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(name)
    }
```

with:

```rust
    fn imports(&mut self) -> Result<(Vec<String>, Vec<String>), CompileError> {
        let mut imports = Vec::new();
        let mut std_imports = Vec::new();
        while self.check(&TokenKind::Import) {
            match self.import_stmt()? {
                ImportStmt::Mod(name) => { if !imports.contains(&name) { imports.push(name); } }
                ImportStmt::Std(name) => { if !std_imports.contains(&name) { std_imports.push(name); } }
            }
        }
        Ok((imports, std_imports))
    }

    fn import_stmt(&mut self) -> Result<ImportStmt, CompileError> {
        self.advance(); // 'import'
        if self.matches(&TokenKind::Mod) {
            let (name, ..) = self.expect_ident("library name after 'mod'")?;
            self.expect(&TokenKind::Semi, "';'")?;
            return Ok(ImportStmt::Mod(name));
        }
        self.expect(&TokenKind::Std, "'mod' or 'std'")?;
        let (name, l, c) = self.expect_ident("module name after 'std'")?;
        if name != "io" {
            return Err(CompileError::new(
                format!("unknown std module '{name}' (known std modules: io)"),
                l, c,
            ));
        }
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(ImportStmt::Std(name))
    }
```

Add the new private enum right above `struct Parser` (~line 71):

```rust
enum ImportStmt {
    Mod(String),
    Std(String),
}

struct Parser {
```

- [ ] **Step 5: Update `parse` to use the new tuple return and `Program` shape**

Change (current lines 4-16):

```rust
pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let imports = p.imports()?;
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            return Err(p.err("'import' must appear before any other statement"));
        }
        body.push(p.statement()?);
    }
    Ok(Program { imports, body })
}
```

to:

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

- [ ] **Step 6: Update `parse_recovering` the same way**

Change (current lines 27-40):

```rust
    let mut imports = Vec::new();
    let mut errors = Vec::new();
    while p.check(&TokenKind::Import) {
        match p.import_stmt() {
            Ok(name) => {
                if !imports.contains(&name) { imports.push(name); }
            }
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
```

to:

```rust
    let mut imports = Vec::new();
    let mut std_imports = Vec::new();
    let mut errors = Vec::new();
    while p.check(&TokenKind::Import) {
        match p.import_stmt() {
            Ok(ImportStmt::Mod(name)) => {
                if !imports.contains(&name) { imports.push(name); }
            }
            Ok(ImportStmt::Std(name)) => {
                if !std_imports.contains(&name) { std_imports.push(name); }
            }
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
```

And its final `Ok`/return (current line 68):

```rust
    (Program { imports, body }, errors)
```

to:

```rust
    (Program { imports, std_imports, body }, errors)
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test --lib parser::`
Expected: all PASS, including the 6 new tests and every pre-existing import test (`parses_single_import`, `parses_multiple_imports_and_dedups`, `import_before_body_is_fine`, `import_after_a_statement_is_a_compile_error`, `program_with_no_imports_has_empty_imports_vec`, `recovering_collects_imports_too`, `recovering_matches_parse_on_valid_input`).

- [ ] **Step 8: Run the whole crate's tests to check nothing else broke**

Run: `cargo test --lib`
Expected: all PASS (this also exercises `src/codegen.rs`'s and `src/bin/verb-lsp.rs`'s untouched call sites, confirming Step 2's read-only check was correct)

- [ ] **Step 9: Commit**

```bash
git add src/ast.rs src/parser.rs
git commit -m "$(cat <<'EOF'
feat: parse import std io

Program gains std_imports, parallel to the existing import mod imports
list. Unlike import mod library names, std module names are known
ahead of time, so an unrecognized one (anything but 'io' in v1) is a
compile error at parse time rather than a link-time surprise.
EOF
)"
```

---

### Task 3: Codegen — `io` function table + call resolution + arity checking

**Files:**
- Modify: `src/codegen.rs` (`Codegen` struct ~line 15-27, `new` ~line 30-53, `compile_program` ~line 638-639, `gen_call` ~line 947-966, add `gen_std_io_call` + `io_func_arity` near `gen_extern_call` ~line 998-1040)
- Modify: `src/main.rs` (`cg.compile_program(&stmts, &stmt_files, &imports)` call, ~line 138 — add 4th arg; full CLI plumbing of `std_imports` is Task 5's job, so here just pass `&[]` to keep it compiling)
- Modify: `src/bin/verb-lsp.rs` (`cg.compile_program(&program.body, &stmt_files, &program.imports)` call, ~line 239 — add `&program.std_imports`)
- Test: `src/codegen.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Program.std_imports` (Task 2).
- Produces: `Codegen::compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String], imports: &[String], std_imports: &[String]) -> Result<(), CompileError>` — new signature, 4 params instead of 3. Every call site in the crate must be updated in this task.

- [ ] **Step 1: Write the failing tests**

In `src/codegen.rs`, inside `mod tests`, add next to `no_error_when_program_is_valid`:

```rust
    #[test]
    fn std_io_call_with_correct_arity_compiles_ok() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::Assign {
            name: "line".to_string(),
            value: Expr::Call {
                callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
                args: vec![],
                line: 1, col: 1,
            },
        }];
        let stmt_files = vec!["a.verb".to_string()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["io".to_string()]).is_ok());
    }

    #[test]
    fn std_io_arity_mismatch_is_a_compile_error() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
            args: vec![Expr::Int(1)],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg
            .compile_program(&stmts, &stmt_files, &[], &["io".to_string()])
            .unwrap_err();
        assert!(err.msg.contains("read_line"), "{}", err.msg);
        assert!(err.msg.contains("takes 0 argument"), "{}", err.msg);
    }

    #[test]
    fn std_io_name_ignored_without_import_std_io() {
        // 'read_line' with no `import std io;` present falls through to the
        // ordinary undefined-variable path, same as any unknown name.
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("read_line".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
        assert!(err.msg.contains("undefined variable"), "{}", err.msg);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib codegen::`
Expected: FAIL to compile — `error[E0061]: this method takes 4 arguments but 3 arguments were supplied` (from the new tests) once the signature is changed in Step 3 below; before that, they fail because `compile_program` doesn't yet accept a 4th argument at all. Confirm the pre-change baseline compiles (`cargo test --lib codegen::` on current code, minus the 3 new tests, still passes) isn't necessary — go straight to Step 3.

- [ ] **Step 3: Add `std_imports` field and the `io` function table**

In `src/codegen.rs`, change the struct (current lines 15-27):

```rust
pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    value_ty: StructType<'ctx>,
    closure_ty: StructType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    functions: HashMap<String, (FunctionValue<'ctx>, usize)>,
    externs: HashMap<String, FunctionValue<'ctx>>,
    imports: Vec<String>,
    fn_depth: u32,
    fn_counter: u32,
    cur_file: String,
}
```

to:

```rust
pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    value_ty: StructType<'ctx>,
    closure_ty: StructType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    functions: HashMap<String, (FunctionValue<'ctx>, usize)>,
    externs: HashMap<String, FunctionValue<'ctx>>,
    imports: Vec<String>,
    std_imports: Vec<String>,
    fn_depth: u32,
    fn_counter: u32,
    cur_file: String,
}
```

In `new` (current line 41), change:

```rust
            imports: Vec::new(), fn_depth: 0, fn_counter: 0,
```

to:

```rust
            imports: Vec::new(), std_imports: Vec::new(), fn_depth: 0, fn_counter: 0,
```

Add the known `io` function table as a free function near `levenshtein` (current line ~1042, right after the closing `}` of `impl<'ctx> Codegen<'ctx>`):

```rust
/// Fixed name -> arity table for the `io` module's built-in functions
/// (see runtime/verb_std_io.cpp and the design spec). Unlike generic
/// `import mod` externs, these signatures are first-party and known
/// ahead of time, so arity is checked on every call site, not just
/// against a previous one.
const IO_FUNCS: &[(&str, usize)] = &[
    ("read_line", 0),
    ("file_read", 1),
    ("file_write", 2),
    ("file_append", 2),
    ("tcp_connect", 2),
    ("tcp_listen", 1),
    ("tcp_accept", 1),
    ("send_line", 2),
    ("recv_line", 1),
    ("close_conn", 1),
];

fn io_func_arity(name: &str) -> Option<usize> {
    IO_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}
```

- [ ] **Step 4: Thread `std_imports` through `compile_program`**

Change (current line 638-639):

```rust
    pub fn compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String], imports: &[String]) -> Result<(), CompileError> {
        self.imports = imports.to_vec();
```

to:

```rust
    pub fn compile_program(&mut self, stmts: &[Stmt], stmt_files: &[String], imports: &[String], std_imports: &[String]) -> Result<(), CompileError> {
        self.imports = imports.to_vec();
        self.std_imports = std_imports.to_vec();
```

- [ ] **Step 5: Add the resolution tier in `gen_call` and the `gen_std_io_call` helper**

In `gen_call` (current lines 947-966), change:

```rust
    fn gen_call(&mut self, callee: &Expr, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        // built-in print
        if let Expr::Var(name, ..) = callee {
            if name == "print" {
                if args.len() != 1 {
                    return Err(CompileError::new("print takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                return Ok(self.nil_val());
            }
            if !self.imports.is_empty()
                && self.lookup(name).is_none()
                && !self.functions.contains_key(name)
            {
                return self.gen_extern_call(name, args, line, col);
            }
        }
```

to:

```rust
    fn gen_call(&mut self, callee: &Expr, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        // built-in print
        if let Expr::Var(name, ..) = callee {
            if name == "print" {
                if args.len() != 1 {
                    return Err(CompileError::new("print takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                return Ok(self.nil_val());
            }
            let is_bound = self.lookup(name).is_some() || self.functions.contains_key(name);
            if !is_bound && self.std_imports.iter().any(|m| m == "io") {
                if let Some(arity) = io_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, line, col);
                }
            }
            if !is_bound && !self.imports.is_empty() {
                return self.gen_extern_call(name, args, line, col);
            }
        }
```

Add `gen_std_io_call` right above `gen_extern_call` (current line ~998):

```rust
    /// A call to one of the `io` module's built-in functions (see
    /// runtime/verb_std_io.cpp), reachable only when `import std io;` is
    /// present. Arity is checked against the function's fixed, known
    /// signature (`IO_FUNCS`) on every call site — including the first —
    /// unlike `gen_extern_call`, whose arity is only checked against a
    /// prior call site of the same name, because generic `import mod`
    /// externs have no statically known signature to check against.
    fn gen_std_io_call(&mut self, name: &str, expected_arity: usize, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        if args.len() != expected_arity {
            return Err(CompileError::new(
                format!(
                    "std io fn '{name}' takes {expected_arity} argument(s), got {}",
                    args.len()
                ),
                line, col,
            ));
        }
        let argvals: Vec<StructValue<'ctx>> =
            args.iter().map(|a| self.gen_expr(a)).collect::<Result<_, _>>()?;
        let fnv = match self.externs.get(name).copied() {
            Some(fnv) => fnv,
            None => {
                let param_tys: Vec<_> = (0..expected_arity).map(|_| self.value_ty.into()).collect();
                let fnty = self.value_ty.fn_type(&param_tys, false);
                let fnv = self.module.add_function(name, fnty, None);
                self.externs.insert(name.to_string(), fnv);
                fnv
            }
        };
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        Ok(self.builder.build_call(fnv, &args_bv, "std_io_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value())
    }

```

- [ ] **Step 6: Fix the pre-existing `compile_program` test call sites (3-arg -> 4-arg)**

In `src/codegen.rs`'s own `mod tests` (current lines ~1076 and ~1089), change both:

```rust
        let err = cg.compile_program(&stmts, &stmt_files, &[]).unwrap_err();
```
```rust
        assert!(cg.compile_program(&stmts, &stmt_files, &[]).is_ok());
```

to:

```rust
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
```
```rust
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &[]).is_ok());
```

- [ ] **Step 7: Fix `src/main.rs` and `src/bin/verb-lsp.rs` call sites**

In `src/main.rs` (current line 138):

```rust
    cg.compile_program(&stmts, &stmt_files, &imports).unwrap_or_else(|e| die(e, &sources));
```

to:

```rust
    cg.compile_program(&stmts, &stmt_files, &imports, &[]).unwrap_or_else(|e| die(e, &sources));
```

(Task 5 replaces this `&[]` with a real accumulated `std_imports` vector — for now this keeps the crate compiling without changing `verb`'s CLI behavior.)

In `src/bin/verb-lsp.rs` (current line 239):

```rust
    if let Err(e) = cg.compile_program(&program.body, &stmt_files, &program.imports) {
```

to:

```rust
    if let Err(e) = cg.compile_program(&program.body, &stmt_files, &program.imports, &program.std_imports) {
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --lib codegen::`
Expected: all PASS, including the 3 new tests.

- [ ] **Step 9: Run the whole crate's tests (lib + bins) to check nothing else broke**

Run: `cargo build && cargo test --lib`
Expected: builds clean (including `src/bin/verb-lsp.rs`), all lib tests PASS.

- [ ] **Step 10: Commit**

```bash
git add src/codegen.rs src/main.rs src/bin/verb-lsp.rs
git commit -m "$(cat <<'EOF'
feat: codegen support for import std io calls

Adds a fixed name->arity table for the io module and a new call
resolution tier ahead of the generic import mod extern path, with
arity checked against the known signature on every call site.
EOF
)"
```

---

### Task 4: `runtime/verb_std_io.cpp` implementation

**Files:**
- Create: `runtime/verb_std_io.cpp`
- Test: `tests/e2e.rs` (new standalone-compile test)

**Interfaces:**
- Consumes: `runtime/verb.h`'s `VerbValue`/`verb_nil`/`verb_bool`/`verb_int`/`verb_string`/`verb_as_int`/`verb_as_string` helpers (already exist, unchanged).
- Produces: the 10 `extern "C" VerbValue` functions Task 3's `IO_FUNCS` table names: `read_line`, `file_read`, `file_write`, `file_append`, `tcp_connect`, `tcp_listen`, `tcp_accept`, `send_line`, `recv_line`, `close_conn`. Task 5 compiles and links this file whenever a program uses `import std io;`.

- [ ] **Step 1: Write the failing test**

In `tests/e2e.rs`, add near `build_mathlib_fixture` (the existing pattern for compiling a `.cpp` fixture with `c++`):

```rust
#[test]
fn verb_std_io_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_std_io_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-c",
            "runtime/verb_std_io.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_std_io.cpp");
    assert!(status.success(), "runtime/verb_std_io.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test e2e verb_std_io_cpp_compiles_standalone`
Expected: FAIL — `c++: error: runtime/verb_std_io.cpp: No such file or directory`

- [ ] **Step 3: Write `runtime/verb_std_io.cpp`**

```cpp
// Built-in bindings for `import std io;` -- stdin, whole-file
// read/write, and blocking TCP sockets. Compiled and linked in
// automatically by `verb build`/`compile` whenever a program uses
// `import std io;`; unlike the generic `import mod` mechanism, the
// user never writes or links this file themselves.
//
// Every function returns verb_nil() on failure -- no C++ exception
// ever crosses the extern "C" boundary. File/socket handles reuse the
// existing VERB_INT tag (a POSIX fd is already an integer).
#include "verb.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#include <netdb.h>
#include <sys/socket.h>
#include <unistd.h>

extern "C" VerbValue read_line() {
    std::string line;
    int c = std::getchar();
    if (c == EOF) return verb_nil();
    while (c != EOF && c != '\n') {
        line.push_back(static_cast<char>(c));
        c = std::getchar();
    }
    char* out = static_cast<char*>(std::malloc(line.size() + 1));
    std::memcpy(out, line.data(), line.size());
    out[line.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue file_read(VerbValue path) {
    FILE* f = std::fopen(verb_as_string(path), "rb");
    if (!f) return verb_nil();
    std::fseek(f, 0, SEEK_END);
    long size = std::ftell(f);
    if (size < 0) { std::fclose(f); return verb_nil(); }
    std::fseek(f, 0, SEEK_SET);
    char* buf = static_cast<char*>(std::malloc(static_cast<size_t>(size) + 1));
    size_t got = std::fread(buf, 1, static_cast<size_t>(size), f);
    std::fclose(f);
    buf[got] = '\0';
    return verb_string(buf);
}

static VerbValue write_file(const char* path, const char* mode, VerbValue contents) {
    FILE* f = std::fopen(path, mode);
    if (!f) return verb_nil();
    const char* s = verb_as_string(contents);
    size_t len = std::strlen(s);
    size_t written = std::fwrite(s, 1, len, f);
    std::fclose(f);
    if (written != len) return verb_nil();
    return verb_bool(1);
}

extern "C" VerbValue file_write(VerbValue path, VerbValue contents) {
    return write_file(verb_as_string(path), "wb", contents);
}

extern "C" VerbValue file_append(VerbValue path, VerbValue contents) {
    return write_file(verb_as_string(path), "ab", contents);
}

extern "C" VerbValue tcp_connect(VerbValue host, VerbValue port) {
    std::string port_str = std::to_string(verb_as_int(port));
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* res = nullptr;
    if (getaddrinfo(verb_as_string(host), port_str.c_str(), &hints, &res) != 0) {
        return verb_nil();
    }
    int fd = -1;
    for (addrinfo* p = res; p != nullptr; p = p->ai_next) {
        fd = socket(p->ai_family, p->ai_socktype, p->ai_protocol);
        if (fd == -1) continue;
        if (connect(fd, p->ai_addr, p->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    if (fd == -1) return verb_nil();
    return verb_int(fd);
}

extern "C" VerbValue tcp_listen(VerbValue port) {
    std::string port_str = std::to_string(verb_as_int(port));
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;
    addrinfo* res = nullptr;
    if (getaddrinfo(nullptr, port_str.c_str(), &hints, &res) != 0) {
        return verb_nil();
    }
    int fd = -1;
    for (addrinfo* p = res; p != nullptr; p = p->ai_next) {
        fd = socket(p->ai_family, p->ai_socktype, p->ai_protocol);
        if (fd == -1) continue;
        int yes = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
        if (bind(fd, p->ai_addr, p->ai_addrlen) == 0) break;
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    if (fd == -1) return verb_nil();
    if (listen(fd, 16) != 0) {
        close(fd);
        return verb_nil();
    }
    return verb_int(fd);
}

extern "C" VerbValue tcp_accept(VerbValue fd) {
    int client = accept(static_cast<int>(verb_as_int(fd)), nullptr, nullptr);
    if (client == -1) return verb_nil();
    return verb_int(client);
}

extern "C" VerbValue send_line(VerbValue fd, VerbValue s) {
    std::string line = verb_as_string(s);
    line.push_back('\n');
    int sock = static_cast<int>(verb_as_int(fd));
    size_t sent_total = 0;
    while (sent_total < line.size()) {
        ssize_t n = send(sock, line.data() + sent_total, line.size() - sent_total, 0);
        if (n <= 0) return verb_nil();
        sent_total += static_cast<size_t>(n);
    }
    return verb_bool(1);
}

extern "C" VerbValue recv_line(VerbValue fd) {
    int sock = static_cast<int>(verb_as_int(fd));
    std::string line;
    char c;
    while (true) {
        ssize_t n = recv(sock, &c, 1, 0);
        if (n <= 0) {
            if (line.empty()) return verb_nil();
            break;
        }
        if (c == '\n') break;
        line.push_back(c);
    }
    char* out = static_cast<char*>(std::malloc(line.size() + 1));
    std::memcpy(out, line.data(), line.size());
    out[line.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue close_conn(VerbValue fd) {
    close(static_cast<int>(verb_as_int(fd)));
    return verb_nil();
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test e2e verb_std_io_cpp_compiles_standalone`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add runtime/verb_std_io.cpp tests/e2e.rs
git commit -m "$(cat <<'EOF'
feat: add runtime/verb_std_io.cpp (import std io backing implementation)

Implements the 10 functions codegen's IO_FUNCS table expects: stdin
read_line, whole-file read/write/append, and blocking TCP
connect/listen/accept/send_line/recv_line/close_conn. Every function
returns verb_nil() on failure per the design spec's error convention.
EOF
)"
```

---

### Task 5: CLI/build integration — link `verb_std_io.cpp`, JIT rejection, Windows cross-target guard

**Files:**
- Modify: `src/main.rs` (`main`, `build_aot_host`, `build_aot_cross`, `build_aot_all`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `Program.std_imports` (Task 2), `Codegen::compile_program`'s 4-arg signature (Task 3), `runtime/verb_std_io.cpp` (Task 4), `targets::Target::is_windows()` (already exists in `src/targets.rs`).
- Produces: `build_aot_host(cg, out, imports, std_imports, lib_dirs)`, `build_aot_cross(cg, out, target, imports, std_imports, lib_dirs) -> Result<(), String>`, `build_aot_all(cg, out, imports, std_imports, lib_dirs)` — all gain a `std_imports: &[String]` parameter. New free function `compile_std_io_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String>`.

- [ ] **Step 1: Write the failing tests**

In `tests/e2e.rs`, add:

```rust
// ----- std io -----

#[test]
fn run_rejects_programs_with_std_io_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_io_file_roundtrip.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std io"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_io_files() {
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_file_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_file_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_file_roundtrip.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file("verb_e2e_std_io_roundtrip.tmp");
}

#[test]
fn windows_cross_target_rejects_std_io_import() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_windows_reject");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_file_roundtrip.verb",
            "-o", out_path.to_str().unwrap(),
            "--target", "windows-x86_64",
        ])
        .output()
        .unwrap();
    assert!(!build.status.success());
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("not supported when cross-compiling to a Windows target"),
        "stderr: {stderr}"
    );
}
```

Add the fixture files these reference:

`tests/fixtures/std_io_file_roundtrip.verb`:

```
import std io;

assign path "verb_e2e_std_io_roundtrip.tmp";
file_write(path, "hello");
file_append(path, " world");
print(file_read(path));
```

`tests/fixtures/std_io_file_roundtrip.expected`:

```
hello world
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e run_rejects_programs_with_std_io_import build_links_and_runs_a_program_using_std_io_files windows_cross_target_rejects_std_io_import`
Expected: FAIL — `run_rejects_programs_with_std_io_import` fails because `import std io;` currently JIT-runs instead of being rejected (main.rs doesn't know about `std_imports` yet); `build_links_and_runs_a_program_using_std_io_files` fails at build/link (undefined symbols `file_write`/`file_append`/`file_read`, since nothing links `verb_std_io.cpp` yet); `windows_cross_target_rejects_std_io_import` fails because there's no guard yet (it'll either "succeed" at building a broken binary or fail with an unrelated link error, not the expected message).

- [ ] **Step 3: Add the `compile_std_io_obj` helper to `src/main.rs`**

Add near the top of `src/main.rs`, after the `use` block:

```rust
use std::path::PathBuf;

/// Compiles the bundled `runtime/verb_std_io.cpp` into an object file with
/// `compiler` (`"cc"`/`"c++"` for the host, `"zig"` for cross targets),
/// prepending `extra_args` (e.g. `["c++", "-target", triple]` for zig).
/// Returns the object file's path on success.
fn compile_std_io_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_std_io_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-Iruntime", "-c", "runtime/verb_std_io.cpp", "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile runtime/verb_std_io.cpp: {e}"))?;
    if !status.success() {
        return Err("failed to compile runtime/verb_std_io.cpp".to_string());
    }
    Ok(obj)
}
```

- [ ] **Step 4: Accumulate `std_imports` in `main` and use it for the `run` rejection + `compile_program` call**

Change (current lines 118-121):

```rust
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut stmts = Vec::new();
    let mut stmt_files = Vec::new();
    let mut imports: Vec<String> = Vec::new();
```

to:

```rust
    let mut sources: Vec<(String, String)> = Vec::new();
    let mut stmts = Vec::new();
    let mut stmt_files = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut std_imports: Vec<String> = Vec::new();
```

Change (current line 137):

```rust
        imports.extend(prog.imports);
```

to:

```rust
        imports.extend(prog.imports);
        std_imports.extend(prog.std_imports);
```

Change (current line 138):

```rust
    cg.compile_program(&stmts, &stmt_files, &imports).unwrap_or_else(|e| die(e, &sources));
```

to:

```rust
    cg.compile_program(&stmts, &stmt_files, &imports, &std_imports).unwrap_or_else(|e| die(e, &sources));
```

Change the `"run"` branch's rejection (current lines 145-152):

```rust
        "run" => {
            if !imports.is_empty() {
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    imports.join(", ")
                );
                exit(1);
            }
```

to:

```rust
        "run" => {
            if !imports.is_empty() || !std_imports.is_empty() {
                let mut names = imports.clone();
                names.extend(std_imports.iter().map(|m| format!("std {m}")));
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    names.join(", ")
                );
                exit(1);
            }
```

Change the `"build" | "compile"` branch (current lines 165-179) to pass `&std_imports` through:

```rust
        "build" | "compile" => {
            let out = parsed.out.unwrap_or_else(|| usage());
            match parsed.target.as_deref() {
                None => build_aot_host(&cg, &out, &imports, &parsed.lib_dirs),
                Some("all") => build_aot_all(&cg, &out, &imports, &parsed.lib_dirs),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
                    check_zig_available();
                    if let Err(e) = build_aot_cross(&cg, &out, &target, &imports, &parsed.lib_dirs) {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            }
        }
```

to:

```rust
        "build" | "compile" => {
            let out = parsed.out.unwrap_or_else(|| usage());
            match parsed.target.as_deref() {
                None => build_aot_host(&cg, &out, &imports, &std_imports, &parsed.lib_dirs),
                Some("all") => build_aot_all(&cg, &out, &imports, &std_imports, &parsed.lib_dirs),
                Some(t) => {
                    let target = targets::Target::parse(t).unwrap_or_else(|e| {
                        eprintln!("error: {e}");
                        exit(2);
                    });
                    check_zig_available();
                    if let Err(e) = build_aot_cross(&cg, &out, &target, &imports, &std_imports, &parsed.lib_dirs) {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
            }
        }
```

- [ ] **Step 5: Update `build_aot_host` to compile+link `verb_std_io.cpp`**

Change the function signature and body (current, full function):

```rust
fn build_aot_host(cg: &codegen::Codegen, out: &str, imports: &[String], lib_dirs: &[String]) {
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
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let linker = if imports.is_empty() { "cc" } else { "c++" };
    let mut cmd = Command::new(linker);
    cmd.arg(&obj).arg("-o").arg(out);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = match cmd.status() {
        Ok(status) => status,
        Err(e) => {
            let _ = std::fs::remove_file(&obj);
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}
```

to:

```rust
fn build_aot_host(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String]) {
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
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let linker = if imports.is_empty() && !wants_std_io { "cc" } else { "c++" };

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };

    let mut cmd = Command::new(linker);
    cmd.arg(&obj).arg("-o").arg(out);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = match cmd.status() {
        Ok(status) => status,
        Err(e) => {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}
```

- [ ] **Step 6: Update `build_aot_cross` to add the Windows guard and compile+link `verb_std_io.cpp`**

Change the function signature and body (current, full function):

```rust
fn build_aot_cross(
    cg: &codegen::Codegen,
    out: &str,
    target: &targets::Target,
    imports: &[String],
    lib_dirs: &[String],
) -> Result<(), String> {
    use inkwell::targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetTriple,
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
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    // Imports/lib_dirs are forwarded to zig cc so cross-linking works when the imported
    // C++ libraries are available for the chosen target via -L<dir>. Host-built .o/.a
    // fixtures won't link for a foreign target -- that requires target-built libraries.
    let mut cmd = Command::new("zig");
    cmd.args(["cc", "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()]);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}
```

to:

```rust
fn build_aot_cross(
    cg: &codegen::Codegen,
    out: &str,
    target: &targets::Target,
    imports: &[String],
    std_imports: &[String],
    lib_dirs: &[String],
) -> Result<(), String> {
    use inkwell::targets::{
        CodeModel, FileType, InitializationConfig, RelocMode, Target as LlvmTarget, TargetTriple,
    };

    let wants_std_io = std_imports.iter().any(|m| m == "io");
    if wants_std_io && target.is_windows() {
        return Err(
            "'import std io' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std io'".to_string(),
        );
    }

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
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let out = target.adjust_output(out);
    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .map_err(|e| format!("object emit error: {e}"))?;

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };

    // Imports/lib_dirs are forwarded to zig cc so cross-linking works when the imported
    // C++ libraries are available for the chosen target via -L<dir>. Host-built .o/.a
    // fixtures won't link for a foreign target -- that requires target-built libraries.
    let mut cmd = Command::new("zig");
    cmd.args(["cc", "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()]);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().map_err(|e| format!("zig failed to start: {e}"))?;
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
}
```

- [ ] **Step 7: Update `build_aot_all` to pass `std_imports` through**

Change (current, full function):

```rust
fn build_aot_all(cg: &codegen::Codegen, out: &str, imports: &[String], lib_dirs: &[String]) {
    check_zig_available();
    let mut failures = 0;
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for target in targets::ALL {
        let labeled_out = format!("{out}-{}", target.label());
        let res = build_aot_cross(cg, &labeled_out, &target, imports, lib_dirs);
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

to:

```rust
fn build_aot_all(cg: &codegen::Codegen, out: &str, imports: &[String], std_imports: &[String], lib_dirs: &[String]) {
    check_zig_available();
    let mut failures = 0;
    let mut results: Vec<(String, Result<(), String>)> = Vec::new();
    for target in targets::ALL {
        let labeled_out = format!("{out}-{}", target.label());
        let res = build_aot_cross(cg, &labeled_out, &target, imports, std_imports, lib_dirs);
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

(`--target all` needs no special-case Windows handling here: the guard inside `build_aot_cross` already returns `Err` for the two Windows targets whenever `std_imports` wants `io`, and this function's existing best-effort/summary logic already treats any `Err` as a counted failure.)

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --test e2e run_rejects_programs_with_std_io_import build_links_and_runs_a_program_using_std_io_files windows_cross_target_rejects_std_io_import`
Expected: all PASS

- [ ] **Step 9: Run the full test suite to check nothing else broke**

Run: `cargo test`
Expected: all PASS (this re-runs every existing `import mod`/AOT/cross-target test with the now-5-and-6-arg `build_aot_*` signatures)

- [ ] **Step 10: Commit**

```bash
git add src/main.rs tests/e2e.rs tests/fixtures/std_io_file_roundtrip.verb tests/fixtures/std_io_file_roundtrip.expected
git commit -m "$(cat <<'EOF'
feat: link runtime/verb_std_io.cpp into builds using import std io

verb build/compile now compiles and links the bundled io backing file
whenever a program uses import std io, for both host and (non-Windows)
cross targets; verb run rejects std imports the same way it already
rejects import mod. Windows cross targets reject import std io
explicitly rather than failing with a confusing link error, since the
implementation uses POSIX socket APIs the mingw cross toolchain
doesn't provide.
EOF
)"
```

---

### Task 6: TCP loopback e2e test + README documentation

**Files:**
- Create: `tests/fixtures/std_io_tcp_loopback.verb`
- Create: `tests/fixtures/std_io_tcp_loopback.expected`
- Modify: `tests/e2e.rs` (new test)
- Modify: `README.md` (new "Standard library I/O" section)

**Interfaces:**
- Consumes: everything from Tasks 1-5 (this task is pure verification + docs, no new production code).

- [ ] **Step 1: Write the fixture files**

`tests/fixtures/std_io_tcp_loopback.verb`:

```
import std io;

assign listener tcp_listen(58712);
assign client tcp_connect("127.0.0.1", 58712);
assign server tcp_accept(listener);
send_line(client, "ping");
print(recv_line(server));
send_line(server, "pong");
print(recv_line(client));
close_conn(client);
close_conn(server);
close_conn(listener);
```

`tests/fixtures/std_io_tcp_loopback.expected`:

```
ping
pong
```

- [ ] **Step 2: Write the failing test**

In `tests/e2e.rs`, add next to `build_links_and_runs_a_program_using_std_io_files`:

```rust
#[test]
fn build_links_and_runs_a_program_using_std_io_tcp_loopback() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_io_tcp_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_io_tcp_loopback.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_io_tcp_loopback.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test e2e build_links_and_runs_a_program_using_std_io_tcp_loopback`
Expected: FAIL — `assertion failed: build.status.success()`, "No such file or directory" for the fixture, since it doesn't exist until Step 1... wait, Step 1 already created it. Re-check: this step should actually PASS if Tasks 1-5 are correctly implemented, since all the production code already exists. Run it anyway to confirm — if it unexpectedly fails, that's a real bug in an earlier task to fix before continuing, not something to paper over here.

- [ ] **Step 4: Run test again / debug if needed**

Run: `cargo test --test e2e build_links_and_runs_a_program_using_std_io_tcp_loopback -- --nocapture`
Expected: PASS. If it fails, inspect stderr from the `build`/`run` output the test prints and fix the root cause in `runtime/verb_std_io.cpp` or `src/main.rs` (do not weaken the test).

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: all PASS

- [ ] **Step 6: Document the feature in README.md**

Add a new section to `README.md` right after the existing "## Importing C++ libraries" section (which ends with the `See docs/superpowers/specs/2026-07-20-cpp-import-design.md...` line):

```markdown
## Standard library I/O (`import std io`)

Unlike `import mod`, which requires writing your own `extern "C"`
wrapper, `import std io;` gives Verb programs a small set of built-in
functions for stdin, whole-file read/write, and blocking TCP sockets —
Verb compiles and links the C++ implementation itself.

    import std io;

    assign contents file_read("notes.txt");
    print(contents);

Available functions: `read_line()`, `file_read(path)`,
`file_write(path, contents)`, `file_append(path, contents)`,
`tcp_connect(host, port)`, `tcp_listen(port)`, `tcp_accept(fd)`,
`send_line(fd, s)`, `recv_line(fd)`, `close_conn(fd)`. Every function
returns `nil` on failure — check with `check x eq nil`.

- Only the `io` module exists in v1 (`import std io;`); an unrecognized
  module name after `std` is a compile error.
- Like `import mod`, `import std io;` must appear before any other
  top-level statement, and `verb run` (JIT) does not support it — use
  `verb build`/`compile`.
- Cross-compiling to a Windows target (`--target windows-x86_64` /
  `windows-arm64`) with `import std io;` is not supported in v1 — the
  implementation uses POSIX socket APIs unavailable under the mingw
  cross toolchain.

See `docs/superpowers/specs/2026-07-20-std-io-import-design.md` for
the full design.
```

- [ ] **Step 7: Commit**

```bash
git add tests/fixtures/std_io_tcp_loopback.verb tests/fixtures/std_io_tcp_loopback.expected tests/e2e.rs README.md
git commit -m "$(cat <<'EOF'
test: add TCP loopback e2e coverage for import std io; docs: README section

Closes out the design spec's testing plan (file roundtrip + TCP
loopback e2e both covered) and documents import std io in README
alongside the existing import mod section.
EOF
)"
```

---

## Final verification

- [ ] Run `cargo test` once more from a clean state and confirm every test in the crate passes.
- [ ] Run `cargo build --release` to confirm the release profile also builds clean.
- [ ] Manually run `./target/release/verb build tests/fixtures/std_io_file_roundtrip.verb -o /tmp/verb_manual_check && /tmp/verb_manual_check` and confirm it prints `hello world`.
