// Built-in bindings for `import std env;` -- getenv/setenv/unsetenv.
// Compiled and linked in automatically by `verb build`/`compile` whenever
// a program uses `import std env;`. Mirrors runtime/verb_std_io.cpp's
// shape: build-only, never linked into `verb run` (JIT).
#include "verb.h"

#include <cstdlib>
#include <cstring>
#include <string>

#ifdef _WIN32
#include <stdlib.h> // _putenv_s / _dupenv_s
#endif

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

// Named env_get/env_set/env_unset rather than getenv/setenv/unsetenv:
// those names collide at C-linkage level with libc's own getenv/setenv/
// unsetenv declared by <cstdlib> (extern "C" functions share one flat
// symbol table regardless of return type or C++ overload rules), which
// makes this translation unit fail to compile if it tries to both call
// and re-export those exact names. Matches std io's own naming
// convention anyway (file_read, not fread).
extern "C" VerbValue env_get(VerbValue name) {
    if (name.tag != VERB_STRING) return verb_nil();
#ifdef _WIN32
    char* buf = nullptr;
    size_t len = 0;
    if (_dupenv_s(&buf, &len, verb_as_string(name)) != 0 || !buf) return verb_nil();
    VerbValue v = verb_string_from(std::string(buf, len > 0 ? len - 1 : 0));
    free(buf);
    return v;
#else
    const char* v = std::getenv(verb_as_string(name));
    if (!v) return verb_nil();
    return verb_string_from(v);
#endif
}

extern "C" VerbValue env_set(VerbValue name, VerbValue value) {
    if (name.tag != VERB_STRING || value.tag != VERB_STRING) return verb_bool(0);
#ifdef _WIN32
    bool ok = _putenv_s(verb_as_string(name), verb_as_string(value)) == 0;
#else
    bool ok = ::setenv(verb_as_string(name), verb_as_string(value), 1) == 0;
#endif
    return verb_bool(ok);
}

extern "C" VerbValue env_unset(VerbValue name) {
    if (name.tag != VERB_STRING) return verb_bool(0);
#ifdef _WIN32
    bool ok = _putenv_s(verb_as_string(name), "") == 0;
#else
    bool ok = ::unsetenv(verb_as_string(name)) == 0;
#endif
    return verb_bool(ok);
}
