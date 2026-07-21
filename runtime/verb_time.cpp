// Built-in bindings for `import std time;` -- wall-clock/monotonic
// millisecond timestamps and a blocking sleep. Compiled and linked in
// automatically by `verb build`/`compile` whenever a program uses
// `import std time;`, the same opt-in mechanism as `import std io;`/
// `import std map;`.
//
// Unlike `std io`/`std map`, none of these functions touch a VerbValue
// tag directly -- plain int64_t in, int64_t/nothing out -- so they're
// defined with VERB_EXPORT (see runtime/verb.h and docs/superpowers/
// specs/2026-07-20-verb-export-macro-design.md) instead of hand-written
// extern "C" VerbValue wrappers.
#include "verb.h"

#include <chrono>
#include <thread>

namespace {

int64_t now_ms_impl() {
    using namespace std::chrono;
    return duration_cast<milliseconds>(system_clock::now().time_since_epoch()).count();
}

int64_t monotonic_ms_impl() {
    using namespace std::chrono;
    return duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count();
}

// Negative/zero durations are a no-op rather than an error, matching the
// "degrade gracefully instead of aborting" convention `std io`/`std map`
// already use for invalid input.
void sleep_ms_impl(int64_t ms) {
    if (ms > 0) std::this_thread::sleep_for(std::chrono::milliseconds(ms));
}

} // namespace

VERB_EXPORT(now_ms, 0, now_ms_impl)
VERB_EXPORT(monotonic_ms, 0, monotonic_ms_impl)
VERB_EXPORT(sleep_ms, 1, sleep_ms_impl)
