# Verb Compiler — Design Spec (v1)

Date: 2026-07-19
Status: approved

## Purpose

Educational compiler for **Verb**, a dynamically typed toy language, compiled to LLVM IR. Goal: learn compiler construction end-to-end (lexer → parser → AST → LLVM IR → binary), not build a production language.

## Language: Verb (v1)

Dynamic typing, C-like braces, Lox-adjacent structure with word-operators.

### Comments

```
%% line comment
!?! block comment !?!
```

### Types (dynamic, no annotations)

| Type | Example | Notes |
|---|---|---|
| Int | `42` | 64-bit signed |
| Float | `3.24` | f64. Int and Float are **separate** types; mixed arithmetic promotes Int → Float |
| String | `"hello"` | immutable; escapes: `\n \t \" \\` |
| Boolean | `true`, `false` | |
| Nil | `nil` | |

No arrays, maps, or other compound data types in v1 (deferred to v2).

### Variables

```
assign x 10;          %% declaration
x be x plus 1;        %% re-assignment (error if x not declared)
```

Lexical block scoping. Re-assignment of undeclared variable is a compile-time error. `assign` shadowing in inner scopes allowed.

### Operators

- Arithmetic: `plus minus mul div mod`
- Comparison: `eqeq neq lo hi loeq hieq`
- Logical: `and or not` (short-circuit `and`/`or`)
- String concat: `c` (both operands must be strings; runtime error otherwise)
- Unary: `minus` (numeric negate), `not` (logical)

Precedence (low → high): `or` < `and` < equality (`eqeq neq`) < comparison (`lo hi loeq hieq`) < term (`plus minus` and `c`) < factor (`mul div mod`) < unary (`not`, `minus`) < call/primary.

Truthiness: `nil` and `false` are falsy; everything else truthy (Lox rule).
Equality: `eqeq`/`neq` work across types (different types compare unequal; Int 1 eqeq Float 1.0 is **true** via promotion).

### Control flow

```
if (cond) { ... } else if (other) { ... } else { ... }
while (cond) { ... }
for (assign i 0; i lo 10; i be i plus 1) { ... }
```

`for` desugars to `while` in the parser. No `break`/`continue` in v1.

### Functions & closures

```
fn add(a, b) {
  return a plus b;
}
assign result add(1, 2);
```

- First-class: functions are values, can be passed/returned/stored.
- Closures: capture enclosing variables **by reference** (captured vars are heap-boxed).
- `fn name(...)` at any scope declares + binds; anonymous fn expressions **not** in v1.
- Implicit `return nil;` at function end. `return` outside function = compile error.
- Arity checked at runtime (calling with wrong arg count = runtime error).

### Built-ins

`print(x);` — prints any value + newline. Only built-in in v1.

## Compiler: architecture

Host: **Rust + inkwell 0.9** (feature `llvm20-1`), LLVM 20.1.3 via Homebrew (keg-only: `LLVM_SYS_201_PREFIX=/opt/homebrew/opt/llvm`).

Pipeline:

```
source.verb → Lexer → [Token] → Parser → AST → Codegen (inkwell) → LLVM Module
                                                      ├─ JIT execute (verb run)
                                                      └─ object file + cc link (verb build)
```

### Crate layout

- `src/main.rs` — CLI: `verb run f.verb` (JIT, default), `verb build f.verb -o out` (AOT), `--emit-llvm` (dump .ll to stdout)
- `src/lexer.rs` — `Token { kind, lexeme, line, col }`, hand-written scanner
- `src/ast.rs` — `Expr` / `Stmt` enums
- `src/parser.rs` — recursive descent; expressions via precedence climbing
- `src/codegen.rs` — AST → LLVM IR; scope stack of `HashMap<String, PointerValue>`
- `src/value.rs` — tagged-value construction/inspection helpers used by codegen

### Runtime value model

Every Verb value = LLVM struct `%verb.value = { i8, i64 }` (tag, payload), passed by value.

| Tag | Type | Payload |
|---|---|---|
| 0 | Nil | 0 |
| 1 | Bool | 0/1 |
| 2 | Int | i64 |
| 3 | Float | f64 bitcast to i64 |
| 4 | String | ptr to NUL-terminated heap bytes |
| 5 | Closure | ptr to heap `{ fn_ptr, env_ptr }` |

Closures: compiled fn takes `(env_ptr, args...) -> value`. Captured variables heap-boxed (`malloc`'d cell) so inner/outer share mutation. Env = malloc'd array of cell pointers.

**No GC in v1.** Strings, closures, boxed cells malloc'd, never freed. Deliberate simplification.

External symbols: libc only — `printf`, `malloc`, `exit`, plus `strlen`/`strcpy`-family (or `snprintf`) for concat/printing. No custom runtime library.

### Error handling

- **Compile-time** (lex, parse, undeclared variable, `return` outside fn): `Result<T, CompileError { msg, line, col }>` through Rust; print `error [line:col]: msg`, exit 1, no codegen.
- **Runtime** (operand type mismatch, div/mod by zero, calling non-closure, wrong arity, `c` on non-strings): codegen emits tag-check branches at each op; failure branch calls `printf` with message then `exit(1)`.

## Testing

- Unit: lexer (source → expected token kinds), parser (source → AST shape / expected errors).
- Golden/e2e: `tests/fixtures/*.verb` each with `*.expected` stdout; test harness JIT-runs and diffs output. Covers arithmetic, promotion, strings, control flow, functions, closures, runtime-error messages.
- Snapshot: a few small snippets → emitted `.ll` textual checks (substring assertions, not full-file, to stay robust across LLVM versions).

## Out of scope (v2+)

Arrays/maps, GC, `break`/`continue`, anonymous fns, string methods, modules/imports, result-style error handling.
