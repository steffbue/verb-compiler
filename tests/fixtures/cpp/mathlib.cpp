#include "verb.h"
#include <cmath>
#include <cctype>
#include <cstring>
#include <cstdlib>
#include <cstdio>

// Returns a malloc'd buffer. Verb defensively copies `const char*` returns
// into a verb_alloc'd block (see wrap<const char*> in runtime/verb.h), so
// this original buffer is never freed and leaks — acceptable at the FFI
// boundary, and invisible to the verb_gc_live leak counter.
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
