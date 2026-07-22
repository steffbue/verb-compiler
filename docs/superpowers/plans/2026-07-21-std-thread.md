# `import std thread` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `import std thread;` giving Verb programs OS threads (`thread_spawn`/`thread_join`), a mutex, and a blocking channel, following the exact `import std io`/`import std map` pattern already in the codebase.

**Architecture:** A new `runtime/verb_std_thread.cpp` (C++ externs, linked in only when requested) plus a `THREAD_FUNCS` arity table in `src/codegen.rs` dispatched through the existing generic `gen_std_io_call` path — for every function except `thread_spawn`, which needs one bespoke codegen branch to hand a closure's raw `fn_ptr`/`env` (not a `VerbValue`, per `verb.h`'s documented boundary rule) to a C++ trampoline that starts a `std::thread`.

**Tech Stack:** Rust (`inkwell`/LLVM codegen), C++17 (`std::thread`/`std::mutex`/`std::condition_variable`), the existing `verb`/`cc`/`zig` build toolchain.

## Global Constraints

- Base branch: `refcounting-gc-v2` (this worktree is already rebased onto it — do not rebase onto `main` or `refcounting-gc`).
- No heap-tagged `VerbValue` (`STRING`/`ARRAY`/`MAP`/`CLOSURE`) may cross a thread boundary — only `NIL`/`BOOL`/`INT`/`FLOAT`. This is enforced by `thread_spawn` requiring a 0-arity closure and by `channel_send` runtime-rejecting non-primitive payloads. See `docs/superpowers/specs/2026-07-21-std-thread-design.md`.
- No changes to the GC (`verb_alloc`, retain/release, or anything under `docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md`). That work is in flight on this same branch from other tasks; this plan must not touch `build_alloc_fn`, `declare_gc_globals`, `inc_live_counter`, or any retain/release logic once it lands.
- Function names are prefixed: `thread_spawn`, `thread_join`, `thread_sleep_ms`, `mutex_new`, `mutex_lock`, `mutex_unlock`, `channel_new`, `channel_send`, `channel_recv`. Do not use bare `spawn`/`lock`/`send`/etc.
- `import std thread` is rejected when cross-compiling to Windows (`--target windows-*`), same restriction `import std io` already has, same error-message shape.
- **This branch currently has no GC leak-check test infrastructure** (`verb_gc_live`/`VERB_GC_DEBUG`/`assert_no_leaks` do not exist here yet — they're mid-flight on this same branch via other work). Do not add a leak-check test in this plan; note it as follow-up once that infra lands.

---

## File Structure

- **Create** `runtime/verb_std_thread.cpp` — all nine functions' C++ implementations. One file, mirrors `runtime/verb_std_io.cpp`'s shape (a handful of small `extern "C"` functions, no internal module boundaries needed at this size).
- **Modify** `runtime/verb.h` — no changes. (`thread_spawn_raw`'s raw-pointer signature is declared directly in `src/codegen.rs`'s codegen, not in this header, since it's an internal codegen-to-C++ contract, not something Verb-level C++ import code needs — see Task 4.)
- **Modify** `src/parser.rs` — extend the std-module allow-list (line 155) to accept `"thread"`.
- **Modify** `src/codegen.rs` — add `THREAD_FUNCS` table + `thread_func_arity` (mirroring `IO_FUNCS`/`io_func_arity` at lines 1661–1676), one new dispatch arm in `gen_call` (mirroring lines 1523–1531), and a new `gen_thread_spawn` method (sibling to `gen_std_io_call`) for the one bespoke case.
- **Modify** `src/main.rs` — add `compile_std_thread_obj` (mirroring `compile_std_io_obj` at lines 210–223), wire `wants_std_thread` through `build_aot_host` and `build_aot_cross` the same way `wants_std_io` already is, add the Windows-cross-compile rejection, add `-pthread` on Linux link/compile.
- **Create** `tests/fixtures/std_thread_spawn_join.verb` + `.expected`, `tests/fixtures/std_thread_mutex.verb` + `.expected`, `tests/fixtures/std_thread_channel.verb` + `.expected`, `tests/fixtures/std_thread_channel_rejects_non_primitive.verb` + `.expected`, `tests/fixtures/std_thread_sleep.verb` + `.expected`.
- **Modify** `tests/e2e.rs` — new `// ----- std thread -----` section mirroring the `// ----- std io -----` section at line 593.

## Task Dependency Order

Tasks 1 and 2 have no dependency on each other and can run in parallel. Task 3 depends on Task 1 (parser) and Task 2 (runtime `.cpp` must exist for the "compiles standalone" test, though codegen itself only needs the *names*, not the file). Task 4 depends on Task 3 (shares the `THREAD_FUNCS`-adjacent dispatch code in `gen_call`). Task 5 depends on Task 2 (needs `verb_std_thread.cpp` to exist to compile/link). Task 6 depends on all of Tasks 1–5 (it's the end-to-end proof).

```
Task 1 (parser) ---\
                     >--- Task 3 (codegen: generic THREAD_FUNCS dispatch) --- Task 4 (codegen: thread_spawn) ---\
Task 2 (runtime .cpp) --- Task 5 (main.rs linking) ----------------------------------------------------------- Task 6 (e2e tests)
```

---

### Task 1: Parser accepts `import std thread;`

**Files:**
- Modify: `src/parser.rs:155`
- Test: `src/parser.rs` (inline `#[cfg(test)] mod tests`, appended after line 722's `recovering_collects_std_imports_too`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Program.std_imports` now may contain `"thread"`. Later tasks' codegen checks `self.std_imports.iter().any(|m| m == "thread")`.

- [ ] **Step 1: Write the failing tests**

Add to `src/parser.rs`'s `mod tests` (after the existing `recovering_collects_std_imports_too` test, following the exact style of `parses_std_io_import`/`parses_std_map_import`/`unknown_std_module_is_a_compile_error`):

```rust
    #[test]
    fn parses_std_thread_import() {
        let p = parse(lex("import std thread;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["thread".to_string()]);
        assert!(p.imports.is_empty());
    }

    #[test]
    fn dedups_repeated_std_thread_import() {
        let p = parse(lex("import std thread; import std thread;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["thread".to_string()]);
    }

    #[test]
    fn std_io_map_and_thread_imports_coexist() {
        let p = parse(lex("import std io; import std map; import std thread; print(1);").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["io".to_string(), "map".to_string(), "thread".to_string()]);
        assert_eq!(p.body.len(), 1);
    }
```

Also update the existing `unknown_std_module_is_a_compile_error` test (it currently only asserts `"io"` and `"map"` appear in the message) to also assert `"thread"`:

```rust
    #[test]
    fn unknown_std_module_is_a_compile_error() {
        let err = parse(lex("import std vector;").unwrap()).unwrap_err();
        assert!(err.msg.contains("unknown std module 'vector'"), "{}", err.msg);
        assert!(err.msg.contains("io"), "{}", err.msg);
        assert!(err.msg.contains("map"), "{}", err.msg);
        assert!(err.msg.contains("thread"), "{}", err.msg);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib parser:: 2>&1 | tail -40`
Expected: `parses_std_thread_import`, `dedups_repeated_std_thread_import`, `std_io_map_and_thread_imports_coexist` FAIL (unknown std module 'thread'), and `unknown_std_module_is_a_compile_error` FAILs its new `contains("thread")` assertion.

- [ ] **Step 3: Implement**

In `src/parser.rs`, change line 155 and the error message on the following lines:

```rust
        if name != "io" && name != "map" && name != "thread" {
            return Err(CompileError::new(
                format!("unknown std module '{name}' (known std modules: io, map, thread)"),
                l, c,
            ));
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib parser:: 2>&1 | tail -40`
Expected: all `parser::tests::*` PASS.

- [ ] **Step 5: Commit**

```bash
git add src/parser.rs
git commit -m "feat(parser): accept 'import std thread;'"
```

---

### Task 2: `runtime/verb_std_thread.cpp` — mutex, channel, sleep, join, spawn trampoline

**Files:**
- Create: `runtime/verb_std_thread.cpp`
- Test: `tests/e2e.rs` (standalone-compile test, appended after `verb_map_cpp_compiles_standalone`)

**Interfaces:**
- Consumes: `runtime/verb.h`'s `VerbValue`/`verb_nil`/`verb_bool`/`verb_int`/`verb_as_int`/`VERB_NIL`/`VERB_BOOL`/`VERB_INT`/`VERB_FLOAT` (all already exist, unchanged).
- Produces: nine `extern "C"` symbols later tasks reference by exact name: `mutex_new`, `mutex_lock`, `mutex_unlock`, `channel_new`, `channel_send`, `channel_recv`, `thread_join`, `thread_sleep_ms` (all `VerbValue`-in/`VerbValue`-out — Task 3 wires these through the generic path) and `thread_spawn_raw` (raw-pointer signature `void* thread_spawn_raw(void* fn_ptr, void* env)` — Task 4 calls this one directly, not through the generic path).

- [ ] **Step 1: Write the file**

```cpp
// Built-in bindings for `import std thread;` -- OS threads, a mutex, and
// a blocking channel. Compiled and linked in automatically by
// `verb build`/`compile` whenever a program uses `import std thread`;
// see docs/superpowers/specs/2026-07-21-std-thread-design.md.
//
// Every handle here (thread/mutex/channel) is a bare `new`'d C++ object
// referenced by a VERB_INT-tagged VerbValue carrying its address as an
// int64 payload -- the same "reuse VERB_INT as an opaque handle" pattern
// runtime/verb_std_io.cpp already uses for POSIX fds. None of these
// handles are refcounted by Verb's GC (they aren't
// STRING/ARRAY/MAP/CLOSURE), so misuse (double join, unlock without
// lock, a bogus handle) is undefined behavior at the C++ level -- the
// same trust level POSIX fd misuse already gets in verb_std_io.cpp.
//
// Only NIL/BOOL/INT/FLOAT VerbValues may cross a thread boundary:
// thread_spawn's closure is always 0-arity (checked by src/codegen.rs
// before thread_spawn_raw is ever called) so it receives no args, and
// channel_send runtime-rejects anything else below. Verb's refcounting
// GC is not thread-safe, so no heap-tagged value is ever allowed to be
// touched by two threads.
#include "verb.h"

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <deque>
#include <mutex>
#include <thread>

namespace {

struct ThreadHandle {
    std::thread t;
};

struct Channel {
    std::mutex m;
    std::condition_variable cv;
    std::deque<VerbValue> q;
};

bool is_primitive(VerbValue v) {
    return v.tag == VERB_NIL || v.tag == VERB_BOOL || v.tag == VERB_INT || v.tag == VERB_FLOAT;
}

// Stores/reads an arbitrary heap pointer in a VERB_INT payload, the same
// memcpy round-trip runtime/verb.h's own verb_map()/verb_as_map() use for
// VERB_MAP -- avoids relying on reinterpret_cast<int64_t> pointer-to-int
// conversion, which verb.h's existing helpers deliberately don't do either.
VerbValue verb_handle(void* p) {
    VerbValue v;
    v.tag = VERB_INT;
    std::memcpy(&v.payload, &p, sizeof(p));
    return v;
}

void* as_handle(VerbValue v) {
    void* p;
    std::memcpy(&p, &v.payload, sizeof(p));
    return p;
}

} // namespace

// The exact signature src/codegen.rs's closure struct's fn_ptr field
// points at: VerbValue(*)(void* env, void* argv). Called here with
// argv=nullptr, valid only because src/codegen.rs's gen_thread_spawn
// checks the closure's arity is 0 before ever calling thread_spawn_raw
// -- a 0-param function body never indexes argv.
using ClosureFn = VerbValue (*)(void*, void*);

extern "C" void* thread_spawn_raw(void* fn_ptr, void* env) {
    auto* h = new ThreadHandle{
        std::thread([fn_ptr, env]() { reinterpret_cast<ClosureFn>(fn_ptr)(env, nullptr); })
    };
    return h;
}

extern "C" VerbValue thread_join(VerbValue handle) {
    auto* h = static_cast<ThreadHandle*>(as_handle(handle));
    h->t.join();
    delete h;
    return verb_nil();
}

extern "C" VerbValue thread_sleep_ms(VerbValue ms) {
    std::this_thread::sleep_for(std::chrono::milliseconds(verb_as_int(ms)));
    return verb_nil();
}

extern "C" VerbValue mutex_new() {
    return verb_handle(new std::mutex());
}

extern "C" VerbValue mutex_lock(VerbValue handle) {
    static_cast<std::mutex*>(as_handle(handle))->lock();
    return verb_nil();
}

extern "C" VerbValue mutex_unlock(VerbValue handle) {
    static_cast<std::mutex*>(as_handle(handle))->unlock();
    return verb_nil();
}

extern "C" VerbValue channel_new() {
    return verb_handle(new Channel());
}

extern "C" VerbValue channel_send(VerbValue handle, VerbValue v) {
    if (!is_primitive(v)) return verb_bool(0);
    auto* c = static_cast<Channel*>(as_handle(handle));
    {
        std::lock_guard<std::mutex> lock(c->m);
        c->q.push_back(v);
    }
    c->cv.notify_one();
    return verb_bool(1);
}

extern "C" VerbValue channel_recv(VerbValue handle) {
    auto* c = static_cast<Channel*>(as_handle(handle));
    std::unique_lock<std::mutex> lock(c->m);
    c->cv.wait(lock, [c]() { return !c->q.empty(); });
    VerbValue v = c->q.front();
    c->q.pop_front();
    return v;
}
```

- [ ] **Step 2: Write the failing standalone-compile test**

In `tests/e2e.rs`, add after `verb_map_cpp_compiles_standalone` (the test immediately following `verb_std_io_cpp_compiles_standalone` at line 379 — find it by name, not line number, since Task 1/2 run in parallel and line numbers may have shifted):

```rust
#[test]
fn verb_std_thread_cpp_compiles_standalone() {
    let obj = std::env::temp_dir().join("verb_std_thread_syntax_check.o");
    let status = Command::new("c++")
        .args([
            "-std=c++17", "-Iruntime", "-pthread", "-c",
            "runtime/verb_std_thread.cpp",
            "-o", obj.to_str().unwrap(),
        ])
        .status()
        .expect("failed to invoke c++ to compile runtime/verb_std_thread.cpp");
    assert!(status.success(), "runtime/verb_std_thread.cpp failed to compile");
    let _ = std::fs::remove_file(&obj);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --test e2e verb_std_thread_cpp_compiles_standalone`
Expected: FAIL — `runtime/verb_std_thread.cpp` doesn't exist yet (if Step 1 hasn't been done yet in your working copy) or compiler errors (if there's a typo). Confirm it fails for the *expected* reason before moving on.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test e2e verb_std_thread_cpp_compiles_standalone`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add runtime/verb_std_thread.cpp tests/e2e.rs
git commit -m "feat(runtime): add verb_std_thread.cpp (mutex, channel, join, sleep, spawn trampoline)"
```

---

### Task 3: Codegen — generic `THREAD_FUNCS` dispatch (mutex/channel/sleep/join)

**Files:**
- Modify: `src/codegen.rs` (near `IO_FUNCS`/`MAP_FUNCS` at lines 1661–1691, and the dispatch arms in `gen_call` at lines 1522–1532)
- Test: `src/codegen.rs`'s `mod tests` (after `std_map_call_with_correct_arity_compiles_ok`, following the exact style of `std_io_call_with_correct_arity_compiles_ok`/`std_io_arity_mismatch_is_a_compile_error`/`std_io_name_ignored_without_import_std_io`)

**Interfaces:**
- Consumes: `runtime/verb_std_thread.cpp`'s eight generic-shape functions from Task 2 (by name only — codegen doesn't touch the `.cpp` file, it just needs to know their names/arities up front; the actual `.cpp` isn't compiled during `cargo test --lib`, only during the e2e build/link path in Task 6).
- Produces: `THREAD_FUNCS: &[(&str, usize)]` and `fn thread_func_arity(name: &str) -> Option<usize>`, consumed by Task 4's `gen_thread_spawn` is NOT needed (spawn has its own dedicated dispatch, added in Task 4) — but Task 4 does add itself as a *tenth* name that must be checked *before* falling into `thread_func_arity`, in the same `gen_call` block this task adds.

- [ ] **Step 1: Write the failing tests**

Add to `src/codegen.rs`'s `mod tests`, after `std_map_call_with_correct_arity_compiles_ok`:

```rust
    #[test]
    fn std_thread_mutex_call_with_correct_arity_compiles_ok() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::Assign {
            name: "m".to_string(),
            value: Expr::Call {
                callee: Box::new(Expr::Var("mutex_new".to_string(), 1, 1)),
                args: vec![],
                line: 1, col: 1,
            },
        }];
        let stmt_files = vec!["a.verb".to_string()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["thread".to_string()]).is_ok());
    }

    #[test]
    fn std_thread_arity_mismatch_is_a_compile_error() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("mutex_lock".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg
            .compile_program(&stmts, &stmt_files, &[], &["thread".to_string()])
            .unwrap_err();
        assert!(err.msg.contains("mutex_lock"), "{}", err.msg);
        assert!(err.msg.contains("takes 1 argument"), "{}", err.msg);
    }

    #[test]
    fn std_thread_name_ignored_without_import_std_thread() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("mutex_new".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
        assert!(err.msg.contains("undefined variable"), "{}", err.msg);
    }

    #[test]
    fn all_std_thread_generic_funcs_compile_ok() {
        // channel_send/channel_recv/mutex_lock/mutex_unlock/thread_join/
        // thread_sleep_ms all take a plausible number of int args; this
        // just proves each name+arity in THREAD_FUNCS (other than
        // thread_spawn, covered separately in Task 4's tests) type-checks
        // through the generic gen_std_io_call path.
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let call1 = |name: &str, argc: usize| Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var(name.to_string(), 1, 1)),
            args: (0..argc).map(|_| Expr::Int(1)).collect(),
            line: 1, col: 1,
        });
        let stmts = vec![
            call1("mutex_lock", 1),
            call1("mutex_unlock", 1),
            call1("channel_send", 2),
            call1("channel_recv", 1),
            call1("thread_join", 1),
            call1("thread_sleep_ms", 1),
        ];
        let stmt_files = vec!["a.verb".to_string(); stmts.len()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["thread".to_string()]).is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib codegen:: 2>&1 | tail -60`
Expected: all four new tests FAIL — `mutex_new`/`mutex_lock`/etc. resolve as undefined variables (the `"thread"` module isn't recognized in `gen_call` yet).

- [ ] **Step 3: Implement**

In `src/codegen.rs`, add the table right after `MAP_FUNCS`/`map_func_arity` (after line 1691's closing brace):

```rust
/// Fixed name -> arity table for the `thread` module's built-in
/// functions that fit the generic `gen_std_io_call` VerbValue-in/out
/// shape (see runtime/verb_std_thread.cpp and the design spec).
/// `thread_spawn` is deliberately absent -- its closure argument can't
/// cross the C++ boundary as a VerbValue, so it gets its own dispatch
/// arm and codegen (`gen_thread_spawn`), added in the next task. See
/// `IO_FUNCS`.
const THREAD_FUNCS: &[(&str, usize)] = &[
    ("thread_join", 1),
    ("thread_sleep_ms", 1),
    ("mutex_new", 0),
    ("mutex_lock", 1),
    ("mutex_unlock", 1),
    ("channel_new", 0),
    ("channel_send", 2),
    ("channel_recv", 1),
];

fn thread_func_arity(name: &str) -> Option<usize> {
    THREAD_FUNCS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}
```

Then in `gen_call`, add a new arm immediately after the `map` arm (after line 1531's closing `}`, before the `!self.imports.is_empty()` fallback at line 1533):

```rust
            if !is_bound && self.std_imports.iter().any(|m| m == "thread") {
                if let Some(arity) = thread_func_arity(name) {
                    return self.gen_std_io_call(name, arity, args, line, col);
                }
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib codegen:: 2>&1 | tail -60`
Expected: all four new tests PASS, and no previously-passing test regresses.

- [ ] **Step 5: Commit**

```bash
git add src/codegen.rs
git commit -m "feat(codegen): dispatch std thread's generic-shape functions (mutex/channel/sleep/join)"
```

---

### Task 4: Codegen — `thread_spawn` (closure ABI extraction + trampoline call)

**Files:**
- Modify: `src/codegen.rs` (new dispatch arm in `gen_call`, new `gen_thread_spawn` method next to `gen_std_io_call`)
- Test: `src/codegen.rs`'s `mod tests`

**Interfaces:**
- Consumes: `self.closure_ty` (struct `{fn_ptr, i64 arity, env_ptr}`, GEP indices 0/1/2 — unchanged, verified at `src/codegen.rs:1052-1061`'s `make_closure`), `self.call_named("verb_check_call", ...)` (already declared eagerly, used identically to `gen_call`'s own fallback tail at lines 1537-1542), `self.make_val(tag: u64, payload: IntValue) -> StructValue` (line ~85), `TAG_INT` (from `src/value.rs`, already `use`d in `codegen.rs`), `self.externs: HashMap<String, FunctionValue>` (already used by `gen_std_io_call`/`gen_extern_call` for lazy-declared C++ externs).
- Produces: `thread_spawn(closure)` callable from Verb once `import std thread;` is present; returns a `VERB_INT`-tagged handle `thread_join` (Task 3) consumes.

- [ ] **Step 1: Write the failing tests**

Add to `src/codegen.rs`'s `mod tests`, after Task 3's `all_std_thread_generic_funcs_compile_ok`:

```rust
    #[test]
    fn std_thread_spawn_with_0_arity_closure_compiles_ok() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![
            Stmt::Fn {
                name: "work".to_string(),
                params: vec![],
                body: vec![],
                line: 1, col: 1,
            },
            Stmt::ExprStmt(Expr::Call {
                callee: Box::new(Expr::Var("thread_spawn".to_string(), 2, 1)),
                args: vec![Expr::Var("work".to_string(), 2, 13)],
                line: 2, col: 1,
            }),
        ];
        let stmt_files = vec!["a.verb".to_string(); stmts.len()];
        assert!(cg.compile_program(&stmts, &stmt_files, &[], &["thread".to_string()]).is_ok());
    }

    #[test]
    fn std_thread_spawn_arity_mismatch_is_a_compile_error() {
        // thread_spawn itself always takes exactly 1 argument (the
        // closure) -- passing 0 or 2+ args is the same "wrong argument
        // count" error every other std fn gives, independent of the
        // closure's own arity (checked separately, at the
        // verb_check_call/runtime-abort level, not here).
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("thread_spawn".to_string(), 1, 1)),
            args: vec![],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg
            .compile_program(&stmts, &stmt_files, &[], &["thread".to_string()])
            .unwrap_err();
        assert!(err.msg.contains("thread_spawn"), "{}", err.msg);
        assert!(err.msg.contains("takes 1 argument"), "{}", err.msg);
    }

    #[test]
    fn std_thread_spawn_name_ignored_without_import_std_thread() {
        let ctx = Context::create();
        let mut cg = Codegen::new(&ctx);
        let stmts = vec![Stmt::ExprStmt(Expr::Call {
            callee: Box::new(Expr::Var("thread_spawn".to_string(), 1, 1)),
            args: vec![Expr::Int(1)],
            line: 1, col: 1,
        })];
        let stmt_files = vec!["a.verb".to_string()];
        let err = cg.compile_program(&stmts, &stmt_files, &[], &[]).unwrap_err();
        assert!(err.msg.contains("undefined variable"), "{}", err.msg);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib codegen:: 2>&1 | tail -60`
Expected: `std_thread_spawn_with_0_arity_closure_compiles_ok` and `std_thread_spawn_arity_mismatch_is_a_compile_error` FAIL (`thread_spawn` resolves as an undefined variable, since it isn't in `THREAD_FUNCS` and has no dispatch arm yet). `std_thread_spawn_name_ignored_without_import_std_thread` should already PASS (it exercises the pre-existing undefined-variable fallback) — confirm it does, as a sanity check that the fixture itself is correct before Step 3 changes anything.

- [ ] **Step 3: Implement**

In `src/codegen.rs`, add a new arm in `gen_call` immediately *before* the `THREAD_FUNCS` arm added in Task 3 (so `thread_spawn` — which is NOT in `THREAD_FUNCS` — is checked first; order relative to the `io`/`map` arms doesn't matter since the name doesn't collide):

```rust
            if !is_bound && name == "thread_spawn" && self.std_imports.iter().any(|m| m == "thread") {
                return self.gen_thread_spawn(args, line, col);
            }
```

Then add `gen_thread_spawn` as a new method, placed right after `gen_std_io_call` (after its closing brace, before the `gen_extern_call` doc comment):

```rust
    /// `thread_spawn(closure)` -- the one `std thread` function that can't
    /// go through `gen_std_io_call`'s generic VerbValue-in/out path,
    /// because a closure's VerbValue can't cross the C++ boundary
    /// (verb.h: "Tag 5 (closure) never crosses this boundary"). Instead:
    /// arity-check the closure via the same `verb_check_call` runtime
    /// helper `gen_call`'s own closure-invocation fallback tail uses
    /// (line ~1540), pull `fn_ptr`/`env` straight out of the closure
    /// struct (same GEP indices `make_closure` writes), and hand those
    /// two raw pointers to `thread_spawn_raw` (runtime/verb_std_thread.cpp)
    /// -- which sidesteps the boundary rule entirely since it never
    /// receives a VerbValue closure, only plain pointers.
    fn gen_thread_spawn(&mut self, args: &[Expr], line: u32, col: u32)
        -> Result<StructValue<'ctx>, CompileError>
    {
        if args.len() != 1 {
            return Err(CompileError::new(
                format!("std thread fn 'thread_spawn' takes 1 argument(s), got {}", args.len()),
                line, col,
            ));
        }
        let cv = self.gen_expr(&args[0])?;
        let argc = self.ctx.i64_type().const_zero();
        let (lc, cc) = self.loc_consts(line, col);
        let clos_ptr = self.call_named(
            "verb_check_call", &[cv.into(), argc.into(), lc.into(), cc.into()])
            .unwrap().into_pointer_value();

        let fpp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 0, "fpp").unwrap();
        let fp = self.builder.build_load(self.ptr_ty, fpp, "fp").unwrap();
        let epp = self.builder.build_struct_gep(self.closure_ty, clos_ptr, 2, "epp").unwrap();
        let env = self.builder.build_load(self.ptr_ty, epp, "env").unwrap();

        let fnv = match self.externs.get("thread_spawn_raw").copied() {
            Some(fnv) => fnv,
            None => {
                let fnty = self.ptr_ty.fn_type(&[self.ptr_ty.into(), self.ptr_ty.into()], false);
                let fnv = self.module.add_function("thread_spawn_raw", fnty, None);
                self.externs.insert("thread_spawn_raw".to_string(), fnv);
                fnv
            }
        };
        let handle_ptr = self.builder.build_call(fnv, &[fp.into(), env.into()], "spawned")
            .unwrap().try_as_basic_value().basic().unwrap().into_pointer_value();
        let handle_int = self.builder.build_ptr_to_int(handle_ptr, self.ctx.i64_type(), "handlei").unwrap();
        Ok(self.make_val(TAG_INT, handle_int))
    }

```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib codegen:: 2>&1 | tail -60`
Expected: all three tests in this task PASS, plus every test from Tasks 1–3 still PASSES (run `cargo test --lib` with no filter to confirm no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/codegen.rs
git commit -m "feat(codegen): add thread_spawn (closure fn_ptr/env extraction + trampoline call)"
```

---

### Task 5: `main.rs` linking — compile and link `verb_std_thread.cpp`

**Files:**
- Modify: `src/main.rs` (new `const STD_THREAD_CPP`, new `compile_std_thread_obj`, wiring in `build_aot_host` lines 242-316 and `build_aot_cross` lines 318-394)

**Interfaces:**
- Consumes: `runtime/verb_std_thread.cpp` (Task 2), `targets::Target::os`/`targets::Os::Linux` (`src/targets.rs`, unchanged), `targets::Target::is_windows()` (unchanged).
- Produces: `verb build`/`verb compile` produce a working binary for programs using `import std thread;`, on macOS and Linux hosts and for non-Windows cross targets; a clear compile error for Windows cross targets.

- [ ] **Step 1: Write the failing tests**

Add to `tests/e2e.rs`, in a new section after the `// ----- std map -----` section's last test (find the end of that section — it's the last test before end-of-file, `aot_cross_build_produces_a_working_binary_for_std_map` or similar; append after it):

```rust
// ----- std thread -----

#[test]
fn run_rejects_programs_with_std_thread_import() {
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args(["run", "tests/fixtures/std_thread_spawn_join.verb"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support imports"), "stderr: {stderr}");
    assert!(stderr.contains("std thread"), "stderr: {stderr}");
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_spawn_join() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_spawn_join_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_spawn_join.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_spawn_join.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn cross_build_rejects_std_thread_import_for_windows_target() {
    if !zig_available() {
        eprintln!("skipping: zig not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join("verb_std_thread_windows_reject_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("std_thread_windows");
    let out = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_spawn_join.verb",
            "-o", bin.to_str().unwrap(),
            "--target", "windows-x86_64",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("import std thread"), "stderr: {stderr}");
    assert!(stderr.contains("Windows"), "stderr: {stderr}");
}
```

Note: `tests/fixtures/std_thread_spawn_join.verb`/`.expected` are created in Task 6, which depends on this task's linking working — but per the dependency graph these two tasks' *tests* reference each other's fixtures. Whoever implements this task first should create a minimal placeholder fixture (see Task 6 Step 1 for the exact content) so these tests can run; Task 6 will not need to recreate it if it already exists.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test e2e std_thread`
Expected: FAIL — `run_rejects_programs_with_std_thread_import` fails because parsing `import std thread;` isn't yet wired to produce that exact rejection path if Tasks 1-4 aren't merged yet in your working copy (should pass once they are, since that's a generic `std_imports`-driven message); `build_links_and_runs_a_program_using_std_thread_spawn_join` fails to link (`undefined symbol` for `thread_join`/`mutex_new`/etc., or a missing `-lpthread`).

- [ ] **Step 3: Implement**

In `src/main.rs`, add a new const next to `MAP_CPP` (line 204):

```rust
const STD_THREAD_CPP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/verb_std_thread.cpp");
```

Add a new function next to `compile_map_obj` (after line 240's closing brace):

```rust
/// Compiles the bundled `runtime/verb_std_thread.cpp` into an object
/// file. See `compile_std_io_obj`. `-pthread` is required by
/// `std::thread`/`std::mutex`/`std::condition_variable` on Linux
/// (glibc splits pthread symbols into a separate archive there); macOS's
/// libc++ links threading support unconditionally, so the flag is a
/// harmless no-op there, and is applied unconditionally rather than
/// gated on host OS to keep this function symmetric with its zig-cross
/// caller in `build_aot_cross`, which cannot check the *host*'s OS
/// (only the *target*'s).
fn compile_std_thread_obj(compiler: &str, extra_args: &[&str]) -> Result<PathBuf, String> {
    let obj = std::env::temp_dir().join(format!("verb_std_thread_{}.o", std::process::id()));
    let mut cmd = Command::new(compiler);
    cmd.args(extra_args);
    cmd.args(["-std=c++17", "-I", RUNTIME_DIR, "-pthread", "-c", STD_THREAD_CPP, "-o"]);
    cmd.arg(&obj);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run '{compiler}' to compile {STD_THREAD_CPP}: {e}"))?;
    if !status.success() {
        return Err(format!("failed to compile {STD_THREAD_CPP}"));
    }
    Ok(obj)
}
```

In `build_aot_host` (around line 242), extend the `wants_*`/linker/obj-compile/link/cleanup steps exactly parallel to `wants_map`/`map_obj`:

```rust
    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_map = std_imports.iter().any(|m| m == "map");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
    let linker = if imports.is_empty() && !wants_std_io && !wants_map && !wants_std_thread { "cc" } else { "c++" };

    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let map_obj = if wants_map {
        Some(compile_map_obj(linker, &[]).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: {e}");
            exit(1);
        }))
    } else {
        None
    };
    let std_thread_obj = if wants_std_thread {
        let extra_link_args: &[&str] = if cfg!(target_os = "linux") { &["-pthread"] } else { &[] };
        Some(compile_std_thread_obj(linker, extra_link_args).unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&obj);
            if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
            if let Some(p) = &map_obj { let _ = std::fs::remove_file(p); }
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
    if let Some(p) = &map_obj {
        cmd.arg(p);
    }
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && cfg!(target_os = "linux") {
        cmd.arg("-pthread");
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
            if let Some(p) = &map_obj { let _ = std::fs::remove_file(p); }
            if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
            eprintln!("error: failed to run linker '{linker}': {e}");
            exit(1);
        }
    };
    let _ = std::fs::remove_file(&obj);
    if let Some(p) = &std_io_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &map_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        eprintln!("link failed");
        exit(1);
    }
```

(This replaces the corresponding existing block in `build_aot_host` — every line that previously handled only `std_io_obj`/`map_obj` gets a parallel `std_thread_obj` line added; nothing existing is removed.)

In `build_aot_cross` (around line 318), add the Windows rejection right next to the existing `wants_std_io`/Windows check:

```rust
    let wants_std_io = std_imports.iter().any(|m| m == "io");
    let wants_map = std_imports.iter().any(|m| m == "map");
    let wants_std_thread = std_imports.iter().any(|m| m == "thread");
    if wants_std_io && target.is_windows() {
        return Err(
            "'import std io' is not supported when cross-compiling to a Windows target in v1 \
             (POSIX socket APIs aren't available under the mingw cross toolchain) -- build \
             natively on Windows instead, or drop 'import std io'".to_string(),
        );
    }
    if wants_std_thread && target.is_windows() {
        return Err(
            "'import std thread' is not supported when cross-compiling to a Windows target in v1 \
             (std::thread isn't available under the mingw cross toolchain used here) -- build \
             natively on Windows instead, or drop 'import std thread'".to_string(),
        );
    }
```

And extend the object-compile/link/cleanup steps the same way as `build_aot_host`:

```rust
    let std_io_obj = if wants_std_io {
        Some(compile_std_io_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };
    let map_obj = if wants_map {
        Some(compile_map_obj("zig", &["c++", "-target", target.zig_triple()])?)
    } else {
        None
    };
    let std_thread_obj = if wants_std_thread {
        let extra: Vec<&str> = if target.os == targets::Os::Linux {
            vec!["c++", "-target", target.zig_triple(), "-pthread"]
        } else {
            vec!["c++", "-target", target.zig_triple()]
        };
        Some(compile_std_thread_obj("zig", &extra)?)
    } else {
        None
    };

    let linker_subcmd = if imports.is_empty() && !wants_std_io && !wants_map && !wants_std_thread { "cc" } else { "c++" };
    let mut cmd = Command::new("zig");
    cmd.args([linker_subcmd, "-target", target.zig_triple(), obj.as_str(), "-o", out.as_str()]);
    if let Some(p) = &std_io_obj {
        cmd.arg(p);
    }
    if let Some(p) = &map_obj {
        cmd.arg(p);
    }
    if let Some(p) = &std_thread_obj {
        cmd.arg(p);
    }
    if wants_std_thread && target.os == targets::Os::Linux {
        cmd.arg("-pthread");
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
    if let Some(p) = &map_obj { let _ = std::fs::remove_file(p); }
    if let Some(p) = &std_thread_obj { let _ = std::fs::remove_file(p); }
    if !status.success() {
        return Err("link failed".to_string());
    }
    Ok(())
```

(Same rule: this extends the existing blocks in `build_aot_cross`, doesn't remove any existing `std_io_obj`/`map_obj` handling.)

Also update `build_aot_all` (whichever function fans out to all six targets — check `src/main.rs` for its definition; it calls `build_aot_cross` per target, so it needs no changes itself, just confirm it doesn't have its own separate `wants_*`/link logic duplicated. If it does, mirror the same `wants_std_thread` addition there too.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo build && cargo test --test e2e std_thread`
Expected: `build_links_and_runs_a_program_using_std_thread_spawn_join` and `cross_build_rejects_std_thread_import_for_windows_target` PASS (the latter only if `zig` is on `PATH`; it self-skips otherwise — confirm with `which zig` first so you know which outcome to expect). `run_rejects_programs_with_std_thread_import` PASSES.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/e2e.rs tests/fixtures/std_thread_spawn_join.verb tests/fixtures/std_thread_spawn_join.expected
git commit -m "feat(build): compile and link runtime/verb_std_thread.cpp for 'import std thread'"
```

---

### Task 6: End-to-end fixtures — spawn/join, mutex contention, channel, sleep

**Files:**
- Create: `tests/fixtures/std_thread_spawn_join.verb` + `.expected` (if not already created by Task 5)
- Create: `tests/fixtures/std_thread_mutex.verb` + `.expected`
- Create: `tests/fixtures/std_thread_channel.verb` + `.expected`
- Create: `tests/fixtures/std_thread_channel_rejects_non_primitive.verb` + `.expected`
- Create: `tests/fixtures/std_thread_sleep.verb` + `.expected`
- Modify: `tests/e2e.rs` (add `run_ok`-based tests for the four new fixtures, in the `// ----- std thread -----` section Task 5 started)

**Interfaces:**
- Consumes: the full `import std thread;` surface from Tasks 1-5. No new Rust/C++ interfaces produced — this task only adds `.verb` programs and their `run_ok`-style assertions.

- [ ] **Step 1: Write the fixtures**

`tests/fixtures/std_thread_spawn_join.verb` (create only if Task 5 didn't already; content must match exactly if it did):

```
import std thread;

assign counter 0;

make bump() begin
  counter be counter add 1;
end

assign t thread_spawn(bump);
thread_join(t);
print(counter);
```

`tests/fixtures/std_thread_spawn_join.expected`:

```
1
```

`tests/fixtures/std_thread_mutex.verb`:

```
import std thread;

assign counter 0;
assign m mutex_new();

make bump() begin
  loop assign i 0; i trails 1000; i be i add 1 begin
    mutex_lock(m);
    counter be counter add 1;
    mutex_unlock(m);
  end
end

assign t1 thread_spawn(bump);
assign t2 thread_spawn(bump);
assign t3 thread_spawn(bump);
assign t4 thread_spawn(bump);
thread_join(t1);
thread_join(t2);
thread_join(t3);
thread_join(t4);
print(counter);
```

`tests/fixtures/std_thread_mutex.expected`:

```
4000
```

`tests/fixtures/std_thread_channel.verb`:

```
import std thread;

assign ch channel_new();

make producer() begin
  channel_send(ch, 42);
end

assign t thread_spawn(producer);
print(channel_recv(ch));
thread_join(t);
```

`tests/fixtures/std_thread_channel.expected`:

```
42
```

`tests/fixtures/std_thread_channel_rejects_non_primitive.verb`:

```
import std thread;

assign ch channel_new();
assign ok channel_send(ch, "nope");
print(ok);
assign ok2 channel_send(ch, 7);
print(ok2);
print(channel_recv(ch));
```

`tests/fixtures/std_thread_channel_rejects_non_primitive.expected`:

```
false
true
7
```

`tests/fixtures/std_thread_sleep.verb`:

```
import std thread;

thread_sleep_ms(1);
print("done");
```

`tests/fixtures/std_thread_sleep.expected`:

```
done
```

- [ ] **Step 2: Write the failing tests**

Add to `tests/e2e.rs`'s `// ----- std thread -----` section (after whatever Task 5 added there):

```rust
#[test]
fn build_links_and_runs_a_program_using_std_thread_mutex() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_mutex_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_mutex.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_mutex.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_channel() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_channel_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_channel.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_channel.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn channel_send_rejects_a_non_primitive_value() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_channel_reject_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_channel_rejects_non_primitive.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_channel_rejects_non_primitive.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn build_links_and_runs_a_program_using_std_thread_sleep() {
    let out_path = std::env::temp_dir().join("verb_e2e_std_thread_sleep_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_verb"))
        .args([
            "build", "tests/fixtures/std_thread_sleep.verb",
            "-o", out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed: {}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&out_path).output().unwrap();
    assert!(run.status.success(), "run failed: {}", String::from_utf8_lossy(&run.stderr));
    let expected = std::fs::read_to_string("tests/fixtures/std_thread_sleep.expected").unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);

    let _ = std::fs::remove_file(&out_path);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test e2e std_thread`
Expected: FAIL if Tasks 1-5 aren't complete in your working copy yet (undefined variable / link errors); if all prior tasks are already merged, these should already PASS on first run since the fixtures are new but the underlying machinery isn't — in that case skip ahead, there's nothing left to implement, just confirm PASS in Step 4.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test e2e std_thread -- --test-threads=1`

(`--test-threads=1` here is about Rust's own test *harness* concurrency, unrelated to Verb's threads — it just avoids five separate `verb build` subprocesses fighting over CPU at once on a slow CI box, matching how the existing std-io/std-map e2e tests are run elsewhere in this suite. Not required, just reduces flakiness on constrained machines.)

Run the mutex test at least 20 times in a loop to catch a race before calling it done — a single green run doesn't prove mutual exclusion:

```bash
for i in $(seq 1 20); do cargo test --test e2e build_links_and_runs_a_program_using_std_thread_mutex -- --exact || break; done
```

Expected: PASS every time, all 20 iterations. If `print(counter)` is ever not exactly `4000`, `mutex_lock`/`mutex_unlock` aren't providing real mutual exclusion — stop and re-check Task 2's `Channel`/mutex implementation before proceeding (do not weaken the test to tolerate a wrong count).

- [ ] **Step 5: Commit**

```bash
git add tests/fixtures/std_thread_mutex.verb tests/fixtures/std_thread_mutex.expected \
        tests/fixtures/std_thread_channel.verb tests/fixtures/std_thread_channel.expected \
        tests/fixtures/std_thread_channel_rejects_non_primitive.verb tests/fixtures/std_thread_channel_rejects_non_primitive.expected \
        tests/fixtures/std_thread_sleep.verb tests/fixtures/std_thread_sleep.expected \
        tests/e2e.rs
git commit -m "test(e2e): cover std thread mutex contention, channel handoff, sleep"
```

---

## Known Follow-Ups (explicitly out of scope for this plan)

- **GC leak-check coverage**: this branch (`refcounting-gc-v2`) doesn't yet have the `verb_gc_live`/`VERB_GC_DEBUG`/`assert_no_leaks` test infrastructure that the (separately-branched, closed-unmerged) `refcounting-gc` branch built. Once that infrastructure lands on `refcounting-gc-v2`, add a leak-check test for `std_thread_spawn_join.verb` — but note the mutex/channel-contention fixtures should probably be *excluded* from any such leak-check, since the underlying `verb_gc_live` counter itself is a plain (non-atomic) global int, and two threads each calling a Verb function concurrently already race on it independent of anything this plan does (every function call currently does at least a self-referential closure-cell allocation/free in its prologue). That's a pre-existing property of combining OS threads with the current single-threaded-assumption codegen, not a bug introduced here — flag it to whoever picks up that follow-up rather than silently working around it.
- **`thread_spawn` argument forwarding**: deferred per the design spec's non-goals — v1 spawned closures are 0-arity only.
- **Passing heap values across threads**: deferred — would require either atomic refcounts or deep-copy-on-cross, both explicitly out of scope for this spec.
