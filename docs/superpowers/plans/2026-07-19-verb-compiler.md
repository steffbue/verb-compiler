# Verb Compiler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `verb`, a Rust/inkwell compiler for the dynamically typed Verb language, producing LLVM IR that can be JIT-run or AOT-compiled to a native binary.

**Architecture:** Classic pipeline: hand-written lexer → recursive-descent parser → AST → inkwell codegen emitting one LLVM module (tagged `{i8,i64}` values, heap-boxed variables, closures as `{fn_ptr, arity, env}` heap objects). Runtime errors abort via inline `printf`+`exit(1)`; only external symbols are libc.

**Tech Stack:** Rust 2021, inkwell 0.9 (`llvm20-1`), LLVM 20.1.3 (Homebrew, keg-only), libc (`printf`, `malloc`, `exit`, `strlen`, `strcpy`, `strcat`, `strcmp`), `cc` for final link.

**Spec:** `docs/superpowers/specs/2026-07-19-verb-compiler-design.md` — read it first.

## Global Constraints

- Crate name / binary name: `verb`
- inkwell: `version = "0.9"`, features `["llvm20-1"]`; env `LLVM_SYS_201_PREFIX=/opt/homebrew/opt/llvm` (set via `.cargo/config.toml`, never required in shell)
- Value struct: `{ i8 tag, i64 payload }`; tags: 0 nil, 1 bool, 2 int, 3 float (f64 bits), 4 string ptr, 5 closure ptr
- No GC: `malloc` and never free
- Compile errors: `error [line:col]: msg` on **stderr**, exit ≠ 0
- Runtime errors: `runtime error: msg\n` on **stdout** via printf, then `exit(1)`
- Truthiness: nil/false falsy, everything else truthy. `and`/`or` return operand values (Lox semantics)
- API-drift note: inkwell `build_*` methods return `Result` — `.unwrap()` them. If a method name differs slightly in 0.9 (e.g. `build_bitcast` vs `build_bit_cast`), check https://docs.rs/inkwell/0.9.0 and adapt; do not change the plan's semantics.

## File Structure

```
Cargo.toml
.cargo/config.toml        # LLVM_SYS_201_PREFIX
src/main.rs               # CLI: run (JIT) / build (AOT) / --emit-llvm
src/error.rs              # CompileError { msg, line, col }
src/lexer.rs              # Token, TokenKind, lex()
src/ast.rs                # Expr, Stmt, BinOp, UnOp
src/parser.rs             # parse(Vec<Token>) -> Result<Vec<Stmt>, CompileError>
src/value.rs              # tag constants
src/codegen.rs            # Codegen struct, helper fns, AST -> LLVM IR
tests/e2e.rs              # golden-fixture harness (JIT via CARGO_BIN_EXE_verb)
tests/fixtures/*.verb / *.expected
```

---

### Task 1: Project scaffold

**Files:**
- Create: `Cargo.toml`, `.cargo/config.toml`, `src/main.rs`, `src/error.rs`, `.gitignore`

**Interfaces:**
- Produces: `error::CompileError { pub msg: String, pub line: u32, pub col: u32 }` — every later task returns this from fallible phases.

- [ ] **Step 1: Write files**

`Cargo.toml`:
```toml
[package]
name = "verb"
version = "0.1.0"
edition = "2021"

[dependencies]
inkwell = { version = "0.9", features = ["llvm20-1"] }
```

`.cargo/config.toml`:
```toml
[env]
LLVM_SYS_201_PREFIX = "/opt/homebrew/opt/llvm"
```

`.gitignore`:
```
/target
```

`src/error.rs`:
```rust
#[derive(Debug, Clone)]
pub struct CompileError {
    pub msg: String,
    pub line: u32,
    pub col: u32,
}

impl CompileError {
    pub fn new(msg: impl Into<String>, line: u32, col: u32) -> Self {
        Self { msg: msg.into(), line, col }
    }
}
```

`src/main.rs`:
```rust
mod error;

fn main() {
    // smoke: prove LLVM links
    let ctx = inkwell::context::Context::create();
    let module = ctx.create_module("smoke");
    println!("{}", module.get_name().to_str().unwrap());
}
```

- [ ] **Step 2: Verify build + run**

Run: `cargo run 2>&1 | tail -1`
Expected: `smoke` (first build compiles llvm-sys — takes minutes)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock .cargo .gitignore src
git commit -m "chore: scaffold verb crate with inkwell/LLVM 20"
```

---

### Task 2: Lexer

**Files:**
- Create: `src/lexer.rs`
- Modify: `src/main.rs` (add `mod lexer;`)

**Interfaces:**
- Produces: `lexer::lex(src: &str) -> Result<Vec<Token>, CompileError>`; `Token { kind: TokenKind, line: u32, col: u32 }`; `TokenKind` variants exactly as below (parser matches on them).

- [ ] **Step 1: Write failing tests** (bottom of `src/lexer.rs`, code from Step 3 not yet present — create the file with only the test module and a stub `lex` returning `Ok(vec![])` if you want it to compile, or write tests after types; simplest: write the full file in Step 3 and tests first as below in a temporary form)

Practical TDD here: create `src/lexer.rs` containing ONLY the enum/struct definitions plus `pub fn lex(_: &str) -> Result<Vec<Token>, CompileError> { Ok(vec![]) }` and these tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn scans_keywords_and_operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("assign x 10; x be x plus 1;"),
            vec![Assign, Ident("x".into()), Int(10), Semi,
                 Ident("x".into()), Be, Ident("x".into()), Plus, Int(1), Semi, Eof]
        );
    }

    #[test]
    fn scans_literals() {
        use TokenKind::*;
        assert_eq!(
            kinds(r#"3.24 "hi\n" true false nil"#),
            vec![Float(3.24), Str("hi\n".into()), True, False, Nil, Eof]
        );
    }

    #[test]
    fn skips_comments() {
        use TokenKind::*;
        assert_eq!(kinds("%% line\n1 !?! block\nstill !?! 2"), vec![Int(1), Int(2), Eof]);
    }

    #[test]
    fn tracks_position() {
        let t = &lex("\n  fn").unwrap()[0];
        assert_eq!((t.line, t.col), (2, 3));
    }

    #[test]
    fn rejects_unknown_char() {
        assert!(lex("@").is_err());
    }
}
```

- [ ] **Step 2: Run tests, verify FAIL**

Run: `cargo test lexer -- --nocapture`
Expected: FAIL (stub returns empty vec)

- [ ] **Step 3: Implement lexer**

Full `src/lexer.rs` (keep tests from Step 1 at bottom):

```rust
use crate::error::CompileError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64), Float(f64), Str(String), Ident(String),
    Assign, Be, Fn, Return, If, Else, While, For, True, False, Nil,
    Plus, Minus, Mul, Div, Mod,
    Eqeq, Neq, Lo, Hi, Loeq, Hieq,
    And, Or, Not, Concat,
    LParen, RParen, LBrace, RBrace, Semi, Comma,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let (mut line, mut col) = (1u32, 1u32);

    while i < chars.len() {
        let c = chars[i];
        let (tl, tc) = (line, col);
        match c {
            ' ' | '\t' | '\r' => { i += 1; col += 1; }
            '\n' => { i += 1; line += 1; col = 1; }
            '%' if chars.get(i + 1) == Some(&'%') => {
                while i < chars.len() && chars[i] != '\n' { i += 1; col += 1; }
            }
            '!' if chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'!') => {
                i += 3; col += 3;
                loop {
                    if i + 2 >= chars.len() + 1 && i >= chars.len() {
                        return Err(CompileError::new("unterminated block comment", tl, tc));
                    }
                    if chars[i] == '!' && chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'!') {
                        i += 3; col += 3; break;
                    }
                    if chars[i] == '\n' { line += 1; col = 1; } else { col += 1; }
                    i += 1;
                    if i >= chars.len() {
                        return Err(CompileError::new("unterminated block comment", tl, tc));
                    }
                }
            }
            '(' => { toks.push(Token { kind: TokenKind::LParen, line: tl, col: tc }); i += 1; col += 1; }
            ')' => { toks.push(Token { kind: TokenKind::RParen, line: tl, col: tc }); i += 1; col += 1; }
            '{' => { toks.push(Token { kind: TokenKind::LBrace, line: tl, col: tc }); i += 1; col += 1; }
            '}' => { toks.push(Token { kind: TokenKind::RBrace, line: tl, col: tc }); i += 1; col += 1; }
            ';' => { toks.push(Token { kind: TokenKind::Semi, line: tl, col: tc }); i += 1; col += 1; }
            ',' => { toks.push(Token { kind: TokenKind::Comma, line: tl, col: tc }); i += 1; col += 1; }
            '"' => {
                i += 1; col += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err(CompileError::new("unterminated string", tl, tc));
                    }
                    match chars[i] {
                        '"' => { i += 1; col += 1; break; }
                        '\\' => {
                            let esc = chars.get(i + 1).copied()
                                .ok_or_else(|| CompileError::new("unterminated string", tl, tc))?;
                            s.push(match esc {
                                'n' => '\n', 't' => '\t', '"' => '"', '\\' => '\\',
                                other => return Err(CompileError::new(
                                    format!("unknown escape '\\{other}'"), line, col)),
                            });
                            i += 2; col += 2;
                        }
                        '\n' => return Err(CompileError::new("unterminated string", tl, tc)),
                        ch => { s.push(ch); i += 1; col += 1; }
                    }
                }
                toks.push(Token { kind: TokenKind::Str(s), line: tl, col: tc });
            }
            d if d.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() { i += 1; col += 1; }
                let mut is_float = false;
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    is_float = true;
                    i += 1; col += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() { i += 1; col += 1; }
                }
                let text: String = chars[start..i].iter().collect();
                let kind = if is_float {
                    TokenKind::Float(text.parse().map_err(|_| CompileError::new("bad float", tl, tc))?)
                } else {
                    TokenKind::Int(text.parse().map_err(|_| CompileError::new("int too large", tl, tc))?)
                };
                toks.push(Token { kind, line: tl, col: tc });
            }
            a if a.is_ascii_alphabetic() || a == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1; col += 1;
                }
                let word: String = chars[start..i].iter().collect();
                use TokenKind::*;
                let kind = match word.as_str() {
                    "assign" => Assign, "be" => Be, "fn" => Fn, "return" => Return,
                    "if" => If, "else" => Else, "while" => While, "for" => For,
                    "true" => True, "false" => False, "nil" => Nil,
                    "plus" => Plus, "minus" => Minus, "mul" => Mul, "div" => Div, "mod" => Mod,
                    "eqeq" => Eqeq, "neq" => Neq, "lo" => Lo, "hi" => Hi,
                    "loeq" => Loeq, "hieq" => Hieq,
                    "and" => And, "or" => Or, "not" => Not, "c" => Concat,
                    _ => Ident(word),
                };
                toks.push(Token { kind, line: tl, col: tc });
            }
            other => {
                return Err(CompileError::new(format!("unexpected character '{other}'"), tl, tc));
            }
        }
    }
    toks.push(Token { kind: TokenKind::Eof, line, col });
    Ok(toks)
}
```

Add `mod lexer;` to `src/main.rs`.

- [ ] **Step 4: Run tests, verify PASS**

Run: `cargo test lexer`
Expected: 5 passed

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs src/main.rs
git commit -m "feat: lexer for Verb tokens, comments, literals"
```

---

### Task 3: AST + expression parsing

**Files:**
- Create: `src/ast.rs`, `src/parser.rs`
- Modify: `src/main.rs` (add `mod ast; mod parser;`)

**Interfaces:**
- Consumes: `lexer::{Token, TokenKind}`, `error::CompileError`
- Produces: `ast::{Expr, Stmt, BinOp, UnOp}` exactly as below; `parser::parse(toks: Vec<Token>) -> Result<Vec<Stmt>, CompileError>`. Statement parsing lands in Task 4; this task exposes `Parser` internals used there.

- [ ] **Step 1: Write `src/ast.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp { Add, Sub, Mul, Div, Mod, Eq, Ne, Lt, Gt, Le, Ge, And, Or, Concat }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp { Neg, Not }

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
    Var(String, u32, u32), // name, line, col (for undefined-variable errors)
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: UnOp, expr: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<Expr>, line: u32, col: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { name: String, value: Expr },                    // assign x expr;
    Reassign { name: String, value: Expr, line: u32, col: u32 }, // x be expr;
    ExprStmt(Expr),
    If { cond: Expr, then_body: Vec<Stmt>, else_body: Option<Vec<Stmt>> },
    While { cond: Expr, body: Vec<Stmt> },
    Fn { name: String, params: Vec<String>, body: Vec<Stmt>, line: u32, col: u32 },
    Return { value: Option<Expr> },
    Block(Vec<Stmt>),
}
```

- [ ] **Step 2: Write failing expression tests** (in `src/parser.rs`, with a stub)

Create `src/parser.rs` with a stub `pub fn parse(toks: Vec<Token>) -> Result<Vec<Stmt>, CompileError> { let _ = toks; Ok(vec![]) }` plus:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::lexer::lex;

    fn expr(src: &str) -> Expr {
        // parse "src;" as a single expression statement
        let stmts = parse(lex(&format!("{src};")).unwrap()).unwrap();
        match stmts.into_iter().next().unwrap() {
            Stmt::ExprStmt(e) => e,
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn precedence_mul_over_plus() {
        assert_eq!(
            expr("1 plus 2 mul 3"),
            Expr::Binary {
                op: BinOp::Add,
                lhs: Box::new(Expr::Int(1)),
                rhs: Box::new(Expr::Binary {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Int(2)),
                    rhs: Box::new(Expr::Int(3)),
                }),
            }
        );
    }

    #[test]
    fn unary_and_grouping() {
        assert_eq!(
            expr("minus (1 plus 2)"),
            Expr::Unary {
                op: UnOp::Neg,
                expr: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Int(1)),
                    rhs: Box::new(Expr::Int(2)),
                }),
            }
        );
    }

    #[test]
    fn call_with_args() {
        let e = expr("add(1, 2)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Var(ref n, _, _) if n == "add"));
                assert_eq!(args, vec![Expr::Int(1), Expr::Int(2)]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn logic_precedence_or_lowest() {
        // a or b and c  =>  a or (b and c)
        let e = expr("true or false and true");
        assert!(matches!(e, Expr::Binary { op: BinOp::Or, .. }));
    }
}
```

- [ ] **Step 3: Run tests, verify FAIL**

Run: `cargo test parser`
Expected: FAIL (stub returns empty)

- [ ] **Step 4: Implement parser core + expressions**

Replace stub in `src/parser.rs` (keep tests):

```rust
use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{Token, TokenKind};

pub fn parse(toks: Vec<Token>) -> Result<Vec<Stmt>, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let mut stmts = Vec::new();
    while !p.check(&TokenKind::Eof) {
        stmts.push(p.statement()?);
    }
    Ok(stmts)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    fn_depth: u32,
}

impl Parser {
    fn peek(&self) -> &TokenKind { &self.toks[self.pos].kind }
    fn peek2(&self) -> &TokenKind {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)].kind
    }
    fn here(&self) -> (u32, u32) {
        let t = &self.toks[self.pos];
        (t.line, t.col)
    }
    fn check(&self, k: &TokenKind) -> bool { self.peek() == k }
    fn advance(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 { self.pos += 1; }
        t
    }
    fn matches(&mut self, k: &TokenKind) -> bool {
        if self.check(k) { self.advance(); true } else { false }
    }
    fn expect(&mut self, k: &TokenKind, what: &str) -> Result<Token, CompileError> {
        if self.check(k) { Ok(self.advance()) } else { Err(self.err(format!("expected {what}"))) }
    }
    fn err(&self, msg: impl Into<String>) -> CompileError {
        let (l, c) = self.here();
        CompileError::new(msg, l, c)
    }
    fn expect_ident(&mut self, what: &str) -> Result<(String, u32, u32), CompileError> {
        let (l, c) = self.here();
        match self.peek().clone() {
            TokenKind::Ident(n) => { self.advance(); Ok((n, l, c)) }
            _ => Err(self.err(format!("expected {what}"))),
        }
    }

    // ----- statements (Task 4 fills the rest; ExprStmt suffices for expr tests) -----
    fn statement(&mut self) -> Result<Stmt, CompileError> {
        let e = self.expression()?;
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(Stmt::ExprStmt(e))
    }

    // ----- expressions: precedence climbing -----
    pub(crate) fn expression(&mut self) -> Result<Expr, CompileError> { self.or_expr() }

    fn or_expr(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.and_expr()?;
        while self.matches(&TokenKind::Or) {
            let r = self.and_expr()?;
            e = Expr::Binary { op: BinOp::Or, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn and_expr(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.equality()?;
        while self.matches(&TokenKind::And) {
            let r = self.equality()?;
            e = Expr::Binary { op: BinOp::And, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn equality(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.comparison()?;
        loop {
            let op = match self.peek() {
                TokenKind::Eqeq => BinOp::Eq,
                TokenKind::Neq => BinOp::Ne,
                _ => break,
            };
            self.advance();
            let r = self.comparison()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn comparison(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.term()?;
        loop {
            let op = match self.peek() {
                TokenKind::Lo => BinOp::Lt,
                TokenKind::Hi => BinOp::Gt,
                TokenKind::Loeq => BinOp::Le,
                TokenKind::Hieq => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let r = self.term()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn term(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.factor()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                TokenKind::Concat => BinOp::Concat,
                _ => break,
            };
            self.advance();
            let r = self.factor()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn factor(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Mul => BinOp::Mul,
                TokenKind::Div => BinOp::Div,
                TokenKind::Mod => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let r = self.unary()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, CompileError> {
        let op = match self.peek() {
            TokenKind::Not => Some(UnOp::Not),
            TokenKind::Minus => Some(UnOp::Neg),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let e = self.unary()?;
            return Ok(Expr::Unary { op, expr: Box::new(e) });
        }
        self.call()
    }
    fn call(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.primary()?;
        while self.check(&TokenKind::LParen) {
            let (l, c) = self.here();
            self.advance();
            let mut args = Vec::new();
            if !self.check(&TokenKind::RParen) {
                loop {
                    args.push(self.expression()?);
                    if !self.matches(&TokenKind::Comma) { break; }
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            e = Expr::Call { callee: Box::new(e), args, line: l, col: c };
        }
        Ok(e)
    }
    fn primary(&mut self) -> Result<Expr, CompileError> {
        let (l, c) = self.here();
        let e = match self.peek().clone() {
            TokenKind::Int(v) => { self.advance(); Expr::Int(v) }
            TokenKind::Float(v) => { self.advance(); Expr::Float(v) }
            TokenKind::Str(s) => { self.advance(); Expr::Str(s) }
            TokenKind::True => { self.advance(); Expr::Bool(true) }
            TokenKind::False => { self.advance(); Expr::Bool(false) }
            TokenKind::Nil => { self.advance(); Expr::Nil }
            TokenKind::Ident(n) => { self.advance(); Expr::Var(n, l, c) }
            TokenKind::LParen => {
                self.advance();
                let e = self.expression()?;
                self.expect(&TokenKind::RParen, "')'")?;
                e
            }
            _ => return Err(self.err("expected expression")),
        };
        Ok(e)
    }
}
```

Add `mod ast; mod parser;` to `src/main.rs`.

- [ ] **Step 5: Run tests, verify PASS**

Run: `cargo test parser`
Expected: 4 passed

- [ ] **Step 6: Commit**

```bash
git add src/ast.rs src/parser.rs src/main.rs
git commit -m "feat: AST types and expression parsing with precedence"
```

---

### Task 4: Statement parsing

**Files:**
- Modify: `src/parser.rs` (replace the temporary `statement()`)

**Interfaces:**
- Produces: full `Stmt` parsing — `assign`, `be`, `fn`, `return`, `if`/`else if`/`else`, `while`, `for` (desugared to `Block[init, While{cond, body+incr}]`), braces block, expression statement. `return` outside a function is a parse error.

- [ ] **Step 1: Add failing tests** to `parser::tests`

```rust
    #[test]
    fn parses_assign_and_reassign() {
        let s = parse(lex("assign x 10; x be x plus 1;").unwrap()).unwrap();
        assert!(matches!(&s[0], Stmt::Assign { name, .. } if name == "x"));
        assert!(matches!(&s[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_if_else_chain() {
        let s = parse(lex("if (true) { print(1); } else if (false) { print(2); } else { print(3); }").unwrap()).unwrap();
        match &s[0] {
            Stmt::If { else_body: Some(eb), .. } => {
                assert!(matches!(&eb[0], Stmt::If { else_body: Some(_), .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn desugars_for_to_while() {
        let s = parse(lex("for (assign i 0; i lo 10; i be i plus 1) { print(i); }").unwrap()).unwrap();
        match &s[0] {
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
        let s = parse(lex("fn add(a, b) { return a plus b; }").unwrap()).unwrap();
        match &s[0] {
            Stmt::Fn { name, params, body, .. } => {
                assert_eq!(name, "add");
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
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test parser`
Expected: new tests FAIL ("expected ';'" errors etc.)

- [ ] **Step 3: Implement statements** — replace `statement()` with:

```rust
    fn statement(&mut self) -> Result<Stmt, CompileError> {
        match self.peek() {
            TokenKind::Assign => self.assign_stmt(true),
            TokenKind::Fn => self.fn_stmt(),
            TokenKind::Return => self.return_stmt(),
            TokenKind::If => self.if_stmt(),
            TokenKind::While => self.while_stmt(),
            TokenKind::For => self.for_stmt(),
            TokenKind::LBrace => Ok(Stmt::Block(self.block()?)),
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(true),
            _ => {
                let e = self.expression()?;
                self.expect(&TokenKind::Semi, "';'")?;
                Ok(Stmt::ExprStmt(e))
            }
        }
    }

    fn assign_stmt(&mut self, semi: bool) -> Result<Stmt, CompileError> {
        self.advance(); // assign
        let (name, _, _) = self.expect_ident("variable name after 'assign'")?;
        let value = self.expression()?;
        if semi { self.expect(&TokenKind::Semi, "';'")?; }
        Ok(Stmt::Assign { name, value })
    }

    fn reassign_stmt(&mut self, semi: bool) -> Result<Stmt, CompileError> {
        let (name, line, col) = self.expect_ident("variable name")?;
        self.expect(&TokenKind::Be, "'be'")?;
        let value = self.expression()?;
        if semi { self.expect(&TokenKind::Semi, "';'")?; }
        Ok(Stmt::Reassign { name, value, line, col })
    }

    fn fn_stmt(&mut self) -> Result<Stmt, CompileError> {
        let (line, col) = self.here();
        self.advance(); // fn
        let (name, _, _) = self.expect_ident("function name")?;
        self.expect(&TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                params.push(self.expect_ident("parameter name")?.0);
                if !self.matches(&TokenKind::Comma) { break; }
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        self.fn_depth += 1;
        let body = self.block()?;
        self.fn_depth -= 1;
        Ok(Stmt::Fn { name, params, body, line, col })
    }

    fn return_stmt(&mut self) -> Result<Stmt, CompileError> {
        if self.fn_depth == 0 {
            return Err(self.err("'return' outside function"));
        }
        self.advance(); // return
        let value = if self.check(&TokenKind::Semi) { None } else { Some(self.expression()?) };
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(Stmt::Return { value })
    }

    fn if_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // if
        self.expect(&TokenKind::LParen, "'('")?;
        let cond = self.expression()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let then_body = self.block()?;
        let else_body = if self.matches(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                Some(vec![self.if_stmt()?]) // else if
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If { cond, then_body, else_body })
    }

    fn while_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // while
        self.expect(&TokenKind::LParen, "'('")?;
        let cond = self.expression()?;
        self.expect(&TokenKind::RParen, "')'")?;
        let body = self.block()?;
        Ok(Stmt::While { cond, body })
    }

    fn for_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // for
        self.expect(&TokenKind::LParen, "'('")?;
        let init = match self.peek() {
            TokenKind::Assign => self.assign_stmt(true)?, // consumes ';'
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(true)?,
            _ => return Err(self.err("expected 'assign' or reassignment in for-init")),
        };
        let cond = self.expression()?;
        self.expect(&TokenKind::Semi, "';'")?;
        let incr = match self.peek() {
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(false)?,
            _ => Stmt::ExprStmt(self.expression()?),
        };
        self.expect(&TokenKind::RParen, "')'")?;
        let mut body = self.block()?;
        body.push(incr);
        Ok(Stmt::Block(vec![init, Stmt::While { cond, body }]))
    }

    fn block(&mut self) -> Result<Vec<Stmt>, CompileError> {
        self.expect(&TokenKind::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            stmts.push(self.statement()?);
        }
        self.expect(&TokenKind::RBrace, "'}'")?;
        Ok(stmts)
    }
```

- [ ] **Step 4: Run, verify PASS**

Run: `cargo test parser`
Expected: 9 passed

- [ ] **Step 5: Commit**

```bash
git add src/parser.rs
git commit -m "feat: statement parsing incl. for-to-while desugar"
```

---

### Task 5: Codegen skeleton — literals, print, JIT, CLI, e2e harness

**Files:**
- Create: `src/value.rs`, `src/codegen.rs`, `tests/e2e.rs`, `tests/fixtures/literals.verb`, `tests/fixtures/literals.expected`
- Modify: `src/main.rs` (full CLI)

**Interfaces:**
- Consumes: `parser::parse`, `ast::*`
- Produces:
  - `value.rs` tag constants: `TAG_NIL=0, TAG_BOOL=1, TAG_INT=2, TAG_FLOAT=3, TAG_STR=4, TAG_CLOSURE=5` (all `u64`)
  - `Codegen::new(ctx: &Context) -> Codegen` — declares libc + builds `verb_print`
  - `Codegen::compile_program(&mut self, stmts: &[Stmt]) -> Result<(), CompileError>` — emits `i32 @main`
  - `Codegen::module(&self) -> &Module` for emit/JIT/AOT
  - Internal (later tasks): `gen_stmt`, `gen_expr -> Result<StructValue, CompileError>`, `make_val(tag, IntValue) -> StructValue`, `nil_val()`, `tag_of`, `payload_of`, `cstr`, `abort(msg)`, `check_or_abort(cond, msg)`, `malloc_bytes(n)`, `lookup(name)`, `self.scopes: Vec<HashMap<String, PointerValue>>`
  - CLI: `verb run <file> [--emit-llvm]`

- [ ] **Step 1: Write e2e harness + failing fixture**

`tests/e2e.rs`:
```rust
use std::process::Command;

fn run_ok(name: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(format!("tests/fixtures/{name}.expected")).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[allow(dead_code)]
fn run_err(name: &str, msg: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", &format!("tests/fixtures/{name}.verb")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(msg),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn literals() { run_ok("literals"); }

#[test]
fn emits_llvm_ir() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/literals.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define i32 @main"), "no main in IR: {ir}");
}
```

`tests/fixtures/literals.verb`:
```
print(42);
print(3.5);
print("hello");
print(true);
print(false);
print(nil);
```

`tests/fixtures/literals.expected`:
```
42
3.5
hello
true
false
nil
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test --test e2e`
Expected: FAIL (main.rs still smoke stub)

- [ ] **Step 3: Implement `src/value.rs`, `src/codegen.rs`, CLI**

`src/value.rs`:
```rust
//! Runtime value model: every Verb value is the LLVM struct { i8 tag, i64 payload }.
pub const TAG_NIL: u64 = 0;
pub const TAG_BOOL: u64 = 1;
pub const TAG_INT: u64 = 2;
pub const TAG_FLOAT: u64 = 3; // payload = f64 bits
pub const TAG_STR: u64 = 4;   // payload = ptr to NUL-terminated bytes
pub const TAG_CLOSURE: u64 = 5; // payload = ptr to { fn_ptr, i64 arity, env_ptr }
```

`src/codegen.rs`:
```rust
use std::collections::HashMap;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{PointerType, StructType};
use inkwell::values::{IntValue, PointerValue, StructValue};
use inkwell::AddressSpace;

use crate::ast::*;
use crate::error::CompileError;
use crate::value::*;

pub struct Codegen<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    value_ty: StructType<'ctx>,
    closure_ty: StructType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    scopes: Vec<HashMap<String, PointerValue<'ctx>>>,
    fn_counter: u32,
}

impl<'ctx> Codegen<'ctx> {
    pub fn new(ctx: &'ctx Context) -> Self {
        let module = ctx.create_module("verb");
        let builder = ctx.create_builder();
        let ptr_ty = ctx.ptr_type(AddressSpace::default());
        let value_ty = ctx.struct_type(&[ctx.i8_type().into(), ctx.i64_type().into()], false);
        let closure_ty =
            ctx.struct_type(&[ptr_ty.into(), ctx.i64_type().into(), ptr_ty.into()], false);
        let cg = Self {
            ctx, module, builder, value_ty, closure_ty, ptr_ty,
            scopes: Vec::new(), fn_counter: 0,
        };
        cg.declare_libc();
        cg.build_print_fn();
        cg
    }

    pub fn module(&self) -> &Module<'ctx> { &self.module }

    fn declare_libc(&self) {
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let pt = self.ptr_ty;
        self.module.add_function("printf", i32t.fn_type(&[pt.into()], true), None);
        self.module.add_function("malloc", pt.fn_type(&[i64t.into()], false), None);
        self.module.add_function("exit", self.ctx.void_type().fn_type(&[i32t.into()], false), None);
        self.module.add_function("strlen", i64t.fn_type(&[pt.into()], false), None);
        self.module.add_function("strcpy", pt.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("strcat", pt.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
    }

    // ----- value helpers -----

    fn make_val(&self, tag: u64, payload: IntValue<'ctx>) -> StructValue<'ctx> {
        let t = self.ctx.i8_type().const_int(tag, false);
        let v = self.value_ty.get_undef();
        let v = self.builder.build_insert_value(v, t, 0, "vt").unwrap().into_struct_value();
        self.builder.build_insert_value(v, payload, 1, "vp").unwrap().into_struct_value()
    }

    fn nil_val(&self) -> StructValue<'ctx> {
        self.make_val(TAG_NIL, self.ctx.i64_type().const_zero())
    }

    fn tag_of(&self, v: StructValue<'ctx>) -> IntValue<'ctx> {
        self.builder.build_extract_value(v, 0, "tag").unwrap().into_int_value()
    }

    fn payload_of(&self, v: StructValue<'ctx>) -> IntValue<'ctx> {
        self.builder.build_extract_value(v, 1, "pay").unwrap().into_int_value()
    }

    fn cstr(&self, s: &str) -> PointerValue<'ctx> {
        self.builder.build_global_string_ptr(s, "str").unwrap().as_pointer_value()
    }

    fn call_named(&self, name: &str, args: &[inkwell::values::BasicMetadataValueEnum<'ctx>])
        -> Option<inkwell::values::BasicValueEnum<'ctx>>
    {
        let f = self.module.get_function(name).unwrap();
        self.builder.build_call(f, args, "").unwrap().try_as_basic_value().left()
    }

    fn abort(&self, msg: &str) {
        let s = self.cstr(&format!("runtime error: {msg}\n"));
        self.call_named("printf", &[s.into()]);
        self.call_named("exit", &[self.ctx.i32_type().const_int(1, false).into()]);
        self.builder.build_unreachable().unwrap();
    }

    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("malloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }

    // ----- generated runtime helper: verb_print(value) -----

    fn build_print_fn(&self) {
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_print", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let tag = self.tag_of(v);
        let pay = self.payload_of(v);

        let nil_bb = self.ctx.append_basic_block(f, "nil");
        let bool_bb = self.ctx.append_basic_block(f, "bool");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let float_bb = self.ctx.append_basic_block(f, "float");
        let str_bb = self.ctx.append_basic_block(f, "string");
        let clos_bb = self.ctx.append_basic_block(f, "closure");
        let done = self.ctx.append_basic_block(f, "done");

        let i8t = self.ctx.i8_type();
        self.builder.build_switch(tag, done, &[
            (i8t.const_int(TAG_NIL, false), nil_bb),
            (i8t.const_int(TAG_BOOL, false), bool_bb),
            (i8t.const_int(TAG_INT, false), int_bb),
            (i8t.const_int(TAG_FLOAT, false), float_bb),
            (i8t.const_int(TAG_STR, false), str_bb),
            (i8t.const_int(TAG_CLOSURE, false), clos_bb),
        ]).unwrap();

        self.builder.position_at_end(nil_bb);
        self.call_named("printf", &[self.cstr("nil\n").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(bool_bb);
        let is_true = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, pay, self.ctx.i64_type().const_zero(), "istrue").unwrap();
        let ts = self.cstr("true\n");
        let fs = self.cstr("false\n");
        let sel = self.builder.build_select(is_true, ts, fs, "boolstr").unwrap();
        self.call_named("printf", &[sel.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(int_bb);
        self.call_named("printf", &[self.cstr("%lld\n").into(), pay.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(float_bb);
        let fv = self.builder.build_bitcast(pay, self.ctx.f64_type(), "f").unwrap();
        self.call_named("printf", &[self.cstr("%g\n").into(), fv.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(pay, self.ptr_ty, "sptr").unwrap();
        self.call_named("printf", &[self.cstr("%s\n").into(), sp.into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(clos_bb);
        self.call_named("printf", &[self.cstr("<fn>\n").into()]);
        self.builder.build_unconditional_branch(done).unwrap();

        self.builder.position_at_end(done);
        self.builder.build_return(None).unwrap();
    }

    // ----- program -----

    pub fn compile_program(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        let main_ty = self.ctx.i32_type().fn_type(&[], false);
        let main = self.module.add_function("main", main_ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.scopes.push(HashMap::new());
        self.gen_stmts(stmts)?;
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }

    fn cur_block_open(&self) -> bool {
        self.builder.get_insert_block().unwrap().get_terminator().is_none()
    }

    fn gen_stmts(&mut self, stmts: &[Stmt]) -> Result<(), CompileError> {
        for s in stmts {
            self.gen_stmt(s)?;
            if !self.cur_block_open() { break; } // dead code after return/abort
        }
        Ok(())
    }

    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::ExprStmt(e) => { self.gen_expr(e)?; Ok(()) }
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
        }
    }

    fn gen_expr(&mut self, expr: &Expr) -> Result<StructValue<'ctx>, CompileError> {
        match expr {
            Expr::Int(v) => Ok(self.make_val(TAG_INT, self.ctx.i64_type().const_int(*v as u64, true))),
            Expr::Float(v) => {
                let bits = self.builder.build_bitcast(
                    self.ctx.f64_type().const_float(*v), self.ctx.i64_type(), "bits",
                ).unwrap().into_int_value();
                Ok(self.make_val(TAG_FLOAT, bits))
            }
            Expr::Str(s) => {
                let p = self.cstr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
            Expr::Bool(b) => Ok(self.make_val(TAG_BOOL, self.ctx.i64_type().const_int(*b as u64, false))),
            Expr::Nil => Ok(self.nil_val()),
            Expr::Call { callee, args, line, col } => self.gen_call(callee, args, *line, *col),
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
        }
    }

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
        }
        Err(CompileError::new("codegen for user calls arrives in Task 8", line, col))
    }
}
```

`src/main.rs` (full replacement):
```rust
mod ast;
mod codegen;
mod error;
mod lexer;
mod parser;
mod value;

use std::process::exit;

use error::CompileError;

fn die(e: CompileError) -> ! {
    eprintln!("error [{}:{}]: {}", e.line, e.col, e.msg);
    exit(1)
}

fn usage() -> ! {
    eprintln!("usage: verb run <file.verb> [--emit-llvm]");
    eprintln!("       verb build <file.verb> -o <out> [--emit-llvm]");
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

    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => { eprintln!("error: cannot read {file}: {e}"); exit(1); }
    };
    let toks = lexer::lex(&src).unwrap_or_else(|e| die(e));
    let prog = parser::parse(toks).unwrap_or_else(|e| die(e));

    let ctx = inkwell::context::Context::create();
    let mut cg = codegen::Codegen::new(&ctx);
    cg.compile_program(&prog).unwrap_or_else(|e| die(e));

    if emit_llvm {
        println!("{}", cg.module().print_to_string().to_string());
    }

    match cmd {
        "run" => {
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
            build_aot(&cg, &out); // implemented in Task 9; stub for now
        }
        _ => usage(),
    }
}

fn build_aot(_cg: &codegen::Codegen, _out: &str) {
    eprintln!("build: not implemented yet");
    exit(1);
}
```

- [ ] **Step 4: Run, verify PASS**

Run: `cargo test --test e2e && cargo test`
Expected: `literals` and `emits_llvm_ir` pass; all unit tests still pass

- [ ] **Step 5: Commit**

```bash
git add src tests
git commit -m "feat: codegen skeleton — literals, print, JIT run, CLI, e2e harness"
```

---

### Task 6: Operators — arithmetic, comparison, equality, logic, concat, unary

**Files:**
- Modify: `src/codegen.rs`
- Create: `tests/fixtures/arith.verb|.expected`, `tests/fixtures/strings.verb|.expected`, `tests/fixtures/err_types.verb`, `tests/fixtures/err_divzero.verb`

**Interfaces:**
- Produces module-level generated helpers (all take/return `{i8,i64}` by value):
  `verb_truthy(value) -> i1`, `verb_add/sub/mul/div/mod(value,value) -> value`,
  `verb_lt/gt/le/ge(value,value) -> value(bool)`, `verb_eq(value,value) -> value(bool)`,
  `verb_concat(value,value) -> value(str)`, `verb_neg(value) -> value`.
  `gen_expr` handles all `Expr::Binary` / `Expr::Unary`.

- [ ] **Step 1: Add failing fixtures + tests**

`tests/fixtures/arith.verb`:
```
print(1 plus 2 mul 3);
print(7 div 2);
print(7.0 div 2);
print(7 mod 3);
print(minus 5);
print(1 plus 2.5);
print(1 lo 2);
print(2.5 hieq 2.5);
print(1 eqeq 1.0);
print("a" eqeq "a");
print("a" eqeq "b");
print(1 neq 2);
print(not nil);
print(false or 3);
print(false and 3);
print(nil eqeq false);
```

`tests/fixtures/arith.expected`:
```
7
3
3.5
1
-5
3.5
true
true
true
true
false
true
true
3
false
false
```

`tests/fixtures/strings.verb`:
```
print("hello" c " " c "world");
```

`tests/fixtures/strings.expected`:
```
hello world
```

`tests/fixtures/err_types.verb`:
```
print(1 plus "x");
```

`tests/fixtures/err_divzero.verb`:
```
print(1 div 0);
```

Add to `tests/e2e.rs`:
```rust
#[test]
fn arith() { run_ok("arith"); }

#[test]
fn strings() { run_ok("strings"); }

#[test]
fn type_error_aborts() { run_err("err_types", "operands must be numbers"); }

#[test]
fn div_zero_aborts() { run_err("err_divzero", "division by zero"); }
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test --test e2e`
Expected: new tests FAIL with "codegen not yet implemented"

- [ ] **Step 3: Implement helper builders** — add to `impl Codegen`, call the four `build_*` groups from `new()` right after `build_print_fn()`:

```rust
    // in new(): after cg.build_print_fn();
    //   cg.build_truthy_fn();
    //   cg.build_arith_fns();
    //   cg.build_cmp_fns();
    //   cg.build_eq_fn();
    //   cg.build_concat_fn();
    //   cg.build_neg_fn();

    /// truthy = tag != NIL && (tag != BOOL || payload != 0)   (branch-free)
    fn build_truthy_fn(&self) {
        let f = self.module.add_function(
            "verb_truthy", self.ctx.bool_type().fn_type(&[self.value_ty.into()], false), None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let tag = self.tag_of(v);
        let pay = self.payload_of(v);
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let not_nil = self.builder.build_int_compare(NE, tag, i8t.const_int(TAG_NIL, false), "nn").unwrap();
        let not_bool = self.builder.build_int_compare(NE, tag, i8t.const_int(TAG_BOOL, false), "nb").unwrap();
        let pay_nz = self.builder.build_int_compare(NE, pay, self.ctx.i64_type().const_zero(), "pnz").unwrap();
        let bool_ok = self.builder.build_or(not_bool, pay_nz, "bok").unwrap();
        let r = self.builder.build_and(not_nil, bool_ok, "truthy").unwrap();
        self.builder.build_return(Some(&r)).unwrap();
    }

    fn is_numeric(&self, tag: IntValue<'ctx>) -> IntValue<'ctx> {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let is_i = self.builder.build_int_compare(EQ, tag, i8t.const_int(TAG_INT, false), "isi").unwrap();
        let is_f = self.builder.build_int_compare(EQ, tag, i8t.const_int(TAG_FLOAT, false), "isf").unwrap();
        self.builder.build_or(is_i, is_f, "isnum").unwrap()
    }

    /// payload -> f64: int payload sitofp, float payload bitcast (select, both computed)
    fn to_f64(&self, tag: IntValue<'ctx>, pay: IntValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        use inkwell::IntPredicate::*;
        let is_int = self.builder.build_int_compare(
            EQ, tag, self.ctx.i8_type().const_int(TAG_INT, false), "isint").unwrap();
        let from_int = self.builder.build_signed_int_to_float(pay, self.ctx.f64_type(), "si").unwrap();
        let from_bits = self.builder.build_bitcast(pay, self.ctx.f64_type(), "fb").unwrap().into_float_value();
        self.builder.build_select(is_int, from_int, from_bits, "f").unwrap().into_float_value()
    }

    fn f64_val(&self, f: inkwell::values::FloatValue<'ctx>) -> StructValue<'ctx> {
        let bits = self.builder.build_bitcast(f, self.ctx.i64_type(), "bits").unwrap().into_int_value();
        self.make_val(TAG_FLOAT, bits)
    }

    fn bool_val(&self, b: IntValue<'ctx>) -> StructValue<'ctx> {
        let z = self.builder.build_int_z_extend(b, self.ctx.i64_type(), "bz").unwrap();
        self.make_val(TAG_BOOL, z)
    }

    fn build_arith_fns(&self) {
        for (name, op) in [("verb_add", BinOp::Add), ("verb_sub", BinOp::Sub),
                           ("verb_mul", BinOp::Mul), ("verb_div", BinOp::Div),
                           ("verb_mod", BinOp::Mod)] {
            self.build_arith_fn(name, op);
        }
    }

    fn build_arith_fn(&self, name: &str, op: BinOp) {
        use inkwell::IntPredicate::*;
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function(name, fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chknum");
        let flt_bb = self.ctx.append_basic_block(f, "float");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let i8t = self.ctx.i8_type();
        let ai = self.builder.build_int_compare(EQ, ta, i8t.const_int(TAG_INT, false), "ai").unwrap();
        let bi = self.builder.build_int_compare(EQ, tb, i8t.const_int(TAG_INT, false), "bi").unwrap();
        let both_int = self.builder.build_and(ai, bi, "bothint").unwrap();
        self.builder.build_conditional_branch(both_int, int_bb, chk_bb).unwrap();

        // integer path
        self.builder.position_at_end(int_bb);
        if matches!(op, BinOp::Div | BinOp::Mod) {
            let zero_bb = self.ctx.append_basic_block(f, "izero");
            let go_bb = self.ctx.append_basic_block(f, "igo");
            let nz = self.builder.build_int_compare(
                NE, pb, self.ctx.i64_type().const_zero(), "nz").unwrap();
            self.builder.build_conditional_branch(nz, go_bb, zero_bb).unwrap();
            self.builder.position_at_end(zero_bb);
            self.abort("division by zero");
            self.builder.position_at_end(go_bb);
        }
        let ir = match op {
            BinOp::Add => self.builder.build_int_add(pa, pb, "r").unwrap(),
            BinOp::Sub => self.builder.build_int_sub(pa, pb, "r").unwrap(),
            BinOp::Mul => self.builder.build_int_mul(pa, pb, "r").unwrap(),
            BinOp::Div => self.builder.build_int_signed_div(pa, pb, "r").unwrap(),
            BinOp::Mod => self.builder.build_int_signed_rem(pa, pb, "r").unwrap(),
            _ => unreachable!(),
        };
        let rv = self.make_val(TAG_INT, ir);
        self.builder.build_return(Some(&rv)).unwrap();

        // numeric check
        self.builder.position_at_end(chk_bb);
        let an = self.is_numeric(ta);
        let bn = self.is_numeric(tb);
        let both_num = self.builder.build_and(an, bn, "bothnum").unwrap();
        self.builder.build_conditional_branch(both_num, flt_bb, err_bb).unwrap();

        // float path (mixed promotes)
        self.builder.position_at_end(flt_bb);
        let fa = self.to_f64(ta, pa);
        let fb = self.to_f64(tb, pb);
        if matches!(op, BinOp::Div | BinOp::Mod) {
            let zero_bb = self.ctx.append_basic_block(f, "fzero");
            let go_bb = self.ctx.append_basic_block(f, "fgo");
            let nz = self.builder.build_float_compare(
                inkwell::FloatPredicate::ONE, fb, self.ctx.f64_type().const_zero(), "fnz").unwrap();
            self.builder.build_conditional_branch(nz, go_bb, zero_bb).unwrap();
            self.builder.position_at_end(zero_bb);
            self.abort("division by zero");
            self.builder.position_at_end(go_bb);
        }
        let fr = match op {
            BinOp::Add => self.builder.build_float_add(fa, fb, "fr").unwrap(),
            BinOp::Sub => self.builder.build_float_sub(fa, fb, "fr").unwrap(),
            BinOp::Mul => self.builder.build_float_mul(fa, fb, "fr").unwrap(),
            BinOp::Div => self.builder.build_float_div(fa, fb, "fr").unwrap(),
            BinOp::Mod => self.builder.build_float_rem(fa, fb, "fr").unwrap(),
            _ => unreachable!(),
        };
        let rv = self.f64_val(fr);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort("operands must be numbers");
    }

    fn build_cmp_fns(&self) {
        use inkwell::{FloatPredicate as FP, IntPredicate as IP};
        for (name, ip, fp) in [
            ("verb_lt", IP::SLT, FP::OLT), ("verb_gt", IP::SGT, FP::OGT),
            ("verb_le", IP::SLE, FP::OLE), ("verb_ge", IP::SGE, FP::OGE),
        ] {
            let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
            let f = self.module.add_function(name, fnty, None);
            let entry = self.ctx.append_basic_block(f, "entry");
            let int_bb = self.ctx.append_basic_block(f, "int");
            let chk_bb = self.ctx.append_basic_block(f, "chk");
            let flt_bb = self.ctx.append_basic_block(f, "flt");
            let err_bb = self.ctx.append_basic_block(f, "err");

            self.builder.position_at_end(entry);
            let a = f.get_nth_param(0).unwrap().into_struct_value();
            let b = f.get_nth_param(1).unwrap().into_struct_value();
            let (ta, pa) = (self.tag_of(a), self.payload_of(a));
            let (tb, pb) = (self.tag_of(b), self.payload_of(b));
            let i8t = self.ctx.i8_type();
            let ai = self.builder.build_int_compare(IP::EQ, ta, i8t.const_int(TAG_INT, false), "ai").unwrap();
            let bi = self.builder.build_int_compare(IP::EQ, tb, i8t.const_int(TAG_INT, false), "bi").unwrap();
            let both_int = self.builder.build_and(ai, bi, "bi2").unwrap();
            self.builder.build_conditional_branch(both_int, int_bb, chk_bb).unwrap();

            self.builder.position_at_end(int_bb);
            let r = self.builder.build_int_compare(ip, pa, pb, "c").unwrap();
            let rv = self.bool_val(r);
            self.builder.build_return(Some(&rv)).unwrap();

            self.builder.position_at_end(chk_bb);
            let an = self.is_numeric(ta);
            let bn = self.is_numeric(tb);
            let both = self.builder.build_and(an, bn, "bn2").unwrap();
            self.builder.build_conditional_branch(both, flt_bb, err_bb).unwrap();

            self.builder.position_at_end(flt_bb);
            let fa = self.to_f64(ta, pa);
            let fb = self.to_f64(tb, pb);
            let r = self.builder.build_float_compare(fp, fa, fb, "fc").unwrap();
            let rv = self.bool_val(r);
            self.builder.build_return(Some(&rv)).unwrap();

            self.builder.position_at_end(err_bb);
            self.abort("operands must be numbers");
        }
    }

    fn build_eq_fn(&self) {
        use inkwell::{FloatPredicate as FP, IntPredicate as IP};
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function("verb_eq", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let same_bb = self.ctx.append_basic_block(f, "same");
        let raw_bb = self.ctx.append_basic_block(f, "raw");
        let feq_bb = self.ctx.append_basic_block(f, "feq");
        let seq_bb = self.ctx.append_basic_block(f, "seq");
        let diff_bb = self.ctx.append_basic_block(f, "diff");
        let mix_bb = self.ctx.append_basic_block(f, "mixed");
        let false_bb = self.ctx.append_basic_block(f, "no");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let same = self.builder.build_int_compare(IP::EQ, ta, tb, "same").unwrap();
        self.builder.build_conditional_branch(same, same_bb, diff_bb).unwrap();

        let i8t = self.ctx.i8_type();
        self.builder.position_at_end(same_bb);
        self.builder.build_switch(ta, raw_bb, &[
            (i8t.const_int(TAG_FLOAT, false), feq_bb),
            (i8t.const_int(TAG_STR, false), seq_bb),
        ]).unwrap();

        // nil/bool/int/closure: payload equality
        self.builder.position_at_end(raw_bb);
        let r = self.builder.build_int_compare(IP::EQ, pa, pb, "pe").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(feq_bb);
        let fa = self.builder.build_bitcast(pa, self.ctx.f64_type(), "fa").unwrap().into_float_value();
        let fb = self.builder.build_bitcast(pb, self.ctx.f64_type(), "fb").unwrap().into_float_value();
        let r = self.builder.build_float_compare(FP::OEQ, fa, fb, "fe").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(seq_bb);
        let sa = self.builder.build_int_to_ptr(pa, self.ptr_ty, "sa").unwrap();
        let sb = self.builder.build_int_to_ptr(pb, self.ptr_ty, "sb").unwrap();
        let c = self.call_named("strcmp", &[sa.into(), sb.into()]).unwrap().into_int_value();
        let r = self.builder.build_int_compare(IP::EQ, c, self.ctx.i32_type().const_zero(), "se").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        // different tags: numbers cross-compare, everything else unequal
        self.builder.position_at_end(diff_bb);
        let an = self.is_numeric(ta);
        let bn = self.is_numeric(tb);
        let both = self.builder.build_and(an, bn, "bn").unwrap();
        self.builder.build_conditional_branch(both, mix_bb, false_bb).unwrap();

        self.builder.position_at_end(mix_bb);
        let fa = self.to_f64(ta, pa);
        let fb = self.to_f64(tb, pb);
        let r = self.builder.build_float_compare(FP::OEQ, fa, fb, "me").unwrap();
        let rv = self.bool_val(r);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(false_bb);
        let rv = self.bool_val(self.ctx.bool_type().const_zero());
        self.builder.build_return(Some(&rv)).unwrap();
    }

    fn build_concat_fn(&self) {
        use inkwell::IntPredicate::*;
        let fnty = self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false);
        let f = self.module.add_function("verb_concat", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let ok_bb = self.ctx.append_basic_block(f, "ok");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let a = f.get_nth_param(0).unwrap().into_struct_value();
        let b = f.get_nth_param(1).unwrap().into_struct_value();
        let (ta, pa) = (self.tag_of(a), self.payload_of(a));
        let (tb, pb) = (self.tag_of(b), self.payload_of(b));
        let i8t = self.ctx.i8_type();
        let as_ = self.builder.build_int_compare(EQ, ta, i8t.const_int(TAG_STR, false), "as").unwrap();
        let bs = self.builder.build_int_compare(EQ, tb, i8t.const_int(TAG_STR, false), "bs").unwrap();
        let both = self.builder.build_and(as_, bs, "both").unwrap();
        self.builder.build_conditional_branch(both, ok_bb, err_bb).unwrap();

        self.builder.position_at_end(ok_bb);
        let sa = self.builder.build_int_to_ptr(pa, self.ptr_ty, "sa").unwrap();
        let sb = self.builder.build_int_to_ptr(pb, self.ptr_ty, "sb").unwrap();
        let la = self.call_named("strlen", &[sa.into()]).unwrap().into_int_value();
        let lb = self.call_named("strlen", &[sb.into()]).unwrap().into_int_value();
        let sum = self.builder.build_int_add(la, lb, "sum").unwrap();
        let size = self.builder.build_int_add(sum, self.ctx.i64_type().const_int(1, false), "sz").unwrap();
        let buf = self.call_named("malloc", &[size.into()]).unwrap().into_pointer_value();
        self.call_named("strcpy", &[buf.into(), sa.into()]);
        self.call_named("strcat", &[buf.into(), sb.into()]);
        let bits = self.builder.build_ptr_to_int(buf, self.ctx.i64_type(), "bits").unwrap();
        let rv = self.make_val(TAG_STR, bits);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort("operands of 'c' must be strings");
    }

    fn build_neg_fn(&self) {
        use inkwell::IntPredicate::*;
        let fnty = self.value_ty.fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_neg", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let int_bb = self.ctx.append_basic_block(f, "int");
        let chk_bb = self.ctx.append_basic_block(f, "chk");
        let flt_bb = self.ctx.append_basic_block(f, "flt");
        let err_bb = self.ctx.append_basic_block(f, "err");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let i8t = self.ctx.i8_type();
        let isi = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_INT, false), "isi").unwrap();
        self.builder.build_conditional_branch(isi, int_bb, chk_bb).unwrap();

        self.builder.position_at_end(int_bb);
        let n = self.builder.build_int_neg(p, "n").unwrap();
        let rv = self.make_val(TAG_INT, n);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(chk_bb);
        let isf = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_FLOAT, false), "isf").unwrap();
        self.builder.build_conditional_branch(isf, flt_bb, err_bb).unwrap();

        self.builder.position_at_end(flt_bb);
        let fv = self.builder.build_bitcast(p, self.ctx.f64_type(), "f").unwrap().into_float_value();
        let n = self.builder.build_float_neg(fv, "fn").unwrap();
        let rv = self.f64_val(n);
        self.builder.build_return(Some(&rv)).unwrap();

        self.builder.position_at_end(err_bb);
        self.abort("operand must be a number");
    }
```

- [ ] **Step 4: Wire into `gen_expr`** — replace the `other =>` arm's coverage of Binary/Unary:

```rust
            Expr::Binary { op, lhs, rhs } => self.gen_binary(*op, lhs, rhs),
            Expr::Unary { op, expr } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => Ok(self.call_named("verb_neg", &[v.into()])
                        .unwrap().into_struct_value()),
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
```

And add:

```rust
    fn gen_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr)
        -> Result<StructValue<'ctx>, CompileError>
    {
        // short-circuit: 'and'/'or' return operand values (Lox semantics)
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.gen_expr(lhs)?;
            let t = self.call_named("verb_truthy", &[l.into()]).unwrap().into_int_value();
            let cur_fn = self.builder.get_insert_block().unwrap().get_parent().unwrap();
            let lhs_end = self.builder.get_insert_block().unwrap();
            let rhs_bb = self.ctx.append_basic_block(cur_fn, "sc.rhs");
            let merge = self.ctx.append_basic_block(cur_fn, "sc.end");
            match op {
                BinOp::And => self.builder.build_conditional_branch(t, rhs_bb, merge).unwrap(),
                _ => self.builder.build_conditional_branch(t, merge, rhs_bb).unwrap(),
            };
            self.builder.position_at_end(rhs_bb);
            let r = self.gen_expr(rhs)?;
            let rhs_end = self.builder.get_insert_block().unwrap();
            self.builder.build_unconditional_branch(merge).unwrap();
            self.builder.position_at_end(merge);
            let phi = self.builder.build_phi(self.value_ty, "sc").unwrap();
            phi.add_incoming(&[(&l, lhs_end), (&r, rhs_end)]);
            return Ok(phi.as_basic_value().into_struct_value());
        }

        let l = self.gen_expr(lhs)?;
        let r = self.gen_expr(rhs)?;
        let helper = match op {
            BinOp::Add => "verb_add", BinOp::Sub => "verb_sub", BinOp::Mul => "verb_mul",
            BinOp::Div => "verb_div", BinOp::Mod => "verb_mod",
            BinOp::Lt => "verb_lt", BinOp::Gt => "verb_gt",
            BinOp::Le => "verb_le", BinOp::Ge => "verb_ge",
            BinOp::Eq | BinOp::Ne => "verb_eq",
            BinOp::Concat => "verb_concat",
            BinOp::And | BinOp::Or => unreachable!(),
        };
        let out = self.call_named(helper, &[l.into(), r.into()]).unwrap().into_struct_value();
        if matches!(op, BinOp::Ne) {
            let p = self.payload_of(out);
            let flipped = self.builder.build_xor(
                p, self.ctx.i64_type().const_int(1, false), "ne").unwrap();
            return Ok(self.make_val(TAG_BOOL, flipped));
        }
        Ok(out)
    }
```

- [ ] **Step 5: Run, verify PASS**

Run: `cargo test --test e2e`
Expected: arith, strings, type_error_aborts, div_zero_aborts pass

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs tests
git commit -m "feat: operators via generated runtime helper functions"
```

---

### Task 7: Variables, scopes, control flow

**Files:**
- Modify: `src/codegen.rs` (`gen_stmt` arms: Assign, Reassign, Block, If, While; `gen_expr` arm: Var)
- Create: `tests/fixtures/vars.verb|.expected`, `tests/fixtures/control.verb|.expected`

**Interfaces:**
- Produces: variables as `malloc`ed 16-byte cells; `self.scopes` stack with `lookup(name) -> Option<PointerValue>`; undefined variable read/reassign = CompileError with the node's line/col.

- [ ] **Step 1: Failing fixtures + tests**

`tests/fixtures/vars.verb`:
```
assign x 10;
assign name "compiler";
x be x plus 1;
print(x);
print(name);
{
  assign x 99;
  print(x);
}
print(x);
```

`tests/fixtures/vars.expected`:
```
11
compiler
99
11
```

`tests/fixtures/control.verb`:
```
assign n 3;
if (n hi 5) {
  print("big");
} else if (n hi 2) {
  print("mid");
} else {
  print("small");
}
assign i 0;
while (i lo 3) {
  print(i);
  i be i plus 1;
}
for (assign j 0; j lo 3; j be j plus 1) {
  print(j mul 10);
}
```

`tests/fixtures/control.expected`:
```
mid
0
1
2
0
10
20
```

Add to `tests/e2e.rs`:
```rust
#[test]
fn vars() { run_ok("vars"); }

#[test]
fn control() { run_ok("control"); }

#[test]
fn undefined_var_is_compile_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/err_undef.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("undefined variable"), "stderr: {stderr}");
}
```

`tests/fixtures/err_undef.verb`:
```
print(ghost);
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test --test e2e`
Expected: vars/control/undefined FAIL

- [ ] **Step 3: Implement** — add to `impl Codegen`:

```rust
    fn lookup(&self, name: &str) -> Option<PointerValue<'ctx>> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }
```

Replace `gen_stmt` with:

```rust
    fn gen_stmt(&mut self, stmt: &Stmt) -> Result<(), CompileError> {
        match stmt {
            Stmt::ExprStmt(e) => { self.gen_expr(e)?; Ok(()) }
            Stmt::Assign { name, value } => {
                let v = self.gen_expr(value)?;
                let cell = self.malloc_bytes(16);
                self.builder.build_store(cell, v).unwrap();
                self.scopes.last_mut().unwrap().insert(name.clone(), cell);
                Ok(())
            }
            Stmt::Reassign { name, value, line, col } => {
                let cell = self.lookup(name).ok_or_else(|| CompileError::new(
                    format!("undefined variable '{name}' (declare with 'assign')"), *line, *col))?;
                let v = self.gen_expr(value)?;
                self.builder.build_store(cell, v).unwrap();
                Ok(())
            }
            Stmt::Block(stmts) => {
                self.scopes.push(HashMap::new());
                let r = self.gen_stmts(stmts);
                self.scopes.pop();
                r
            }
            Stmt::If { cond, then_body, else_body } => {
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let then_bb = self.ctx.append_basic_block(f, "if.then");
                let else_bb = self.ctx.append_basic_block(f, "if.else");
                let merge = self.ctx.append_basic_block(f, "if.end");
                self.builder.build_conditional_branch(t, then_bb, else_bb).unwrap();

                self.builder.position_at_end(then_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(then_body)?;
                self.scopes.pop();
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }

                self.builder.position_at_end(else_bb);
                if let Some(eb) = else_body {
                    self.scopes.push(HashMap::new());
                    self.gen_stmts(eb)?;
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }
                self.builder.position_at_end(merge);
                Ok(())
            }
            Stmt::While { cond, body } => {
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let cond_bb = self.ctx.append_basic_block(f, "while.cond");
                let body_bb = self.ctx.append_basic_block(f, "while.body");
                let end_bb = self.ctx.append_basic_block(f, "while.end");
                self.builder.build_unconditional_branch(cond_bb).unwrap();

                self.builder.position_at_end(cond_bb);
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.builder.build_conditional_branch(t, body_bb, end_bb).unwrap();

                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                self.scopes.pop();
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
                self.builder.position_at_end(end_bb);
                Ok(())
            }
            other => Err(CompileError::new(
                format!("codegen not yet implemented for {other:?}"), 0, 0)),
        }
    }
```

Add `Expr::Var` arm to `gen_expr`:

```rust
            Expr::Var(name, line, col) => {
                let cell = self.lookup(name).ok_or_else(|| CompileError::new(
                    format!("undefined variable '{name}'"), *line, *col))?;
                Ok(self.builder.build_load(self.value_ty, cell, name).unwrap().into_struct_value())
            }
```

- [ ] **Step 4: Run, verify PASS**

Run: `cargo test`
Expected: all pass (unit + e2e)

- [ ] **Step 5: Commit**

```bash
git add src/codegen.rs tests
git commit -m "feat: variables as heap cells, scopes, if/while codegen"
```

---

### Task 8: Functions, calls, closures

**Files:**
- Modify: `src/codegen.rs` (`Stmt::Fn`, `Stmt::Return`, user-call path in `gen_call`; add `free_vars`, `check_or_abort`)
- Create: `tests/fixtures/funcs.verb|.expected`, `tests/fixtures/closures.verb|.expected`, `tests/fixtures/err_call.verb`, `tests/fixtures/err_arity.verb`

**Interfaces:**
- Consumes: Task 5-7 helpers (`malloc_bytes`, `lookup`, `make_val`, `nil_val`, `abort`)
- Produces:
  - User functions as LLVM fns `verb_user_<n>_<name>` with signature `value(ptr env, value p0, ..., value pk)`
  - Closure heap object typed `closure_ty = {ptr fn, i64 arity, ptr env}`; env = malloc'd array of cell pointers (captured by reference)
  - `check_or_abort(&mut self, ok: IntValue, msg: &str)` — splits block, aborts on false, leaves builder in ok-block (reused verbatim by any later runtime check)
  - Known v1 limitation (document in README, Task 9): captured names must be declared before the `fn` statement; no mutual recursion.

- [ ] **Step 1: Failing fixtures + tests**

`tests/fixtures/funcs.verb`:
```
fn add(a, b) {
  return a plus b;
}
print(add(1, 2));
fn fib(n) {
  if (n lo 2) { return n; }
  return fib(n minus 1) plus fib(n minus 2);
}
print(fib(10));
fn greet() {
  print("hi");
}
greet();
print(greet());
```

`tests/fixtures/funcs.expected`:
```
3
55
hi
hi
nil
```

`tests/fixtures/closures.verb`:
```
fn make_counter() {
  assign n 0;
  fn inc() {
    n be n plus 1;
    return n;
  }
  return inc;
}
assign counter make_counter();
print(counter());
print(counter());
assign other make_counter();
print(other());
print(counter());

fn adder(x) {
  fn add(y) {
    return x plus y;
  }
  return add;
}
assign add5 adder(5);
print(add5(3));
```

`tests/fixtures/closures.expected`:
```
1
2
1
3
8
```

`tests/fixtures/err_call.verb`:
```
assign x 5;
x(1);
```

`tests/fixtures/err_arity.verb`:
```
fn f(a) { return a; }
f(1, 2);
```

Add to `tests/e2e.rs`:
```rust
#[test]
fn funcs() { run_ok("funcs"); }

#[test]
fn closures() { run_ok("closures"); }

#[test]
fn calling_non_function_aborts() { run_err("err_call", "can only call functions"); }

#[test]
fn wrong_arity_aborts() { run_err("err_arity", "wrong number of arguments"); }
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test --test e2e`
Expected: 4 new tests FAIL

- [ ] **Step 3: Implement free-variable analysis** — add at bottom of `src/codegen.rs` (module level, not in impl):

```rust
use std::collections::HashSet;

/// Names referenced by `stmts` that are not bound within them (params pre-seeded
/// by caller). "print" is a builtin, never captured. Order = first reference.
fn free_vars(stmts: &[Stmt], bound: &HashSet<String>) -> Vec<String> {
    let mut bound = bound.clone();
    let mut out = Vec::new();
    fv_stmts(stmts, &mut bound, &mut out);
    out
}

fn fv_stmts(stmts: &[Stmt], bound: &mut HashSet<String>, out: &mut Vec<String>) {
    for s in stmts {
        fv_stmt(s, bound, out);
    }
}

fn fv_stmt(s: &Stmt, bound: &mut HashSet<String>, out: &mut Vec<String>) {
    match s {
        Stmt::Assign { name, value } => {
            fv_expr(value, bound, out);
            bound.insert(name.clone());
        }
        Stmt::Reassign { name, value, .. } => {
            fv_name(name, bound, out);
            fv_expr(value, bound, out);
        }
        Stmt::ExprStmt(e) | Stmt::Return { value: Some(e) } => fv_expr(e, bound, out),
        Stmt::Return { value: None } => {}
        Stmt::Block(b) => {
            let mut inner = bound.clone();
            fv_stmts(b, &mut inner, out);
        }
        Stmt::If { cond, then_body, else_body } => {
            fv_expr(cond, bound, out);
            let mut t = bound.clone();
            fv_stmts(then_body, &mut t, out);
            if let Some(eb) = else_body {
                let mut e = bound.clone();
                fv_stmts(eb, &mut e, out);
            }
        }
        Stmt::While { cond, body } => {
            fv_expr(cond, bound, out);
            let mut b = bound.clone();
            fv_stmts(body, &mut b, out);
        }
        Stmt::Fn { name, params, body, .. } => {
            bound.insert(name.clone());
            let mut inner = bound.clone();
            for p in params { inner.insert(p.clone()); }
            fv_stmts(body, &mut inner, out);
        }
    }
}

fn fv_expr(e: &Expr, bound: &HashSet<String>, out: &mut Vec<String>) {
    match e {
        Expr::Var(n, ..) => fv_name(n, bound, out),
        Expr::Binary { lhs, rhs, .. } => {
            fv_expr(lhs, bound, out);
            fv_expr(rhs, bound, out);
        }
        Expr::Unary { expr, .. } => fv_expr(expr, bound, out),
        Expr::Call { callee, args, .. } => {
            fv_expr(callee, bound, out);
            for a in args { fv_expr(a, bound, out); }
        }
        _ => {}
    }
}

fn fv_name(n: &str, bound: &HashSet<String>, out: &mut Vec<String>) {
    if n != "print" && !bound.contains(n) && !out.iter().any(|x| x == n) {
        out.push(n.to_string());
    }
}
```

- [ ] **Step 4: Implement Fn/Return/call codegen** — add to `impl Codegen`:

```rust
    fn check_or_abort(&mut self, ok: IntValue<'ctx>, msg: &str) {
        let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let ok_bb = self.ctx.append_basic_block(f, "chk.ok");
        let err_bb = self.ctx.append_basic_block(f, "chk.err");
        self.builder.build_conditional_branch(ok, ok_bb, err_bb).unwrap();
        self.builder.position_at_end(err_bb);
        self.abort(msg);
        self.builder.position_at_end(ok_bb);
    }
```

Add `Stmt::Return` and `Stmt::Fn` arms to `gen_stmt` (replacing the leftover `other =>` arm entirely — after this task every variant is covered, so the match becomes exhaustive):

```rust
            Stmt::Return { value } => {
                let v = match value {
                    Some(e) => self.gen_expr(e)?,
                    None => self.nil_val(),
                };
                self.builder.build_return(Some(&v)).unwrap();
                Ok(())
            }
            Stmt::Fn { name, params, body, line, col } => {
                // 1. cell for the fn name FIRST — recursion captures this cell
                let name_cell = self.malloc_bytes(16);
                let nil = self.nil_val();
                self.builder.build_store(name_cell, nil).unwrap();
                self.scopes.last_mut().unwrap().insert(name.clone(), name_cell);

                // 2. captures: free vars of body minus params
                let mut seed: HashSet<String> = params.iter().cloned().collect();
                seed.insert(name.clone());
                let caps = free_vars(body, &seed);
                let mut cap_cells = Vec::new();
                for c in &caps {
                    cap_cells.push(self.lookup(c).ok_or_else(|| CompileError::new(
                        format!("undefined variable '{c}' captured by fn '{name}'"), *line, *col))?);
                }
                // name itself is always captured so recursive calls resolve
                let caps: Vec<String> =
                    std::iter::once(name.clone()).chain(caps).collect();
                let cap_cells: Vec<PointerValue> =
                    std::iter::once(name_cell).chain(cap_cells).collect();

                // 3. env array of cell pointers
                let env = self.malloc_bytes(8 * cap_cells.len() as u64);
                for (i, cc) in cap_cells.iter().enumerate() {
                    let slot = unsafe {
                        self.builder.build_gep(
                            self.ptr_ty, env,
                            &[self.ctx.i64_type().const_int(i as u64, false)], "slot",
                        ).unwrap()
                    };
                    self.builder.build_store(slot, *cc).unwrap();
                }

                // 4. compile the LLVM function
                let mut ptypes: Vec<inkwell::types::BasicMetadataTypeEnum> =
                    vec![self.ptr_ty.into()];
                ptypes.extend(params.iter().map(|_| self.value_ty.into()));
                let fnty = self.value_ty.fn_type(&ptypes, false);
                self.fn_counter += 1;
                let lf = self.module.add_function(
                    &format!("verb_user_{}_{}", self.fn_counter, name), fnty, None);

                let saved_block = self.builder.get_insert_block().unwrap();
                let saved_scopes = std::mem::take(&mut self.scopes);

                let entry = self.ctx.append_basic_block(lf, "entry");
                self.builder.position_at_end(entry);
                let mut top = HashMap::new();
                let envp = lf.get_nth_param(0).unwrap().into_pointer_value();
                for (i, cname) in caps.iter().enumerate() {
                    let slot = unsafe {
                        self.builder.build_gep(
                            self.ptr_ty, envp,
                            &[self.ctx.i64_type().const_int(i as u64, false)], "cslot",
                        ).unwrap()
                    };
                    let cellp = self.builder.build_load(self.ptr_ty, slot, cname)
                        .unwrap().into_pointer_value();
                    top.insert(cname.clone(), cellp);
                }
                self.scopes = vec![top];
                self.scopes.push(HashMap::new()); // param/local scope shadows captures
                for (i, p) in params.iter().enumerate() {
                    let pv = lf.get_nth_param(i as u32 + 1).unwrap().into_struct_value();
                    let cell = self.malloc_bytes(16);
                    self.builder.build_store(cell, pv).unwrap();
                    self.scopes.last_mut().unwrap().insert(p.clone(), cell);
                }
                self.gen_stmts(body)?;
                if self.cur_block_open() {
                    let nil = self.nil_val();
                    self.builder.build_return(Some(&nil)).unwrap();
                }
                self.scopes = saved_scopes;
                self.builder.position_at_end(saved_block);

                // 5. closure object {fn_ptr, arity, env}
                let clos = self.malloc_bytes(24);
                let fp_slot = self.builder
                    .build_struct_gep(self.closure_ty, clos, 0, "fp").unwrap();
                self.builder.build_store(
                    fp_slot, lf.as_global_value().as_pointer_value()).unwrap();
                let ar_slot = self.builder
                    .build_struct_gep(self.closure_ty, clos, 1, "ar").unwrap();
                self.builder.build_store(
                    ar_slot, self.ctx.i64_type().const_int(params.len() as u64, false)).unwrap();
                let env_slot = self.builder
                    .build_struct_gep(self.closure_ty, clos, 2, "ev").unwrap();
                self.builder.build_store(env_slot, env).unwrap();

                let bits = self.builder
                    .build_ptr_to_int(clos, self.ctx.i64_type(), "cbits").unwrap();
                let cv = self.make_val(TAG_CLOSURE, bits);
                self.builder.build_store(name_cell, cv).unwrap();
                Ok(())
            }
```

Replace the user-call fallthrough in `gen_call` (`Err(... Task 8 ...)`) with:

```rust
        let cv = self.gen_expr(callee)?;
        let (tag, pay) = (self.tag_of(cv), self.payload_of(cv));
        let is_clos = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, tag,
            self.ctx.i8_type().const_int(TAG_CLOSURE, false), "isclos").unwrap();
        self.check_or_abort(is_clos, "can only call functions");

        let clos = self.builder.build_int_to_ptr(pay, self.ptr_ty, "clos").unwrap();
        let fp_slot = self.builder.build_struct_gep(self.closure_ty, clos, 0, "fps").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fp_slot, "fp").unwrap().into_pointer_value();
        let ar_slot = self.builder.build_struct_gep(self.closure_ty, clos, 1, "ars").unwrap();
        let ar = self.builder.build_load(self.ctx.i64_type(), ar_slot, "ar").unwrap().into_int_value();
        let env_slot = self.builder.build_struct_gep(self.closure_ty, clos, 2, "evs").unwrap();
        let env = self.builder.build_load(self.ptr_ty, env_slot, "env").unwrap().into_pointer_value();

        let arity_ok = self.builder.build_int_compare(
            inkwell::IntPredicate::EQ, ar,
            self.ctx.i64_type().const_int(args.len() as u64, false), "arok").unwrap();
        self.check_or_abort(arity_ok, "wrong number of arguments");

        let mut argv: Vec<inkwell::values::BasicMetadataValueEnum> = vec![env.into()];
        for a in args {
            argv.push(self.gen_expr(a)?.into());
        }
        let mut ptypes: Vec<inkwell::types::BasicMetadataTypeEnum> = vec![self.ptr_ty.into()];
        ptypes.extend(args.iter().map(|_| self.value_ty.into()));
        let fnty = self.value_ty.fn_type(&ptypes, false);
        let ret = self.builder.build_indirect_call(fnty, fp, &argv, "call").unwrap();
        Ok(ret.try_as_basic_value().left().unwrap().into_struct_value())
```

(Note `line`/`col` params of `gen_call` become unused for the user path — prefix with `_` or keep for the print error.)

- [ ] **Step 5: Run, verify PASS**

Run: `cargo test`
Expected: all pass, incl. funcs/closures/err_call/err_arity

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs tests
git commit -m "feat: first-class functions and closures with by-reference capture"
```

---

### Task 9: AOT build, IR snapshot test, README

**Files:**
- Modify: `src/main.rs` (real `build_aot`)
- Create: `README.md`
- Modify: `tests/e2e.rs` (AOT test, IR snapshot test)

**Interfaces:**
- Consumes: `Codegen::module()`
- Produces: `verb build f.verb -o out` → native executable (TargetMachine object emit + `cc` link)

- [ ] **Step 1: Failing tests** — add to `tests/e2e.rs`:

```rust
#[test]
fn aot_build_produces_working_binary() {
    let dir = std::env::temp_dir().join("verb_aot_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("closures_bin");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/closures.verb", "-o", bin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&bin).output().unwrap();
    let expected = std::fs::read_to_string("tests/fixtures/closures.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

#[test]
fn ir_snapshot_contains_runtime_helpers() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/arith.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    for sym in ["define i32 @main", "verb_add", "verb_print", "verb_truthy", "declare"] {
        assert!(ir.contains(sym), "IR missing {sym}");
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test --test e2e aot`
Expected: FAIL ("build: not implemented yet")

- [ ] **Step 3: Implement `build_aot`** in `src/main.rs`:

```rust
fn build_aot(cg: &codegen::Codegen, out: &str) {
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

    let obj = format!("{out}.o");
    tm.write_to_file(cg.module(), FileType::Object, obj.as_ref())
        .unwrap_or_else(|e| { eprintln!("object emit error: {e}"); exit(1); });

    let status = std::process::Command::new("cc")
        .args([obj.as_str(), "-o", out])
        .status()
        .unwrap_or_else(|e| { eprintln!("cc failed to start: {e}"); exit(1); });
    let _ = std::fs::remove_file(&obj);
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
}
```

(Adjust the earlier stub's signature/callsite accordingly.)

- [ ] **Step 4: Run, verify PASS**

Run: `cargo test`
Expected: everything passes

- [ ] **Step 5: Write `README.md`**

```markdown
# Verb

A tiny dynamically typed language compiled to LLVM IR. Educational project:
lexer → parser → AST → LLVM IR (inkwell) → JIT or native binary.

## Requirements

- Rust (2021)
- LLVM 20.1 (`brew install llvm`) — path wired via `.cargo/config.toml`
- A C compiler (`cc`) for linking AOT builds

## Usage

    cargo run -- run examples/hello.verb          # JIT
    cargo run -- run examples/hello.verb --emit-llvm
    cargo run -- build examples/hello.verb -o hello   # native binary

## Language

See `docs/superpowers/specs/2026-07-19-verb-compiler-design.md` for the spec.

    %% comment
    assign x 41;
    x be x plus 1;
    fn make_counter() {
      assign n 0;
      fn inc() { n be n plus 1; return n; }
      return inc;
    }
    assign counter make_counter();
    print(counter());   %% 1

## Known v1 limitations

- No GC — heap allocations are never freed
- No arrays/maps, no `break`/`continue`, no anonymous functions
- Captured variables must be declared before the `fn` statement
  (no mutual recursion)
- Shadowing the builtin `print` has no effect — calls named `print`
  always hit the builtin
```

Also create `examples/hello.verb`:
```
print("hello from verb");
```

- [ ] **Step 6: Full verification**

Run: `cargo test && cargo run -- run examples/hello.verb`
Expected: all tests pass; prints `hello from verb`

- [ ] **Step 7: Commit**

```bash
git add src/main.rs tests README.md examples
git commit -m "feat: AOT build via TargetMachine + cc link; README"
```
