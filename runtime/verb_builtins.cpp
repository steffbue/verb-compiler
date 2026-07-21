// Core builtins that need no `import`: exit, abort, get_pid. Unlike
// verb_std_io.cpp/verb_env.cpp/verb_process.cpp (build-only, linked only
// when their `std` module is imported), this unit is *always* linked --
// see build.rs and src/main.rs's build_aot_host/build_aot_cross, which
// treat it the same way they already treat verb_map.cpp -- and always
// compiled into the `verb` binary itself so `verb run` (JIT) can resolve
// these symbols too, since exit/abort/get_pid must work without any
// import, exactly like `print` does.
#include "verb.h"

#include <cstdlib>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

extern "C" VerbValue builtin_exit(VerbValue code) {
    // Deliberately skips GC/refcount cleanup -- matches C's exit()
    // semantics exactly (see design spec, D-09). Never returns.
    std::exit(static_cast<int>(verb_as_int(code)));
}

extern "C" VerbValue builtin_abort() {
    // Hard SIGABRT-style crash, not a "friendly" Verb-level panic
    // (design spec, D-10). Never returns.
    std::abort();
}

extern "C" VerbValue builtin_get_pid() {
#ifdef _WIN32
    return verb_int(static_cast<int64_t>(GetCurrentProcessId()));
#else
    return verb_int(static_cast<int64_t>(getpid()));
#endif
}
