// Verb's C-ABI boundary type. A VerbValue is byte-identical to the LLVM
// struct `%verb.value = { i8, i64 }` that Verb's own compiled code passes
// by value everywhere — this header just gives C++ code the same layout,
// plus constructors/accessors so extern "C" functions can build and read
// Verb values without knowing the tag encoding by heart.
//
// Tag 5 (closure) never crosses this boundary: Verb closures aren't
// representable in C++ and extern fns can't receive or return one.
#ifndef VERB_H
#define VERB_H

#include <stdint.h>
#include <string.h>

typedef struct { int8_t tag; int64_t payload; } VerbValue;

enum {
    VERB_NIL = 0,
    VERB_BOOL = 1,
    VERB_INT = 2,
    VERB_FLOAT = 3,
    VERB_STRING = 4,
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

static inline int verb_is(VerbValue v, int tag) { return v.tag == tag; }

static inline int64_t verb_as_int(VerbValue v) { return v.payload; }
static inline double verb_as_float(VerbValue v) {
    double d; memcpy(&d, &v.payload, sizeof(d)); return d;
}
static inline const char* verb_as_string(VerbValue v) {
    const char* s; memcpy(&s, &v.payload, sizeof(s)); return s;
}
static inline int verb_as_bool(VerbValue v) { return v.payload != 0; }

#endif // VERB_H
