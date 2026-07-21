# `VERB_EXPORT` Macro Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a C++ function exposed to Verb via `import mod` be written as one `VERB_EXPORT(name, arity, callable)` line instead of a hand-written `extern "C" VerbValue` wrapper.

**Architecture:** A C++17 template (`verb_detail::invoke<Fn>`, `Fn` an `auto` non-type template parameter) inspects the wrapped callable's real signature via `function_traits`, unwraps/wraps `VerbValue` args/results per-type, and `static_assert`s arity and type support at compile time. `VERB_EXPORT_IMPL_0`..`_6` are literal per-arity macros (an `extern "C"` function's parameter list must be literal C++ syntax, not template-deduced); `VERB_EXPORT` dispatches to the right one via token-pasting on the literal `arity` argument.

**Tech Stack:** C++17 (already used by the project's C++ fixtures, `-std=c++17`), header-only addition to `runtime/verb.h`, Rust `Command`-based integration tests (matches `tests/e2e.rs`'s existing style).

## Global Constraints

- C++17 only, matching `tests/e2e.rs:242`'s existing `-std=c++17` fixture build flag — no newer standard.
- Header-only: all new code lives in `runtime/verb.h`, guarded so the file's existing C-compatible section (used by plain C consumers) is untouched.
- Exactly 4 supported Verb value types map to C++: `int64_t`, `double`, `const char*`, `int` (Verb bool — NOT C++ `bool`), plus `void` as a return-only type mapping to `verb_nil()`.
- Arity 0–6 only. No support beyond that; no lambdas/function-objects as `callable`, only function pointers / plain function names / casts thereof.
- No changes to `src/` (parser, codegen, CLI) or the `.verb` language — this plan touches only `runtime/verb.h` and `tests/`.
- Full design rationale: `docs/superpowers/specs/2026-07-20-verb-export-macro-design.md`.

---

## Task 1: Implement `VERB_EXPORT` in `runtime/verb.h`, with a standalone compile-check test suite

**Files:**
- Modify: `runtime/verb.h`
- Create: `tests/verb_export_macro.rs`

**Interfaces:**
- Produces: `VERB_EXPORT(name, arity, callable)` macro, usable from any `.cpp` file that does `#include "verb.h"` and is compiled with `-std=c++17`. Expands to a full `extern "C" VerbValue name(...)` function definition (no trailing semicolon needed, though one is harmless).
- Consumes: nothing from other tasks (this task is the ground floor).

This task is entirely self-contained: it proves the macro compiles and behaves correctly via direct `c++` invocations, without touching the existing `mathlib.cpp` fixture or `tests/e2e.rs` (that's Task 2, which depends on this task's `runtime/verb.h` being done).

- [ ] **Step 1: Write the failing test file**

Create `tests/verb_export_macro.rs`:

```rust
use std::io::Write;
use std::process::Command;

/// Compiles `snippet` (arbitrary top-level C++ code) as a standalone
/// translation unit against `runtime/verb.h`, which is `#include`d
/// automatically. Returns (compile succeeded, captured stderr).
fn compile_snippet(name: &str, snippet: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join("verb_export_macro_test");
    std::fs::create_dir_all(&dir).unwrap();
    let src_path = dir.join(format!("{name}.cpp"));
    let obj_path = dir.join(format!("{name}.o"));
    let mut f = std::fs::File::create(&src_path).unwrap();
    writeln!(f, "#include \"verb.h\"\n{snippet}").unwrap();
    let out = Command::new("c++")
        .args([
            "-std=c++17",
            "-Iruntime",
            "-c",
            "-o", obj_path.to_str().unwrap(),
            src_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to invoke c++");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&obj_path);
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn exports_a_cast_stdlib_function() {
    let (ok, stderr) = compile_snippet("export_sqrt", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 1, static_cast<double(*)(double)>(std::sqrt))
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_plain_two_arg_function() {
    let (ok, stderr) = compile_snippet("export_add", r#"
        int64_t add_int(int64_t a, int64_t b) { return a + b; }
        VERB_EXPORT(c_add_int, 2, add_int)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_zero_arity_void_function() {
    let (ok, stderr) = compile_snippet("export_void", r#"
        #include <cstdio>
        void say_hello() { std::printf("hi\n"); }
        VERB_EXPORT(c_hello, 0, say_hello)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn exports_a_bool_returning_function() {
    let (ok, stderr) = compile_snippet("export_bool", r#"
        int is_positive(int64_t n) { return n > 0; }
        VERB_EXPORT(c_is_positive, 1, is_positive)
    "#);
    assert!(ok, "expected compile success, got:\n{stderr}");
}

#[test]
fn arity_mismatch_fails_to_compile() {
    let (ok, stderr) = compile_snippet("bad_arity", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 2, static_cast<double(*)(double)>(std::sqrt))
    "#);
    assert!(!ok, "expected compile failure for arity mismatch");
    assert!(
        stderr.contains("VERB_EXPORT arity does not match callable's parameter count"),
        "stderr: {stderr}"
    );
}

#[test]
fn unsupported_parameter_type_fails_to_compile() {
    let (ok, stderr) = compile_snippet("bad_param_type", r#"
        float half(float x) { return x / 2; }
        VERB_EXPORT(c_half, 1, half)
    "#);
    assert!(!ok, "expected compile failure for unsupported type");
    assert!(
        stderr.contains("VERB_EXPORT: unsupported parameter type")
            || stderr.contains("VERB_EXPORT: unsupported return type"),
        "stderr: {stderr}"
    );
}

#[test]
fn bare_overloaded_callable_fails_to_compile() {
    let (ok, _stderr) = compile_snippet("bad_overload", r#"
        #include <cmath>
        VERB_EXPORT(c_sqrt, 1, std::sqrt)
    "#);
    assert!(!ok, "expected compile failure for bare overloaded name");
}

#[test]
fn lambda_callable_fails_to_compile() {
    let (ok, _stderr) = compile_snippet("bad_lambda", r#"
        auto lam = [](int64_t x) -> int64_t { return x; };
        VERB_EXPORT(c_lam, 1, lam)
    "#);
    assert!(!ok, "expected compile failure for lambda callable");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test verb_export_macro -- --test-threads=1`
Expected: 5 of 8 tests FAIL — the four `exports_*` tests (compile error: `VERB_EXPORT` / `verb_detail` not declared, since the macro doesn't exist in `runtime/verb.h` yet) plus `arity_mismatch_fails_to_compile` and `unsupported_parameter_type_fails_to_compile` (their snippets do fail to compile already, but for the wrong reason — `VERB_EXPORT` undefined — so the `stderr.contains(...)` assertion on the specific `static_assert` message fails). `bare_overloaded_callable_fails_to_compile` and `lambda_callable_fails_to_compile` PASS already at this point — they only assert `!ok`, which is trivially true when the macro doesn't exist at all; that's expected and not a bug, both get real (non-vacuous) coverage once Step 3 lands the actual implementation and their snippets fail for the *documented* reason instead.

- [ ] **Step 3: Add the C++-only section to `runtime/verb.h`**

The file currently ends with:

```c
static inline int verb_as_bool(VerbValue v) { return v.payload != 0; }

#endif // VERB_H
```

Replace that ending with (i.e. insert everything below before the final `#endif // VERB_H`):

```c
static inline int verb_as_bool(VerbValue v) { return v.payload != 0; }

#ifdef __cplusplus
#include <tuple>
#include <utility>
#include <type_traits>

// VERB_EXPORT(name, arity, callable) generates an `extern "C" VerbValue
// name(...)` wrapper around `callable`, deducing and type-checking its
// real signature at compile time instead of requiring a hand-written
// VerbValue-typed wrapper. See docs/superpowers/specs/
// 2026-07-20-verb-export-macro-design.md for the full design.
namespace verb_detail {

template <typename T> T unwrap(VerbValue v);
template <> inline int64_t     unwrap<int64_t>(VerbValue v)     { return verb_as_int(v); }
template <> inline double      unwrap<double>(VerbValue v)      { return verb_as_float(v); }
template <> inline const char* unwrap<const char*>(VerbValue v) { return verb_as_string(v); }
template <> inline int         unwrap<int>(VerbValue v)         { return verb_as_bool(v); }

template <typename T> VerbValue wrap(T v);
template <> inline VerbValue wrap<int64_t>(int64_t v)         { return verb_int(v); }
template <> inline VerbValue wrap<double>(double v)           { return verb_float(v); }
template <> inline VerbValue wrap<const char*>(const char* v) { return verb_string(v); }
template <> inline VerbValue wrap<int>(int v)                 { return verb_bool(v); }

inline VerbValue wrap_void() { return verb_nil(); }

template <typename F> struct function_traits;

template <typename R, typename... Args>
struct function_traits<R(Args...)> {
    using return_type = R;
    static constexpr size_t arity = sizeof...(Args);
    template <size_t I> using arg_type = std::tuple_element_t<I, std::tuple<Args...>>;
};

template <typename R, typename... Args>
struct function_traits<R(*)(Args...)> : function_traits<R(Args...)> {};

template <typename T> struct is_supported_type : std::false_type {};
template <> struct is_supported_type<int64_t> : std::true_type {};
template <> struct is_supported_type<double> : std::true_type {};
template <> struct is_supported_type<const char*> : std::true_type {};
template <> struct is_supported_type<int> : std::true_type {};

template <typename T> struct is_supported_return : is_supported_type<T> {};
template <> struct is_supported_return<void> : std::true_type {};

template <typename Traits, size_t... I>
constexpr bool all_args_supported(std::index_sequence<I...>) {
    bool results[] = { is_supported_type<typename Traits::template arg_type<I>>::value..., true };
    for (bool b : results) if (!b) return false;
    return true;
}

template <auto Fn, typename Traits, typename Tuple, size_t... I>
VerbValue invoke_call(Tuple& args, std::index_sequence<I...>) {
    if constexpr (std::is_void_v<typename Traits::return_type>) {
        Fn(unwrap<typename Traits::template arg_type<I>>(std::get<I>(args))...);
        return wrap_void();
    } else {
        return wrap<typename Traits::return_type>(
            Fn(unwrap<typename Traits::template arg_type<I>>(std::get<I>(args))...));
    }
}

template <auto Fn, typename... As>
VerbValue invoke(As... as) {
    using traits = function_traits<decltype(Fn)>;
    static_assert(traits::arity == sizeof...(As),
        "VERB_EXPORT arity does not match callable's parameter count");
    static_assert(all_args_supported<traits>(std::make_index_sequence<traits::arity>{}),
        "VERB_EXPORT: unsupported parameter type");
    static_assert(is_supported_return<typename traits::return_type>::value,
        "VERB_EXPORT: unsupported return type");
    std::tuple<As...> args(as...);
    return invoke_call<Fn, traits>(args, std::make_index_sequence<sizeof...(As)>{});
}

} // namespace verb_detail

#define VERB_EXPORT_IMPL_0(name, fn) \
    extern "C" VerbValue name() { return ::verb_detail::invoke<fn>(); }
#define VERB_EXPORT_IMPL_1(name, fn) \
    extern "C" VerbValue name(VerbValue a0) { return ::verb_detail::invoke<fn>(a0); }
#define VERB_EXPORT_IMPL_2(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1) { return ::verb_detail::invoke<fn>(a0, a1); }
#define VERB_EXPORT_IMPL_3(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1, VerbValue a2) { return ::verb_detail::invoke<fn>(a0, a1, a2); }
#define VERB_EXPORT_IMPL_4(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1, VerbValue a2, VerbValue a3) { return ::verb_detail::invoke<fn>(a0, a1, a2, a3); }
#define VERB_EXPORT_IMPL_5(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1, VerbValue a2, VerbValue a3, VerbValue a4) { return ::verb_detail::invoke<fn>(a0, a1, a2, a3, a4); }
#define VERB_EXPORT_IMPL_6(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1, VerbValue a2, VerbValue a3, VerbValue a4, VerbValue a5) { return ::verb_detail::invoke<fn>(a0, a1, a2, a3, a4, a5); }

#define VERB_EXPORT_CONCAT_(a, b) a##b
#define VERB_EXPORT_CONCAT(a, b) VERB_EXPORT_CONCAT_(a, b)
#define VERB_EXPORT(name, arity, fn) VERB_EXPORT_CONCAT(VERB_EXPORT_IMPL_, arity)(name, fn)

#endif // __cplusplus

#endif // VERB_H
```

Note the final `#endif // VERB_H` moves down to after the new `#endif // __cplusplus` — there is only ever one closing `#endif // VERB_H` in the file; do not duplicate it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test verb_export_macro -- --test-threads=1`
Expected: all 8 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add runtime/verb.h tests/verb_export_macro.rs
git commit -m "feat: add VERB_EXPORT macro for C++ import wrapper generation"
```

---

## Task 2: Rewrite `mathlib.cpp` to use `VERB_EXPORT`, add void/bool fixture coverage

**Depends on:** Task 1 (`runtime/verb.h` must already have `VERB_EXPORT`).

**Files:**
- Modify: `tests/fixtures/cpp/mathlib.cpp`
- Modify: `tests/fixtures/import_mathlib.verb`
- Modify: `tests/fixtures/import_mathlib.expected`

**Interfaces:**
- Consumes: `VERB_EXPORT(name, arity, callable)` from Task 1's `runtime/verb.h`.
- Produces: nothing new for later tasks — this is the last task in the plan.

- [ ] **Step 1: Extend the failing fixture (`.verb` + `.expected`) first**

Current `tests/fixtures/import_mathlib.verb`:

```
import mod mathlib;

print(c_sqrt(9.0));
print(c_add_int(2, 3));
print(c_shout("hi"));
```

Replace with:

```
import mod mathlib;

print(c_sqrt(9.0));
print(c_add_int(2, 3));
print(c_shout("hi"));
c_hello();
print(c_is_positive(5));
print(c_is_positive(neg 5));
```

Current `tests/fixtures/import_mathlib.expected`:

```
3
5
HI!
```

Replace with:

```
3
5
HI!
hello from cpp
true
false
```

- [ ] **Step 2: Run the e2e test to verify it fails**

Run: `cargo test --test e2e imports_cpp_library_and_calls_extern_functions`
Expected: FAIL — `verb build` link step errors with undefined symbols `c_hello` and `c_is_positive` (they don't exist in `mathlib.cpp` yet).

- [ ] **Step 3: Rewrite `tests/fixtures/cpp/mathlib.cpp`**

Replace the entire file with:

```cpp
#include "verb.h"
#include <cmath>
#include <cctype>
#include <cstring>
#include <cstdlib>
#include <cstdio>

static const char* shout_impl(const char* in) {
    size_t len = std::strlen(in);
    char* out = static_cast<char*>(std::malloc(len + 2));
    for (size_t i = 0; i < len; i++) {
        out[i] = static_cast<char>(std::toupper(static_cast<unsigned char>(in[i])));
    }
    out[len] = '!';
    out[len + 1] = '\0';
    return out;
}

static int64_t add_int(int64_t a, int64_t b) { return a + b; }

static void say_hello() { std::printf("hello from cpp\n"); }

static int is_positive(int64_t n) { return n > 0; }

VERB_EXPORT(c_sqrt, 1, static_cast<double(*)(double)>(std::sqrt))
VERB_EXPORT(c_add_int, 2, add_int)
VERB_EXPORT(c_shout, 1, shout_impl)
VERB_EXPORT(c_hello, 0, say_hello)
VERB_EXPORT(c_is_positive, 1, is_positive)
```

Note `shout_impl` is declared returning `const char*`, not `char*` — `VERB_EXPORT`'s type table only supports `const char*`, and `function_traits` reads the function's *declared* signature (not what it implicitly converts to), so the declared return type must exactly match. This whole file has been verified to compile and link as a shared library, and to produce the exact fixture output above, ahead of handing off this plan.

- [ ] **Step 4: Run the e2e test to verify it passes**

Run: `cargo test --test e2e imports_cpp_library_and_calls_extern_functions`
Expected: PASS.

- [ ] **Step 5: Run the full test suite to confirm no regressions**

Run: `cargo test`
Expected: all tests pass, including Task 1's `verb_export_macro` tests and the rest of `tests/e2e.rs`.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/cpp/mathlib.cpp tests/fixtures/import_mathlib.verb tests/fixtures/import_mathlib.expected
git commit -m "test: rewrite mathlib.cpp fixture to use VERB_EXPORT, cover void/bool"
```

---

## Suggested reviewer checklist (both tasks)

- Task 1: deliberately break each `static_assert` (arity, param type, return type, bare overloaded callable) by hand and confirm the compiler error is the one documented, not a confusing cascade — some cascading noise from `invoke_call`'s own instantiation is expected and acceptable (see spec's Error handling section), but the primary message must be present and legible.
- Task 1: confirm `runtime/verb.h`'s pre-existing C-compatible section (`verb_nil`, `verb_bool`, ..., `verb_as_bool`) is byte-for-byte unchanged, and the entire new section is inside `#ifdef __cplusplus` — a C compiler including this header must still succeed.
- Task 2: confirm `tests/fixtures/import_mathlib.expected`'s new lines (`hello from cpp`, `true`, `false`) exactly match actual `cargo test` output, not just visually plausible values.
- Task 2: confirm the old hand-written `extern "C" VerbValue c_sqrt(VerbValue x) { ... }` style (still valid, still supported) isn't left dangling anywhere as dead code after the rewrite.
