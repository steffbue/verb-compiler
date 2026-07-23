// Verb's C-ABI boundary type. A VerbValue is byte-identical to the LLVM
// struct `%verb.value = { i8, i64 }` that Verb's own compiled code passes
// by value everywhere — this header just gives C++ code the same layout,
// plus constructors/accessors so extern "C" functions can build and read
// Verb values without knowing the tag encoding by heart.
//
// Tag 5 (closure) never crosses this boundary: Verb closures aren't
// representable in C++ and extern fns can't receive or return one.
// Tag 6 (map) crosses fine as an opaque payload -- extern fns that don't
// need to interpret a map's contents (e.g. storing one as another map's
// value) can pass it through untouched; only runtime/verb_map.cpp itself
// dereferences the pointer.
#ifndef VERB_H
#define VERB_H

#include <stdint.h>
#include <string.h>

// Defined by Verb's own generated LLVM module (src/codegen.rs,
// build_alloc_fn): allocates n bytes with an 8-byte GC refcount header
// prefixed, initialized to 1. Any C++ code that hands a heap-owned
// value back to Verb MUST allocate through this, not malloc/new/strdup --
// verb_retain_value/verb_release_value (Task 2) read a refcount at
// ptr-8 for every string/array/map they see, and an unheadered pointer
// there is undefined behavior the first time Verb retains or releases it.
extern "C" void* verb_alloc(int64_t n);

typedef struct { int8_t tag; int64_t payload; } VerbValue;

enum {
    VERB_NIL = 0,
    VERB_BOOL = 1,
    VERB_INT = 2,
    VERB_FLOAT = 3,
    VERB_STRING = 4,
    VERB_MAP = 6,
    VERB_ARRAY = 7,
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
static inline VerbValue verb_map(void* p) {
    VerbValue v; v.tag = VERB_MAP; memcpy(&v.payload, &p, sizeof(p)); return v;
}
// Tags a pointer to an array header (the { i64 len, i64 cap, ptr elems }
// block src/codegen.rs's Expr::ArrayLit builds). Like verb_map, this only
// stamps the tag -- the header itself must be verb_alloc'd with the exact
// layout Verb's own array get/len/release code expects; see
// runtime/verb_map.cpp's build_array for the one place that does so.
static inline VerbValue verb_array(void* p) {
    VerbValue v; v.tag = VERB_ARRAY; memcpy(&v.payload, &p, sizeof(p)); return v;
}

static inline int verb_is(VerbValue v, int tag) { return v.tag == tag; }

static inline int64_t verb_as_int(VerbValue v) { return v.payload; }
static inline double verb_as_float(VerbValue v) {
    double d; memcpy(&d, &v.payload, sizeof(d)); return d;
}
static inline const char* verb_as_string(VerbValue v) {
    const char* s; memcpy(&s, &v.payload, sizeof(s)); return s;
}
static inline void* verb_as_map(VerbValue v) {
    void* p; memcpy(&p, &v.payload, sizeof(p)); return p;
}
static inline void* verb_as_array(VerbValue v) {
    void* p; memcpy(&p, &v.payload, sizeof(p)); return p;
}
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
    if constexpr (traits::arity == sizeof...(As)) {
        static_assert(all_args_supported<traits>(std::make_index_sequence<traits::arity>{}),
            "VERB_EXPORT: unsupported parameter type");
        static_assert(is_supported_return<typename traits::return_type>::value,
            "VERB_EXPORT: unsupported return type");
        std::tuple<As...> args(as...);
        return invoke_call<Fn, traits>(args, std::make_index_sequence<sizeof...(As)>{});
    } else {
        return VerbValue{};
    }
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
