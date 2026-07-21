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
#include <unordered_map>

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
    return verb_map(new VerbMapImpl());
}

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
