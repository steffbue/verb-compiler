# C++ Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Verb programs `import mod <lib>;` a native C++ library and call its `extern "C"` functions directly, by reusing Verb's own tagged `{i8,i64}` value struct as the C-ABI boundary type — no per-function type declarations, no marshalling code.

**Architecture:** `import mod <ident>;` is a new top-level-only statement, parsed into a `Program { imports: Vec<String>, body: Vec<Stmt> }` (replacing the bare `Vec<Stmt>` `parse` used to return). Any call to an unresolved name, in a program that has at least one import, is codegen'd as a direct call to a lazily-declared external LLVM function with signature `VerbValue(VerbValue, VerbValue, ...)` — the exact same struct Verb's own runtime helpers already pass by value, so no unboxing/reboxing is needed at the boundary. `verb build` (currently an unimplemented stub) is implemented for real, since extern symbols only resolve at link time; `verb run` (JIT) rejects any program with imports.

**Tech Stack:** Rust 2021, inkwell 0.9 (`llvm20-1`), LLVM 20.1.3 (Homebrew, keg-only), `c++`/`cc` (system compiler driver) for final link, C++17 for the test fixture library.

**Spec:** `docs/superpowers/specs/2026-07-20-cpp-import-design.md` — read it first.

## Global Constraints

- Import syntax: `import mod <ident>;` — bare identifier (no quotes), top-level only, must appear before any other top-level statement, repeatable, deduplicated in source order.
- ABI: an extern call passes/returns `%verb.value = { i8, i64 }` directly — byte-identical to a non-packed C/C++ `struct { int8_t tag; int64_t payload; }`. No per-function signatures, no type annotations, no marshalling.
- Call resolution: a call to an unresolved name is treated as extern **iff** `Program.imports` is non-empty; arity is fixed by the first call site seen during codegen — a later call to the same name with a different arg count is a compile error, not a linker error.
- `verb run` (JIT) rejects any program with imports (clear compile-time-style error, before touching the JIT engine). `verb build` (AOT) is where imports actually work.
- `verb build` links with `c++` when `imports` is non-empty, else `cc` (unchanged for import-free programs). New repeatable CLI flag: `-L<dir>`, passed straight through to the linker.
- No new `Expr`/`Stmt` variants. Imports live only in `ast::Program`.
- inkwell `build_*`/target-machine methods return `Result`/`Option` — `.unwrap()`/`.expect()` them per this codebase's existing style. If a method name differs slightly from what's shown below (e.g. between patch versions), check https://docs.rs/inkwell/0.9.0 and adapt without changing the plan's semantics.
- Platform: macOS, Homebrew LLVM (`LLVM_SYS_201_PREFIX=/opt/homebrew/opt/llvm@20`, already configured in `.cargo/config.toml`). Shared-library commands below use macOS's `-dynamiclib` / `.dylib` / `DYLD_LIBRARY_PATH`.

## Execution: waves & parallelism

Tasks are grouped into **waves**. Every task in a wave is safe to dispatch to a parallel implementer+reviewer pair at the same time (disjoint files, no task in the wave depends on another task in the same wave). A wave only starts once every task in the **previous** wave has been implemented, reviewed, and merged — later waves need the actual landed code of earlier ones (this is a single Cargo crate; multiple binaries share the lib target, so a signature change in one file is only safe to build against once it's really landed, not just "documented").

```
Wave 0 (parallel x2): Task 1 (lexer)        Task 2 (runtime/verb.h)
                            |                        |
Wave 1 (parallel x2): Task 3 (ast+parser)   Task 4 (mathlib.cpp fixture)
                            |
Wave 2 (sequential):  Task 5 (codegen + verb-lsp)
                            |
Wave 3 (sequential):  Task 6 (main.rs: CLI + AOT build)
                            |
Wave 4 (sequential):  Task 7 (e2e integration test)  <- also needs Task 4
```

Rationale for what's *not* parallel: `ast::Program` (Task 3) is consumed by `codegen::compile_program`'s signature (Task 5), which `main.rs` (Task 6) calls unchanged — a signature mismatch anywhere in that chain fails the whole-crate build, so Tasks 3 → 5 → 6 must land in order. Task 4 (a standalone `.cpp` file with zero Rust dependency) and Task 7 (the e2e test that compiles and links against it) are the only pieces that don't sit on that chain, so Task 4 moves as early as its one real dependency (Task 2's header) allows.

**Known transient state:** after Task 3 lands, `src/main.rs` and `src/bin/verb-lsp.rs` do not compile (they still call the old `compile_program(&[Stmt])`) until Task 5 lands. Task 3's own verification is explicitly scoped to `cargo test --lib` for this reason — see Task 3's steps.

## File Structure

```
src/lexer.rs                       # + TokenKind::Import, TokenKind::Mod
src/formatter.rs                   # + token_text arms for Import/Mod
src/ast.rs                         # + Program struct
src/parser.rs                      # + import-statement parsing in parse()/parse_recovering()
src/codegen.rs                     # + Program-based compile_program, imports/externs fields, extern-call codegen
src/main.rs                        # + Program threading, JIT-rejects-imports, real build_aot, -L flag
src/bin/verb-lsp.rs                # + adapt to Program-returning parse_recovering
runtime/verb.h                     # NEW: C header shipped for extern "C" implementers
tests/fixtures/cpp/mathlib.cpp     # NEW: e2e fixture C++ library, built against runtime/verb.h
tests/fixtures/import_extern_call.verb   # NEW: fixture used by Task 5's codegen tests
tests/fixtures/err_extern_arity.verb     # NEW: fixture for the arity-mismatch compile error
tests/fixtures/import_mathlib.verb/.expected  # NEW: full AOT e2e fixture
tests/e2e.rs                       # + AOT build+run tests, IR-shape test, arity test
```

---

### Task 1: Lexer — `import` / `mod` keywords

**Wave 0 — parallel with Task 2.**

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/formatter.rs`

**Interfaces:**
- Produces: `TokenKind::Import`, `TokenKind::Mod` — consumed by Task 3 (parser).

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block at the bottom of `src/lexer.rs` (after `scans_verb_keywords`):

```rust
    #[test]
    fn scans_import_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("import mod mathlib;"),
            vec![Import, Mod, Ident("mathlib".into()), Semi, Eof]
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib scans_import_keywords`
Expected: FAIL to compile — `TokenKind::Import`/`TokenKind::Mod` do not exist yet.

- [ ] **Step 3: Implement**

In `src/lexer.rs`, add the two variants to the enum (line 6):

```rust
    Assign, Be, Declare, Make, Return, Check, Orelse, Repeat, Loop, True, False, Nil, Begin, End,
    Import, Mod,
```

Add the two keywords to the match in `lex_with_comments` (inside the `a if a.is_ascii_alphabetic() ...` arm, alongside the other keyword mappings):

```rust
                    "assign" => Assign, "be" => Be, "declare" => Declare, "make" => Make, "return" => Return,
                    "check" => Check, "orelse" => Orelse, "repeat" => Repeat, "loop" => Loop,
                    "true" => True, "false" => False, "nil" => Nil,
                    "begin" => Begin, "end" => End,
                    "import" => Import, "mod" => Mod,
```

In `src/formatter.rs`, add two arms to `token_text`'s exhaustive match (this is required for the crate to compile at all — the match has no wildcard arm):

```rust
        Begin => "begin".into(),
        End => "end".into(),
        Import => "import".into(),
        Mod => "mod".into(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib`
Expected: PASS — all lexer, parser, and formatter unit tests green (this is a purely additive change; nothing else references `TokenKind` exhaustively except `formatter::token_text`, which Step 3 already covers).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs src/formatter.rs
git commit -m "feat: lex import/mod keywords"
```

---

### Task 2: Runtime header — `runtime/verb.h`

**Wave 0 — parallel with Task 1.**

**Files:**
- Create: `runtime/verb.h`

**Interfaces:**
- Produces: `VerbValue` struct (`{ int8_t tag; int64_t payload; }`) + tag constants + constructor/accessor inline functions — consumed by Task 4 (`mathlib.cpp` fixture) and by any real C++ library author targeting Verb.

- [ ] **Step 1: Write the header**

Create `runtime/verb.h`:

```c
// Verb's C-ABI boundary type. A VerbValue is byte-identical to the LLVM
// struct `%verb.value = { i8, i64 }` that Verb's own compiled code passes
// by value everywhere — this header just gives C++ code the same layout,
// plus constructors/accessors so extern "C" functions can build and read
// Verb values without knowing the tag encoding by heart.
//
// Tag 5 (closure) never crosses this boundary: Verb closures aren't
// representable in C++ and extern fns can't receive or return one.
#ifndef VERB_H
#define VERB_H

#include <stdint.h>
#include <string.h>

typedef struct { int8_t tag; int64_t payload; } VerbValue;

enum {
    VERB_NIL = 0,
    VERB_BOOL = 1,
    VERB_INT = 2,
    VERB_FLOAT = 3,
    VERB_STRING = 4,
};

static inline VerbValue verb_nil(void) {
    VerbValue v; v.tag = VERB_NIL; v.payload = 0; return v;
}
static inline VerbValue verb_bool(int b) {
    VerbValue v; v.tag = VERB_BOOL; v.payload = b ? 1 : 0; return v;
}
static inline VerbValue verb_int(int64_t n) {
    VerbValue v; v.tag = VERB_INT; v.payload = n; return v;
}
static inline VerbValue verb_float(double d) {
    VerbValue v; v.tag = VERB_FLOAT; memcpy(&v.payload, &d, sizeof(d)); return v;
}
static inline VerbValue verb_string(const char* s) {
    VerbValue v; v.tag = VERB_STRING; memcpy(&v.payload, &s, sizeof(s)); return v;
}

static inline int verb_is(VerbValue v, int tag) { return v.tag == tag; }

static inline int64_t verb_as_int(VerbValue v) { return v.payload; }
static inline double verb_as_float(VerbValue v) {
    double d; memcpy(&d, &v.payload, sizeof(d)); return d;
}
static inline const char* verb_as_string(VerbValue v) {
    const char* s; memcpy(&s, &v.payload, sizeof(s)); return s;
}
static inline int verb_as_bool(VerbValue v) { return v.payload != 0; }

#endif // VERB_H
```

- [ ] **Step 2: Verify it compiles standalone**

Run:
```bash
cat > /tmp/verbh_check.cpp <<'EOF'
#include "verb.h"
int main() {
    VerbValue v = verb_float(3.5);
    return verb_is(v, VERB_FLOAT) && verb_as_float(v) == 3.5 ? 0 : 1;
}
EOF
c++ -std=c++17 -Iruntime -o /tmp/verbh_check /tmp/verbh_check.cpp && /tmp/verbh_check
echo "exit: $?"
rm -f /tmp/verbh_check /tmp/verbh_check.cpp
```
Expected: `exit: 0`, no compiler warnings/errors.

- [ ] **Step 3: Commit**

```bash
git add runtime/verb.h
git commit -m "docs: ship runtime/verb.h C-ABI header for extern C++ imports"
```

---

### Task 3: AST `Program` + parser import statements

**Wave 1 — parallel with Task 4. Depends on Task 1 (needs `TokenKind::Import`/`Mod`).**

**Files:**
- Modify: `src/ast.rs`
- Modify: `src/parser.rs`

**Interfaces:**
- Consumes: `TokenKind::Import`, `TokenKind::Mod` (Task 1).
- Produces: `pub struct Program { pub imports: Vec<String>, pub body: Vec<Stmt> }` (ast.rs); `pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError>`; `pub fn parse_recovering(toks: Vec<Token>) -> (Program, Vec<CompileError>)` — consumed by Task 5 (codegen) and Task 6 (main.rs, unchanged call site).

**Note:** after this task, `src/main.rs` and `src/bin/verb-lsp.rs` (both still expect `compile_program(&[Stmt])`) will **not** compile. That's expected and fixed by Task 5. Verification below is scoped to `cargo test --lib`, which only builds the library target (`ast`, `codegen`, `error`, `formatter`, `lexer`, `parser`, `value` — none of which call `compile_program`).

- [ ] **Step 1: Add `Program` to the AST**

Append to `src/ast.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub body: Vec<Stmt>,
}
```

- [ ] **Step 2: Write the failing tests**

In `src/parser.rs`'s `mod tests`, replace the existing block from `fn expr` through `rejects_return_at_top_level` (inclusive) with:

```rust
    fn expr(src: &str) -> Expr {
        // parse "src;" as a single expression statement
        let prog = parse(lex(&format!("{src};")).unwrap()).unwrap();
        match prog.body.into_iter().next().unwrap() {
            Stmt::ExprStmt(e) => e,
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn precedence_mul_over_plus() {
        match expr("1 add 2 times 3") {
            Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                assert_eq!(*lhs, Expr::Int(1));
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unary_and_grouping() {
        match expr("neg (1 add 2)") {
            Expr::Unary { op: UnOp::Neg, expr: inner, .. } => {
                assert!(matches!(*inner, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn call_with_args() {
        let e = expr("sum(1, 2)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Var(ref n, _, _) if n == "sum"));
                assert_eq!(args, vec![Expr::Int(1), Expr::Int(2)]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn logic_precedence_or_lowest() {
        let e = expr("true or false and true");
        assert!(matches!(e, Expr::Binary { op: BinOp::Or, .. }));
    }

    #[test]
    fn parses_assign_and_reassign() {
        let p = parse(lex("assign x 10; x be x add 1;").unwrap()).unwrap();
        assert!(matches!(&p.body[0], Stmt::Assign { name, .. } if name == "x"));
        assert!(matches!(&p.body[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_declare() {
        let p = parse(lex("declare x; x be 1;").unwrap()).unwrap();
        assert!(matches!(&p.body[0], Stmt::Declare { name } if name == "x"));
        assert!(matches!(&p.body[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_if_else_chain() {
        let p = parse(lex("check true begin print(1); end orelse check false begin print(2); end orelse begin print(3); end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::If { else_body: Some(eb), .. } => {
                assert!(matches!(&eb[0], Stmt::If { else_body: Some(_), .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn desugars_for_to_while() {
        let p = parse(lex("loop assign i 0; i trails 10; i be i add 1 begin print(i); end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::Block(inner) => {
                assert!(matches!(&inner[0], Stmt::Assign { name, .. } if name == "i"));
                match &inner[1] {
                    Stmt::While { body, .. } => {
                        assert!(matches!(body.last().unwrap(), Stmt::Reassign { name, .. } if name == "i"));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_fn_and_return() {
        let p = parse(lex("make sum(a, b) begin return a add b; end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::Fn { name, params, body, .. } => {
                assert_eq!(name, "sum");
                assert_eq!(params, &vec!["a".to_string(), "b".to_string()]);
                assert!(matches!(&body[0], Stmt::Return { value: Some(_) }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_return_at_top_level() {
        assert!(parse(lex("return 1;").unwrap()).is_err());
    }

    #[test]
    fn parses_single_import() {
        let p = parse(lex("import mod mathlib;").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert!(p.body.is_empty());
    }

    #[test]
    fn parses_multiple_imports_and_dedups() {
        let p = parse(lex("import mod mathlib; import mod strlib; import mod mathlib;").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string(), "strlib".to_string()]);
    }

    #[test]
    fn import_before_body_is_fine() {
        let p = parse(lex("import mod mathlib; print(1);").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert_eq!(p.body.len(), 1);
    }

    #[test]
    fn import_after_a_statement_is_a_compile_error() {
        let err = parse(lex("print(1); import mod mathlib;").unwrap()).unwrap_err();
        assert!(err.msg.contains("must appear before"), "{}", err.msg);
    }

    #[test]
    fn program_with_no_imports_has_empty_imports_vec() {
        let p = parse(lex("print(1);").unwrap()).unwrap();
        assert!(p.imports.is_empty());
    }

    #[test]
    fn recovering_collects_imports_too() {
        let src = "import mod mathlib; print(1);";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(prog.imports, vec!["mathlib".to_string()]);
        assert_eq!(prog.body.len(), 1);
    }
```

Then update the remaining `parse_recovering`-based tests further down in the same `mod tests` block (they currently destructure into `stmts`/`s`, which must become `prog`/`prog.body`):

```rust
    #[test]
    fn recovering_collects_every_error_across_semicolons() {
        // three broken statements in a row, each missing its expression
        let src = "assign a ; assign b ; assign c ;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(prog.body.is_empty());
    }

    #[test]
    fn recovering_keeps_good_statements_around_a_bad_one() {
        let src = "assign a 1; assign b ; assign c 3;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(prog.body.len(), 2);
        assert!(matches!(&prog.body[0], Stmt::Assign { name, .. } if name == "a"));
        assert!(matches!(&prog.body[1], Stmt::Assign { name, .. } if name == "c"));
    }

    #[test]
    fn recovering_resyncs_at_begin_after_a_broken_condition() {
        let src = "check begin print(1); end assign x 2;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(matches!(prog.body.last(), Some(Stmt::Assign { name, .. }) if name == "x"));
    }

    #[test]
    fn recovering_resets_fn_depth_so_return_is_still_rejected_after_an_error() {
        let src = "make broken(n) begin assign ; return 1;";
        let (_, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.iter().any(|e| e.msg.contains("return")), "{errors:?}");
    }

    #[test]
    fn recovering_matches_parse_on_valid_input() {
        let src = "make sum(a, b) begin return a add b; end print(sum(1, 2));";
        let ok = parse(lex(src).unwrap()).unwrap();
        let (recovering, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(ok, recovering);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib`
Expected: FAIL to compile — `parse`/`parse_recovering` still return `Vec<Stmt>`/`(Vec<Stmt>, ...)`, and `p.imports`/`p.body` don't exist on that type.

- [ ] **Step 4: Implement**

Replace `parse` and `parse_recovering` (lines 5–51 of `src/parser.rs`) with:

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

/// Same grammar as `parse`, but doesn't stop at the first syntax error:
/// after a bad statement it synchronizes to the next likely statement
/// boundary and keeps going, collecting every statement it could parse
/// and every error it hit along the way. Meant for editor tooling (an
/// LSP wants every syntax mistake in a file at once); the compiler proper
/// keeps using `parse`, which stops at the first error like a normal
/// compiler.
pub fn parse_recovering(toks: Vec<Token>) -> (Program, Vec<CompileError>) {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
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
        let pos_before = p.pos;
        match p.statement() {
            Ok(s) => body.push(s),
            Err(e) => {
                errors.push(e);
                // An error deep inside an unfinished `make` body leaves
                // fn_depth incremented (the decrement after `block()?`
                // never runs); recovery always resumes at top level, so
                // depth tracking mid-error can't be trusted regardless.
                p.fn_depth = 0;
                p.synchronize();
                // Some productions (e.g. `return` outside a function)
                // fail without consuming their own token, and
                // `synchronize` treats that same token as an
                // already-safe boundary it stops at without advancing
                // either — so the two together can make zero progress.
                // Force one token forward whenever that happens, or the
                // next iteration would hit the exact same error forever.
                if p.pos == pos_before {
                    p.advance();
                }
            }
        }
    }
    (Program { imports, body }, errors)
}
```

Then add two methods to `impl Parser` (anywhere in the block — e.g. right after `expect_ident`):

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

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib`
Expected: PASS — all lexer/parser/formatter unit tests green. (`src/main.rs`/`src/bin/verb-lsp.rs` still won't build; that's expected until Task 5. Do not run plain `cargo test` yet.)

- [ ] **Step 6: Commit**

```bash
git add src/ast.rs src/parser.rs
git commit -m "feat: parse 'import mod <lib>;' into ast::Program"
```

---

### Task 4: C++ fixture library — `mathlib.cpp`

**Wave 1 — parallel with Task 3. Depends on Task 2 (needs `runtime/verb.h`).**

**Files:**
- Create: `tests/fixtures/cpp/mathlib.cpp`

**Interfaces:**
- Consumes: `runtime/verb.h`'s `VerbValue`/`verb_*` API (Task 2).
- Produces: `extern "C"` functions `c_sqrt`, `c_add_int`, `c_shout` — consumed by Task 7's e2e test.

- [ ] **Step 1: Write the fixture**

Create `tests/fixtures/cpp/mathlib.cpp`:

```cpp
#include "verb.h"
#include <cmath>
#include <cctype>
#include <cstring>
#include <cstdlib>

extern "C" VerbValue c_sqrt(VerbValue x) {
    return verb_float(std::sqrt(verb_as_float(x)));
}

extern "C" VerbValue c_add_int(VerbValue a, VerbValue b) {
    return verb_int(verb_as_int(a) + verb_as_int(b));
}

extern "C" VerbValue c_shout(VerbValue s) {
    const char* in = verb_as_string(s);
    size_t len = std::strlen(in);
    char* out = static_cast<char*>(std::malloc(len + 2));
    for (size_t i = 0; i < len; i++) {
        out[i] = static_cast<char>(std::toupper(static_cast<unsigned char>(in[i])));
    }
    out[len] = '!';
    out[len + 1] = '\0';
    return verb_string(out);
}
```

- [ ] **Step 2: Verify it compiles to a shared library standalone**

Run:
```bash
c++ -std=c++17 -Iruntime -dynamiclib -o /tmp/libmathlib_check.dylib tests/fixtures/cpp/mathlib.cpp
echo "exit: $?"
rm -f /tmp/libmathlib_check.dylib
```
Expected: `exit: 0`, no compiler errors.

- [ ] **Step 3: Commit**

```bash
git add tests/fixtures/cpp/mathlib.cpp
git commit -m "test: add mathlib.cpp e2e fixture library"
```

---

### Task 5: Codegen — extern-call resolution + `verb-lsp` adaptation

**Wave 2 — sequential. Depends on Task 3 (`ast::Program`, `parse`/`parse_recovering`).**

**Files:**
- Modify: `src/codegen.rs`
- Modify: `src/bin/verb-lsp.rs`
- Create: `tests/fixtures/import_extern_call.verb`
- Create: `tests/fixtures/err_extern_arity.verb`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `ast::Program` (Task 3).
- Produces: `pub fn compile_program(&mut self, program: &Program) -> Result<(), CompileError>` (replaces the old `&[Stmt]` signature) — consumed by Task 6 (`main.rs`, call site unchanged) and already by `verb-lsp.rs` (fixed in this task).

**Note:** at the start of this task, the whole workspace does not build (`src/main.rs` and `src/bin/verb-lsp.rs` both call the old `compile_program(&[Stmt])`). `main.rs`'s call site (`cg.compile_program(&prog)`) needs **no edit** — it never destructured the return type, so it type-checks automatically once this task lands. `verb-lsp.rs` **does** need edits (below) since it destructures the parser's return value directly. By the end of this task, `cargo test` (whole workspace) is green again.

- [ ] **Step 1: Write the failing tests and fixtures**

Create `tests/fixtures/import_extern_call.verb`:
```
import mod mathlib;

print(c_sqrt(4.0));
```

Create `tests/fixtures/err_extern_arity.verb`:
```
import mod mathlib;

print(c_sqrt(1.0));
print(c_sqrt(1.0, 2.0));
```

Append to `tests/e2e.rs`:

```rust
#[test]
fn extern_call_compiles_to_a_direct_call_instruction() {
    let tmp = std::env::temp_dir().join("verb_test_extern_ir_out");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            "tests/fixtures/import_extern_call.verb",
            "-o", tmp.to_str().unwrap(),
            "--emit-llvm",
        ])
        .output()
        .unwrap();
    // build_aot isn't implemented until Task 6, so this still exits non-zero —
    // --emit-llvm prints IR to stdout before build_aot ever runs, so the IR
    // shape is already checkable here.
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("@c_sqrt"), "no call to c_sqrt in IR:\n{ir}");
}

#[test]
fn extern_arity_mismatch_across_call_sites_is_a_compile_error() {
    compile_err("err_extern_arity", &[
        "extern fn 'c_sqrt' called with 2 argument(s), previously called with 1",
    ]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e extern_call_compiles_to_a_direct_call_instruction extern_arity_mismatch_across_call_sites_is_a_compile_error`
Expected: FAIL to compile — the whole workspace is currently broken (see the task note above): `compile_program` still takes `&[Stmt]`, `prog` from `parser::parse` is now `Program`.

- [ ] **Step 3: Implement — codegen.rs**

Add two fields to `Codegen` (in the struct defined around line 14):

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
}
```

Initialize them in `Codegen::new` (in the `Self { ... }` literal):

```rust
        let cg = Self {
            ctx, module, builder, value_ty, closure_ty, ptr_ty,
            scopes: Vec::new(), functions: HashMap::new(), externs: HashMap::new(),
            imports: Vec::new(), fn_depth: 0, fn_counter: 0,
        };
```

Replace `compile_program` (around line 633):

```rust
    pub fn compile_program(&mut self, program: &Program) -> Result<(), CompileError> {
        self.imports = program.imports.clone();
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.scopes.push(HashMap::new());
        self.gen_stmts(&program.body)?;
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }
```

Replace `gen_call` (around line 930) — the only change is the new `if !self.imports.is_empty() ...` branch inside the existing `if let Expr::Var(name, ..) = callee` block, right after the `print` special case:

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
        let cv = self.gen_expr(callee)?;
        let argc = self.ctx.i64_type().const_int(args.len() as u64, false);
        let (lc, cc) = self.loc_consts(line, col);
        let clos_ptr = self.call_named(
            "verb_check_call", &[cv.into(), argc.into(), lc.into(), cc.into()])
            .unwrap().into_pointer_value();

        let arr_ty = self.value_ty.array_type(args.len() as u32);
        let argv = self.entry_alloca(arr_ty.into(), "argv");
        for (i, a) in args.iter().enumerate() {
            let v = self.gen_expr(a)?;
            let ap = unsafe {
                self.builder.build_in_bounds_gep(
                    self.value_ty, argv,
                    &[self.ctx.i64_type().const_int(i as u64, false)], "argp")
            }.unwrap();
            self.builder.build_store(ap, v).unwrap();
        }

        let fpp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 0, "fpp").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fpp, "fp").unwrap().into_pointer_value();
        let epp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 2, "epp").unwrap();
        let env = self.builder.build_load(self.ptr_ty, epp, "env").unwrap();

        let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
        let out = self.builder.build_indirect_call(
            fnty, fp, &[env.into(), argv.into()], "call").unwrap();
        Ok(out.try_as_basic_value().basic().unwrap().into_struct_value())
    }

    /// A call to a name that isn't a local variable or a known Verb `fn`,
    /// in a program that has at least one `import mod`. Declares (once
    /// per name, lazily, on first sight) a raw external function of type
    /// `VerbValue(VerbValue, VerbValue, ...)` — the same struct Verb's
    /// own runtime helpers already pass by value — and calls it directly.
    /// No unboxing: the extern C++ side receives Verb's tagged value
    /// as-is and is responsible for interpreting it (see runtime/verb.h).
    fn gen_extern_call(&mut self, name: &str, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        let argvals: Vec<StructValue<'ctx>> =
            args.iter().map(|a| self.gen_expr(a)).collect::<Result<_, _>>()?;
        let fnv = match self.externs.get(name).copied() {
            Some(fnv) => {
                if fnv.count_params() as usize != argvals.len() {
                    return Err(CompileError::new(
                        format!(
                            "extern fn '{name}' called with {} argument(s), previously called with {}",
                            argvals.len(), fnv.count_params()
                        ),
                        line, col,
                    ));
                }
                fnv
            }
            None => {
                let param_tys: Vec<_> = argvals.iter().map(|_| self.value_ty.into()).collect();
                let fnty = self.value_ty.fn_type(&param_tys, false);
                let fnv = self.module.add_function(name, fnty, None);
                self.externs.insert(name.to_string(), fnv);
                fnv
            }
        };
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        Ok(self.builder.build_call(fnv, &args_bv, "extern_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value())
    }
```

- [ ] **Step 4: Implement — verb-lsp.rs**

In `src/bin/verb-lsp.rs`, replace `compute_diagnostics` (around line 227):

```rust
fn compute_diagnostics(src: &str) -> Vec<Value> {
    let toks = match lexer::lex(src) {
        Ok(t) => t,
        Err(e) => return vec![diagnostic_from(&e)],
    };
    let (program, parse_errors) = parser::parse_recovering(toks);
    if !parse_errors.is_empty() {
        return parse_errors.iter().map(diagnostic_from).collect();
    }
    let ctx = Context::create();
    let mut cg = Codegen::new(&ctx);
    if let Err(e) = cg.compile_program(&program) {
        return vec![diagnostic_from(&e)];
    }
    vec![]
}
```

And `collect_symbols` (around line 414):

```rust
fn collect_symbols(src: &str) -> Symbols {
    let mut symbols = Symbols::default();
    if let Ok(toks) = lexer::lex(src) {
        // best-effort: use whatever parsed even if part of the file has
        // a syntax error elsewhere, so hover/completion still work
        let (program, _errors) = parser::parse_recovering(toks);
        collect_from_stmts(&program.body, &mut symbols);
    }
    symbols
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS — the whole workspace builds and every test (lexer, parser, formatter, e2e) is green, including the two new e2e tests from Step 1.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs src/bin/verb-lsp.rs tests/fixtures/import_extern_call.verb tests/fixtures/err_extern_arity.verb tests/e2e.rs
git commit -m "feat: codegen extern C++ calls; adapt verb-lsp to ast::Program"
```

---

### Task 6: `main.rs` — CLI wiring + real AOT build

**Wave 3 — sequential. Depends on Task 5 (`Codegen::compile_program(&Program)`).**

**Files:**
- Modify: `src/main.rs`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `ast::Program.imports` (Task 3), `Codegen::compile_program(&Program)` (Task 5).
- Produces: working `verb build <file> -o <out> [-L<dir>]...` (was a stub); `verb run` rejects imports — consumed by Task 7's e2e test.

- [ ] **Step 1: Write the failing tests**

Append to `tests/e2e.rs`:

```rust
#[test]
fn build_produces_a_runnable_binary_for_import_free_programs() {
    let out_path = std::env::temp_dir().join("verb_test_build_literals");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/literals.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success());
    let expected = std::fs::read_to_string("tests/fixtures/literals.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn run_rejects_programs_with_imports() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/import_extern_call.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("mathlib"), "stderr: {stderr}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e build_produces_a_runnable_binary_for_import_free_programs run_rejects_programs_with_imports`
Expected: FAIL — `build` currently prints `build: not implemented yet` and exits 1; `run` currently doesn't check `imports` at all (it would try to JIT-run and print the actual value instead of rejecting).

- [ ] **Step 3: Implement**

Replace `src/main.rs` in full:

```rust
use std::process::{exit, Command};

use verb::codegen;
use verb::error::CompileError;
use verb::lexer;
use verb::parser;

fn die(e: CompileError, src: &str) -> ! {
    eprintln!("error [{}:{}]: {}", e.line, e.col, e.msg);
    if e.line > 0 {
        if let Some(text) = src.lines().nth(e.line as usize - 1) {
            let num = e.line.to_string();
            eprintln!(" {num} | {text}");
            let pad = " ".repeat(num.len());
            let offset = " ".repeat(e.col.saturating_sub(1) as usize);
            eprintln!(" {pad} | {offset}^");
        }
    }
    if let Some(hint) = &e.hint {
        eprintln!("   hint: {hint}");
    }
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage: verb run <file.verb> [--emit-llvm]");
    eprintln!("       verb build <file.verb> -o <out> [-L<dir>]... [--emit-llvm]");
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 { usage(); }
    let cmd = args[1].as_str();
    let file = args[2].as_str();
    let emit_llvm = args.iter().any(|a| a == "--emit-llvm");
    let out = args.iter().position(|a| a == "-o").map(|i| {
        args.get(i + 1).cloned().unwrap_or_else(|| usage())
    });
    let lib_dirs: Vec<String> = args.iter()
        .filter(|a| a.starts_with("-L") && a.as_str() != "-L")
        .cloned()
        .collect();

    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("error: cannot read {file}: {e}"); exit(1); }
    };
    let toks = lexer::lex(&src).unwrap_or_else(|e| die(e, &src));
    let prog = parser::parse(toks).unwrap_or_else(|e| die(e, &src));

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&prog).unwrap_or_else(|e| die(e, &src));

    if emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match cmd {
        "run" => {
            if !prog.imports.is_empty() {
                eprintln!(
                    "error: 'verb run' does not support imports ({}); use 'verb build' instead",
                    prog.imports.join(", ")
                );
                exit(1);
            }
            let ee = cg.module()
                .create_jit_execution_engine(inkwell::OptimizationLevel::None)
                .unwrap_or_else(|e| { eprintln!("JIT error: {e}"); exit(1); });
            unsafe {
                let main_fn = ee.get_function::<unsafe extern "C" fn() -> i32>("main")
                    .expect("no main");
                exit(main_fn.call());
            }
        }
        "build" => {
            let out = out.unwrap_or_else(|| usage());
            build_aot(&cg, &out, &prog.imports, &lib_dirs);
        }
        _ => usage(),
    }
}

fn build_aot(cg: &codegen::Codegen, out: &str, imports: &[String], lib_dirs: &[String]) {
    use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};

    Target::initialize_native(&InitializationConfig::default())
        .unwrap_or_else(|e| { eprintln!("error: failed to initialize target: {e}"); exit(1); });

    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .unwrap_or_else(|e| { eprintln!("error: unsupported target: {e}"); exit(1); });
    let tm = target
        .create_target_machine(
            &triple,
            &TargetMachine::get_host_cpu_name().to_string(),
            &TargetMachine::get_host_cpu_features().to_string(),
            inkwell::OptimizationLevel::None,
            RelocMode::Default,
            CodeModel::Default,
        )
        .expect("failed to create target machine");

    cg.module().set_triple(&triple);
    cg.module().set_data_layout(&tm.get_target_data().get_data_layout());

    let obj_path = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, std::path::Path::new(&obj_path))
        .unwrap_or_else(|e| { eprintln!("error: failed to emit object file: {e}"); exit(1); });

    let linker = if imports.is_empty() { "cc" } else { "c++" };
    let mut cmd = Command::new(linker);
    cmd.arg(&obj_path).arg("-o").arg(out);
    for dir in lib_dirs {
        cmd.arg(dir);
    }
    for lib in imports {
        cmd.arg(format!("-l{lib}"));
    }
    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("error: failed to run linker '{linker}': {e}");
        exit(1);
    });
    let _ = std::fs::remove_file(&obj_path);
    if !status.success() {
        eprintln!("error: link failed");
        exit(1);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS — full workspace, including the two new tests and every existing e2e test (which now also exercises the real `build_aot` for the first time, via any test that happened to call it — check none did before this task; `emits_llvm_ir` uses `run`, unaffected).

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/e2e.rs
git commit -m "feat: implement AOT build (verb build); JIT rejects imports"
```

---

### Task 7: E2E integration test — real C++ import end-to-end

**Wave 4 — sequential. Depends on Task 6 (real `build_aot`) and Task 4 (`mathlib.cpp`).**

**Files:**
- Create: `tests/fixtures/import_mathlib.verb`
- Create: `tests/fixtures/import_mathlib.expected`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb build -L<dir>` (Task 6), `tests/fixtures/cpp/mathlib.cpp`'s `c_sqrt`/`c_add_int`/`c_shout` (Task 4).

- [ ] **Step 1: Write the fixture**

Create `tests/fixtures/import_mathlib.verb`:
```
import mod mathlib;

print(c_sqrt(9.0));
print(c_add_int(2, 3));
print(c_shout("hi"));
```

Create `tests/fixtures/import_mathlib.expected`:
```
3
5
HI!
```

(Verb's `print` uses `%g` for floats and `%lld` for ints — see `Codegen::build_print_fn` — so `sqrt(9.0) == 3.0` prints as `3`, not `3.0`.)

- [ ] **Step 2: Write the failing test**

Append to `tests/e2e.rs`:

```rust
/// Compiles tests/fixtures/cpp/mathlib.cpp into a shared library once per
/// test run and returns the directory it landed in (for `-L`).
fn build_mathlib_fixture() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("verb_e2e_cpp_libs");
    std::fs::create_dir_all(&dir).unwrap();
    let lib_path = dir.join("libmathlib.dylib");
    let status = Command::new("c++")
        .args([
            "-std=c++17",
            "-Iruntime",
            "-dynamiclib",
            "-o", lib_path.to_str().unwrap(),
            "tests/fixtures/cpp/mathlib.cpp",
        ])
        .status()
        .expect("failed to invoke c++ to build the mathlib test fixture");
    assert!(status.success(), "failed to compile tests/fixtures/cpp/mathlib.cpp");
    dir
}

fn build_and_run_ok(name: &str, lib_dir: &std::path::Path) {
    let out_path = std::env::temp_dir().join(format!("verb_e2e_build_{name}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build",
            &format!("tests/fixtures/{name}.verb"),
            "-o", out_path.to_str().unwrap(),
            &format!("-L{}", lib_dir.display()),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path)
        .env("DYLD_LIBRARY_PATH", lib_dir)
        .output()
        .unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn imports_cpp_library_and_calls_extern_functions() {
    let lib_dir = build_mathlib_fixture();
    build_and_run_ok("import_mathlib", &lib_dir);
}
```

- [ ] **Step 3: Run test to verify it fails, then confirm the fixture alone fixes it**

Before Step 1's fixture files exist, run: `cargo test --test e2e imports_cpp_library_and_calls_extern_functions`
Expected: FAIL — `tests/fixtures/import_mathlib.verb`/`.expected` don't exist yet (`build` step reads a missing file). This confirms the test harness (Step 2's code) is exercising real machinery, not a no-op.

Add Step 1's two fixture files, then run the same command again.
Expected: PASS already — Task 6 (`build_aot`) and Task 4 (`mathlib.cpp`) are both real and working by this point, so the fixture is the only missing piece.

- [ ] **Step 4: Run full suite**

Run: `cargo test`
Expected: PASS — every test in the workspace green, including the new C++ integration test. This is the final task: at this point `import mod mathlib; print(c_sqrt(9.0));` works end-to-end from source to a real linked, executed binary.

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/import_mathlib.verb tests/fixtures/import_mathlib.expected tests/e2e.rs
git commit -m "test: e2e-verify importing and calling a real C++ library"
```
