# Reference-Counting GC v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Verb's heap allocations (string buffers, closure structs, array headers, map objects, boxed variable cells, and top-level globals) implicitly reference-counted, covering everything that exists on `main` today, with zero new syntax and zero observable behavior change (except fixing the pre-existing `push`-grow leak).

**Architecture:** Every heap block gets an 8-byte refcount header at `ptr-8`; string literals get a static sentinel header instead (immortal, never freed). `verb_retain_value`/`verb_release_value` dispatch on tag (STR/CLOSURE/ARRAY/MAP; nil/bool/int/float always no-op) and are generated once as LLVM IR, the same way `verb_concat` etc. already are. Codegen inserts calls to these at every value-copy and scope-exit point. No cycle collector: this is refcounting only — self-referential/cyclic arrays and maps are a documented, tested, *known* leak, resolved by a separate follow-up sub-project, not this one.

**Tech Stack:** Rust + inkwell (`src/codegen.rs`), C++ runtime (`runtime/verb_map.cpp`, `runtime/verb_std_io.cpp`, `runtime/verb.h`).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md` — read it before starting; this plan implements it task-by-task.
- No new language syntax. No cycle detection/collection anywhere in this plan.
- Only heap-identity tags (STR, CLOSURE, ARRAY, MAP) ever touch a retain/release call; nil/bool/int/float are always no-ops.
- Every existing test in `tests/e2e.rs`, `tests/formatter_roundtrip.rs`, `tests/parser_recovery_fuzz.rs`, `tests/verb_export_macro.rs` must keep passing after every task — run `cargo test` at the end of every task.
- `cargo build` must produce zero new warnings at the end of every task.
- All code in this plan is written against `main` @ `1bc678f` (branch `refcounting-gc-v2`). If your local file doesn't match a "Find" block verbatim, stop and report — don't guess.

---

## Task Dependency Graph — read this before picking up any task

```
Phase 0 (sequential — must land, in order, before Phase 1 starts)
  Task 1: Allocator + static string sentinel        [src/codegen.rs, runtime/verb.h]
      |
      v
  Task 2: Full retain/release dispatch (4 tags)      [src/codegen.rs]
      |
      +-------------+-------------+-------------+-------------+
      v             v             v             v             v
Phase 1 (fully parallel — 5 independent tasks, 5 independent developers/reviewers)
  Task 3         Task 4         Task 5         Task 6         Task 7
  Core value     Scope/fn-exit  Array          Map            std-io
  lifecycle      /globals       call-site      wiring         contract
  wiring         wiring         wiring         (verb_map.cpp  (verb_std_io.cpp
                                                only)          only)
      |             |             |             |             |
      +-------------+-------------+-------------+-------------+
                              v
Phase 2 (sequential — depends on ALL of Phase 1 merged)
  Task 8: Cycle-limitation fixture + full leak verification
```

**File ownership per task (this is what makes Phase 1 safe to parallelize):**

| Task | Files touched | Regions touched |
|---|---|---|
| 3 | `src/codegen.rs` | `Expr::Var`, `Stmt::ExprStmt`, `Stmt::If`/`Stmt::While` condition lines, `Expr::Unary`, `gen_binary`, `gen_call`'s `print` + general (indirect-call) path, `gen_std_io_call`, `gen_extern_call`, `Stmt::Reassign` |
| 4 | `src/codegen.rs` | `Stmt::Block`/`Stmt::If`/`Stmt::While` scope push/pop, `Stmt::Return`, `Stmt::Fn`'s body-end, `compile_program`'s tail, `bind()`, `global_slot()` |
| 5 | `src/codegen.rs` | `gen_call`'s inline `get`/`set`/`push`/`pop`/`len` dispatch block, `build_array_set_fn`, `build_array_push_fn` |
| 6 | `runtime/verb_map.cpp` | entire file |
| 7 | `runtime/verb_std_io.cpp` | `verb_string_from`, `file_read` |

Tasks 3, 4, and 5 all edit `src/codegen.rs` but in disjoint line regions (verified against `main` @ `1bc678f` when this plan was written) — three people can have open PRs against this file simultaneously without touching the same lines, **as long as each rebases onto the Task 1+2 merge commit first and nothing else lands on `src/codegen.rs` in between.** Tasks 6 and 7 touch entirely separate files and can proceed with zero coordination.

If in doubt about ordering: **Phase 0's two tasks are strictly sequential and block everyone. Phase 1's five tasks can start the moment Task 2 merges, in any order, by any number of people. Task 8 cannot start until all five of Phase 1 are merged.**

---

## Task 1: Header-carrying allocator (`verb_alloc`) + static string sentinel

**Files:**
- Modify: `src/codegen.rs`, `runtime/verb.h`
- Test: `tests/e2e.rs`

**Interfaces:**
- Produces: `fn build_alloc_fn(&self)` (LLVM function `verb_alloc(i64) -> ptr`), `fn declare_gc_globals(&self)` (module global `verb_gc_live: i64`), `fn inc_live_counter(&self)`, `fn static_string_ptr(&self, s: &str) -> PointerValue<'ctx>`, `GC_STATIC_SENTINEL: i64` in `src/value.rs`. `malloc_bytes`/`malloc_bytes_dyn` now call `verb_alloc` instead of raw `malloc`. `extern "C" void* verb_alloc(int64_t n);` declared in `runtime/verb.h` for Tasks 6/7 to call.
- Consumed by: every later task in this plan.

Every heap block gets an `i64` refcount header immediately before the
pointer Verb already carries: `[i64 refcount][payload...]`. `verb_alloc(n)`
mallocs `n + 8`, stores `1` at offset 0, returns `raw + 8`. A module-global
`i64 verb_gc_live` counts outstanding `verb_alloc` blocks (incremented
here; decremented wherever a block is actually freed, in Task 2) — the
leak oracle Task 8 verifies against zero. String literals get the same
header shape baked into their LLVM global, with a sentinel
(`i64::MIN`) instead of a live count, so retain/release can always read
a header at `payload - 8` for any string, static or heap.

- [ ] **Step 1: Add the sentinel constant to `src/value.rs`**

Add at the end of the file:
```rust
/// Refcount-header value that marks a string as static (a source literal,
/// never heap-allocated, never freed). Never a value a real refcount can
/// reach from 1 by increment/decrement in any real program.
pub const GC_STATIC_SENTINEL: i64 = i64::MIN;
```

- [ ] **Step 2: Add `free`/`getenv` to `declare_libc`, and the `verb_gc_live` global**

Find:
```rust
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
    }
```
change to:
```rust
        self.module.add_function("strcmp", i32t.fn_type(&[pt.into(), pt.into()], false), None);
        self.module.add_function("free", self.ctx.void_type().fn_type(&[pt.into()], false), None);
        self.module.add_function("getenv", pt.fn_type(&[pt.into()], false), None);
    }

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

- [ ] **Step 3: Add `build_alloc_fn` and register it + `declare_gc_globals` in `Codegen::new`**

Add this method next to `malloc_bytes`:
```rust
    /// Runtime helper: verb_alloc(i64 n) -> ptr. Wraps `malloc` with an
    /// 8-byte refcount header (initialized to 1) prefixed to every heap
    /// block Verb owns; the returned pointer is the payload -- the header
    /// lives at payload-8. String literals get the same header shape
    /// baked into their LLVM global (see `static_string_ptr`) so
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
change to:
```rust
        cg.declare_libc();
        cg.declare_gc_globals();
        cg.build_alloc_fn();
        cg.build_type_name_fn();
```

- [ ] **Step 4: Switch `malloc_bytes`/`malloc_bytes_dyn` and the concat buffer to `verb_alloc`**

Find:
```rust
    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("malloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }

    /// Like `malloc_bytes`, but the size is a runtime value (used when an
    /// array buffer's size depends on its element count, not a fixed layout).
    fn malloc_bytes_dyn(&self, n: IntValue<'ctx>) -> PointerValue<'ctx> {
        self.call_named("malloc", &[n.into()]).unwrap().into_pointer_value()
    }
```
change both bodies' `"malloc"` to `"verb_alloc"`:
```rust
    fn malloc_bytes(&self, n: u64) -> PointerValue<'ctx> {
        self.call_named("verb_alloc", &[self.ctx.i64_type().const_int(n, false).into()])
            .unwrap().into_pointer_value()
    }

    /// Like `malloc_bytes`, but the size is a runtime value (used when an
    /// array buffer's size depends on its element count, not a fixed layout).
    fn malloc_bytes_dyn(&self, n: IntValue<'ctx>) -> PointerValue<'ctx> {
        self.call_named("verb_alloc", &[n.into()]).unwrap().into_pointer_value()
    }
```

Find the concat buffer allocation inside `build_concat_fn` (search for
`let buf = self.call_named("malloc"`) and change `"malloc"` to
`"verb_alloc"` there too.

- [ ] **Step 5: Add `static_string_ptr` and switch `Expr::Str` to use it**

Add next to `cstr`:
```rust
    /// Builds a global for a Verb string *literal*: an i64 sentinel header
    /// immediately followed by the NUL-terminated bytes, laid out
    /// identically to a heap `verb_alloc` block (header at payload-8) so
    /// `verb_retain_value`/`verb_release_value` (Task 2) can treat every
    /// string pointer the same way. Returns a pointer to the byte data
    /// (not the header) -- exactly what `Expr::Str` needs.
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
        g.set_linkage(inkwell::module::Linkage::Private);
        g.set_unnamed_addr(true);
        unsafe {
            self.builder.build_in_bounds_gep(
                struct_ty, g.as_pointer_value(),
                &[i32t.const_zero(), i32t.const_int(1, false), i32t.const_zero()],
                "strdata",
            )
        }.unwrap()
    }
```

Find:
```rust
            Expr::Str(s) => {
                let p = self.cstr(s);
                let bits = self.builder.build_ptr_to_int(p, self.ctx.i64_type(), "sbits").unwrap();
                Ok(self.make_val(TAG_STR, bits))
            }
```
change `self.cstr(s)` to `self.static_string_ptr(s)`. Leave `cstr` itself
untouched — it's still used for internal format/type-name strings that
never become a Verb `VerbValue`.

- [ ] **Step 6: Declare `verb_alloc` in `runtime/verb.h` for C++ callers**

Add after the `#include` block, before the tag enum:
```c
// Defined by Verb's own generated LLVM module (src/codegen.rs,
// build_alloc_fn): allocates n bytes with an 8-byte GC refcount header
// prefixed, initialized to 1. Any C++ code that hands a heap-owned
// value back to Verb MUST allocate through this, not malloc/new/strdup --
// verb_retain_value/verb_release_value (Task 2) read a refcount at
// ptr-8 for every string/array/map they see, and an unheadered pointer
// there is undefined behavior the first time Verb retains or releases it.
extern "C" void* verb_alloc(int64_t n);
```

- [ ] **Step 7: Add IR-shape regression tests**

In `tests/e2e.rs`, add:
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

#[test]
fn string_literals_carry_a_static_gc_sentinel_header() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/strings.verb", "--emit-llvm"])
        .output()
        .unwrap();
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("-9223372036854775808"), "no GC static sentinel in IR:\n{ir}");
    assert!(ir.contains("private unnamed_addr constant { i64,"),
        "string literal global isn't private/unnamed_addr:\n{ir}");
}
```

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS — every existing test stays green (this task only changes
where bytes come from and adds header bytes before them; nothing yet
reads those headers, and string literals are still valid NUL-terminated
C strings at the pointer Verb sees).

- [ ] **Step 9: Commit**

```bash
git add src/codegen.rs src/value.rs runtime/verb.h tests/e2e.rs
git commit -m "feat(gc): add header-carrying verb_alloc allocator and static string sentinel"
```

---

## Task 2: Full retain/release dispatch (strings, closures, arrays, maps, cells)

**Files:**
- Modify: `src/codegen.rs`

**Interfaces:**
- Consumes: `verb_alloc`'s header layout, `GC_STATIC_SENTINEL` (Task 1).
- Produces: `verb_retain_value(VerbValue) -> void`, `verb_release_value(VerbValue) -> void`, `verb_retain_cell(ptr) -> void`, `verb_release_cell(ptr) -> void` (LLVM IR functions, callable by name from any later task via `self.call_named(...)`). Also `fn header_ptr(&self, payload: PointerValue<'ctx>) -> PointerValue<'ctx>` and `fn dec_live_counter(&self)`. Declares (but does not define) `extern "C" void verb_map_destroy_contents(void* payload);` — Task 6 defines it in `runtime/verb_map.cpp`.

These four functions are dead code until Phase 1's tasks wire in call
sites — that's fine, they're LLVM module functions emitted by
`Codegen::new` regardless of whether anything calls them yet, so there's
no Rust-level dead-code warning.

- [ ] **Step 1: Add `header_ptr` and `dec_live_counter`**

Add next to `inc_live_counter`:
```rust
    /// Given a payload pointer (what a `VerbValue` or a cell already
    /// points at), returns a pointer to its 8-byte refcount header,
    /// living immediately before it. Valid for every string, closure,
    /// array, map, and cell pointer Verb ever produces.
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

- [ ] **Step 2: Declare the map-destroy callback**

Add to `declare_libc` (this is the one shared declaration Task 6 will
define the body of, in a different file — declaring it here means
Task 2 and Task 6 never touch the same line):
```rust
        self.module.add_function(
            "verb_map_destroy_contents", self.ctx.void_type().fn_type(&[pt.into()], false), None);
```
(add this line right after the `getenv` declaration from Task 1, inside
`declare_libc`).

- [ ] **Step 3: Add `build_retain_value_fn`**

```rust
    /// Runtime helper: verb_retain_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag (string, closure, array, map).
    /// Static string literals (sentinel header) are skipped -- immortal,
    /// count never moves.
    fn build_retain_value_fn(&self) {
        use inkwell::IntPredicate::*;
        let i8t = self.ctx.i8_type();
        let i64t = self.ctx.i64_type();
        let fnty = self.ctx.void_type().fn_type(&[self.value_ty.into()], false);
        let f = self.module.add_function("verb_retain_value", fnty, None);
        let entry = self.ctx.append_basic_block(f, "entry");
        let str_bb = self.ctx.append_basic_block(f, "str");
        let str_bump_bb = self.ctx.append_basic_block(f, "str.bump");
        let heap_check_bb = self.ctx.append_basic_block(f, "heap.check");
        let heap_bump_bb = self.ctx.append_basic_block(f, "heap.bump");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, heap_check_bb).unwrap();

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

        // closure/array/map all share the same "always heap, always just
        // bump the header" behavior for retain -- only release (Step 4)
        // needs different cascade logic per tag.
        self.builder.position_at_end(heap_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        let is_arr = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ARRAY, false), "is_arr").unwrap();
        let is_map = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_MAP, false), "is_map").unwrap();
        let is_clos_or_arr = self.builder.build_or(is_clos, is_arr, "is_clos_or_arr").unwrap();
        let is_heap = self.builder.build_or(is_clos_or_arr, is_map, "is_heap").unwrap();
        self.builder.build_conditional_branch(is_heap, heap_bump_bb, done_bb).unwrap();

        self.builder.position_at_end(heap_bump_bb);
        let hp = self.builder.build_int_to_ptr(p, self.ptr_ty, "hp").unwrap();
        let hhdr = self.header_ptr(hp);
        let hcur = self.builder.build_load(i64t, hhdr, "hcur").unwrap().into_int_value();
        let hnext = self.builder.build_int_add(hcur, i64t.const_int(1, false), "hnext").unwrap();
        self.builder.build_store(hhdr, hnext).unwrap();
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }
```

- [ ] **Step 4: Add `build_release_value_fn`**

```rust
    /// Runtime helper: verb_release_value(VerbValue v) -> void. No-op
    /// unless v is a heap-identity tag; on those, decrements the header
    /// and, at zero, cascades per-tag before freeing:
    /// - STR: no cascade, just free (skip entirely if static sentinel).
    /// - CLOSURE: no cascade (`env` is always null), just free.
    /// - ARRAY: release every element 0..len (cascading into any
    ///   heap-owned element), free `elems`, free the header.
    /// - MAP: call `verb_map_destroy_contents` (defined in
    ///   runtime/verb_map.cpp) to cascade-release every key/value and run
    ///   the map's C++ destructor, then free the header here (the one
    ///   place every heap kind's header actually gets `free`d).
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
        let clos_dec_bb = self.ctx.append_basic_block(f, "clos.dec");
        let clos_free_bb = self.ctx.append_basic_block(f, "clos.free");
        let arr_check_bb = self.ctx.append_basic_block(f, "arr.check");
        let arr_bb = self.ctx.append_basic_block(f, "arr");
        let arr_dec_bb = self.ctx.append_basic_block(f, "arr.dec");
        let arr_free_bb = self.ctx.append_basic_block(f, "arr.free");
        let arr_loop_cond_bb = self.ctx.append_basic_block(f, "arr.loop.cond");
        let arr_loop_body_bb = self.ctx.append_basic_block(f, "arr.loop.body");
        let arr_loop_end_bb = self.ctx.append_basic_block(f, "arr.loop.end");
        let map_check_bb = self.ctx.append_basic_block(f, "map.check");
        let map_bb = self.ctx.append_basic_block(f, "map");
        let map_dec_bb = self.ctx.append_basic_block(f, "map.dec");
        let map_free_bb = self.ctx.append_basic_block(f, "map.free");
        let done_bb = self.ctx.append_basic_block(f, "done");

        self.builder.position_at_end(entry);
        let v = f.get_nth_param(0).unwrap().into_struct_value();
        let (t, p) = (self.tag_of(v), self.payload_of(v));
        let is_str = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_STR, false), "is_str").unwrap();
        self.builder.build_conditional_branch(is_str, str_bb, clos_check_bb).unwrap();

        // ----- string -----
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

        // ----- closure (env always null: no cascade) -----
        self.builder.position_at_end(clos_check_bb);
        let is_clos = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_CLOSURE, false), "is_clos").unwrap();
        self.builder.build_conditional_branch(is_clos, clos_bb, arr_check_bb).unwrap();

        self.builder.position_at_end(clos_bb);
        let cp = self.builder.build_int_to_ptr(p, self.ptr_ty, "cp").unwrap();
        let chdr = self.header_ptr(cp);
        let ccur = self.builder.build_load(i64t, chdr, "ccur").unwrap().into_int_value();
        let cnext = self.builder.build_int_sub(ccur, i64t.const_int(1, false), "cnext").unwrap();
        self.builder.build_store(chdr, cnext).unwrap();
        let czero = self.builder.build_int_compare(EQ, cnext, i64t.const_zero(), "czero").unwrap();
        self.builder.build_conditional_branch(czero, clos_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(clos_dec_bb);
        self.builder.build_unconditional_branch(clos_free_bb).unwrap();

        self.builder.position_at_end(clos_free_bb);
        self.dec_live_counter();
        self.call_named("free", &[chdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- array: cascade into every element, then free elems + header -----
        self.builder.position_at_end(arr_check_bb);
        let is_arr = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_ARRAY, false), "is_arr").unwrap();
        self.builder.build_conditional_branch(is_arr, arr_bb, map_check_bb).unwrap();

        self.builder.position_at_end(arr_bb);
        let ap = self.builder.build_int_to_ptr(p, self.ptr_ty, "ap").unwrap();
        let ahdr = self.header_ptr(ap);
        let acur = self.builder.build_load(i64t, ahdr, "acur").unwrap().into_int_value();
        let anext = self.builder.build_int_sub(acur, i64t.const_int(1, false), "anext").unwrap();
        self.builder.build_store(ahdr, anext).unwrap();
        let azero = self.builder.build_int_compare(EQ, anext, i64t.const_zero(), "azero").unwrap();
        self.builder.build_conditional_branch(azero, arr_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(arr_dec_bb);
        self.builder.build_unconditional_branch(arr_free_bb).unwrap();

        self.builder.position_at_end(arr_free_bb);
        let lenp = self.builder.build_struct_gep(self.array_ty, ap, 0, "lenp").unwrap();
        let elemsp = self.builder.build_struct_gep(self.array_ty, ap, 2, "elemsp").unwrap();
        let len = self.builder.build_load(i64t, lenp, "len").unwrap().into_int_value();
        let elems = self.builder.build_load(self.ptr_ty, elemsp, "elems").unwrap().into_pointer_value();
        let idxp = self.entry_alloca(i64t.into(), "relidx");
        self.builder.build_store(idxp, i64t.const_zero()).unwrap();
        self.builder.build_unconditional_branch(arr_loop_cond_bb).unwrap();

        self.builder.position_at_end(arr_loop_cond_bb);
        let i = self.builder.build_load(i64t, idxp, "i").unwrap().into_int_value();
        let more = self.builder.build_int_compare(
            inkwell::IntPredicate::SLT, i, len, "more").unwrap();
        self.builder.build_conditional_branch(more, arr_loop_body_bb, arr_loop_end_bb).unwrap();

        self.builder.position_at_end(arr_loop_body_bb);
        let slot = unsafe {
            self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot")
        }.unwrap();
        let elemv = self.builder.build_load(self.value_ty, slot, "elemv").unwrap().into_struct_value();
        self.call_named("verb_release_value", &[elemv.into()]);
        let inext = self.builder.build_int_add(i, i64t.const_int(1, false), "inext").unwrap();
        self.builder.build_store(idxp, inext).unwrap();
        self.builder.build_unconditional_branch(arr_loop_cond_bb).unwrap();

        self.builder.position_at_end(arr_loop_end_bb);
        self.dec_live_counter();
        self.call_named("free", &[elems.into()]);
        self.call_named("free", &[ahdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        // ----- map: cascade via runtime/verb_map.cpp, then free header -----
        self.builder.position_at_end(map_check_bb);
        let is_map = self.builder.build_int_compare(EQ, t, i8t.const_int(TAG_MAP, false), "is_map").unwrap();
        self.builder.build_conditional_branch(is_map, map_bb, done_bb).unwrap();

        self.builder.position_at_end(map_bb);
        let mp = self.builder.build_int_to_ptr(p, self.ptr_ty, "mp").unwrap();
        let mhdr = self.header_ptr(mp);
        let mcur = self.builder.build_load(i64t, mhdr, "mcur").unwrap().into_int_value();
        let mnext = self.builder.build_int_sub(mcur, i64t.const_int(1, false), "mnext").unwrap();
        self.builder.build_store(mhdr, mnext).unwrap();
        let mzero = self.builder.build_int_compare(EQ, mnext, i64t.const_zero(), "mzero").unwrap();
        self.builder.build_conditional_branch(mzero, map_dec_bb, done_bb).unwrap();
        self.builder.position_at_end(map_dec_bb);
        self.builder.build_unconditional_branch(map_free_bb).unwrap();

        self.builder.position_at_end(map_free_bb);
        self.call_named("verb_map_destroy_contents", &[mp.into()]);
        self.dec_live_counter();
        self.call_named("free", &[mhdr.into()]);
        self.builder.build_unconditional_branch(done_bb).unwrap();

        self.builder.position_at_end(done_bb);
        self.builder.build_return(None).unwrap();
    }
```

- [ ] **Step 5: Add `build_retain_cell_fn` and `build_release_cell_fn`**

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
    /// inside (cascading into a heap-owned string/closure/array/map if
    /// that's what the cell holds) and frees the cell block itself.
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

- [ ] **Step 6: Register all four in `Codegen::new`**

Find:
```rust
        cg.build_array_pop_fn();
        cg
```
change to:
```rust
        cg.build_array_pop_fn();
        cg.build_retain_value_fn();
        cg.build_release_value_fn();
        cg.build_retain_cell_fn();
        cg.build_release_cell_fn();
        cg
```

- [ ] **Step 7: Add an IR-shape test**

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
    assert!(ir.contains("declare void @verb_map_destroy_contents"),
        "verb_map_destroy_contents not declared:\n{ir}");
}
```

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS. Nothing calls these functions yet (Phase 1 wires them
in), so program behavior is unchanged. `verb_map_destroy_contents` is
only *declared*, never defined, in this task — that's fine, nothing
calls it yet either, and Rust/LLVM only complain about an undefined
symbol at *link* time, which doesn't happen until a program actually
uses `import std map` AND something calls it (Task 6 defines it before
Task 8 needs it).

- [ ] **Step 9: Commit**

```bash
git add src/codegen.rs tests/e2e.rs
git commit -m "feat(gc): add verb_retain_value/verb_release_value/verb_retain_cell/verb_release_cell for all 4 heap tags"
```

---

## Task 3: Core value-lifecycle wiring (strings, closures, cells — the general convention)

**Owner note:** This task can start the moment Task 2 is merged. It runs in parallel with Tasks 4, 5, 6, 7 — none of them touch the same lines.

**Files:**
- Modify: `src/codegen.rs` (`Expr::Var`, `Stmt::ExprStmt`, `Stmt::If`/`Stmt::While` condition lines, `Expr::Unary`, `gen_binary`, `gen_call`'s `print` + general call path, `gen_std_io_call`, `gen_extern_call`, `Stmt::Reassign`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_retain_value`/`verb_release_value` (Task 2).
- Produces: the convention every other task in this plan relies on — **every `gen_expr` result is an owned temporary**: transferred into a fresh cell (no extra op) or released with `verb_release_value` once its use ends without being stored.

- [ ] **Step 1: Retain on `Expr::Var` load**

Find:
```rust
            Expr::Var(name, line, col) => {
                if let Some(cell) = self.lookup(name) {
                    return Ok(self.builder.build_load(self.value_ty, cell, name)
                        .unwrap().into_struct_value());
                }
                Err(self.undefined_var(name, *line, *col))
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
                Err(self.undefined_var(name, *line, *col))
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
            Stmt::If { cond, then_body, else_body } => {
                let cv = self.gen_expr(cond)?;
                let t = self.call_named("verb_truthy", &[cv.into()]).unwrap().into_int_value();
                let f = self.builder.get_insert_block().unwrap().get_parent().unwrap();
```
change to:
```rust
            Stmt::If { cond, then_body, else_body } => {
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

- [ ] **Step 5: Release binary operands (non-short-circuit path), and the discarded operand of short-circuit `and`/`or`**

Find:
```rust
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
        let out = if matches!(op, BinOp::Eq | BinOp::Ne) {
            // eq never aborts, so it takes no location
            self.call_named("verb_eq", &[l.into(), r.into()]).unwrap().into_struct_value()
        } else {
            let helper = match op {
                BinOp::Add => "verb_add", BinOp::Sub => "verb_sub", BinOp::Mul => "verb_mul",
                BinOp::Div => "verb_div", BinOp::Mod => "verb_mod",
                BinOp::Lt => "verb_lt", BinOp::Gt => "verb_gt",
                BinOp::Le => "verb_le", BinOp::Ge => "verb_ge",
                BinOp::Concat => "verb_concat",
                BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or => unreachable!(),
            };
            let (lc, cc) = self.loc_consts(line, col);
            self.call_named(helper, &[l.into(), r.into(), lc.into(), cc.into()])
                .unwrap().into_struct_value()
        };
        if matches!(op, BinOp::Ne) {
            let p = self.payload_of(out);
            let flipped = self.builder.build_xor(
                p, self.ctx.i64_type().const_int(1, false), "ne").unwrap();
            return Ok(self.make_val(TAG_BOOL, flipped));
        }
        Ok(out)
```
change to (two changes: release `l` in `rhs_bb` — since `rhs_bb` is
entered *only* when `r` becomes the phi result and `l` is discarded —
and release `l`/`r` after computing `out` on the non-short-circuit path):
```rust
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
            // rhs_bb is only entered when `r` becomes the result instead of
            // `l`, so the owned temporary `l` is being discarded here.
            self.call_named("verb_release_value", &[l.into()]);
            let rhs_end = self.builder.get_insert_block().unwrap();
            self.builder.build_unconditional_branch(merge).unwrap();
            self.builder.position_at_end(merge);
            let phi = self.builder.build_phi(self.value_ty, "sc").unwrap();
            phi.add_incoming(&[(&l, lhs_end), (&r, rhs_end)]);
            return Ok(phi.as_basic_value().into_struct_value());
        }

        let l = self.gen_expr(lhs)?;
        let r = self.gen_expr(rhs)?;
        let out = if matches!(op, BinOp::Eq | BinOp::Ne) {
            // eq never aborts, so it takes no location
            self.call_named("verb_eq", &[l.into(), r.into()]).unwrap().into_struct_value()
        } else {
            let helper = match op {
                BinOp::Add => "verb_add", BinOp::Sub => "verb_sub", BinOp::Mul => "verb_mul",
                BinOp::Div => "verb_div", BinOp::Mod => "verb_mod",
                BinOp::Lt => "verb_lt", BinOp::Gt => "verb_gt",
                BinOp::Le => "verb_le", BinOp::Ge => "verb_ge",
                BinOp::Concat => "verb_concat",
                BinOp::Eq | BinOp::Ne | BinOp::And | BinOp::Or => unreachable!(),
            };
            let (lc, cc) = self.loc_consts(line, col);
            self.call_named(helper, &[l.into(), r.into(), lc.into(), cc.into()])
                .unwrap().into_struct_value()
        };
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

- [ ] **Step 6: Release the arg to `print`**

Find:
```rust
            if name == "print" {
                if args.len() != 1 {
                    return Err(CompileError::new("print takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                return Ok(self.nil_val());
            }
```
change to:
```rust
            if name == "print" {
                if args.len() != 1 {
                    return Err(CompileError::new("print takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                self.call_named("verb_print", &[v.into()]);
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(self.nil_val());
            }
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
(`fp`/`env` are already loaded into SSA registers by this point, so
releasing — and potentially freeing — the closure struct here is safe.
The `argv`-filling loop just above is untouched: argument ownership
transfers into the callee's param cells with no extra op, unchanged.)

- [ ] **Step 8: Release std-io/std-map call arguments after the call**

`gen_std_io_call` passes `argvals` directly as raw LLVM call arguments
(no cell, unlike a Verb-to-Verb call) — the extern function reads them
but never takes ownership, so the caller must release them once the
call returns. This function is shared by both `import std io` and
`import std map` (`MAP_FUNCS`), so this one change covers both.

Find:
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

**Important for Task 6 (maps):** since `gen_std_io_call`'s convention now
releases *every* argument after the call, any std-io/std-map function
whose return value *aliases* one of its own arguments (same underlying
reference, e.g. `map_set(m, k, v)` returning `m`) needs to explicitly
`verb_retain_value` that value before returning it on the C++ side —
otherwise this generic release would undercount it. This is Task 6's
responsibility (`runtime/verb_map.cpp`), not this task's; flagged here
so whoever picks up Task 6 knows why.

- [ ] **Step 9: Release the old value on `Reassign`**

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

- [ ] **Step 10: Add a fixture + test for reassignment and short-circuit and/or**

Create `tests/fixtures/gc_reassign_and_or.verb`:
```
assign s "a";
loop assign i 0; i trails 5; i be i add 1 begin
  s be s join "x";
end
print(s);
assign a "x" join "1";
assign b "y" join "2";
assign c a and b;
print(c);
assign d a or b;
print(d);
```

Create `tests/fixtures/gc_reassign_and_or.expected`:
```
axxxxx
y2
x1
```

Register in `tests/e2e.rs`:
```rust
#[test]
fn reassign_and_short_circuit_release_correctly() { run_ok("gc_reassign_and_or"); }
```

- [ ] **Step 11: Run the full test suite**

Run: `cargo test`
Expected: PASS with identical output to before this task on every
existing fixture — this task only balances individual value
reads/uses; it does not yet release variable cells or globals (Task 4),
so no leaks are fully fixed yet, but nothing should crash or change
program output.

- [ ] **Step 12: Commit**

```bash
git add src/codegen.rs tests/e2e.rs tests/fixtures/gc_reassign_and_or.verb tests/fixtures/gc_reassign_and_or.expected
git commit -m "feat(gc): retain-on-load, release-on-discard for strings/closures across core expr/stmt codegen"
```

---

## Task 4: Scope, function-exit, and globals wiring

**Owner note:** This task can start the moment Task 2 is merged. It runs in parallel with Tasks 3, 5, 6, 7 — none of them touch the same lines.

**Files:**
- Modify: `src/codegen.rs` (`Stmt::Block`/`Stmt::If`/`Stmt::While` scope push/pop, `Stmt::Return`, `Stmt::Fn`'s body-end, `compile_program`'s tail, `bind()`, `global_slot()`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_retain_cell`/`verb_release_cell`/`verb_release_value` (Task 2).
- Produces: `fn release_scope(&self, scope: &HashMap<String, PointerValue<'ctx>>)`, `fn release_all_open_scopes(&self)`, `fn emit_gc_debug_dump(&self, main: FunctionValue<'ctx>)`.

- [ ] **Step 1: Add `release_scope`**

Add next to `malloc_bytes`:
```rust
    fn release_scope(&self, scope: &HashMap<String, PointerValue<'ctx>>) {
        for cell in scope.values() {
            self.call_named("verb_release_cell", &[(*cell).into()]);
        }
    }
```

- [ ] **Step 2: Release on `Stmt::Block`/`Stmt::If`/`Stmt::While` scope exit (guarded — never release into an already-terminated block)**

**Why the guard matters:** if `gen_stmts` returned because a nested
`return` already emitted `build_return`, the current basic block is
terminated — appending more instructions (the release calls) into it is
invalid LLVM IR. Only release when the block is still open
(`cur_block_open()`); otherwise just pop, no release (Step 4 below
handles the early-return case correctly, at the return site itself,
before its own terminator).

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

Find:
```rust
                self.builder.position_at_end(body_bb);
                self.scopes.push(HashMap::new());
                self.gen_stmts(body)?;
                self.scopes.pop();
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

- [ ] **Step 3: Add `release_all_open_scopes` — read-only over `self.scopes`, never pops**

**This is the one detail that must not be gotten wrong.** `self.scopes`
is a single `Vec` shared across a whole function's codegen pass. If this
helper popped scopes when handling an early `return` inside one branch
of an `if`, the *other* branch (compiled afterward, in the same pass,
sharing the same `Vec`) would find `self.scopes` missing frames it still
needs for variable lookups — corrupting unrelated code. The fix: only
**read**, never pop. Every pre-existing `self.scopes.pop()` (Step 2
above, `Stmt::Fn`'s trailing pop, `compile_program`) remains the *only*
code removing frames.

Add next to `release_scope`:
```rust
    /// Releases every cell in every currently-open scope (this function's
    /// own scope stack -- already isolated per-function via the
    /// `saved_scopes` swap in `Stmt::Fn`), innermost first. Read-only over
    /// `self.scopes`: never pops. Must run immediately before *every*
    /// path that can leave a function or the top-level program -- an
    /// explicit `return`, or an implicit end-of-body/end-of-program
    /// return -- since Step 2's scope-pop cleanup only fires on normal
    /// block fall-through and is skipped once a block is already
    /// terminated.
    fn release_all_open_scopes(&self) {
        for scope in self.scopes.iter().rev() {
            self.release_scope(scope);
        }
    }
```

- [ ] **Step 4: Call it before `Stmt::Return`'s `build_return`, and `Stmt::Fn`'s implicit end-of-body return**

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
(`v` was computed before releasing scopes, so a returned value that
came straight out of a local cell — e.g. `return x;` — already carries
its own independent, retained reference from `Expr::Var`'s Task-3 retain;
releasing that cell here doesn't touch `v`'s own count.)

Find (inside `Stmt::Fn`):
```rust
                self.scopes.push(scope);
                let r = self.gen_stmts(body);
                if self.cur_block_open() {
                    self.builder.build_return(Some(&self.nil_val())).unwrap();
                }
                self.scopes.pop();
```
change to:
```rust
                self.scopes.push(scope);
                let r = self.gen_stmts(body);
                if self.cur_block_open() {
                    self.release_all_open_scopes();
                    self.builder.build_return(Some(&self.nil_val())).unwrap();
                }
                self.scopes.pop();
```
(Guarded by `cur_block_open()`: if the body already returned explicitly,
that `Stmt::Return` already released everything via Step 4's first
change — this branch only fires for the synthesized implicit `nil`
return. The trailing `self.scopes.pop()` stays exactly where it is,
unconditional, removing this function's own top-level frame in both
cases.)

- [ ] **Step 5: Release the old value on global rebind (`bind()`)**

Find:
```rust
    fn bind(&mut self, name: &str, value: StructValue<'ctx>) {
        if self.scopes.is_empty() {
            let slot = self.global_slot(name);
            self.builder.build_store(slot, value).unwrap();
        } else {
            let cell = self.malloc_bytes(16);
            self.builder.build_store(cell, value).unwrap();
            self.scopes.last_mut().unwrap().insert(name.to_string(), cell);
        }
    }
```
change to:
```rust
    fn bind(&mut self, name: &str, value: StructValue<'ctx>) {
        if self.scopes.is_empty() {
            let slot = self.global_slot(name);
            let old = self.builder.build_load(self.value_ty, slot, "old_global").unwrap().into_struct_value();
            self.call_named("verb_release_value", &[old.into()]);
            self.builder.build_store(slot, value).unwrap();
        } else {
            let cell = self.malloc_bytes(16);
            self.builder.build_store(cell, value).unwrap();
            if let Some(old_cell) = self.scopes.last_mut().unwrap().insert(name.to_string(), cell) {
                self.call_named("verb_release_cell", &[old_cell.into()]);
            }
        }
    }
```
(The release-before-store on the global path is unconditional, including
the very first bind of a given name — a freshly-created global slot is
zero-initialized to `{tag: NIL, payload: 0}` via `global_slot`, and
releasing a NIL value is always a no-op, so no special-casing is needed.
The `insert`-returns-old-value check on the cell path fixes the same
leak class for a name re-`assign`ed/re-`declare`d twice in one scope,
which previously silently orphaned the earlier cell.)

- [ ] **Step 6: Release all globals and emit the `VERB_GC_DEBUG` diagnostic at program exit**

Find:
```rust
        if self.cur_block_open() {
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }
```
(this is the tail of `compile_program`) change to:
```rust
        if self.cur_block_open() {
            for slot in self.globals.values() {
                let v = self.builder.build_load(self.value_ty, *slot, "gval").unwrap().into_struct_value();
                self.call_named("verb_release_value", &[v.into()]);
            }
            self.emit_gc_debug_dump(main);
            self.builder.build_return(Some(&self.ctx.i32_type().const_zero())).unwrap();
        }
        Ok(())
    }
```

Add this method next to `release_all_open_scopes`:
```rust
    /// If `VERB_GC_DEBUG` is set in the environment, prints
    /// `verb_gc_live=<n>` to stdout, where `<n>` is the number of
    /// outstanding `verb_alloc` blocks (strings/closures/arrays/maps/cells)
    /// at program exit. Purely a test/debugging hook -- silent otherwise,
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

- [ ] **Step 7: Add fixtures for a global reassignment and an early return from a nested block**

Create `tests/fixtures/gc_global_reassign.verb`:
```
assign g "first" join "!";
assign g "second" join "!";
print(g);
```

Create `tests/fixtures/gc_global_reassign.expected`:
```
second!
```

Create `tests/fixtures/gc_early_return_nested.verb`:
```
make classify(n) begin
  loop assign i 0; i trails 3; i be i add 1 begin
    check n equals i begin
      return "matched" join "!";
    end
  end
  check n trails 0 begin
    return "negative";
  end orelse begin
    print(n);
    return "non-negative";
  end
end

print(classify(neg 5));
print(classify(5));
print(classify(1));
```

Verify this fixture's actual output by running it (`cargo run -- run
tests/fixtures/gc_early_return_nested.verb`) before writing the
`.expected` file — do not guess.

Register both in `tests/e2e.rs`:
```rust
#[test]
fn global_reassignment_releases_previous_value() { run_ok("gc_global_reassign"); }

#[test]
fn early_return_from_nested_loop_and_if_else_leaves_scopes_intact() { run_ok("gc_early_return_nested"); }
```

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/codegen.rs tests/e2e.rs tests/fixtures/gc_global_reassign.verb tests/fixtures/gc_global_reassign.expected tests/fixtures/gc_early_return_nested.verb tests/fixtures/gc_early_return_nested.expected
git commit -m "feat(gc): release cells/globals on scope exit, early return, and program exit; add VERB_GC_DEBUG diagnostic"
```

---

## Task 5: Array call-site wiring

**Owner note:** This task can start the moment Task 2 is merged. It runs in parallel with Tasks 3, 4, 6, 7 — none of them touch the same lines.

**Files:**
- Modify: `src/codegen.rs` (`gen_call`'s inline `get`/`set`/`push`/`pop`/`len` dispatch, `build_array_set_fn`, `build_array_push_fn`)
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_release_value` (Task 2). Array cascade-release (freeing `elems` + iterating elements) is already handled generically inside `verb_release_value`'s `TAG_ARRAY` case (Task 2) — this task only wires the *call sites* and fixes two return-value subtleties specific to arrays.

**The one subtlety to get right:** array builtins are dispatched
*inline* in `gen_call` (not through `gen_std_io_call`'s generic
argument-release loop), so operand release must be added explicitly at
each of the 5 call sites below. `build_array_set_fn` also needs special
care: it returns the *same* value `v` it just stored into the array's
slot — two live references to one value now exist (the slot's copy and
the returned copy) where before there was only one (the caller's
temporary), so `build_array_set_fn` must `verb_retain_value` that value
once before returning it. `build_array_get_fn`, `build_array_pop_fn`,
and `build_array_push_fn`'s returned/transferred values need no such
fix (see each step below for why).

- [ ] **Step 1: Retain the value `get` reads out**

Find (inside `build_array_get_fn`):
```rust
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        let v = self.builder.build_load(self.value_ty, slot, "v").unwrap().into_struct_value();
        self.builder.build_return(Some(&v)).unwrap();
    }

    // ----- generated runtime helper: verb_array_set(arr, idx, v, line, col) -> value (returns v) -----
```
change to:
```rust
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        let v = self.builder.build_load(self.value_ty, slot, "v").unwrap().into_struct_value();
        // The array's own slot keeps its reference; `get` hands back an
        // independent copy, mirroring Expr::Var's retain-on-load.
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_return(Some(&v)).unwrap();
    }

    // ----- generated runtime helper: verb_array_set(arr, idx, v, line, col) -> value (returns v) -----
```

- [ ] **Step 2: Retain the value `set` both stores and returns**

Find (inside `build_array_set_fn`):
```rust
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        self.builder.build_store(slot, v).unwrap();
        self.builder.build_return(Some(&v)).unwrap();
    }
```
change to:
```rust
        let slot = unsafe { self.builder.build_in_bounds_gep(self.value_ty, elems, &[i], "slot") }.unwrap();
        // `v` (the caller's owned temporary) is about to have two homes --
        // the array slot and the returned value -- where before it had
        // one. One retain covers the second home; the slot's copy is the
        // transfer of `v`'s original ownership (no separate op needed for
        // that half).
        self.call_named("verb_retain_value", &[v.into()]);
        self.builder.build_store(slot, v).unwrap();
        self.builder.build_return(Some(&v)).unwrap();
    }
```

- [ ] **Step 3: Free the old `elems` buffer on `push`'s grow path**

Find (inside `build_array_push_fn`):
```rust
        self.builder.position_at_end(cp_end);
        self.builder.build_store(capp, new_cap).unwrap();
        self.builder.build_store(elemsp, new_elems).unwrap();
        self.builder.build_unconditional_branch(after_grow_bb).unwrap();
```
change to:
```rust
        self.builder.position_at_end(cp_end);
        self.call_named("free", &[elems.into()]);
        self.builder.build_store(capp, new_cap).unwrap();
        self.builder.build_store(elemsp, new_elems).unwrap();
        self.builder.build_unconditional_branch(after_grow_bb).unwrap();
```
(`elems` here is the *old* buffer, already loaded earlier in `grow_bb`
via `let elems = self.builder.build_load(self.ptr_ty, elemsp,
"elems")...`, and every element has already been copied into
`new_elems` by the loop just above this point. This is a plain `free`,
not `verb_release_cell`/`verb_release_value` — `elems` was never
independently refcounted; it's owned outright by the array header, and
Task 2's `TAG_ARRAY` release path already frees whatever the *current*
`elems` pointer is when the header itself is freed. This closes the
pre-existing leak where every reallocation on `push` leaked the prior
buffer. `n == 0` initial arrays have `elems == null` per the array
literal codegen, and growing FROM an empty array only reaches this path
when `cap > 0` already — the very first grow (`cap == 0 -> 1`) also
takes this path since `elems` is `null` in that case, and `free(NULL)`
is always a safe no-op per the C standard, so no special-casing for the
zero-capacity case is needed.)

- [ ] **Step 4: Release array-builtin operands at their `gen_call` sites**

Find:
```rust
            if name == "len" {
                if args.len() != 1 {
                    return Err(CompileError::new("len takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_len", &[v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                return Ok(rv);
            }
```
change to:
```rust
            if name == "len" {
                if args.len() != 1 {
                    return Err(CompileError::new("len takes exactly 1 argument", line, col));
                }
                let v = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_len", &[v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[v.into()]);
                return Ok(rv);
            }
```

Find:
```rust
            if name == "get" {
                if args.len() != 2 {
                    return Err(CompileError::new("get takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_get", &[arr.into(), idx.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                return Ok(rv);
            }
```
change to:
```rust
            if name == "get" {
                if args.len() != 2 {
                    return Err(CompileError::new("get takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_get", &[arr.into(), idx.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                self.call_named("verb_release_value", &[idx.into()]);
                return Ok(rv);
            }
```

Find:
```rust
            if name == "set" {
                if args.len() != 3 {
                    return Err(CompileError::new("set takes exactly 3 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let v = self.gen_expr(&args[2])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_set", &[arr.into(), idx.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                return Ok(rv);
            }
```
change to (release `arr`/`idx` only — `v`'s two homes, slot + return,
are both already accounted for by Step 2's fix inside
`build_array_set_fn`, so the call site must NOT release `v`):
```rust
            if name == "set" {
                if args.len() != 3 {
                    return Err(CompileError::new("set takes exactly 3 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let idx = self.gen_expr(&args[1])?;
                let v = self.gen_expr(&args[2])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_set", &[arr.into(), idx.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                self.call_named("verb_release_value", &[idx.into()]);
                return Ok(rv);
            }
```

Find:
```rust
            if name == "push" {
                if args.len() != 2 {
                    return Err(CompileError::new("push takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let v = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_push", &[arr.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                return Ok(rv);
            }
```
change to (release `arr` only — `v` transfers into the array, no extra
op, matching how any argument-to-param-cell transfer already works;
`rv` is always `nil()`, never heap, so no release needed on it either):
```rust
            if name == "push" {
                if args.len() != 2 {
                    return Err(CompileError::new("push takes exactly 2 arguments", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let v = self.gen_expr(&args[1])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_push", &[arr.into(), v.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                return Ok(rv);
            }
```

Find:
```rust
            if name == "pop" {
                if args.len() != 1 {
                    return Err(CompileError::new("pop takes exactly 1 argument", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_pop", &[arr.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                return Ok(rv);
            }
```
change to (release `arr` only — the popped element transfers *out* of
the array to the caller, ownership moving with it, unchanged from
today's read; no retain needed there):
```rust
            if name == "pop" {
                if args.len() != 1 {
                    return Err(CompileError::new("pop takes exactly 1 argument", line, col));
                }
                let arr = self.gen_expr(&args[0])?;
                let (lc, cc) = self.loc_consts(line, col);
                let rv = self.call_named("verb_array_pop", &[arr.into(), lc.into(), cc.into()])
                    .unwrap().into_struct_value();
                self.call_named("verb_release_value", &[arr.into()]);
                return Ok(rv);
            }
```

- [ ] **Step 5: Add fixtures for nested arrays, arrays of closures, and push-driven regrowth**

Create `tests/fixtures/gc_arrays_nested.verb`:
```
assign inner1 list 1, 2;
assign inner2 list 3, 4;
assign outer list inner1, inner2;
push(get(outer, 1), 5);
print(get(get(outer, 1), 2));
print(len(outer));
```

Create `tests/fixtures/gc_arrays_nested.expected` — verify by running
the fixture first, don't guess:
```
5
2
```

Create `tests/fixtures/gc_arrays_of_closures.verb`:
```
make double(x) begin
  return x add x;
end
make triple(x) begin
  return x add x add x;
end
assign fns list double, triple;
print(get(fns, 0)(10));
print(get(fns, 1)(10));
```

Create `tests/fixtures/gc_arrays_of_closures.expected` — verify by
running the fixture first:
```
20
30
```

Create `tests/fixtures/gc_arrays_regrow.verb` (the array's own buffer
reallocates on every capacity doubling regardless of what's stored in
it, so plain ints are enough to exercise the Step 3 grow-path fix — no
need for heap-allocated elements here):
```
assign a list;
loop assign i 0; i trails 50; i be i add 1 begin
  push(a, i);
end
print(len(a));
print(get(a, 49));
```

Create `tests/fixtures/gc_arrays_regrow.expected`:
```
50
49
```

Register all three in `tests/e2e.rs`:
```rust
#[test]
fn nested_arrays_retain_and_release_correctly() { run_ok("gc_arrays_nested"); }

#[test]
fn arrays_of_closures_retain_and_release_correctly() { run_ok("gc_arrays_of_closures"); }

#[test]
fn array_push_regrowth_frees_old_buffers() { run_ok("gc_arrays_regrow"); }
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS, including all pre-existing `arrays_*` fixtures with
identical output.

- [ ] **Step 7: Commit**

```bash
git add src/codegen.rs tests/e2e.rs tests/fixtures/gc_arrays_nested.verb tests/fixtures/gc_arrays_nested.expected tests/fixtures/gc_arrays_of_closures.verb tests/fixtures/gc_arrays_of_closures.expected tests/fixtures/gc_arrays_regrow.verb tests/fixtures/gc_arrays_regrow.expected
git commit -m "feat(gc): wire array builtin call sites (get/set/push/pop/len); fix push grow-path leak"
```

---

## Task 6: Map wiring (runtime/verb_map.cpp only)

**Owner note:** This task can start the moment Task 2 is merged. It runs in parallel with Tasks 3, 4, 5, 7 — this task touches zero lines of `src/codegen.rs`.

**Files:**
- Modify: `runtime/verb_map.cpp`
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_alloc` (Task 1, declared in `runtime/verb.h`), `verb_retain_value`/`verb_release_value` (Task 2 — these are extern `"C"` symbols defined in the LLVM module; declare them the same way `verb_alloc` is declared, since `runtime/verb_map.cpp` needs to call both). `verb_map_destroy_contents` (declared as an extern by Task 2's `declare_libc` addition) is *defined* here.

**The one subtlety to get right:** `map_set(m, k, v)` returns `m` — the
*same* map reference it was given, not a fresh one. Because `map_set` is
called through `gen_std_io_call` (Task 3's Step 8 change), every one of
its arguments — including `m` — gets `verb_release_value`d generically
right after the call returns. If `map_set`'s return value is treated as
a brand-new owned temporary (per the standard convention) without an
explicit retain first, the map's refcount would be undercounted by
exactly one the moment a caller does something like `assign m2
map_set(m, k, v);` — `m`'s original reference gets released by the
generic argument-release, and the identical reference then gets stored
into `m2`'s cell with no compensating retain. Fix: `map_set` must
`verb_retain_value(m)` before returning it. `map_get`'s returned value
needs the same treatment for a different reason — mirroring array
`get`, the map keeps its own internal copy of the stored value, so
handing a copy back to the caller needs its own retain.

- [ ] **Step 1: Declare `verb_alloc`, `verb_retain_value`, `verb_release_value` in `verb_map.cpp`**

Find:
```cpp
#include "verb.h"

#include <cstring>
#include <unordered_map>
```
change to:
```cpp
#include "verb.h"

#include <cstring>
#include <unordered_map>

// Defined by Verb's own generated LLVM module (src/codegen.rs). GC
// contract: any heap value this file allocates must go through
// verb_alloc, not new/malloc; any VerbValue this file duplicates into a
// second live home (stored in the map AND handed back to the caller, or
// read out of the map as an independent copy) must be retained first.
extern "C" void* verb_alloc(int64_t n);
extern "C" void verb_retain_value(VerbValue v);
extern "C" void verb_release_value(VerbValue v);
```

- [ ] **Step 2: Switch `map_new` to `verb_alloc` + placement-new**

Find:
```cpp
extern "C" VerbValue map_new() {
    return verb_map(new VerbMapImpl());
}
```
change to:
```cpp
extern "C" VerbValue map_new() {
    void* mem = verb_alloc(sizeof(VerbMapImpl));
    new (mem) VerbMapImpl();
    return verb_map(mem);
}
```
(`verb_alloc` returns a header-carrying block sized `sizeof(VerbMapImpl)`;
placement-`new` constructs the `unordered_map` in that memory. This
needs `#include <new>` for placement-new — add that include alongside
`<cstring>`/`<unordered_map>` at the top of the file.)

- [ ] **Step 3: Retain the aliased/duplicated return values in `map_set` and `map_get`**

Find:
```cpp
extern "C" VerbValue map_set(VerbValue m, VerbValue k, VerbValue v) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_nil();
    (*impl)[k] = v;
    return m;
}

extern "C" VerbValue map_get(VerbValue m, VerbValue k) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_nil();
    auto it = impl->find(k);
    if (it == impl->end()) return verb_nil();
    return it->second;
}
```
change to:
```cpp
extern "C" VerbValue map_set(VerbValue m, VerbValue k, VerbValue v) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_nil();
    (*impl)[k] = v;
    // `m` is about to be released once (by the generic std-io/std-map
    // argument-release convention) and returned once -- two homes for
    // one incoming reference now need one retain to cover the second.
    verb_retain_value(m);
    return m;
}

extern "C" VerbValue map_get(VerbValue m, VerbValue k) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_nil();
    auto it = impl->find(k);
    if (it == impl->end()) return verb_nil();
    // The map keeps its own stored copy; retain before handing back an
    // independent one, mirroring array `get`.
    verb_retain_value(it->second);
    return it->second;
}
```

- [ ] **Step 4: Define `verb_map_destroy_contents`**

Add at the end of the file (after `map_len`):
```cpp
// Called by the LLVM-defined verb_release_value (src/codegen.rs) when a
// map's refcount hits zero, before the map's header is freed. Cascades
// into every stored key/value (releasing any heap-owned string/closure/
// array/map they hold), then explicitly runs the destructor -- required
// because map_new used placement-new, not `new`, so `delete` here would
// be undefined behavior (it would call operator delete on memory that
// wasn't allocated by operator new). The header's actual `free()` happens
// back in verb_release_value, once, the same place every heap kind's
// header gets freed.
extern "C" void verb_map_destroy_contents(void* payload) {
    auto* impl = static_cast<VerbMapImpl*>(payload);
    for (auto& [k, v] : *impl) {
        verb_release_value(k);
        verb_release_value(v);
    }
    impl->~VerbMapImpl();
}
```

- [ ] **Step 5: Add a fixture with heap-valued map entries**

Create `tests/fixtures/gc_map_heap_values.verb`:
```
import std map;

assign m map_new();
map_set(m, "name", "compiler" join "!");
map_set(m, "list", list 1, 2, 3);
print(map_get(m, "name"));
print(len(map_get(m, "list")));
print(map_len(m));
```

Create `tests/fixtures/gc_map_heap_values.expected` — verify by running
the fixture first, don't guess:
```
compiler!
3
2
```

Register in `tests/e2e.rs`:
```rust
#[test]
fn map_with_heap_valued_entries_retains_and_releases_correctly() { run_ok("gc_map_heap_values"); }
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS, including the pre-existing `std_map_basic` fixture with
identical output.

- [ ] **Step 7: Commit**

```bash
git add runtime/verb_map.cpp tests/e2e.rs tests/fixtures/gc_map_heap_values.verb tests/fixtures/gc_map_heap_values.expected
git commit -m "feat(gc): route map allocation through verb_alloc; cascade-release map contents on destroy"
```

---

## Task 7: `std io` contract (runtime/verb_std_io.cpp only)

**Owner note:** This task can start the moment Task 2 is merged. It runs in parallel with Tasks 3, 4, 5, 6 — this task touches zero lines of `src/codegen.rs` and zero lines of `runtime/verb_map.cpp`.

**Files:**
- Modify: `runtime/verb_std_io.cpp`
- Test: `tests/e2e.rs`

**Interfaces:**
- Consumes: `verb_alloc` (Task 1, declared in `runtime/verb.h`).

Any C++ code that hands a heap string back to Verb must allocate it
through `verb_alloc`, not raw `malloc`/`strdup` — a string pointer
without a valid header at `payload - 8` will corrupt memory the first
time `verb_retain_value`/`verb_release_value` touches it.

- [ ] **Step 1: Switch `verb_string_from` and `file_read` to `verb_alloc`**

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

Add the extern declaration near the top of the file, alongside the
existing `#include "verb.h"`:
```cpp
#include "verb.h"

// Defined by Verb's own generated LLVM module (src/codegen.rs).
extern "C" void* verb_alloc(int64_t n);
```

- [ ] **Step 2: Add a leak-check fixture for the file roundtrip**

This mirrors the existing `std_io_file_roundtrip` fixture but under its
own temp-file path, so it doesn't collide with any pre-existing
content-checking test over the same file (a real, previously-hit issue
in this project — two tests writing to the same hardcoded path under
`cargo test`'s default parallelism is a flaky-test risk). Check
`tests/fixtures/std_io_file_roundtrip.verb` for its exact path/content
first, then create:

`tests/fixtures/gc_std_io_file_roundtrip.verb` (same shape as the
existing fixture, but writing to a distinct filename such as
`verb_e2e_gc_v2_roundtrip.tmp`), with a matching
`gc_std_io_file_roundtrip.expected`.

Register in `tests/e2e.rs`, following the existing `assert_no_leaks`/
`run_ok` pattern already used for this fixture family elsewhere in the
file:
```rust
#[test]
fn std_io_file_roundtrip_allocates_through_verb_alloc() { run_ok("gc_std_io_file_roundtrip"); }
```

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: PASS, including both the pre-existing `std_io_file_roundtrip`
and `std_io_tcp_loopback` fixtures with identical output.

- [ ] **Step 4: Commit**

```bash
git add runtime/verb_std_io.cpp tests/e2e.rs tests/fixtures/gc_std_io_file_roundtrip.verb tests/fixtures/gc_std_io_file_roundtrip.expected
git commit -m "feat(gc): route std-io heap strings through verb_alloc"
```

---

## Task 8: Cycle-limitation proof + full leak verification

**Must not start until Tasks 3, 4, 5, 6, and 7 are all merged.** This is
the integration point where every heap kind's wiring gets verified
together.

**Files:**
- Test: `tests/e2e.rs`, new fixtures under `tests/fixtures/`

**Interfaces:**
- Consumes: `verb_gc_live`, `VERB_GC_DEBUG` diagnostic (Tasks 1, 2, 4). `assert_no_leaks` test helper (new in this task, following the same `build` + `VERB_GC_DEBUG=1` + parse-stdout pattern used throughout this plan's manual verification steps).

- [ ] **Step 1: Add the `assert_no_leaks` helper**

In `tests/e2e.rs`:
```rust
fn assert_no_leaks(fixture: &str) {
    let out_path = std::env::temp_dir().join(format!("verb_test_gc_v2_{fixture}"));
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
```

- [ ] **Step 2: Add a stress fixture exercising all four heap kinds together**

Create `tests/fixtures/gc_stress_all_kinds.verb`:
```
import std map;

assign total 0;
loop assign i 0; i trails 500; i be i add 1 begin
  declare s;
  s be "iter" join "-done";
  assign arr list s, i;
  assign m map_new();
  map_set(m, "k", get(arr, 0));
  check map_get(m, "k") equals "iter-done" begin
    total be total add 1;
  end
end
print(total);
```

Create `tests/fixtures/gc_stress_all_kinds.expected` — verify by running
the fixture first:
```
500
```

- [ ] **Step 3: Add the confined-cycle-leak proof fixture**

Create `tests/fixtures/gc_cyclic_array_leaks_confined.verb`:
```
assign a list 1, 2;
push(a, a);
print(len(a));
```

Create `tests/fixtures/gc_cyclic_array_leaks_confined.expected` — verify
by running the fixture first:
```
3
```

- [ ] **Step 4: Register the full leak-verification test list**

In `tests/e2e.rs`:
```rust
#[test]
fn gc_no_leaks_across_all_heap_kinds() {
    for fixture in [
        "strings", "closures", "arrays_literal", "arrays_get_set", "arrays_push_pop",
        "arrays_of_arrays", "arrays_of_closures", "std_map_basic",
        "gc_reassign_and_or", "gc_global_reassign", "gc_early_return_nested",
        "gc_arrays_nested", "gc_arrays_of_closures", "gc_arrays_regrow",
        "gc_map_heap_values", "gc_std_io_file_roundtrip",
    ] {
        assert_no_leaks(fixture);
    }
}

#[test]
fn gc_stress_all_kinds_leaks_nothing() { assert_no_leaks("gc_stress_all_kinds"); }

#[test]
fn gc_cyclic_array_leak_is_confined_not_corrupting() {
    // A self-referential array cannot be reclaimed by pure refcounting --
    // this is a known, accepted limitation (see the design spec's "cycle
    // limitation" section), resolved by a separate follow-up sub-project
    // (a backup cycle collector), not this one. This test's job is only
    // to prove the failure mode is a small, fixed, bounded leak -- the
    // cyclic array's own one block -- not unbounded growth, corruption,
    // or a crash.
    let out_path = std::env::temp_dir().join("verb_test_gc_v2_cyclic");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["build", "tests/fixtures/gc_cyclic_array_leaks_confined.verb", "-o", out_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).env("VERB_GC_DEBUG", "1").output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("3\n"), "unexpected program output:\n{stdout}");
    let live_line = stdout.lines().find(|l| l.starts_with("verb_gc_live="))
        .unwrap_or_else(|| panic!("no verb_gc_live line in stdout:\n{stdout}"));
    // Exactly the cyclic array's own header block leaks (its refcount
    // never reaches zero because it holds a reference to itself) -- a
    // small, fixed, non-zero number, not zero and not unbounded.
    assert_ne!(live_line, "verb_gc_live=0", "expected a confined leak, got none:\n{stdout}");
    let live_n: i64 = live_line.strip_prefix("verb_gc_live=").unwrap().parse().unwrap();
    assert!((1..=2).contains(&live_n), "expected a small, bounded leak count, got {live_n}:\n{stdout}");
    let _ = std::fs::remove_file(&out_path);
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test gc_no_leaks_across_all_heap_kinds gc_stress_all_kinds_leaks_nothing gc_cyclic_array_leak_is_confined_not_corrupting`
Expected: PASS, every fixture in the list reporting `verb_gc_live=0`
except the cyclic one, which reports a small nonzero count.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add tests/e2e.rs tests/fixtures/gc_stress_all_kinds.verb tests/fixtures/gc_stress_all_kinds.expected tests/fixtures/gc_cyclic_array_leaks_confined.verb tests/fixtures/gc_cyclic_array_leaks_confined.expected
git commit -m "test(gc): verify zero leaks across all heap kinds; prove cyclic leaks stay confined"
```

---

## Done

At this point:
- Every heap-owned Verb value (string, closure, array, map, cell) is
  refcounted and freed automatically, including top-level globals.
- `push`'s pre-existing grow-path leak is fixed.
- `nil`/`bool`/`int`/`float` never touch a retain/release call.
- Self-referential/cyclic arrays and maps leak in a confined, tested,
  documented way — no cycle collector exists yet; that's the next
  sub-project.
- `VERB_GC_DEBUG=1` on any built binary prints `verb_gc_live=<n>` at
  exit — `0` for every acyclic fixture in the suite, across strings,
  closures, arrays (including nested, of-closures, and push-regrown),
  maps (including heap-valued entries), globals, and early return from
  nested control flow.
