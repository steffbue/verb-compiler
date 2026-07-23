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
#include <ctime>
#include <thread>

#if defined(_WIN32)
#include <windows.h>
#endif

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

// CPU time consumed by this process (not wall-clock -- doesn't advance
// while the process is blocked/sleeping), the same quantity C's clock()
// reports, in milliseconds rather than clock_t ticks.
int64_t clock_ms_impl() {
    return static_cast<int64_t>(std::clock()) * 1000 / CLOCKS_PER_SEC;
}

// Verb has no dedicated subtraction-of-timestamps syntax gap to fill --
// `b sub a` already does this -- but C's difftime is a familiar enough
// name to offer directly rather than making every caller remember which
// operand order `sub` takes.
int64_t difftime_ms_impl(int64_t later, int64_t earlier) {
    return later - earlier;
}

#if defined(__linux__)
// Direct Linux syscalls rather than the <chrono>/<thread> wrappers above,
// for callers who want nanosecond precision or a specific clock id.
// clock_id matches the raw Linux clockid_t values (see time.h): 0 =
// CLOCK_REALTIME, 1 = CLOCK_MONOTONIC. Only compiled when targeting
// Linux -- calling these from a non-Linux build is a link error, same
// "footgun accepted" tradeoff `import mod` externs already have.
int64_t linux_clock_gettime_ns_impl(int64_t clock_id) {
    struct timespec ts;
    clock_gettime(static_cast<clockid_t>(clock_id), &ts);
    return static_cast<int64_t>(ts.tv_sec) * 1000000000LL + ts.tv_nsec;
}

void linux_nanosleep_ns_impl(int64_t ns) {
    if (ns <= 0) return;
    struct timespec ts;
    ts.tv_sec = static_cast<time_t>(ns / 1000000000LL);
    ts.tv_nsec = static_cast<long>(ns % 1000000000LL);
    nanosleep(&ts, nullptr);
}
#elif defined(_WIN32)
// Direct Windows APIs, mirroring the Linux block above. Only compiled
// when targeting Windows.
int64_t win_filetime_100ns_impl() {
    FILETIME ft;
    GetSystemTimeAsFileTime(&ft);
    ULARGE_INTEGER uli;
    uli.LowPart = ft.dwLowDateTime;
    uli.HighPart = ft.dwHighDateTime;
    return static_cast<int64_t>(uli.QuadPart);
}

void win_sleep_ms_impl(int64_t ms) {
    if (ms > 0) Sleep(static_cast<DWORD>(ms));
}
#endif

} // namespace

VERB_EXPORT(now_ms, 0, now_ms_impl)
VERB_EXPORT(monotonic_ms, 0, monotonic_ms_impl)
VERB_EXPORT(sleep_ms, 1, sleep_ms_impl)
VERB_EXPORT(clock_ms, 0, clock_ms_impl)
VERB_EXPORT(difftime_ms, 2, difftime_ms_impl)

#if defined(__linux__)
VERB_EXPORT(linux_clock_gettime_ns, 1, linux_clock_gettime_ns_impl)
VERB_EXPORT(linux_nanosleep_ns, 1, linux_nanosleep_ns_impl)
#elif defined(_WIN32)
VERB_EXPORT(win_filetime_100ns, 0, win_filetime_100ns_impl)
VERB_EXPORT(win_sleep_ms, 1, win_sleep_ms_impl)
#endif
