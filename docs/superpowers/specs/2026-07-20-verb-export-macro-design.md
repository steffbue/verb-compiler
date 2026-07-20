# `VERB_EXPORT` macro — design spec

Date: 2026-07-20
Status: approved

## Purpose

`import mod` (see `docs/superpowers/specs/2026-07-20-cpp-import-design.md`) requires every extern C++ function to be hand-written against `VerbValue` directly — tagging/untagging args and results by hand, e.g.:

```cpp
extern "C" VerbValue c_sqrt(VerbValue x) {
    return verb_float(std::sqrt(verb_as_float(x)));
}
```

This spec adds `VERB_EXPORT`, a macro (shipped in `runtime/verb.h`, header-only, C++ side only) that generates this wrapper for you, so a plain existing C++ function — your own, or a third-party one like `std::sqrt` — can be exposed to Verb with one line and no restated type list:

```cpp
VERB_EXPORT(c_sqrt, 1, static_cast<double(*)(double)>(std::sqrt))
```

Note the cast is required here, not optional flourish: `std::sqrt` is overloaded (`float`/`double`/`long double` plus integral-promoting templates in `<cmath>`), and `auto Fn` non-type template parameters cannot bind to a bare overloaded name — verified empirically during spec review, see the "overload set" row in Error handling. Most real standard-library math functions are overloaded, so this cast is the common case in practice, not the exception — still a one-line win over hand-writing the full wrapper.

This is purely additive. The existing hand-written `extern "C" VerbValue` style keeps working unchanged, side by side, for any case the macro doesn't cover (arity > 6, types outside the supported 4, overloaded callables — see Non-goals). No change to `src/` (parser/codegen/CLI), no change to the `.verb` language, no change to `verb run`'s existing "imports require verb build" restriction.

## Macro API

```cpp
VERB_EXPORT(exported_name, arity, callable)
```

- `exported_name` — bare identifier. Becomes the `extern "C"` symbol name, i.e. the name called from `.verb` source after `import mod`.
- `arity` — literal integer `0`–`6`, the callable's real parameter count. Required because an `extern "C"` function's parameter list is literal C++ syntax — it cannot be deduced from a template at the point the macro expands. Encodes no type information, only a count.
- `callable` — a function name, function pointer, or any expression usable as a C++17 `auto` non-type template parameter that resolves to one concrete (non-overloaded) function. A non-overloaded plain function (`my_add`) can be passed bare. An overloaded name (most `<cmath>` functions, including `std::sqrt`) must be disambiguated with an explicit cast to a concrete function pointer type: `static_cast<double(*)(double)>(std::sqrt)`.

Expansion for `arity == 1`:

```cpp
extern "C" VerbValue exported_name(VerbValue a0) {
    return ::verb_detail::invoke<callable>(a0);
}
```

General shape for `arity == N`: an `extern "C" VerbValue` function taking exactly `N` `VerbValue` parameters (`a0`..`a(N-1)`), body is a single call to `verb_detail::invoke<callable>(a0, ..., a(N-1))`.

## Implementation: `verb_detail` namespace (in `runtime/verb.h`)

All of the following is new content added to `runtime/verb.h`, guarded by `#ifdef __cplusplus` (the file is currently C-compatible; this addition is C++-only and must not break C consumers of the existing `verb_*` functions).

### 1. Type-directed unwrap/wrap

```cpp
namespace verb_detail {

template <typename T> T unwrap(VerbValue v);
template <> inline int64_t     unwrap<int64_t>(VerbValue v)     { return verb_as_int(v); }
template <> inline double      unwrap<double>(VerbValue v)      { return verb_as_float(v); }
template <> inline const char* unwrap<const char*>(VerbValue v) { return verb_as_string(v); }
template <> inline int         unwrap<int>(VerbValue v)         { return verb_as_bool(v); }

template <typename T> VerbValue wrap(T v);
template <> inline VerbValue wrap<int64_t>(int64_t v)     { return verb_int(v); }
template <> inline VerbValue wrap<double>(double v)       { return verb_float(v); }
template <> inline VerbValue wrap<const char*>(const char* v) { return verb_string(v); }
template <> inline VerbValue wrap<int>(int v)             { return verb_bool(v); }

inline VerbValue wrap_void() { return verb_nil(); }

} // namespace verb_detail
```

Only these 4 primary types (`int64_t`, `double`, `const char*`, `int` standing in for Verb's bool — matching `verb_as_bool`/`verb_bool`'s existing `int` signature, NOT C++ `bool`) get `unwrap`/`wrap` specializations. The primary (unspecialized) templates are declared but not defined, so instantiating `unwrap<T>`/`wrap<T>` for any other `T` is a linker/compile error — see "Type check" below for why the *caller* never actually hits this, and gets a clearer `static_assert` instead.

### 2. `function_traits`

```cpp
namespace verb_detail {

template <typename F> struct function_traits;

template <typename R, typename... Args>
struct function_traits<R(Args...)> {
    using return_type = R;
    static constexpr size_t arity = sizeof...(Args);
    template <size_t I> using arg_type = std::tuple_element_t<I, std::tuple<Args...>>;
};

// Function pointer specialization — covers plain functions and most
// callables passed to VERB_EXPORT (`&fn`, bare `fn` decaying to pointer).
template <typename R, typename... Args>
struct function_traits<R(*)(Args...)> : function_traits<R(Args...)> {};

} // namespace verb_detail
```

`decltype(Fn)` where `Fn` is a plain function name decays to a function pointer type `R(*)(Args...)`, handled by the second specialization. Function objects / lambdas are NOT supported (no `operator()` specialization) — see Non-goals.

### 3. `invoke<Fn>` — the type-checked call

```cpp
namespace verb_detail {

template <auto Fn, size_t... I>
VerbValue invoke_impl(std::index_sequence<I...>, /* one VerbValue param per I */);

// Conceptual body (actual code is generated per-arity by the VERB_EXPORT_IMPL_N
// macros below, since the number of VerbValue parameters must be literal):
//
// using traits = function_traits<decltype(Fn)>;
// static_assert(traits::arity == sizeof...(I),
//     "VERB_EXPORT arity does not match callable's parameter count");
// static_assert(/* every arg_type<I> and return_type is one of the 4 supported
//     types */, "VERB_EXPORT: unsupported parameter or return type");
// if constexpr (std::is_void_v<typename traits::return_type>) {
//     Fn(unwrap<typename traits::template arg_type<I>>(aI)...);
//     return wrap_void();
// } else {
//     return wrap<typename traits::return_type>(
//         Fn(unwrap<typename traits::template arg_type<I>>(aI)...));
// }

} // namespace verb_detail
```

The type-support `static_assert` is implemented as a small trait, e.g. `is_supported_verb_type<T>` (true for the 4 types above, false otherwise), folded across all `arg_type<I>` plus `return_type` (or `void`).

Implementers: write `invoke<Fn>` as a real variadic template (`template <auto Fn, typename... Args> VerbValue invoke(Args... args)`) rather than the `index_sequence` sketch above if simpler — the arity/type `static_assert`s must fire from `invoke` itself (checking `sizeof...(Args)` and each `Args` against `function_traits<decltype(Fn)>`), not rely on the per-arity macro expansion to enforce them, so that a mismatched literal `arity` argument in `VERB_EXPORT` is still caught even though the macro-generated wrapper's own parameter count is trivially correct by construction.

### 4. Per-arity macro dispatch

```cpp
#define VERB_EXPORT_IMPL_0(name, fn) \
    extern "C" VerbValue name() { return ::verb_detail::invoke<fn>(); }
#define VERB_EXPORT_IMPL_1(name, fn) \
    extern "C" VerbValue name(VerbValue a0) { return ::verb_detail::invoke<fn>(a0); }
#define VERB_EXPORT_IMPL_2(name, fn) \
    extern "C" VerbValue name(VerbValue a0, VerbValue a1) { return ::verb_detail::invoke<fn>(a0, a1); }
/* ... _3 through _6, same pattern ... */

#define VERB_EXPORT_CONCAT_(a, b) a##b
#define VERB_EXPORT_CONCAT(a, b) VERB_EXPORT_CONCAT_(a, b)
#define VERB_EXPORT(name, arity, fn) \
    VERB_EXPORT_CONCAT(VERB_EXPORT_IMPL_, arity)(name, fn)
```

`arity` must be a literal decimal integer `0`–`6` (token-pasting requires this — an expression like `1+1` will not work, and should not be documented as working).

## Type mapping table

| Verb tag | C++ type | unwrap | wrap |
|---|---|---|---|
| `VERB_INT` | `int64_t` | `verb_as_int` | `verb_int` |
| `VERB_FLOAT` | `double` | `verb_as_float` | `verb_float` |
| `VERB_STRING` | `const char*` | `verb_as_string` | `verb_string` |
| `VERB_BOOL` | `int` | `verb_as_bool` | `verb_bool` |
| — (return only) | `void` | — | `verb_nil()` |

`bool` (the C++ keyword type) is deliberately NOT a supported type — use `int`, matching `verb_as_bool`/`verb_bool`'s existing signatures. A function declared with a `bool` parameter or return type fails the `static_assert`, same as any other unsupported type.

## Error handling

All errors are C++ compile-time, and should point at the `VERB_EXPORT(...)` call site or as close to it as the compiler's template-error reporting allows — no new runtime behavior, no change to `.verb`-side arity checking (that remains Codegen's existing job, unchanged).

| Mistake | Result |
|---|---|
| `arity` doesn't match `Fn`'s real parameter count | `static_assert` failure: "VERB_EXPORT arity does not match callable's parameter count" |
| A parameter or return type isn't one of the 4 supported types | `static_assert` failure naming/identifying the unsupported type |
| `arity` > 6 or not a literal integer | Preprocessor error: no `VERB_EXPORT_IMPL_<arity>` macro exists — hand-write the `extern "C"` wrapper instead |
| `callable` is a bare overloaded name (e.g. `std::sqrt` without a cast) | Compile error at the `auto Fn` template parameter — Clang: "non-type template parameter 'Fn' with type 'auto' has incompatible initializer of type '\<overloaded function type\>'"; GCC gives an equivalent "no matches converting function ... to non-type template" error. Fix by casting to the concrete function pointer type, e.g. `static_cast<double(*)(double)>(std::sqrt)` |
| `callable` is a lambda / function object | Compile error inside `function_traits<decltype(Fn)>` (no matching specialization) — not supported, see Non-goals |

## Testing plan

- Rewrite all three functions in `tests/fixtures/cpp/mathlib.cpp` to use `VERB_EXPORT` instead of hand-written wrappers:
  - `c_sqrt` → `VERB_EXPORT(c_sqrt, 1, static_cast<double(*)(double)>(std::sqrt))` (the cast is required, not optional — see Macro API above).
  - `c_add_int` → write a plain `int64_t add_int(int64_t, int64_t)` function, `VERB_EXPORT(c_add_int, 2, add_int)`.
  - `c_shout` → keep as a plain function (its logic doesn't change), `VERB_EXPORT(c_shout, 1, shout_impl)` (renaming the existing body to `shout_impl` or equivalent).
  - The existing `tests/fixtures/import_mathlib.verb` / `.expected` pair must still pass unchanged — this is the equivalence proof that the macro produces behaviorally identical wrappers.
- Add two new functions to `mathlib.cpp`, both exercising cases `mathlib.cpp` doesn't currently cover:
  - `void say_hello()` — `std::printf("hello from cpp\n")`, exported as `VERB_EXPORT(c_hello, 0, say_hello)`. Covers `void` return AND arity-0 in one fixture. The `.verb` test calls `c_hello();` and the e2e stdout diff (existing harness) asserts `"hello from cpp\n"` appears in the output — an observable, harness-native way to confirm the void path actually ran rather than just compiling.
  - `int is_positive(int64_t n)` — `return n > 0;`, exported as `VERB_EXPORT(c_is_positive, 1, is_positive)`. Covers `VERB_BOOL`. The `.verb` test calls `print(c_is_positive(5));` and `print(c_is_positive(neg 5));` (Verb has no unary minus on numeric literals, only the `neg` prefix keyword); `.expected` asserts on Verb's boolean print representation (whatever `print` already renders for a `VERB_BOOL` value — match the existing convention used elsewhere in `tests/fixtures/`, don't invent a new one).
  - Both go in the same `import_mathlib.verb`/`.expected` fixture pair (extending it) rather than a new file, since they exercise the same shared library — keeps one `build_mathlib_fixture` call covering all cases.
- No changes to `src/` and no new Rust unit tests — this feature is header-only; e2e fixture coverage via the existing `tests/e2e.rs` harness (`build_mathlib_fixture` + `build_and_run_ok`) is sufficient.

## Non-goals (v1)

- Arity > 6.
- Types beyond the 4 in the mapping table (no `float`, `uint64_t`, `bool`, pointers/structs, etc.).
- Lambdas or other function objects as `callable` (function pointers / plain function names only).
- Auto-resolving ambiguous overload sets (must cast explicitly).
- Any change to `verb run`'s "imports require `verb build`" restriction.
- Any change to `src/` (parser, codegen, CLI) — this is entirely a `runtime/verb.h` addition.
- A macro-based path for *declaring* multiple exports at once, or auto-registering them — each `VERB_EXPORT` line is independent, matching `import mod`'s existing one-line-per-thing style.

## Notes for implementers (dev/reviewer split)

This is header-only and self-contained enough for a single-file change (`runtime/verb.h`) plus one test-fixture rewrite (`tests/fixtures/cpp/mathlib.cpp`, possibly a new fixture `.verb`/`.expected` pair for the void/bool cases). Suggested split for a 1-2 dev / 1-2 reviewer team:

- One task: implement the `verb_detail` namespace + `VERB_EXPORT_IMPL_0`..`_6` + `VERB_EXPORT` in `runtime/verb.h`, with the `static_assert`s described above. This is the only task with real design risk (template error messages, `auto` NTTP edge cases with `std::sqrt`) — should not be split further.
- One task (can run in parallel once `verb.h` compiles standalone against a throwaway test `.cpp`): rewrite `mathlib.cpp` to use `VERB_EXPORT`, add the void/bool fixture coverage, confirm `cargo test` (the existing `import_mathlib` e2e test plus new ones) passes.
- Reviewers should specifically check: (1) the `static_assert` messages are actually readable when deliberately triggered (arity mismatch, unsupported type, ambiguous overload) — try each on purpose; (2) `mathlib.cpp` after rewrite produces byte-identical `import_mathlib.expected` output; (3) `runtime/verb.h`'s existing C-compatible section (`verb_nil`, `verb_bool`, etc.) is untouched and the new C++-only section is correctly gated so a plain C compiler including this header still works.
