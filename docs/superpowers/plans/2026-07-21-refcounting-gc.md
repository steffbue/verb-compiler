# Reference-Counting GC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Verb's heap allocations (string buffers, closure structs, variable "cells") implicitly reference-counted so programs no longer leak, with zero new syntax and zero observable behavior change.

**Architecture:** Every heap block gets an 8-byte refcount header at `ptr - 8`. Codegen inserts `verb_retain_value`/`verb_release_value`/`verb_retain_cell`/`verb_release_cell` calls (all four generated as LLVM IR functions inside the module itself, the same way `verb_concat`/`verb_truthy` already are) at every value-copy and scope-exit point. No tracing/stack-scanning is needed and no cycle collector is needed, because closures never capture (env is always null) and cells never nest — refcounting is exact.

**Tech Stack:** Rust + inkwell (LLVM IR generation), existing `src/codegen.rs`; C++ runtime (`runtime/verb_std_io.cpp`) for the one place outside the module that allocates a Verb-visible string.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-21-refcounting-gc-design.md` — read it before starting; this plan implements it task-by-task.
- No new language syntax. No new heap types (arrays/maps are out of scope — see spec).
- Only string and closure `VerbValue`s are refcounted; nil/bool/int/float never touch any retain/release call (the runtime dispatch no-ops on them, codegen never special-cases them).
- Every existing test in `tests/e2e.rs`, `tests/formatter_roundtrip.rs`, `tests/parser_recovery_fuzz.rs` must keep passing after every task — run `cargo test` at the end of every task, not just at the end of the plan.
- `cargo build` must produce zero new warnings (no unused functions/imports) at the end of every task — if a task adds a helper that's only called by a later task, fold both into one task instead of leaving dead code.

---

## Task 1: Header-carrying allocator (`verb_alloc`)

**Files:**
- Modify: `src/codegen.rs` (`declare_libc`, `Codegen::new`, `malloc_bytes`, `build_concat_fn`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Produces: `fn build_alloc_fn(&self)` (emits LLVM function `verb_alloc(i64) -> ptr`), `fn declare_gc_globals(&self)`, `fn inc_live_counter(&self)`, global `verb_gc_live: i64` in the module. `malloc_bytes` now returns a header-carrying pointer (behavior-transparent to every existing caller — they only ever read/write the 16/24 bytes at the returned address, never the 8 bytes before it).

Every heap-owned block Verb allocates gets an `i64` refcount header immediately before the pointer the rest of the codebase already uses: `[i64 refcount][payload...]`. `verb_alloc(n)` mallocs `n + 8`, stores `1` at offset 0, and returns `raw + 8`. A module-global `i64 verb_gc_live` counts outstanding `verb_alloc` blocks (incremented here; decremented wherever a block is actually freed, added in Task 3) — this is the leak oracle Task 8 verifies against.

- [ ] **Step 1: Add `free` and `getenv` to `declare_libc`, and a `verb_gc_live` global**

In `src/codegen.rs`, find `fn declare_libc(&self)` (around line 62) and add two more declarations after the existing ones (`getenv` isn't used until Task 8's diagnostic, `free` isn't called until Task 3 — declaring both now avoids touching this function twice):

```rust
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
```
becomes
```rust
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("free", self.ctx.void_type().fn_type(&[pt.into()], false), None);
        self.module.add_function("getenv", pt.fn_type(&[pt.into()], false), None);
```

Then add a new method right after `declare_libc`:

```rust
    fn declare_gc_globals(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.add_global(i64t, None, "verb_gc_live");
        g.set_initializer(&i64t.const_zero());
    }

    fn inc_live_counter(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let cur = self.builder.build_load(i64t, g, "gc_live").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i64t.const_int(1, false), "gc_live1").unwrap();
        self.builder.build_store(g, next).unwrap();
    }
```

- [ ] **Step 2: Add `build_alloc_fn` and register it in `Codegen::new`**

Add this method next to the other `build_*_fn` methods (e.g. right before `fn malloc_bytes`):

```rust
    /// Runtime helper: verb_alloc(i64 n) -> ptr. Wraps `malloc` with an
    /// 8-byte refcount header (initialized to 1) prefixed to every heap
    /// block Verb owns; the returned pointer is the payload -- the header
    /// lives at payload-8. String literals get the same header shape
    /// baked into their LLVM global (see `static_string_ptr`, Task 2) so
    /// retain/release never need to know statically whether a given
    /// string pointer is heap or static.
    fn build_alloc_fn(&self) {
        let i64t = self.ctx.i64_type();
        let fnty = self.ptr_ty.fn_type(&[i64t.into()], false);
        let f = self.module.add_function("verb_alloc", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let n = f.get_nth_param(0).unwrap().into_int_value();
        let total = self.builder.build_int_add(n, i64t.const_int(8, false), "total").unwrap();
        let raw = self.call_named("malloc", &[total.into()]).unwrap().into_pointer_value();
        self.builder.build_store(raw, i64t.const_int(1, false)).unwrap();
        let payload = unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), raw, &[i64t.const_int(8, false)], "payload")
        }.unwrap();
        self.inc_live_counter();
        self.builder.build_return(Some(&payload)).unwrap();
    }
```

In `Codegen::new`, find:
```rust
        cg.declare_libc();
        cg.build_type_name_fn();
```
and change it to:
```rust
        cg.declare_libc();
        cg.declare_gc_globals();
        cg.build_alloc_fn();
        cg.build_type_name_fn();
```

- [ ] **Step 3: Switch `malloc_bytes` and the concat buffer allocation to `verb_alloc`**

Find:
```rust
    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("malloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }
```
change `"malloc"` to `"verb_alloc"`:
```rust
    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("verb_alloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }
```

In `build_concat_fn`, find:
```rust
        let buf = self.call_named("malloc", &[size.into()]).unwrap().into_pointer_value();
```
change to:
```rust
        let buf = self.call_named("verb_alloc", &[size.into()]).unwrap().into_pointer_value();
```

- [ ] **Step 4: Add an IR-shape regression test**

In `tests/e2e.rs`, add near `emits_llvm_ir`:

```rust
#[test]
fn verb_alloc_is_emitted() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("define ptr @verb_alloc"), "no verb_alloc in IR:\n{ir}");
}
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: PASS (every existing test still green — this task only changes where bytes come from, not their layout or contents at the pointer callers already use).

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs tests/e2e.rs
git commit -m "feat(gc): add header-carrying verb_alloc allocator"
```

---

## Task 2: Static string literals get a sentinel header

**Files:**
- Modify: `src/codegen.rs` (`Expr::Str` codegen), `src/value.rs`
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `fn static_string_ptr(&self, s: &str) -> PointerValue<'ctx>`, `GC_STATIC_SENTINEL: i64` constant. `Expr::Str` now calls this instead of `cstr` (which stays unchanged and keeps being used for internal format/type-name strings that never become a Verb `VerbValue`).

String literals live in LLVM's rodata (`build_global_string_ptr`), never on the heap — they must never be freed. Give every literal the *same* 8-byte-header shape as heap strings, but with a sentinel value (`i64::MIN`) instead of a live count, so `verb_retain_value`/`verb_release_value` (Task 3) can always read a header at `ptr - 8` for any string, heap or static, and just no-op when they see the sentinel — no branch in codegen needs to know which kind of pointer it has.

- [ ] **Step 1: Add the sentinel constant**

In `src/value.rs`, add:

```rust
/// Refcount-header value that marks a string as static (a source literal,
/// never heap-allocated, never freed). Never a value a real refcount can
/// reach from 1 by increment/decrement in any real program.
pub const GC_STATIC_SENTINEL: i64 = i64::MIN;
```

- [ ] **Step 2: Add `static_string_ptr`**

In `src/codegen.rs`, add this method near `cstr`:

```rust
    /// Builds a global for a Verb string *literal*: an i64 sentinel header
    /// immediately followed by the NUL-terminated bytes, laid out
    /// identically to a heap `verb_alloc` block (header at payload-8) so
    /// `verb_retain_value`/`verb_release_value` can treat every string
    /// pointer the same way. Returns a pointer to the byte data (not the
    /// header) -- exactly what `Expr::Str` needs.
    fn static_string_ptr(&self, s: &str) -> PointerValue<'ctx> {
        let i8t = self.ctx.i8_type();
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let mut data: Vec<u8> = s.as_bytes().to_vec();
        data.push(0);
        let arr_ty = i8t.array_type(data.len() as u32);
        let struct_ty = self.ctx.struct_type(&[i64t.into(), arr_ty.into()], false);
        let hdr = i64t.const_int(GC_STATIC_SENTINEL as u64, true);
        let arr_vals: Vec<_> = data.iter().map(|b| i8t.const_int(*b as u64, false)).collect();
        let arr = i8t.const_array(&arr_vals);
        let init = struct_ty.const_named_struct(&[hdr.into(), arr.into()]);
        let g = self.module.add_global(struct_ty, None, "verb.strlit");
        g.set_initializer(&init);
        g.set_constant(true);
        unsafe {
            self.builder.build_in_bounds_gep(
                struct_ty, g.as_pointer_value(),
                &[i32t.const_zero(), i32t.const_int(1, false), i32t.const_zero()],
                "strdata",
            )
        }.unwrap()
    }
```

- [ ] **Step 3: Use it for `Expr::Str`**

Find:
```rust
            Expr::Str(s) => {
                let p = self.cstr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
```
change `self.cstr(s)` to `self.static_string_ptr(s)`:
```rust
            Expr::Str(s) => {
                let p = self.static_string_ptr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
```

- [ ] **Step 4: Add an IR-shape test for the sentinel**

In `tests/e2e.rs`:

```rust
#[test]
fn string_literals_carry_a_static_gc_sentinel_header() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("-9223372036854775808"), "no GC static sentinel in IR:\n{ir}");
}
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: PASS. String literals still behave as plain NUL-terminated C strings at the pointer Verb sees (`strlen`/`strcpy`/`printf %s`/`verb_concat` all still work identically) — the sentinel header is additional data before that pointer that nothing reads yet.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs src/value.rs tests/e2e.rs
git commit -m "feat(gc): give string literals a static GC sentinel header"
```

---

## Task 3: Retain/release runtime functions

**Files:**
- Modify: `src/codegen.rs` (`Codegen::new`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_alloc`'s header layout (Task 1), `GC_STATIC_SENTINEL` (Task 2).
- Produces: four LLVM IR functions, callable by name via `self.call_named(...)` from any later codegen site: `verb_retain_value(VerbValue) -> void`, `verb_release_value(VerbValue) -> void`, `verb_retain_cell(ptr) -> void`, `verb_release_cell(ptr) -> void`. Also `fn header_ptr(&self, payload: PointerValue<'ctx>) -> PointerValue<'ctx>` and `fn dec_live_counter(&self)`.

These are dead code until Task 4 wires call sites — that's fine, they're LLVM module functions (declared+defined via `add_function` + a body), not Rust items, so there's no Rust `unused function` warning as long as `Codegen::new` calls each `build_*_fn` to emit it into the module.

- [ ] **Step 1: Add `header_ptr` and `dec_live_counter`**

Add next to `inc_live_counter`:

```rust
    /// Given a payload pointer (what a `VerbValue` or a cell already
    /// points at), returns a pointer to its 8-byte refcount header,
    /// living immediately before it. Valid for every string, closure,
    /// and cell pointer Verb ever produces -- all three are allocated via
    /// `verb_alloc`, or (strings only) carry the same shape statically.
    fn header_ptr(&self, payload: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let i64t = self.ctx.i64_type();
        unsafe {
            self.builder.build_in_bounds_gep(
                self.ctx.i8_type(), payload, &[i64t.const_int((-8i64) as u64, true)], "hdr")
        }.unwrap()
    }

    fn dec_live_counter(&self) {
        let i64t = self.ctx.i64_type();
        let g = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let cur = self.builder.build_load(i64t, g, "gc_live").unwrap().into_int_value();
        let next = self.builder.build_int_sub(cur, i64t.const_int(1, false), "gc_live1").unwrap();
        self.builder.build_store(g, next).unwrap();
    }
```

- [ ] **Step 2: Add `build_retain_value_fn` and `build_release_value_fn`**

```rust
    /// Runtime helper: verb_retain_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag (string or closure). Static string
    /// literals (sentinel header) are skipped -- immortal, count never
    /// moves.
    fn build_retain_value_fn(&self) {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_retain_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let str_bb = self.ctx.append_basic_block(f, "str");
        let str_bump_bb = self.ctx.append_basic_block(f, "str.bump");
        let clos_check_bb = self.ctx.append_basic_block(f, "clos.check");
        let clos_bb = self.ctx.append_basic_block(f, "clos");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, clos_check_bb).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(p, self.ptr_ty, "sp").unwrap();
        let shdr = self.header_ptr(sp);
        let scur = self.builder.build_load(i64t, shdr, "scur").unwrap().into_int_value();
        let is_static = self.builder.build_int_compare(
            EQ, scur, i64t.const_int(GC_STATIC_SENTINEL as u64, true), "is_static").unwrap();
        self.builder.build_conditional_branch(is_static, done_bb, str_bump_bb).unwrap();

        self.builder.position_at_end(str_bump_bb);
        let snext = self.builder.build_int_add(scur, i64t.const_int(1, false), "snext").unwrap();
        self.builder.build_store(shdr, snext).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(clos_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        self.builder.build_conditional_branch(is_clos, clos_bb, done_bb).unwrap();

        self.builder.position_at_end(clos_bb);
        let cp = self.builder.build_int_to_ptr(p, self.ptr_ty, "cp").unwrap();
        let chdr = self.header_ptr(cp);
        let ccur = self.builder.build_load(i64t, chdr, "ccur").unwrap().into_int_value();
        let cnext = self.builder.build_int_add(ccur, i64t.const_int(1, false), "cnext").unwrap();
        self.builder.build_store(chdr, cnext).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }

    /// Runtime helper: verb_release_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag; on those, decrements the header
    /// and frees the block once it hits zero. Closures never cascade
    /// further: `env` is always null (capture is unimplemented), so
    /// freeing a closure struct needs no nested release.
    fn build_release_value_fn(&self) {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_release_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let str_bb = self.ctx.append_basic_block(f, "str");
        let str_live_bb = self.ctx.append_basic_block(f, "str.live");
        let str_free_bb = self.ctx.append_basic_block(f, "str.free");
        let clos_check_bb = self.ctx.append_basic_block(f, "clos.check");
        let clos_bb = self.ctx.append_basic_block(f, "clos");
        let clos_free_bb = self.ctx.append_basic_block(f, "clos.free");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, clos_check_bb).unwrap();

        self.builder.position_at_end(str_bb);
        let sp = self.builder.build_int_to_ptr(p, self.ptr_ty, "sp").unwrap();
        let shdr = self.header_ptr(sp);
        let scur = self.builder.build_load(i64t, shdr, "scur").unwrap().into_int_value();
        let is_static = self.builder.build_int_compare(
            EQ, scur, i64t.const_int(GC_STATIC_SENTINEL as u64, true), "is_static").unwrap();
        self.builder.build_conditional_branch(is_static, done_bb, str_live_bb).unwrap();

        self.builder.position_at_end(str_live_bb);
        let snext = self.builder.build_int_sub(scur, i64t.const_int(1, false), "snext").unwrap();
        self.builder.build_store(shdr, snext).unwrap();
        let szero = self.builder.build_int_compare(EQ, snext, i64t.const_zero(), "szero").unwrap();
        self.builder.build_conditional_branch(szero, str_free_bb, done_bb).unwrap();

        self.builder.position_at_end(str_free_bb);
        self.dec_live_counter();
        self.call_named("free", &[shdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(clos_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        self.builder.build_conditional_branch(is_clos, clos_bb, done_bb).unwrap();

        self.builder.position_at_end(clos_bb);
        let cp = self.builder.build_int_to_ptr(p, self.ptr_ty, "cp").unwrap();
        let chdr = self.header_ptr(cp);
        let ccur = self.builder.build_load(i64t, chdr, "ccur").unwrap().into_int_value();
        let cnext = self.builder.build_int_sub(ccur, i64t.const_int(1, false), "cnext").unwrap();
        self.builder.build_store(chdr, cnext).unwrap();
        let czero = self.builder.build_int_compare(EQ, cnext, i64t.const_zero(), "czero").unwrap();
        self.builder.build_conditional_branch(czero, clos_free_bb, done_bb).unwrap();

        self.builder.position_at_end(clos_free_bb);
        self.dec_live_counter();
        self.call_named("free", &[chdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }
```

- [ ] **Step 3: Add `build_retain_cell_fn` and `build_release_cell_fn`**

```rust
    /// Runtime helper: verb_retain_cell(ptr cell) -> void. Cells are
    /// always heap-owned (never static like a string literal can be), so
    /// this always bumps the header at cell-8, no tag/sentinel check.
    fn build_retain_cell_fn(&self) {
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.ptr_ty.into()], false);
        let f = self.module.add_function("verb_retain_cell", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        self.builder.position_at_end(entry);
        let cell = f.get_nth_param(0).unwrap().into_pointer_value();
        let hdr = self.header_ptr(cell);
        let cur = self.builder.build_load(i64t, hdr, "cur").unwrap().into_int_value();
        let next = self.builder.build_int_add(cur, i64t.const_int(1, false), "next").unwrap();
        self.builder.build_store(hdr, next).unwrap();
        self.builder.build_return(None).unwrap();
    }

    /// Runtime helper: verb_release_cell(ptr cell) -> void. Decrements
    /// the header at cell-8; at zero, releases the `VerbValue` stored
    /// inside (cascading into a heap-owned string/closure if that's what
    /// the cell holds) and frees the cell block itself.
    fn build_release_cell_fn(&self) {
        use inkwell::IntPredicate::*;
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.ptr_ty.into()], false);
        let f = self.module.add_function("verb_release_cell", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let free_bb = self.ctx.append_basic_block(f, "free");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let cell = f.get_nth_param(0).unwrap().into_pointer_value();
        let hdr = self.header_ptr(cell);
        let cur = self.builder.build_load(i64t, hdr, "cur").unwrap().into_int_value();
        let next = self.builder.build_int_sub(cur, i64t.const_int(1, false), "next").unwrap();
        self.builder.build_store(hdr, next).unwrap();
        let zero = self.builder.build_int_compare(EQ, next, i64t.const_zero(), "zero").unwrap();
        self.builder.build_conditional_branch(zero, free_bb, done_bb).unwrap();

        self.builder.position_at_end(free_bb);
        let inner = self.builder.build_load(self.value_ty, cell, "inner").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[inner.into()]);
        self.dec_live_counter();
        self.call_named("free", &[hdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }
```

- [ ] **Step 4: Register all four in `Codegen::new`**

Find:
```rust
        cg.build_check_call_fn();
        cg
```
change to:
```rust
        cg.build_check_call_fn();
        cg.build_retain_value_fn();
        cg.build_release_value_fn();
        cg.build_retain_cell_fn();
        cg.build_release_cell_fn();
        cg
```

- [ ] **Step 5: Add an IR-shape test**

In `tests/e2e.rs`:

```rust
#[test]
fn gc_retain_release_functions_are_emitted() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    for sym in ["@verb_retain_value", "@verb_release_value", "@verb_retain_cell", "@verb_release_cell"] {
        assert!(ir.contains(sym), "missing {sym} in IR:\n{ir}");
    }
}
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS. Nothing calls these four functions yet, so program behavior is unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/codegen.rs tests/e2e.rs
git commit -m "feat(gc): add verb_retain_value/verb_release_value/verb_retain_cell/verb_release_cell"
```

---

## Task 4: Wire retain-on-load and release-on-discard

**Files:**
- Modify: `src/codegen.rs` (`Expr::Var`, `Stmt::ExprStmt`, `Stmt::If`, `Stmt::While`, `Expr::Unary`, `gen_binary`, `gen_call`'s `print` and general-call paths, `gen_std_io_call`, `gen_extern_call`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_retain_value`/`verb_release_value` (Task 3).
- Produces: the convention every later task relies on — **every `gen_expr` result is an owned temporary**: transferred into a fresh cell with no extra op, or released with `verb_release_value` once its use ends without being stored.

This task does not yet release variable *cells* (that's Tasks 6-7) — it only makes each individual read/use of a value correctly balanced in isolation. A variable read now retains (a second, independent reference to the same string/closure the cell still owns); every place that value's use ends without being stored somewhere persistent now releases it back down. Net effect on any single variable read-then-discard: refcount goes up by 1, then back down by 1 — a no-op, safe, and does not yet fix any leak by itself (cells still aren't released) — but it's the correctness foundation the later tasks build on.

- [ ] **Step 1: Retain on `Expr::Var` load**

Find:
```rust
            Expr::Var(name, line, col) => {
                if let Some(cell) = self.lookup(name) {
                    return Ok(self.builder.build_load(self.value_ty, cell, name)
                        .unwrap().into_struct_value());
                }
```
change to:
```rust
            Expr::Var(name, line, col) => {
                if let Some(cell) = self.lookup(name) {
                    let v = self.builder.build_load(self.value_ty, cell, name)
                        .unwrap().into_struct_value();
                    self.call_named("verb_retain_value", &[v.into()]);
                    return Ok(v);
                }
```

- [ ] **Step 2: Release on `Stmt::ExprStmt` discard**

Find:
```rust
            Stmt::ExprStmt(e) => { self.gen_expr(e)?; Ok(()) }
```
change to:
```rust
            Stmt::ExprStmt(e) => {
                let v = self.gen_expr(e)?;
                self.call_named("verb_release_value", &[v.into()]);
                Ok(())
            }
```

- [ ] **Step 3: Release the `if`/`while` condition after `verb_truthy`**

In `Stmt::If`, find:
```rust
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
```
change to:
```rust
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.call_named("verb_release_value", &[cv.into()]);
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
```

In `Stmt::While`, find:
```rust
                self.builder.position_at_end(cond_bb);
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.builder.build_conditional_branch(t, body_bb, end_bb).unwrap();
```
change to:
```rust
                self.builder.position_at_end(cond_bb);
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                self.call_named("verb_release_value", &[cv.into()]);
                self.builder.build_conditional_branch(t, body_bb, end_bb).unwrap();
```

- [ ] **Step 4: Release unary operands**

Find:
```rust
            Expr::Unary { op, expr, line, col } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let (lc, cc) = self.loc_consts(*line, *col);
                        Ok(self.call_named("verb_neg", &[v.into(), lc.into(), cc.into()])
                            .unwrap().into_struct_value())
                    }
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
```
change to:
```rust
            Expr::Unary { op, expr, line, col } => {
                let v = self.gen_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let (lc, cc) = self.loc_consts(*line, *col);
                        let out = self.call_named("verb_neg", &[v.into(), lc.into(), cc.into()])
                            .unwrap().into_struct_value();
                        self.call_named("verb_release_value", &[v.into()]);
                        Ok(out)
                    }
                    UnOp::Not => {
                        let t = self.call_named("verb_truthy", &[v.into()])
                            .unwrap().into_int_value();
                        self.call_named("verb_release_value", &[v.into()]);
                        let inv = self.builder.build_not(t, "inv").unwrap();
                        Ok(self.bool_val(inv))
                    }
                }
            }
```

- [ ] **Step 5: Release binary operands**

In `gen_binary`, find:
```rust
        if matches!(op, BinOp::Ne) {
            let p = self.payload_of(out);
            let flipped = self.builder.build_xor(
                p, self.ctx.i64_type().const_int(1, false), "ne").unwrap();
            return Ok(self.make_val(TAG_BOOL, flipped));
        }
        Ok(out)
```
change to:
```rust
        self.call_named("verb_release_value", &[l.into()]);
        self.call_named("verb_release_value", &[r.into()]);
        if matches!(op, BinOp::Ne) {
            let p = self.payload_of(out);
            let flipped = self.builder.build_xor(
                p, self.ctx.i64_type().const_int(1, false), "ne").unwrap();
            return Ok(self.make_val(TAG_BOOL, flipped));
        }
        Ok(out)
```
(The short-circuit `and`/`or` path above this, which `return`s early via the `phi` merge, is untouched — its chosen operand becomes the expression's own result and must not be released.)

- [ ] **Step 6: Release the arg to `print`**

Find:
```rust
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                return Ok(self.nil_val());
```
change to:
```rust
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(self.nil_val());
```

- [ ] **Step 7: Release the callee value in a general (closure) call**

Find:
```rust
        let fpp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 0, "fpp").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fpp, "fp").unwrap().into_pointer_value();
        let epp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 2, "epp").unwrap();
        let env = self.builder.build_load(self.ptr_ty, epp, "env").unwrap();

        let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
        let out = self.builder.build_indirect_call(
            fnty, fp, &[env.into(), argv.into()], "call").unwrap();
        Ok(out.try_as_basic_value().basic().unwrap().into_struct_value())
```
change to:
```rust
        let fpp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 0, "fpp").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fpp, "fp").unwrap().into_pointer_value();
        let epp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 2, "epp").unwrap();
        let env = self.builder.build_load(self.ptr_ty, epp, "env").unwrap();
        self.call_named("verb_release_value", &[cv.into()]);

        let fnty = self.value_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
        let out = self.builder.build_indirect_call(
            fnty, fp, &[env.into(), argv.into()], "call").unwrap();
        Ok(out.try_as_basic_value().basic().unwrap().into_struct_value())
```
(`fp`/`env` are already loaded into SSA registers by this point, so releasing -- and potentially freeing -- the closure struct here is safe; the arguments stored into `argv` just above are a separate matter, handled by Step 8 below: their ownership transfers into the callee's param cells, unchanged from today's behavior, so that loop needs no edit.)

- [ ] **Step 8: Release std-io/extern call arguments after the call**

`gen_std_io_call` and `gen_extern_call` pass `argvals` directly as raw LLVM call arguments (no cell, unlike a Verb-to-Verb call) — the extern function reads them but never takes ownership, so the caller must release them once the call returns.

In `gen_std_io_call`, find:
```rust
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        Ok(self.builder.build_call(fnv, &args_bv, "std_io_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value())
```
change to:
```rust
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        let result = self.builder.build_call(fnv, &args_bv, "std_io_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
        for v in &argvals {
            self.call_named("verb_release_value", &[(*v).into()]);
        }
        Ok(result)
```

In `gen_extern_call`, find:
```rust
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        Ok(self.builder.build_call(fnv, &args_bv, "extern_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value())
```
change to:
```rust
        let args_bv: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            argvals.iter().map(|v| (*v).into()).collect();
        let result = self.builder.build_call(fnv, &args_bv, "extern_call")
            .unwrap().try_as_basic_value().basic().unwrap().into_struct_value();
        for v in &argvals {
            self.call_named("verb_release_value", &[(*v).into()]);
        }
        Ok(result)
```

- [ ] **Step 9: Add an IR-shape test**

In `tests/e2e.rs`:

```rust
#[test]
fn gc_retain_release_calls_are_wired_into_expr_codegen() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("call void @verb_retain_value"), "no retain call site in IR:\n{ir}");
    assert!(ir.contains("call void @verb_release_value"), "no release call site in IR:\n{ir}");
}
```

- [ ] **Step 10: Run the full test suite**

Run: `cargo test`
Expected: PASS with identical output to before this task — this task only balances individual value reads/uses; it does not yet change what happens to a variable's own cell, so no leaks are fixed yet, but nothing should crash or change program output.

- [ ] **Step 11: Commit**

```bash
git add src/codegen.rs tests/e2e.rs
git commit -m "feat(gc): retain on variable read, release discarded temporaries"
```

---

## Task 5: Release the old value on `Reassign`

**Files:**
- Modify: `src/codegen.rs` (`Stmt::Reassign`)
- Test: `tests/fixtures/reassign_strings.verb` (new), `tests/fixtures/reassign_strings.expected` (new), `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_release_value` (Task 3).

- [ ] **Step 1: Release the cell's old value before overwriting it**

Find:
```rust
            Stmt::Reassign { name, value, line, col } => {
                let cell = self.lookup(name).ok_or_else(|| {
                    self.undefined_var(name, *line, *col)
                        .with_hint("declare new variables with 'assign' or 'declare'".to_string())
                })?;
                let v = self.gen_expr(value)?;
                self.builder.build_store(cell, v).unwrap();
                Ok(())
            }
```
change to:
```rust
            Stmt::Reassign { name, value, line, col } => {
                let cell = self.lookup(name).ok_or_else(|| {
                    self.undefined_var(name, *line, *col)
                        .with_hint("declare new variables with 'assign' or 'declare'".to_string())
                })?;
                let v = self.gen_expr(value)?;
                let old = self.builder.build_load(self.value_ty, cell, "old").unwrap().into_struct_value();
                self.call_named("verb_release_value", &[old.into()]);
                self.builder.build_store(cell, v).unwrap();
                Ok(())
            }
```
(`v` is already an owned temporary per the Task 4 convention -- either fresh, or retained on load -- so it needs no extra retain here; it's simply transferred into the cell. Self-reassignment, e.g. `x be x;`, stays correct: the load in `Expr::Var` already retained a second reference before this point, so releasing `old` here just removes the cell's original claim, leaving exactly the one the incoming temporary carries.)

- [ ] **Step 2: Add a fixture exercising repeated string reassignment**

Create `tests/fixtures/reassign_strings.verb`:
```
assign s "a";
loop assign i 0; i trails 5; i be i add 1 begin
  s be s join "x";
end
print(s);
```

Create `tests/fixtures/reassign_strings.expected`:
```
axxxxx
```

- [ ] **Step 3: Register the test**

In `tests/e2e.rs`, add near the other `run_ok` tests:
```rust
#[test]
fn reassign_releases_previous_string_value() { run_ok("reassign_strings"); }
```

- [ ] **Step 4: Run it**

Run: `cargo test reassign_releases_previous_string_value`
Expected: PASS, stdout `axxxxx\n`.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/codegen.rs tests/fixtures/reassign_strings.verb tests/fixtures/reassign_strings.expected tests/e2e.rs
git commit -m "feat(gc): release a variable's old value on reassignment"
```

---

## Task 6: Release cells when a block scope pops (normal fall-through)

**Files:**
- Modify: `src/codegen.rs` (`Stmt::Block`, `Stmt::If`, `Stmt::While`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_release_cell` (Task 3).
- Produces: `fn release_scope(&self, scope: &HashMap<String, PointerValue<'ctx>>)`.

**Important ordering note:** a scope's cells can only be released here when the block is still open (`self.cur_block_open()` is true) at the point its own `gen_stmts` call returns. If the block already ended in a `return` (a terminator instruction was already emitted deeper inside), inserting more instructions into that same basic block is invalid LLVM IR — trying to append after a terminator either panics in the builder or produces a broken module. The early-return case is handled separately, in Task 7, at the `return` site itself, before its terminator is emitted — not here.

- [ ] **Step 1: Add `release_scope`**

Add next to `malloc_bytes`:
```rust
    fn release_scope(&self, scope: &HashMap<String, PointerValue<'ctx>>) {
        for cell in scope.values() {
            self.call_named("verb_release_cell", &[(*cell).into()]);
        }
    }
```

- [ ] **Step 2: Release on `Stmt::Block` exit**

Find:
```rust
            Stmt::Block(stmts) => {
                self.scopes.push(HashMap::new());
                let r = self.gen_stmts(stmts);
                self.scopes.pop();
                r
            }
```
change to:
```rust
            Stmt::Block(stmts) => {
                self.scopes.push(HashMap::new());
                let r = self.gen_stmts(stmts);
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                r
            }
```

- [ ] **Step 3: Release on `Stmt::If`'s then/else exit**

Find:
```rust
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
```
change to:
```rust
                self.builder.position_at_end(then_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(then_body)?;
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }

                self.builder.position_at_end(else_bb);
                if let Some(eb) = else_body {
                    self.scopes.push(HashMap::new());
                    self.gen_stmts(eb)?;
                    if self.cur_block_open() {
                        if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                    } else {
                        self.scopes.pop();
                    }
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(merge).unwrap();
                }
```

- [ ] **Step 4: Release on `Stmt::While`'s body exit**

Find:
```rust
                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
```
change to:
```rust
                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                if self.cur_block_open() {
                    if let Some(scope) = self.scopes.pop() { self.release_scope(&scope); }
                } else {
                    self.scopes.pop();
                }
                if self.cur_block_open() {
                    self.builder.build_unconditional_branch(cond_bb).unwrap();
                }
```

- [ ] **Step 5: Add an IR-shape test**

```rust
#[test]
fn gc_releases_block_scope_cells() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/control.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("call void @verb_release_cell"), "no cell release in IR:\n{ir}");
}
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/codegen.rs tests/e2e.rs
git commit -m "feat(gc): release cells when a block/if/while scope exits normally"
```

---

## Task 7: Release all open scopes before every function/program exit

**Files:**
- Modify: `src/codegen.rs` (`Stmt::Return`, `Stmt::Fn`'s implicit end-of-body return, `compile_program`'s implicit top-level return)
- Test: `tests/fixtures/early_return_releases.verb` (new), `tests/fixtures/early_return_releases.expected` (new), `tests/e2e.rs`

**Interfaces:**
- Consumes: `release_scope` (Task 6).
- Produces: `fn release_all_open_scopes(&self)`.

**Why this is separate from Task 6, and why it must not pop `self.scopes`:** an early `return` deep inside nested `Block`/`If`/`While` bodies terminates its basic block immediately — Task 6's scope-pop cleanup never runs for any of those still-open enclosing scopes, because by the time each enclosing frame's own `gen_stmts` call returns, `cur_block_open()` is already false there too. All of them need releasing *before* the `return`'s own terminator, in one pass, at the `return` site.

Critically, `self.scopes` is a single `Vec` shared across the whole function's codegen, and code generated *after* this point in a sibling branch (an `else` body, or statements after an `if` whose `then` branch returned) still needs it intact for variable lookups. So `release_all_open_scopes` must only **read** every open scope's cells (emit release calls for each) and must **never pop** anything — every existing `self.scopes.pop()` call (in `Stmt::Fn`, `compile_program`, and every site Task 6 touched) keeps sole responsibility for removing frames, unconditionally, exactly as before. Two functions with two separate jobs: one emits runtime cleanup IR (read-only over the Vec), the other maintains the compile-time scope stack (mutates the Vec) — they must stay decoupled or the stack desyncs the moment a return fires inside one branch of an `if`.

- [ ] **Step 1: Add `release_all_open_scopes`**

Add next to `release_scope`:
```rust
    /// Releases every cell in every currently-open scope (this function's
    /// own scope stack -- already isolated per-function via the
    /// `saved_scopes` swap in `Stmt::Fn`), innermost first. Read-only over
    /// `self.scopes`: never pops. Must run immediately before *every*
    /// path that can leave a function or the top-level program -- an
    /// explicit `return`, or an implicit end-of-body/end-of-program
    /// return -- since Task 6's scope-pop cleanup only fires on normal
    /// block fall-through and is skipped once a block is already
    /// terminated.
    fn release_all_open_scopes(&self) {
        for scope in self.scopes.iter().rev() {
            self.release_scope(scope);
        }
    }
```

- [ ] **Step 2: Call it before `Stmt::Return`'s `build_return`**

Find:
```rust
            Stmt::Return { value } => {
                if self.fn_depth == 0 {
                    return Err(CompileError::new("'return' outside function", 0, 0));
                }
                let v = match value {
                    Some(e) => self.gen_expr(e)?,
                    None => self.nil_val(),
                };
                self.builder.build_return(Some(&v)).unwrap();
                Ok(())
            }
```
change to:
```rust
            Stmt::Return { value } => {
                if self.fn_depth == 0 {
                    return Err(CompileError::new("'return' outside function", 0, 0));
                }
                let v = match value {
                    Some(e) => self.gen_expr(e)?,
                    None => self.nil_val(),
                };
                self.release_all_open_scopes();
                self.builder.build_return(Some(&v)).unwrap();
                Ok(())
            }
```
(`v` was computed before releasing scopes, so a returned value that came straight out of a local cell -- e.g. `return x;` -- already carries its own independent, retained reference from `Expr::Var`; releasing that cell here doesn't touch `v`'s own count.)

- [ ] **Step 3: Call it before the implicit end-of-body return in `Stmt::Fn`**

Find:
```rust
                let r = self.gen_stmts(body);
                if self.cur_block_open() {
                    self.builder.build_return(Some(&self.nil_val())).unwrap();
                }
                self.scopes.pop();
```
change to:
```rust
                let r = self.gen_stmts(body);
                if self.cur_block_open() {
                    self.release_all_open_scopes();
                    self.builder.build_return(Some(&self.nil_val())).unwrap();
                }
                self.scopes.pop();
```
(Guarded by `cur_block_open()`: if the body already returned explicitly, that `Stmt::Return` already released everything on Step 2's path — this branch only fires for the synthesized implicit `nil` return. The trailing `self.scopes.pop()` is untouched and still unconditional — it removes exactly this function's own top-level frame in both cases.)

- [ ] **Step 4: Call it before the implicit top-level return in `compile_program`**

Find:
```rust
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
```
change to:
```rust
        if self.cur_block_open() {
            self.release_all_open_scopes();
        }
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
```
(Note the release must happen *before* the pop here, unlike `Stmt::Fn` -- this call site pops first in the existing code, so the release call is moved ahead of it rather than merged into the same guarded block.)

- [ ] **Step 5: Add a fixture with an early return from inside a nested block**

Create `tests/fixtures/early_return_releases.verb`:
```
make first_long(items) begin
  loop assign i 0; i trails 5; i be i add 1 begin
    declare cur;
    cur be items join "!";
    check cur equals "c!" begin
      return cur;
    end
  end
  return "none";
end

print(first_long("c"));
```

Create `tests/fixtures/early_return_releases.expected`:
```
c!
```

- [ ] **Step 6: Register the test**

In `tests/e2e.rs`:
```rust
#[test]
fn early_return_from_nested_block_releases_open_scopes() { run_ok("early_return_releases"); }
```

- [ ] **Step 7: Run it**

Run: `cargo test early_return_from_nested_block_releases_open_scopes`
Expected: PASS, stdout `c!\n`.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/codegen.rs tests/fixtures/early_return_releases.verb tests/fixtures/early_return_releases.expected tests/e2e.rs
git commit -m "feat(gc): release all open scopes before every function/program exit"
```

---

## Task 8: `extern`/`std io` contract update, docs, and end-to-end leak verification

**Files:**
- Modify: `runtime/verb.h`, `runtime/verb_std_io.cpp`, `docs/superpowers/specs/2026-07-20-cpp-import-design.md`, `src/codegen.rs` (`compile_program`, `declare_libc` already has `getenv`/`free` from Task 1)
- Test: `tests/fixtures/gc_stress.verb` (new), `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_alloc` (Task 1), `verb_gc_live` global (Task 1/3).

This task closes the loop: (a) the one place outside the LLVM module that hands a heap string to Verb must allocate it the same way Verb itself does, and (b) a `VERB_GC_DEBUG`-gated diagnostic prints the final live-object count so tests can assert zero leaks end-to-end, across strings, closures, recursion, and `import std io`.

- [ ] **Step 1: Declare `verb_alloc` in `verb.h` for C++ callers**

In `runtime/verb.h`, add after the `#include` block, before the tag enum:
```c
// Defined by Verb's own generated LLVM module (src/codegen.rs,
// build_alloc_fn): allocates n bytes with an 8-byte GC refcount header
// prefixed, initialized to 1. Any C++ code that hands a heap-owned
// string back to Verb MUST allocate through this, not malloc/strdup --
// verb_retain_value/verb_release_value read a refcount at ptr-8 for
// every string they see, static or heap, and an unheadered pointer there
// is undefined behavior the first time Verb retains or releases it.
extern "C" void* verb_alloc(int64_t n);
```

- [ ] **Step 2: Switch `verb_std_io.cpp`'s two heap-string sites to `verb_alloc`**

Find:
```cpp
static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(std::malloc(s.size() + 1));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}
```
change to:
```cpp
static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}
```

Find:
```cpp
    char* buf = static_cast<char*>(std::malloc(static_cast<size_t>(size) + 1));
```
change to:
```cpp
    char* buf = static_cast<char*>(verb_alloc(static_cast<int64_t>(size) + 1));
```

- [ ] **Step 3: Note the contract in the C++ import design doc**

In `docs/superpowers/specs/2026-07-20-cpp-import-design.md`, after the `extern "C" VerbValue c_sqrt(VerbValue x) { ... }` example, add:

```markdown
### Returning heap-owned strings

If an extern function returns a *new* string (not one it received as an
argument and is just echoing back), allocate it with `verb_alloc` (declared
in `verb.h`), not `malloc`/`strdup`:

    extern "C" VerbValue make_greeting(VerbValue name) {
        std::string s = std::string("hello, ") + verb_as_string(name);
        char* out = static_cast<char*>(verb_alloc(s.size() + 1));
        std::memcpy(out, s.data(), s.size() + 1);
        return verb_string(out);
    }

Verb's GC (see `docs/superpowers/specs/2026-07-21-refcounting-gc-design.md`)
reads a refcount header at `ptr - 8` for every string it retains or
releases. A string allocated any other way doesn't have that header, and
the first retain/release Verb performs on it is undefined behavior.
```

- [ ] **Step 4: Add a `VERB_GC_DEBUG`-gated live-count diagnostic**

In `src/codegen.rs`, `compile_program`, find:
```rust
        if self.cur_block_open() {
            self.release_all_open_scopes();
        }
        self.scopes.pop();
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
```
change to:
```rust
        if self.cur_block_open() {
            self.release_all_open_scopes();
        }
        self.scopes.pop();
        if self.cur_block_open() {
            self.emit_gc_debug_dump(main);
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
```

Add this method near `release_all_open_scopes`:
```rust
    /// If `VERB_GC_DEBUG` is set in the environment, prints
    /// `verb_gc_live=<n>` to stdout, where `<n>` is the number of
    /// outstanding `verb_alloc` blocks (strings/closures/cells) at
    /// program exit. Purely a test/debugging hook -- silent otherwise,
    /// and never affects a program's own output.
    fn emit_gc_debug_dump(&self, main: FunctionValue<'ctx>) {
        let i64t = self.ctx.i64_type();
        let env_name = self.cstr("VERB_GC_DEBUG");
        let flag = self.call_named("getenv", &[env_name.into()]).unwrap().into_pointer_value();
        let flag_int = self.builder.build_ptr_to_int(flag, i64t, "flagi").unwrap();
        let is_set = self.builder.build_int_compare(
            inkwell::IntPredicate::NE, flag_int, i64t.const_zero(), "gc_debug").unwrap();
        let dbg_bb = self.ctx.append_basic_block(main, "gc.debug");
        let cont_bb = self.ctx.append_basic_block(main, "gc.cont");
        self.builder.build_conditional_branch(is_set, dbg_bb, cont_bb).unwrap();

        self.builder.position_at_end(dbg_bb);
        let live_ptr = self.module.get_global("verb_gc_live").unwrap().as_pointer_value();
        let live = self.builder.build_load(i64t, live_ptr, "live").unwrap();
        let fmt = self.cstr("verb_gc_live=%lld\n");
        self.call_named("printf", &[fmt.into(), live.into()]);
        self.builder.build_unconditional_branch(cont_bb).unwrap();

        self.builder.position_at_end(cont_bb);
    }
```

- [ ] **Step 5: Add a GC stress fixture**

Create `tests/fixtures/gc_stress.verb`:
```
assign total 0;
loop assign i 0; i trails 2000; i be i add 1 begin
  declare s;
  s be "iteration" join "-done";
  check s equals "iteration-done" begin
    total be total add 1;
  end
end
print(total);
```

Create `tests/fixtures/gc_stress.expected`:
```
2000
```

- [ ] **Step 6: Add the leak-verification test**

In `tests/e2e.rs`:
```rust
fn assert_no_leaks(fixture: &str) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_{fixture}"));
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", &format!("tests/fixtures/{fixture}.verb"), "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "{fixture}: build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).env("VERB_GC_DEBUG", "1").output().unwrap();
    assert!(run.status.success(), "{fixture}: run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("{fixture}: no verb_gc_live line in stdout:\n{stdout}"));
    assert_eq!(live_line, "verb_gc_live=0", "{fixture}: leaked heap objects:\n{stdout}");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn gc_stress_loop_leaks_nothing() { assert_no_leaks("gc_stress"); }

#[test]
fn gc_no_leaks_across_representative_programs() {
    for fixture in ["strings", "functions", "control", "reassign_strings", "early_return_releases"] {
        assert_no_leaks(fixture);
    }
}

#[test]
fn gc_no_leaks_with_std_io_file_roundtrip() { assert_no_leaks("std_io_file_roundtrip"); }
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test gc_stress_loop_leaks_nothing gc_no_leaks_across_representative_programs gc_no_leaks_with_std_io_file_roundtrip`
Expected: PASS, every fixture reporting `verb_gc_live=0`.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add runtime/verb.h runtime/verb_std_io.cpp docs/superpowers/specs/2026-07-20-cpp-import-design.md \
        src/codegen.rs tests/fixtures/gc_stress.verb tests/fixtures/gc_stress.expected tests/e2e.rs
git commit -m "feat(gc): route std-io heap strings through verb_alloc; add leak verification"
```

---

## Done

At this point:
- Every heap-owned Verb value (string, closure, cell) is refcounted and freed automatically.
- `nil`/`bool`/`int`/`float` never touch a retain/release call.
- No cycle collector exists or is needed (closures never capture; cells never nest).
- `VERB_GC_DEBUG=1` on any built binary prints `verb_gc_live=<n>` at exit -- `0` for every fixture in the suite, including a 2000-iteration string-churn stress test, closures/recursion, early return from a nested block, and an `import std io` file round-trip.
