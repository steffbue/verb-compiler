#include "verb.h"
#include <cmath>
#include <cctype>
#include <cstring>
#include <cstdlib>

extern "C" VerbValue c_sqrt(VerbValue x) {
    return verb_float(std::sqrt(verb_as_float(x)));
}

extern "C" VerbValue c_add_int(VerbValue a, VerbValue b) {
    return verb_int(verb_as_int(a) + verb_as_int(b));
}

extern "C" VerbValue c_shout(VerbValue s) {
    const char* in = verb_as_string(s);
    size_t len = std::strlen(in);
    char* out = static_cast<char*>(std::malloc(len + 2));
    for (size_t i = 0; i < len; i++) {
        out[i] = static_cast<char>(std::toupper(static_cast<unsigned char>(in[i])));
    }
    out[len] = '!';
    out[len + 1] = '\0';
    return verb_string(out);
}
