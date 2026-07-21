// Built-in bindings for `import std process;` -- cwd/exe_path/spawn/wait.
// Compiled and linked in automatically by `verb build`/`compile` whenever
// a program uses `import std process;`. Mirrors runtime/verb_std_io.cpp's
// shape: build-only, never linked into `verb run` (JIT).
#include "verb.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#include <sys/wait.h>
#include <climits>
#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif
#endif

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue cwd() {
    char buf[4096];
#ifdef _WIN32
    DWORD n = GetCurrentDirectoryA(sizeof(buf), buf);
    if (n == 0 || n >= sizeof(buf)) return verb_nil();
    return verb_string_from(std::string(buf, n));
#else
    if (!getcwd(buf, sizeof(buf))) return verb_nil();
    return verb_string_from(buf);
#endif
}

extern "C" VerbValue exe_path() {
#ifdef _WIN32
    char buf[MAX_PATH];
    DWORD n = GetModuleFileNameA(nullptr, buf, MAX_PATH);
    if (n == 0) return verb_nil();
    return verb_string_from(std::string(buf, n));
#elif defined(__APPLE__)
    char buf[4096];
    uint32_t size = sizeof(buf);
    if (_NSGetExecutablePath(buf, &size) != 0) return verb_nil();
    return verb_string_from(buf);
#else
    char buf[4096];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    if (n <= 0) return verb_nil();
    buf[n] = '\0';
    return verb_string_from(buf);
#endif
}
