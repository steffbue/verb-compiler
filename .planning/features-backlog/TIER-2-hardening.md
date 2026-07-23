# Tier 2 — Correctness / Hardening Implementation Plan

> Line numbers verified against branch `refcounting-gc-v2`.
> Four independent units, ordered by priority. Tasks 1 & 2 are the real blockers
> (SC2 / INTEG-02); Tasks 3 & 4 are hardening that can land afterward in any order.

---

## Task 1 — `verb build --target all` per-target `-L` fix (INTEG-02 / SC2 blocker)

### Root cause
`build_aot_all` (`src/main.rs:458-480`) loops `targets::ALL` (6 targets) and calls
`build_aot_cross` with **the same `lib_dirs` slice** for every target (`src/main.rs:464`).
`build_aot_cross` forwards those dirs verbatim to the `zig c++` link line
(`src/main.rs:442-444`). A single-arch FFI lib (e.g. `libmathlib.dylib` from
`build_mathlib_fixture` `tests/e2e.rs:378-392`) links for exactly one of the four
non-Windows targets; the other three fail with an arch mismatch inside zig.

### Approach — per-target library-directory resolution by label convention
For each user `-L<dir>`, prefer a per-target subdir named by `Target::label()`
(`src/targets.rs:79`, e.g. `linux-x86_64`, `macos-arm64`) when it exists, else fall
back to the bare `<dir>`. Layout:

```
libs/linux-x86_64/libmathlib.so
libs/macos-arm64/libmathlib.dylib
libs/windows-x86_64/mathlib.lib
```

`verb build --target all app.verb -o app -Llibs` then resolves each target to its
subdir. Flat `-L<dir>` with no subdirs keeps working (pure fallback) → backward
compatible, no new CLI surface.

### Files & functions
- **`src/targets.rs`** — add `impl Target { pub fn resolve_lib_dirs(&self, lib_dirs: &[String]) -> Vec<String> }`.
  For each `-L<dir>` token (per `parse_cli` `src/main.rs:133-134`), strip `-L`, test
  `Path::new(dir).join(self.label()).is_dir()`, emit `-L{dir}/{label}` if present else
  the original token. Leave non-`-L` tokens untouched.
- **`src/main.rs` `build_aot_cross` (`384-457`)** — before line 442 replace the raw
  `lib_dirs` loop with `let resolved = target.resolve_lib_dirs(lib_dirs);` then
  `for dir in &resolved { cmd.arg(dir); }`. This auto-fixes `--target all`
  (funnels through `build_aot_cross`).
- **`build_aot_host` (`src/main.rs:310`)** — leave as-is (single target, flat `-L` correct).

### Tests
- New e2e in `tests/e2e.rs` modeled on
  `build_with_l_flag_forwards_it_without_breaking_the_build` (`:338`) +
  `build_mathlib_fixture` (`:378`): build fixture into temp `libs/<host-label>/`, run
  `verb build --target <host-label> ... -Llibs`, assert success; second assertion with
  lib flat in `libs/` confirms fallback.
- `--target all` test gated behind zig-availability (mirror `check_zig_available`
  `src/main.rs:85`); assert summary (`src/main.rs:470-475`) reports host-arch target `ok`.
- Unit test `resolve_lib_dirs` in `#[cfg(test)]` in `src/targets.rs`.

### Risks
- Both `<dir>` and `<dir>/<label>` holding a lib → label subdir wins by design;
  document in `--help` (`src/main.rs:171-172`).
- `label()` allocates; runs once per target at link time — negligible.

---

## Task 2 — FFI string ABI bug (SIGTRAP retaining FFI-returned strings)

### Root cause
`runtime/verb.h:93` `wrap<const char*>` calls `verb_string(v)` (`verb.h:51`) stuffing the
**raw pointer** into a `VerbValue` with no GC header. FFI callables (e.g. `shout_impl`
`tests/fixtures/cpp/mathlib.cpp:8-17`) return a bare `std::malloc`'d buffer. The header
contract (`runtime/verb.h:19-26`, `build_alloc_fn` `src/codegen.rs:239-262`) requires an
8-byte refcount word at `payload-8`. Assigning the FFI result → `verb_retain_value`
writes `ptr-8`, corrupting allocator metadata → SIGTRAP (exit 133).
Confirmed in `.planning/phases/08-*/08-02-SUMMARY.md`. `print(c_shout("hi"))` survives
(never retained); `assign x c_shout(...)` crashes.

### Approach — defensive-copy in `wrap<const char*>` (Option b)
Copy the incoming C string into a `verb_alloc`-headed buffer:

```cpp
template <> inline VerbValue wrap<const char*>(const char* v) {
    if (!v) return verb_nil();
    size_t n = strlen(v);
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(n) + 1));
    if (!out) return verb_nil();
    memcpy(out, v, n + 1);
    return verb_string(out);
}
```

Mirrors `verb_std_io.cpp:24-30` (`verb_string_from`). `verb_alloc` declared `verb.h:26`;
`<string.h>` included `verb.h:17`. Makes the `VERB_EXPORT` contract safe for any callable
returning `const char*` regardless of how it allocated.

### Ownership caveat
Defensive-copy means a callable returning a `malloc`'d buffer (like `shout_impl`) now
**leaks its own buffer** (Verb copies, never frees original). Acceptable — the FFI boundary
can't know the callee's free convention. Add a one-line comment in `verb.h` near the `wrap`
specialization and in `tests/fixtures/cpp/mathlib.cpp:8`. (Reject Option a — requiring every
FFI author to allocate via `verb_alloc` is more error-prone.)

### Files
- **`runtime/verb.h`** — replace body of `wrap<const char*>` (`:93`). No signature/macro change.
- **No codegen change** — bug is runtime-side.

### Tests
- New fixture `tests/fixtures/ffi_string_retain.verb` (+ `.expected`):
  `import mod mathlib;` then `assign s c_shout("hi"); print(s);` → `HI!`. Wire via existing
  `build_mathlib_fixture` + `-L` + `DYLD_LIBRARY_PATH` pattern (`tests/e2e.rs:378-423`).
  Pre-fix crashes exit 133; post-fix prints `HI!`.
- Zero-leak: reuse `VERB_GC_DEBUG=1` → `verb_gc_live=0` (`tests/e2e.rs:52-67`). The leaked
  *original* malloc buffer is invisible to `verb_gc_live` (counts only `verb_alloc` blocks) →
  count stays 0, test valid.

### Risks
- `nullptr` return now → `verb_nil()` (strictly safer, matches std-io).
- Extra copy per FFI string return — negligible, not a hot path.
- Original callee buffer leaks — documented, not counted by leak test.

---

## Task 3 — Overflow checks (array growth, string concat, int arithmetic)

### Unchecked sites
1. Array capacity doubling — `build_array_push_fn` `src/codegen.rs:1048` (`cap*2`),
   `:1050` (`new_cap*16` bytes) → `malloc_bytes_dyn` (`:318`).
2. String concat — `build_concat_fn` `src/codegen.rs:744` (`la+lb`), `:745` (`sum+1`) →
   `verb_alloc` (`:746`).
3. Integer arithmetic — `build_arith_fn` `src/codegen.rs:548-550` (Add/Sub/Mul).
4. `verb_alloc` header add — `src/codegen.rs:253` (`n+8`); fold into Task 4.

### Approach — LLVM checked-arith intrinsics + abort block
Add `checked_mul_or_abort` / `checked_add_or_abort` helpers on `Codegen` (near `abort_at`
`src/codegen.rs:196`). Declare `llvm.umul/uadd.with.overflow.i64` (sizes, unsigned) and
`llvm.smul/sadd/ssub.with.overflow.i64` (signed language arith) once (near `:84-88`).
Extract `{i64,i1}`, branch on overflow bit to abort, return result on fall-through.
Reuse `abort_at` for language-arith (site 3 — `build_arith_fn` has line/col). Sites 1,2
already thread line/col to their `abort_at` (concat `:753-755`, push `:1029`).

### Sequencing
Site 4 (`n+8`) handled with Task 4 (both in `build_alloc_fn`). Do sites 1–3 first.

### Tests
- `err_int_overflow.verb`: `times`/`add` near `i64::MAX` → `run_err` (`tests/e2e.rs:32,153`)
  with `runtime error [L:C]: integer overflow ...`.
- Array/concat overflow impractical to trigger → IR inspection (pattern `tests/e2e.rs:130`,
  `:246`): assert emitted IR contains `llvm.umul.with.overflow` in push/concat paths.
- `run_ok("arith")` (`:142`), `arrays_push_pop` (`:102`) must still pass.

### Risks
- Perf: overflow branch on every int add/mul. Keep fast path fall-through, abort block cold.
  If regresses, gate language-arith checks behind a flag but keep size-computation checks
  (sites 1,2) unconditional (memory safety).
- Signed vs unsigned intrinsic must match: sizes unsigned, language ints signed.

---

## Task 4 — OOM handling (malloc failure unhandled)

### Gap
`build_alloc_fn` (`src/codegen.rs:246-262`) calls `malloc` (`:254`) then immediately stores
refcount (`:255`) and GEPs payload (`:256-259`) with **no null check** → null deref segfault
on alloc failure. C++ runtime already defensive (`verb_std_io.cpp:26,51`); harden `verb_alloc`
itself and the whole system degrades gracefully.

### Approach — null-check inside `verb_alloc`, abort cleanly
In `build_alloc_fn` after `malloc` (`:254`): compare `raw == null`; null → OOM abort block
that `printf`s `"out of memory\n"` + `exit(1)` (reuse printf/exit from `abort_at` `:203-204`,
or factor `abort_msg`). Non-null → continue store/GEP/`inc_live_counter`/return (`:255-261`).
Fold Task 3 site 4 (`n+8`) here via `checked_add_or_abort`.
Keep `verb_std_io.cpp` / mathlib returning `verb_nil()` on OOM (softer I/O policy); only
`verb_alloc` aborts hard (right default, matches `abort_at`).

### Files
- **`src/codegen.rs`** — edit `build_alloc_fn` (`:246-262`); optional `abort_msg` helper.
- **`runtime/verb_map.cpp`** — audit for raw `malloc`/`new` that could null-deref; add guards.
  (One place needing a read before editing.)

### Tests
- OOM not deterministically triggerable → IR inspection (pattern `tests/e2e.rs:256`
  `verb_alloc_is_emitted`): assert body contains `icmp eq ... null` + branch to `exit` block.
- Optional `#[ignore]` test via `ulimit -v` subprocess forcing malloc fail.
- All `gc_*` fixtures (`tests/e2e.rs:121-127`) still pass; `inc_live_counter` (`:260`) stays
  on non-null path only.

### Risks
- `inc_live_counter` must stay success-path-only (else counts failed allocs).
- Hard abort on OOM acceptable for toy language; propagating would need Result threading
  through all alloc sites (out of scope).

---

## Cross-task notes
- Tasks independent & parallelizable **except** Task 3-site-4 and Task 4 both edit
  `build_alloc_fn` (`src/codegen.rs:246-262`) — same worker or sequence 4 after 3.
- Tasks 1 & 2 touch disjoint files (`src/main.rs`+`src/targets.rs` vs `runtime/verb.h`) —
  fully parallel, do first as SC2/INTEG-02 blockers.
- Full e2e suite (~76 tests) is the backstop. `cargo test --test e2e` after each task.

## Critical files
- `src/main.rs` — `build_aot_all` `:458`, `build_aot_cross` `:384` (Task 1)
- `src/targets.rs` — add `resolve_lib_dirs`, `label` `:79` (Task 1)
- `runtime/verb.h` — `wrap<const char*>` `:93` (Task 2)
- `src/codegen.rs` — `build_alloc_fn` `:246`, `build_concat_fn` `:716`,
  `build_array_push_fn` `:1006`, `build_arith_fn` `:510`, `abort_at` `:196` (Tasks 3 & 4)
- `tests/e2e.rs` — `build_mathlib_fixture` `:378`, `assert_no_leaks` `:52`, `run_err` `:32`,
  IR-inspection `:130`/`:256`
