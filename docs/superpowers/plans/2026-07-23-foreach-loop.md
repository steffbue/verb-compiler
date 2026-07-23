# For-each Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `each <name> in <collection> begin … end` for-each loop that iterates arrays (elements), strings (chars), maps (keys), and integer ranges (`each x in a to b`).

**Architecture:** Range heads desugar at parse time to the existing `While` node (no codegen). Array/string/map iteration is a new `Stmt::ForEach` AST node lowered by a codegen arm that dispatches on the collection value's runtime tag. String char-at is a generated LLVM helper (like the array helpers) — no new C++ needed. Map key enumeration is the only new runtime C++ function (`map_key_at` in `verb_map.cpp`), emitted only when `import std map` is present.

**Tech Stack:** Rust 2021, LLVM 20.1 via inkwell, C++17 runtime (`runtime/verb_map.cpp`), `cargo test` (unit tests in `src/*.rs`, e2e in `tests/e2e.rs` with `.verb`/`.expected` fixtures).

## Global Constraints

- Verb keywords are whimsical aliases; `renamed_keyword` (`src/lexer.rs:40`) maps common words to Verb keywords for error hints.
- Refcounting/GC is manual in codegen: `gen_expr` returns a **+1 (owned)** value the consumer must release; `bind` moves ownership into a scope cell; `release_scope`/`release_all_open_scopes` release scope cells; an early `return` inside a body calls `release_all_open_scopes` (`src/codegen.rs:284`). Every new codegen path MUST keep refcounts balanced — verified by `assert_no_leaks` (GC-debug `verb_gc_live=0`).
- JIT `verb run` does NOT support `import std map`/`io` — map fixtures must be exercised via `verb build` (AOT), mirroring `build_links_and_runs_a_program_using_std_map` (`tests/e2e.rs:792`).
- Block bodies are `begin … end`, parsed by `self.block()` (`src/parser.rs:302`).
- Half-open ranges: `each x in a to b` visits `a .. b-1`.

---

### Task 1: Lexer keywords `each`, `in`, `to`

**Files:**
- Modify: `src/lexer.rs` (TokenKind enum ~line 4-13; keyword matcher ~line 159-171; `renamed_keyword` ~line 40-49)
- Test: `src/lexer.rs` (`#[cfg(test)] mod tests`, near line 210-260)

**Interfaces:**
- Produces: `TokenKind::Each`, `TokenKind::In`, `TokenKind::To` — consumed by Task 2's parser.

- [ ] **Step 1: Write the failing test**

Add to the lexer test module (near the existing `kinds(...)` tests around line 210):

```rust
#[test]
fn lexes_foreach_keywords() {
    use TokenKind::*;
    let ts = lex("each x in xs begin end").unwrap();
    let kinds: Vec<_> = ts.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        vec![Each, Ident("x".into()), In, Ident("xs".into()), Begin, End, Eof]
    );
    let ts2 = lex("each i in 0 to 5 begin end").unwrap();
    let kinds2: Vec<_> = ts2.into_iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds2,
        vec![Each, Ident("i".into()), In, Int(0), To, Int(5), Begin, End, Eof]
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib lexes_foreach_keywords`
Expected: FAIL — `Each`/`In`/`To` not variants of `TokenKind`.

- [ ] **Step 3: Add the token variants and keyword arms**

In the `TokenKind` enum (line ~6, the row containing `Repeat, Loop`), add the three variants:

```rust
    Assign, Be, Declare, Make, Return, Check, Orelse, Repeat, Loop, Each, In, To, True, False, Nil, Begin, End,
```

In the identifier keyword matcher (around line 161, the `match word` arm containing `"repeat" => Repeat, "loop" => Loop,`), add:

```rust
            "each" => Each, "in" => In, "to" => To,
```

In `renamed_keyword` (line 41-49 `match word`), add a hint for the common alias so `foreach` guides users:

```rust
        "foreach" => "each",
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib lexes_foreach_keywords`
Expected: PASS. Also run `cargo build` to confirm the enum change compiles (a non-exhaustive `match` on `TokenKind` elsewhere would surface here).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): add each/in/to keywords for for-each"
```

---

### Task 2: `Stmt::ForEach` AST node + parser (collection form + range desugar)

**Files:**
- Modify: `src/ast.rs` (`Stmt` enum, lines 21-32)
- Modify: `src/parser.rs` (`statement()` dispatch ~line 182-207; add `foreach_stmt`)
- Test: `src/parser.rs` (`#[cfg(test)] mod tests`, near the `desugars_for_to_while` test ~line 539)

**Interfaces:**
- Consumes: `TokenKind::{Each, In, To}` (Task 1).
- Produces:
  - `Stmt::ForEach { name: String, coll: Expr, body: Vec<Stmt> }` — consumed by Task 4's codegen.
  - Range form desugars to `Stmt::Block(vec![Stmt::Assign{..}, Stmt::While{..}])` — no new consumer.

- [ ] **Step 1: Write the failing tests**

Add to the parser test module (near `desugars_for_to_while`, line ~539):

```rust
#[test]
fn parses_foreach_collection() {
    let p = parse(lex("each item in fruits begin print(item); end").unwrap()).unwrap();
    match &p.body[0] {
        Stmt::ForEach { name, coll, body } => {
            assert_eq!(name, "item");
            assert!(matches!(coll, Expr::Var(n, ..) if n == "fruits"));
            assert_eq!(body.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn desugars_foreach_range_to_while() {
    let p = parse(lex("each x in 0 to 3 begin print(x); end").unwrap()).unwrap();
    match &p.body[0] {
        Stmt::Block(inner) => {
            assert!(matches!(&inner[0], Stmt::Assign { name, .. } if name == "x"));
            match &inner[1] {
                Stmt::While { cond, body } => {
                    // half-open: cond is `x trails 3`
                    assert!(matches!(cond, Expr::Binary { op: BinOp::Lt, .. }));
                    // last body stmt is the increment `x be x add 1`
                    assert!(matches!(body.last().unwrap(), Stmt::Reassign { name, .. } if name == "x"));
                }
                other => panic!("{other:?}"),
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn foreach_requires_in() {
    assert!(parse(lex("each x fruits begin end").unwrap()).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib foreach`
Expected: FAIL — `Stmt::ForEach` does not exist; `each` not dispatched.

- [ ] **Step 3: Add the AST variant**

In `src/ast.rs` `Stmt` enum (after the `While` variant, line 28), add:

```rust
    ForEach { name: String, coll: Expr, body: Vec<Stmt> },
```

- [ ] **Step 4: Dispatch and parse**

In `src/parser.rs` `statement()` (the `match self.peek()`), add an arm next to `TokenKind::Loop => self.for_stmt(),` (line 190):

```rust
            TokenKind::Each => self.foreach_stmt(),
```

Add the `foreach_stmt` method (place it right after `for_stmt`, which ends at line 300). It parses `each NAME in EXPR`, then either the range form (`to EXPR`) or the collection form:

```rust
    fn foreach_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // each
        let (name, _, _) = self.expect_ident("loop variable name")?;
        self.expect(&TokenKind::In, "'in'")?;
        let first = self.expression()?;

        if self.matches(&TokenKind::To) {
            // range: `each x in a to b` -> [assign x a; while x trails b { body; x be x add 1 }]
            let (line, col) = self.here();
            let end = self.expression()?;
            let mut body = self.block()?;
            let cond = Expr::Binary {
                op: BinOp::Lt,
                lhs: Box::new(Expr::Var(name.clone(), line, col)),
                rhs: Box::new(end),
                line,
                col,
            };
            let incr = Stmt::Reassign {
                name: name.clone(),
                value: Expr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(name.clone(), line, col)),
                    rhs: Box::new(Expr::Int(1)),
                    line,
                    col,
                },
                line,
                col,
            };
            body.push(incr);
            return Ok(Stmt::Block(vec![
                Stmt::Assign { name, value: first },
                Stmt::While { cond, body },
            ]));
        }

        // collection form: `each x in coll begin ... end`
        let body = self.block()?;
        Ok(Stmt::ForEach { name, coll: first, body })
    }
```

Ensure `BinOp` is in scope in `parser.rs` (it already imports `crate::ast::*` or `BinOp`; if a build error says `BinOp` is unresolved, add it to the existing `use crate::ast::{...}` line).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib foreach`
Expected: PASS (all three).

Then run `cargo build`. The new `Stmt::ForEach` will make the codegen `match stmt` (`src/codegen.rs:1579`) non-exhaustive → **compile error** `non-exhaustive patterns: ForEach not covered`. That is expected and fixed in Task 4. To keep this task green in isolation, add a temporary stub arm at `src/codegen.rs:1579` (just before `Stmt::Fn`):

```rust
            Stmt::ForEach { .. } => unimplemented!("for-each codegen (Task 4)"),
```

- [ ] **Step 6: Commit**

```bash
git add src/ast.rs src/parser.rs src/codegen.rs
git commit -m "feat(parser): parse for-each; desugar range form to while"
```

---

### Task 3: End-to-end range loop (validates parser desugar through existing codegen)

**Files:**
- Create: `tests/fixtures/foreach_range.verb`, `tests/fixtures/foreach_range.expected`
- Create: `tests/fixtures/foreach_range_empty.verb`, `tests/fixtures/foreach_range_empty.expected`
- Modify: `tests/e2e.rs` (add two `#[test]` fns using the existing `run_ok` helper, line 3)

**Interfaces:**
- Consumes: the range desugar from Task 2. No codegen dependency (range → `While`, already lowered).

- [ ] **Step 1: Write the fixtures**

`tests/fixtures/foreach_range.verb`:

```
each x in 0 to 5 begin
  print(x);
end
```

`tests/fixtures/foreach_range.expected` (note trailing newline after `4`):

```
0
1
2
3
4
```

`tests/fixtures/foreach_range_empty.verb`:

```
each x in 3 to 3 begin
  print(x);
end
print("done");
```

`tests/fixtures/foreach_range_empty.expected`:

```
done
```

- [ ] **Step 2: Add the e2e tests**

In `tests/e2e.rs`, add:

```rust
#[test]
fn foreach_over_range_counts_half_open() {
    run_ok("foreach_range");
}

#[test]
fn foreach_over_empty_range_runs_zero_times() {
    run_ok("foreach_range_empty");
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --test e2e foreach_over`
Expected: PASS — range runs `0 1 2 3 4`; empty range prints only `done`. (These pass already because Task 2 desugars ranges to `While`, and the Task 2 stub `unimplemented!` is never hit — no `Stmt::ForEach` node is produced by a range head.)

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures/foreach_range.verb tests/fixtures/foreach_range.expected \
        tests/fixtures/foreach_range_empty.verb tests/fixtures/foreach_range_empty.expected tests/e2e.rs
git commit -m "test(e2e): for-each integer range half-open + empty"
```

---

### Task 4: Codegen `Stmt::ForEach` — array iteration + non-iterable error

**Files:**
- Modify: `src/codegen.rs` (replace the Task 2 stub arm at ~line 1579; model block wiring on `Stmt::While` at 1654-1679, array helpers `verb_array_len`/`verb_array_get` call sites at 1905-1928)
- Create: `tests/fixtures/foreach_array.verb` + `.expected`; `tests/fixtures/err_foreach_not_iterable.verb`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `Stmt::ForEach { name, coll, body }` (Task 2); helpers `verb_array_len(v, lc, cc) -> struct(int)`, `verb_array_get(arr, idx, lc, cc) -> struct` (returns a **+1 retained** element), `tag_of`, `payload_of`, `make_val(TAG_INT, i)`, `abort_at`, `type_name`, `loc_consts`, `bind`, `release_scope`, `cur_block_open`, `TAG_ARRAY` (from `crate::value`).
- Produces: a runtime `switch` on the collection tag with an `array` case and a `default → abort "cannot iterate <type>"`. String/map tags fall into `default` for now (Tasks 5/6 add their cases).

- [ ] **Step 1: Write the fixtures + failing e2e tests**

`tests/fixtures/foreach_array.verb`:

```
assign xs list 10, 20, 30;
assign total 0;
each n in xs begin
  total be total add n;
  print(n);
end
print(total);
```

`tests/fixtures/foreach_array.expected`:

```
10
20
30
60
```

`tests/fixtures/err_foreach_not_iterable.verb`:

```
each x in 42 begin
  print(x);
end
```

In `tests/e2e.rs` add (using `run_ok`, the GC helper `assert_no_leaks` at line ~48, and `run_err` at line 32):

```rust
#[test]
fn foreach_over_array_visits_every_element() {
    run_ok("foreach_array");
}

#[test]
fn foreach_over_array_is_leak_free() {
    assert_no_leaks("foreach_array");
}

#[test]
fn foreach_over_non_iterable_is_runtime_error() {
    run_err("err_foreach_not_iterable", "cannot iterate int");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e foreach_over_array foreach_over_non_iterable`
Expected: FAIL — the Task 2 stub `unimplemented!` panics during codegen (build aborts).

- [ ] **Step 3: Replace the stub with the array-iterating codegen**

At `src/codegen.rs:1579`, replace the `Stmt::ForEach { .. } => unimplemented!(...)` stub with:

```rust
            Stmt::ForEach { name, coll, body } => {
                use crate::value::{TAG_ARRAY, TAG_INT};
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let i64t = self.ctx.i64_type();
                let i8t = self.ctx.i8_type();

                // Evaluate the collection once (+1 owned). Park it in an outer
                // scope cell so a `return` inside the body (which calls
                // release_all_open_scopes) frees it, and so normal loop exit
                // frees it exactly once.
                let collv = self.gen_expr(coll)?;
                self.scopes.push(HashMap::new());
                self.bind("$foreach_coll", collv);

                let tag = self.tag_of(collv);
                // Reuse the collection expression's source span for error
                // locations (ForEach carries none of its own).
                let (el, ec) = match coll {
                    Expr::Var(_, l, c) => (*l, *c),
                    Expr::Binary { line, col, .. }
                    | Expr::Unary { line, col, .. }
                    | Expr::Call { line, col, .. } => (*line, *col),
                    _ => (0, 0),
                };
                let (lc, cc) = self.loc_consts(el, ec);

                // len + kind are computed in the dispatch, read in the loop.
                let lenp = self.builder.build_alloca(i64t, "fe.lenp").unwrap();
                let kindp = self.builder.build_alloca(i8t, "fe.kindp").unwrap();
                let idxp = self.builder.build_alloca(i64t, "fe.idxp").unwrap();

                let arr_bb  = self.ctx.append_basic_block(f, "fe.array");
                let bad_bb  = self.ctx.append_basic_block(f, "fe.badtype");
                let setup_bb = self.ctx.append_basic_block(f, "fe.setup");
                let cond_bb = self.ctx.append_basic_block(f, "fe.cond");
                let body_bb = self.ctx.append_basic_block(f, "fe.body");
                let bound_bb = self.ctx.append_basic_block(f, "fe.bound");
                let end_bb  = self.ctx.append_basic_block(f, "fe.end");

                // dispatch on runtime tag (string/map cases inserted in Tasks 5/6)
                self.builder.build_switch(
                    tag, bad_bb,
                    &[(i8t.const_int(TAG_ARRAY, false), arr_bb)],
                ).unwrap();

                // array: len = verb_array_len(coll)
                self.builder.position_at_end(arr_bb);
                let alen = self.call_named("verb_array_len", &[collv.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.builder.build_store(lenp, self.payload_of(alen)).unwrap();
                self.builder.build_store(kindp, i8t.const_int(0, false)).unwrap();
                self.builder.build_unconditional_branch(setup_bb).unwrap();

                // non-iterable value: abort with its type name
                self.builder.position_at_end(bad_bb);
                self.abort_at(lc, cc, "cannot iterate %s", &[self.type_name(tag)]);

                // setup: idx = 0
                self.builder.position_at_end(setup_bb);
                self.builder.build_store(idxp, i64t.const_zero()).unwrap();
                self.builder.build_unconditional_branch(cond_bb).unwrap();

                // cond: idx < len ?
                self.builder.position_at_end(cond_bb);
                let i = self.builder.build_load(i64t, idxp, "fe.i").unwrap().into_int_value();
                let len = self.builder.build_load(i64t, lenp, "fe.len").unwrap().into_int_value();
                let more = self.builder.build_int_compare(inkwell::IntPredicate::SLT, i, len, "fe.more").unwrap();
                self.builder.build_conditional_branch(more, body_bb, end_bb).unwrap();

                // body: fetch element by kind, store into elemp, branch to bound
                self.builder.position_at_end(body_bb);
                let elemp = self.builder.build_alloca(self.value_ty, "fe.elemp").unwrap();
                let kind = self.builder.build_load(i8t, kindp, "fe.kind").unwrap().into_int_value();
                let fetch_arr_bb = self.ctx.append_basic_block(f, "fe.fetch.array");
                self.builder.build_switch(
                    kind, fetch_arr_bb,
                    &[(i8t.const_int(0, false), fetch_arr_bb)],
                ).unwrap();

                self.builder.position_at_end(fetch_arr_bb);
                let iv = self.make_val(TAG_INT, i);
                let elem = self.call_named("verb_array_get", &[collv.into(), iv.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.builder.build_store(elemp, elem).unwrap();
                self.builder.build_unconditional_branch(bound_bb).unwrap();

                // bound: bind element to `name` in a fresh iteration scope, run body
                self.builder.position_at_end(bound_bb);
                let elemv = self.builder.build_load(self.value_ty, elemp, "fe.elem").unwrap().into_struct_value();
                self.scopes.push(HashMap::new());
                self.bind(name, elemv);
                self.gen_stmts(body)?;
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    let i2 = self.builder.build_load(i64t, idxp, "fe.i2").unwrap().into_int_value();
                    let nxt = self.builder.build_int_add(i2, i64t.const_int(1, false), "fe.next").unwrap();
                    self.builder.build_store(idxp, nxt).unwrap();
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }

                // end: release the collection (outer scope cell)
                self.builder.position_at_end(end_bb);
                if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                Ok(())
            }
```

**Note on the `(el, ec)` match:** `Stmt::ForEach` carries no `line`/`col`, so the arm reuses the collection expression's span for error locations. Check the exact `Expr` variant field names against `src/ast.rs:7-19` and adjust the match arms so it compiles (variants without a span fall through to `(0, 0)`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e foreach_over_array foreach_over_non_iterable`
Expected: PASS — array prints `10 20 30 60`; `each x in 42` aborts with `cannot iterate int`.
Run: `cargo test --lib` and `cargo test --test e2e` to confirm nothing regressed.

- [ ] **Step 5: Commit**

```bash
git add src/codegen.rs tests/fixtures/foreach_array.verb tests/fixtures/foreach_array.expected \
        tests/fixtures/err_foreach_not_iterable.verb tests/e2e.rs
git commit -m "feat(codegen): for-each over arrays; error on non-iterable"
```

---

### Task 5: String iteration via a generated `verb_char_at` helper

**Files:**
- Modify: `src/codegen.rs` (add `build_char_at_fn` helper next to the array helpers ~line 942; register its construction call ~line 61-72 where `build_array_get_fn` etc. are invoked; add a `TAG_STR` case to both switches in the `Stmt::ForEach` arm from Task 4)
- Create: `tests/fixtures/foreach_string.verb` + `.expected`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_alloc` (`src/codegen.rs:249`, header-prefixed heap alloc), `strlen` (declared runtime fn), `payload_of`, `make_val`, `TAG_STR`, `build_int_to_ptr` idiom (`src/codegen.rs:378`).
- Produces: generated fn `verb_char_at(VerbValue s, VerbValue idx) -> VerbValue` returning a **+1** 1-char `TAG_STR` string; a `TAG_STR` arm (kind = 1) in the for-each dispatch/fetch switches.

- [ ] **Step 1: Write the fixture + failing e2e tests**

`tests/fixtures/foreach_string.verb`:

```
assign s "abc";
each ch in s begin
  print(ch);
end
```

`tests/fixtures/foreach_string.expected`:

```
a
b
c
```

In `tests/e2e.rs`:

```rust
#[test]
fn foreach_over_string_visits_each_char() {
    run_ok("foreach_string");
}

#[test]
fn foreach_over_string_is_leak_free() {
    assert_no_leaks("foreach_string");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e foreach_over_string`
Expected: FAIL — `each ch in "abc"` hits the `default → abort "cannot iterate string"` from Task 4.

- [ ] **Step 3: Add the generated `verb_char_at` helper**

Add a builder method near `build_array_get_fn` (~line 942). It reads byte `idx` of the string payload, allocates a 2-byte header-prefixed buffer via `verb_alloc`, writes `[byte, 0]`, and returns a `TAG_STR` value. Model the header/alloc on how string literals and `verb_concat` build strings (`static_string_ptr` at 162, `verb_alloc` at 249, `build_int_to_ptr` at 378):

```rust
    fn build_char_at_fn(&self) {
        // verb_char_at(VerbValue s, VerbValue idx) -> VerbValue (1-char string)
        let f = self.module.add_function(
            "verb_char_at",
            self.value_ty.fn_type(&[self.value_ty.into(), self.value_ty.into()], false),
            None,
        );
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();

        let s = f.get_nth_param(0).unwrap().into_struct_value();
        let idxv = f.get_nth_param(1).unwrap().into_struct_value();
        let i = self.payload_of(idxv); // TAG_INT payload = i64 index (bounds already guaranteed by the loop)
        let sptr = self.builder.build_int_to_ptr(self.payload_of(s), self.ptr_ty, "sptr").unwrap();
        let bytep = unsafe {
            self.builder.build_in_bounds_gep(i8t, sptr, &[i], "bytep").unwrap()
        };
        let byte = self.builder.build_load(i8t, bytep, "byte").unwrap().into_int_value();

        // allocate a 2-byte NUL-terminated string via the GC alloc path
        let buf = self.call_named("verb_alloc", &[i64t.const_int(2, false).into()])
            .unwrap().into_pointer_value();
        self.builder.build_store(buf, byte).unwrap();
        let secondp = unsafe {
            self.builder.build_in_bounds_gep(i8t, buf, &[i64t.const_int(1, false)], "secondp").unwrap()
        };
        self.builder.build_store(secondp, i8t.const_zero()).unwrap();

        let payload = self.builder.build_ptr_to_int(buf, i64t, "cp").unwrap();
        let out = self.make_val(crate::value::TAG_STR, payload);
        self.builder.build_return(Some(&out)).unwrap();
    }
```

Register its construction where the other `build_*_fn` helpers are called during `Codegen::new` (the block around lines 61-72 that calls `build_array_get_fn`, `build_array_len_fn`, …):

```rust
        cg.build_char_at_fn();
```

(Place it after the array helper registrations. Ensure `cg` — or `self`, matching the surrounding lines — is the receiver used there.)

- [ ] **Step 4: Add the `TAG_STR` case to the for-each switches**

In the `Stmt::ForEach` arm (Task 4), extend BOTH switches.

Add a `str_bb` block declaration alongside `arr_bb`:

```rust
                let str_bb = self.ctx.append_basic_block(f, "fe.string");
```

Add it to the tag dispatch switch cases:

```rust
                self.builder.build_switch(
                    tag, bad_bb,
                    &[
                        (i8t.const_int(TAG_ARRAY, false), arr_bb),
                        (i8t.const_int(crate::value::TAG_STR, false), str_bb),
                    ],
                ).unwrap();
```

Emit the string length branch (kind = 1) right after the array `arr_bb` block:

```rust
                // string: len = strlen(payload)
                self.builder.position_at_end(str_bb);
                let sptr = self.builder.build_int_to_ptr(self.payload_of(collv), self.ptr_ty, "fe.sptr").unwrap();
                let slen = self.call_named("strlen", &[sptr.into()]).unwrap().into_int_value();
                self.builder.build_store(lenp, slen).unwrap();
                self.builder.build_store(kindp, i8t.const_int(1, false)).unwrap();
                self.builder.build_unconditional_branch(setup_bb).unwrap();
```

Add the string fetch block and wire it into the fetch (kind) switch:

```rust
                let fetch_str_bb = self.ctx.append_basic_block(f, "fe.fetch.string");
```

Change the fetch switch to include kind 1:

```rust
                self.builder.build_switch(
                    kind, fetch_arr_bb,
                    &[
                        (i8t.const_int(0, false), fetch_arr_bb),
                        (i8t.const_int(1, false), fetch_str_bb),
                    ],
                ).unwrap();
```

Emit the string fetch after `fetch_arr_bb`:

```rust
                self.builder.position_at_end(fetch_str_bb);
                let ivs = self.make_val(TAG_INT, i);
                let selem = self.call_named("verb_char_at", &[collv.into(), ivs.into()])
                    .unwrap().into_struct_value();
                self.builder.build_store(elemp, selem).unwrap();
                self.builder.build_unconditional_branch(bound_bb).unwrap();
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test e2e foreach_over_string`
Expected: PASS — prints `a`, `b`, `c`; GC-clean. Run the full `cargo test` to confirm no regressions.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs tests/fixtures/foreach_string.verb tests/fixtures/foreach_string.expected tests/e2e.rs
git commit -m "feat(codegen): for-each over string chars via verb_char_at"
```

---

### Task 6: Map key iteration via runtime `map_key_at`

**Files:**
- Modify: `runtime/verb_map.cpp` (add `extern "C" VerbValue map_key_at(VerbValue m, VerbValue i)`; near `map_len`/`map_get`, lines ~112-135)
- Modify: `src/codegen.rs` (add `("map_key_at", 2)` to `MAP_FUNCS`, line ~2133; add a gated `TAG_MAP` case to the for-each switches — emitted only when `self.std_imports` contains `"map"`)
- Create: `tests/fixtures/foreach_map.verb` + `.expected`
- Modify: `tests/e2e.rs`

**Interfaces:**
- Consumes: `VerbMapImpl` (`std::unordered_map`, `verb_map.cpp:71`), `map_len` (returns entry count), `verb_retain_value`/the map's existing retain convention (`map_get` at 112 retains before returning), `is_valid_key`.
- Produces: `map_key_at(m, i) -> VerbValue` returning the `i`-th key (**+1 retained**), and a `TAG_MAP` for-each case (kind = 2). Because map programs run only via AOT `verb build` (JIT rejects `import std map`), and the map case is emitted only under `import std map`, `verb_map.cpp` is always linked when `map_key_at` is referenced — no JIT symbol registration needed.

- [ ] **Step 1: Write the fixture + failing e2e tests**

`tests/fixtures/foreach_map.verb` — build keys deterministically and sum values so output does not depend on unordered key order:

```
import std map;

assign m map_new();
map_set(m, "a", 1);
map_set(m, "b", 2);
map_set(m, "c", 3);
assign total 0;
assign count 0;
each k in m begin
  total be total add map_get(m, k);
  count be count add 1;
end
print(total);
print(count);
```

`tests/fixtures/foreach_map.expected`:

```
6
3
```

In `tests/e2e.rs`, add a build-path test modeled on `build_links_and_runs_a_program_using_std_map` (line ~792). Also add a GC leak check (maps use `assert_no_leaks`, which uses the build path — line ~48):

```rust
#[test]
fn build_runs_foreach_over_map_keys() {
    let out_path = std::env::temp_dir().join("verb_e2e_foreach_map_bin");
    let build = std::process::Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/foreach_map.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/foreach_map.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

#[test]
fn foreach_over_map_is_leak_free() {
    assert_no_leaks("foreach_map");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e foreach_over_map build_runs_foreach_over_map`
Expected: FAIL — `map_key_at` undefined; the for-each map tag currently falls to `default → abort "cannot iterate map"`.

- [ ] **Step 3: Add the `map_key_at` runtime function**

In `runtime/verb_map.cpp`, near `map_len` (line ~135), add:

```cpp
// Returns the i-th key in iteration order (std::unordered_map order is
// unspecified but stable between calls when unmodified). O(i) per call via
// std::next -> O(n^2) over a full loop; acceptable for v1's scope. The
// caller (for-each codegen) only ever passes 0 <= i < map_len(m).
extern "C" VerbValue map_key_at(VerbValue m, VerbValue i) {
    if (m.tag != VERB_MAP || i.tag != VERB_INT) return verb_nil();
    auto* impl = reinterpret_cast<VerbMapImpl*>(m.payload.p);
    long idx = i.payload.i;
    if (idx < 0 || static_cast<size_t>(idx) >= impl->size()) return verb_nil();
    auto it = std::next(impl->begin(), idx);
    VerbValue key = it->first;
    verb_retain_value(key); // hand back a +1 reference, like map_get
    return key;
}
```

Check the exact field/enum/helper names against the surrounding file: the tag enum member (`VERB_MAP`/`VERB_INT` per `runtime/verb.h:30-37`), the payload union accessor (`.payload.p` / `.payload.i` — match how `map_get` reads them), `verb_nil()`, and the retain helper name/convention used by `map_get` (line ~112). Mirror `map_get` exactly for the retain call so refcounts match.

- [ ] **Step 4: Register the function and add the gated map case**

In `src/codegen.rs`, add to `MAP_FUNCS` (line ~2133):

```rust
    ("map_key_at", 2),
```

In the `Stmt::ForEach` arm, gate the map case on the import (maps can only exist when imported). Add near the top of the arm, after the block declarations:

```rust
                let has_map = self.std_imports.iter().any(|m| m == "map");
```

Declare a `map_bb` and `fetch_map_bb` only when `has_map`, and include them in the two switches conditionally. Concretely, build the tag-switch case list dynamically:

```rust
                let mut tag_cases = vec![
                    (i8t.const_int(TAG_ARRAY, false), arr_bb),
                    (i8t.const_int(crate::value::TAG_STR, false), str_bb),
                ];
                let map_bb = self.ctx.append_basic_block(f, "fe.map");
                if has_map {
                    tag_cases.push((i8t.const_int(crate::value::TAG_MAP, false), map_bb));
                }
                self.builder.build_switch(tag, bad_bb, &tag_cases).unwrap();
```

Emit the map length branch (kind = 2), only meaningful when reached:

```rust
                // map: len = map_len(m)
                self.builder.position_at_end(map_bb);
                if has_map {
                    let mlen = self.call_named("map_len", &[collv.into()]).unwrap().into_struct_value();
                    self.builder.build_store(lenp, self.payload_of(mlen)).unwrap();
                    self.builder.build_store(kindp, i8t.const_int(2, false)).unwrap();
                    self.builder.build_unconditional_branch(setup_bb).unwrap();
                } else {
                    // unreachable: no map value can exist without `import std map`
                    self.builder.build_unconditional_branch(bad_bb).unwrap();
                }
```

`map_len` and `map_key_at` must be declared as externs so `call_named` resolves them. `MAP_FUNCS` registration makes them known to `gen_std_io_call`, but the for-each arm calls them via `call_named`, which looks them up with `module.get_function(name).unwrap()`. Add a one-time declaration when `has_map`, mirroring `gen_std_io_call`'s lazy `add_function` (line ~2038):

```rust
                if has_map {
                    for (fname, arity) in [("map_len", 1usize), ("map_key_at", 2usize)] {
                        if self.module.get_function(fname).is_none() {
                            let ptys: Vec<_> = (0..arity).map(|_| self.value_ty.into()).collect();
                            self.module.add_function(fname, self.value_ty.fn_type(&ptys, false), None);
                        }
                    }
                }
```

Place this declaration block near the top of the arm, before the switches. Add the map fetch block (kind = 2) parallel to the string fetch:

```rust
                let fetch_map_bb = self.ctx.append_basic_block(f, "fe.fetch.map");
```

Extend the fetch (kind) switch case list the same dynamic way:

```rust
                let mut kind_cases = vec![
                    (i8t.const_int(0, false), fetch_arr_bb),
                    (i8t.const_int(1, false), fetch_str_bb),
                ];
                if has_map { kind_cases.push((i8t.const_int(2, false), fetch_map_bb)); }
                self.builder.build_switch(kind, fetch_arr_bb, &kind_cases).unwrap();
```

Emit the map fetch:

```rust
                self.builder.position_at_end(fetch_map_bb);
                if has_map {
                    let ivm = self.make_val(TAG_INT, i);
                    let melem = self.call_named("map_key_at", &[collv.into(), ivm.into()])
                        .unwrap().into_struct_value();
                    self.builder.build_store(elemp, melem).unwrap();
                    self.builder.build_unconditional_branch(bound_bb).unwrap();
                } else {
                    self.builder.build_unreachable().unwrap();
                }
```

(When `has_map` is false, `map_bb`/`fetch_map_bb` are still appended but only reached by unreachable/bad branches; they must be terminated so the module verifies — the `else` arms above do that.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test e2e foreach_over_map build_runs_foreach_over_map`
Expected: PASS — sum `6`, count `3`; GC-clean.
Run the standalone-compile guard to confirm the C++ still builds: `cargo test --test e2e verb_map_cpp_compiles_standalone`.
Run full `cargo test` — no regressions.

- [ ] **Step 6: Commit**

```bash
git add runtime/verb_map.cpp src/codegen.rs tests/fixtures/foreach_map.verb tests/fixtures/foreach_map.expected tests/e2e.rs
git commit -m "feat: for-each over map keys via map_key_at runtime fn"
```

---

### Task 7: Documentation + demo coverage

**Files:**
- Modify: `README.md` (add a "For-each loops" subsection near the Arrays/control-flow docs)
- Modify: `examples/demo.verb` (add a for-each section)

**Interfaces:**
- Consumes: the full feature (Tasks 1-6). No code produced.

- [ ] **Step 1: Add a demo section**

In `examples/demo.verb`, after the `%% --- fizzbuzz via loop ---` section, add:

```
%% --- for-each over collections ---
assign nums list 1, 2, 3;
each n in nums begin
  print(n);      %% 1, 2, 3
end

each ch in "hi" begin
  print(ch);     %% h, i
end

each x in 0 to 3 begin
  print(x);      %% 0, 1, 2 (half-open)
end
```

- [ ] **Step 2: Verify the demo runs**

Run: `cargo run -- run examples/demo.verb`
Expected: exit 0, and the new section prints `1 2 3`, `h i`, `0 1 2` in order (among the rest of the demo output). (Demo has no map section because JIT `run` rejects `import std map`.)

- [ ] **Step 3: Add the README subsection**

In `README.md`, after the "## Arrays" section, add:

```markdown
## For-each loops

`each <name> in <collection> begin … end` visits every element of a
collection. It dispatches on the value at runtime:

    each n in nums begin print(n); end     %% array: each element
    each ch in "abc" begin print(ch); end  %% string: each char (1-char string)
    each k in m begin print(map_get(m,k)); end  %% map: each key

There is also a counting form over a half-open integer range `[a, b)`:

    each x in 0 to 5 begin print(x); end   %% 0 1 2 3 4

- The loop variable is scoped to the body and fresh each iteration.
- Iterating a non-collection (`each x in 42`) is a runtime error.
- The collection length is snapshot at entry — don't mutate the
  collection you're iterating.
- Map keys iterate in unspecified order; use `map_get(m, key)` for the
  value. Map for-each needs `import std map`, so build with
  `verb build` (JIT `verb run` does not support std imports).

See `docs/superpowers/specs/2026-07-23-foreach-loop-design.md`.
```

- [ ] **Step 4: Commit**

```bash
git add README.md examples/demo.verb
git commit -m "docs: document for-each loops; add demo coverage"
```

---

## Notes for the implementer

- The Task 4 codegen arm is the load-bearing piece; Tasks 5 and 6 only add cases to its two switches and a fetch block each. Read `Stmt::While` (`src/codegen.rs:1654-1679`) and the array helper builders (`src/codegen.rs:847-966`) before starting Task 4 — the block-wiring, `cur_block_open()` guards, and scope push/pop/release idioms must match exactly or GC leak tests fail.
- Inkwell API details (method names, `unsafe` on GEP, `IntPredicate` path) may differ slightly by version — follow the exact forms already used in `src/codegen.rs` (e.g. `build_in_bounds_gep` usage at 421, `build_int_compare` at 405, `build_switch` at the `verb_print_value`/`verb_type_name` switches). The e2e + `cargo build` are the real gates; adjust to match the crate's inkwell version.
- After every task, run the whole suite (`cargo test`) — the `Stmt` enum and the shared codegen arm are touched repeatedly and a break surfaces immediately.
```
