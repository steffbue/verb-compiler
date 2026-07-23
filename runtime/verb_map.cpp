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
#include "verb.h"

#include <cstring>
#include <iterator>
#include <new>
#include <unordered_map>

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
