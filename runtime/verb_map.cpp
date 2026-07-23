// Built-in bindings for `import std map;` -- a hash-map/dictionary type.
// Compiled and linked in automatically by `verb build`/`compile` whenever
// a program uses `import std map;`; see docs/superpowers/specs/
// 2026-07-21-maps-design.md.
//
// A map is a VERB_MAP-tagged pointer to a heap-allocated VerbMapImpl,
// `new`'d and never freed (no GC in v1, matching every other heap value).
//
// Keys are restricted to nil/bool/int/float/string -- closures and nested
// maps have no sensible hash/equality and are rejected. Numeric keys use
// the same cross-tag equality Verb's own `equals` operator uses elsewhere
// (int 1 and float 1.0 are the same key). String keys compare by content.
// Map values may be any VerbValue, including closures and other maps --
// values are only ever copied, never interpreted.
//
// Invalid usage (non-map `m`, unsupported key type) returns nil/false/0
// rather than aborting, matching the `std io` "nil on failure" convention.
//
// ABI note: every non-static `extern "C"` global symbol in this file is
// force-linked into the `verb` binary and dynamically exported (see
// build.rs), making it part of the JIT-relied-upon ABI surface. Mark
// internal helpers `static` so they aren't accidentally added to it.
#include "verb.h"

#include <cstring>
#include <iterator>
#include <new>
#include <unordered_map>
#include <vector>

// Defined by Verb's own generated LLVM module (src/codegen.rs). GC
// contract: any heap value this file allocates must go through
// verb_alloc, not new/malloc; any VerbValue this file duplicates into a
// second live home (stored in the map AND handed back to the caller, or
// read out of the map as an independent copy) must be retained first.
extern "C" void* verb_alloc(int64_t n);
extern "C" void verb_retain_value(VerbValue v);
extern "C" void verb_release_value(VerbValue v);

namespace {

bool is_valid_key(VerbValue k) {
    return k.tag == VERB_NIL || k.tag == VERB_BOOL || k.tag == VERB_INT
        || k.tag == VERB_FLOAT || k.tag == VERB_STRING;
}

bool is_numeric(VerbValue v) { return v.tag == VERB_INT || v.tag == VERB_FLOAT; }

double as_f64(VerbValue v) {
    if (v.tag == VERB_INT) return static_cast<double>(v.payload);
    return verb_as_float(v);
}

struct KeyHash {
    size_t operator()(const VerbValue& k) const {
        if (k.tag == VERB_STRING) {
            size_t h = 1469598103934665603ULL; // FNV-1a
            for (const char* p = verb_as_string(k); *p; ++p) {
                h ^= static_cast<unsigned char>(*p);
                h *= 1099511628211ULL;
            }
            return h;
        }
        if (is_numeric(k)) return std::hash<double>()(as_f64(k));
        return std::hash<int64_t>()(k.payload) ^ (static_cast<size_t>(k.tag) << 56);
    }
};

struct KeyEq {
    bool operator()(const VerbValue& a, const VerbValue& b) const {
        if (is_numeric(a) && is_numeric(b)) return as_f64(a) == as_f64(b);
        if (a.tag != b.tag) return false;
        if (a.tag == VERB_STRING) return std::strcmp(verb_as_string(a), verb_as_string(b)) == 0;
        return a.payload == b.payload;
    }
};

using VerbMapImpl = std::unordered_map<VerbValue, VerbValue, KeyHash, KeyEq>;

VerbMapImpl* as_impl(VerbValue m) {
    if (m.tag != VERB_MAP) return nullptr;
    return static_cast<VerbMapImpl*>(verb_as_map(m));
}

// Byte-identical mirror of the array header src/codegen.rs's
// Expr::ArrayLit builds inline ({ i64 len, i64 cap, ptr elems }). Verb's
// own array get/len/release code never learns whether an array came from
// a literal or from here, so any drift in this layout corrupts arrays
// map_keys/map_values return.
struct VerbArray {
    int64_t len;
    int64_t cap;
    VerbValue* elems;
};
static_assert(sizeof(VerbValue) == 16,
    "elems stride must match codegen's 16-byte %verb.value slot");
static_assert(sizeof(VerbArray) == 24,
    "array header must match codegen's { i64, i64, ptr } (24 bytes)");

// Builds a fresh Verb array holding `items`, retaining each element: the
// array is a second live home for values the map still holds, so -- like
// map_get handing back a stored value -- each must be retained before the
// array's eventual release cascades a matching release over them. Two
// verb_alloc blocks (header + elems buffer), exactly as Expr::ArrayLit
// emits, so the generated array-release path's two decrements balance.
// An empty array's elems is a plain null, never verb_alloc'd, matching
// Expr::ArrayLit (its release path guards on that null).
VerbValue build_array(const std::vector<VerbValue>& items) {
    auto* hdr = static_cast<VerbArray*>(verb_alloc(sizeof(VerbArray)));
    int64_t n = static_cast<int64_t>(items.size());
    hdr->len = n;
    hdr->cap = n;
    if (n == 0) {
        hdr->elems = nullptr;
    } else {
        hdr->elems = static_cast<VerbValue*>(
            verb_alloc(n * static_cast<int64_t>(sizeof(VerbValue))));
        for (int64_t i = 0; i < n; ++i) {
            verb_retain_value(items[static_cast<size_t>(i)]);
            hdr->elems[i] = items[static_cast<size_t>(i)];
        }
    }
    return verb_array(hdr);
}

} // namespace

extern "C" VerbValue map_new() {
    void* mem = verb_alloc(sizeof(VerbMapImpl));
    new (mem) VerbMapImpl();
    return verb_map(mem);
}

extern "C" VerbValue map_set(VerbValue m, VerbValue k, VerbValue v) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_nil();
    auto it = impl->find(k);
    if (it != impl->end()) {
        // Overwriting an existing key would otherwise silently orphan
        // its old value -- the same leak class as reassigning a cell or
        // global without releasing the old value first.
        verb_release_value(it->second);
    }
    (*impl)[k] = v;
    // `v` is now owned by the map (a second home for the caller's
    // argument), but the generic std-io/std-map argument-release
    // convention will still release the caller's `v` temporary right
    // after this call returns -- it has no way to know this particular
    // function took ownership instead of just reading it. One retain
    // covers that second home, mirroring `m`'s retain below for the
    // same reason (aliased into the return value).
    verb_retain_value(v);
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

extern "C" VerbValue map_has(VerbValue m, VerbValue k) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_bool(0);
    return verb_bool(impl->find(k) != impl->end());
}

extern "C" VerbValue map_remove(VerbValue m, VerbValue k) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || !is_valid_key(k)) return verb_bool(0);
    return verb_bool(impl->erase(k) != 0);
}

extern "C" VerbValue map_len(VerbValue m) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl) return verb_int(0);
    return verb_int(static_cast<int64_t>(impl->size()));
}

// Returns the i-th key in iteration order (std::unordered_map order is
// unspecified but stable between calls when unmodified). O(i) per call via
// std::next -> O(n^2) over a full loop; acceptable for v1's scope. The
// caller (for-each codegen) only ever passes 0 <= i < map_len(m).
extern "C" VerbValue map_key_at(VerbValue m, VerbValue i) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl || i.tag != VERB_INT) return verb_nil();
    int64_t idx = i.payload;
    if (idx < 0 || static_cast<size_t>(idx) >= impl->size()) return verb_nil();
    auto it = std::next(impl->begin(), idx);
    // The map keeps its own stored copy of the key; retain before handing
    // back an independent one, mirroring map_get on the value side.
    verb_retain_value(it->first);
    return it->first;
}

// map_keys / map_values -- snapshot a map's keys (resp. values) into a
// fresh array. Order is unspecified (std::unordered_map iteration order),
// but keys line up with values position-by-position within a single
// snapshot only if the map isn't mutated between the two calls; callers
// wanting paired iteration should not rely on that and should look values
// up by key. A non-map argument returns nil, matching map_get's
// invalid-input convention (not an empty array, which is a valid result
// for an empty map).
extern "C" VerbValue map_keys(VerbValue m) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl) return verb_nil();
    std::vector<VerbValue> items;
    items.reserve(impl->size());
    for (const auto& entry : *impl) items.push_back(entry.first);
    return build_array(items);
}

extern "C" VerbValue map_values(VerbValue m) {
    VerbMapImpl* impl = as_impl(m);
    if (!impl) return verb_nil();
    std::vector<VerbValue> items;
    items.reserve(impl->size());
    for (const auto& entry : *impl) items.push_back(entry.second);
    return build_array(items);
}

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
