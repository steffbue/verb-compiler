// Built-in bindings for `import std thread;` -- OS threads, a mutex, and
// a blocking channel. Compiled and linked in automatically by
// `verb build`/`compile` whenever a program uses `import std thread`;
// see docs/superpowers/specs/2026-07-21-std-thread-design.md.
//
// Every handle here (thread/mutex/channel) is a bare `new`'d C++ object
// referenced by a VERB_INT-tagged VerbValue carrying its address as an
// int64 payload -- the same "reuse VERB_INT as an opaque handle" pattern
// runtime/verb_std_io.cpp already uses for POSIX fds. None of these
// handles are refcounted by Verb's GC (they aren't
// STRING/ARRAY/MAP/CLOSURE), so misuse (double join, unlock without
// lock, a bogus handle) is undefined behavior at the C++ level -- the
// same trust level POSIX fd misuse already gets in verb_std_io.cpp.
//
// Only NIL/BOOL/INT/FLOAT VerbValues may cross a thread boundary:
// thread_spawn's closure is always 0-arity (checked by src/codegen.rs
// before thread_spawn_raw is ever called) so it receives no args, and
// channel_send runtime-rejects anything else below. Verb's refcounting
// GC is not thread-safe, so no heap-tagged value is ever allowed to be
// touched by two threads.
#include "verb.h"

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <deque>
#include <mutex>
#include <thread>

namespace {

struct ThreadHandle {
    std::thread t;
};

struct Channel {
    std::mutex m;
    std::condition_variable cv;
    std::deque<VerbValue> q;
};

bool is_primitive(VerbValue v) {
    return v.tag == VERB_NIL || v.tag == VERB_BOOL || v.tag == VERB_INT || v.tag == VERB_FLOAT;
}

// Stores/reads an arbitrary heap pointer in a VERB_INT payload, the same
// memcpy round-trip runtime/verb.h's own verb_map()/verb_as_map() use for
// VERB_MAP -- avoids relying on reinterpret_cast<int64_t> pointer-to-int
// conversion, which verb.h's existing helpers deliberately don't do either.
VerbValue verb_handle(void* p) {
    VerbValue v;
    v.tag = VERB_INT;
    std::memcpy(&v.payload, &p, sizeof(p));
    return v;
}

void* as_handle(VerbValue v) {
    void* p;
    std::memcpy(&p, &v.payload, sizeof(p));
    return p;
}

} // namespace

// The exact signature src/codegen.rs's closure struct's fn_ptr field
// points at: VerbValue(*)(void* env, void* argv). Called here with
// argv=nullptr, valid only because src/codegen.rs's gen_thread_spawn
// checks the closure's arity is 0 before ever calling thread_spawn_raw
// -- a 0-param function body never indexes argv.
using ClosureFn = VerbValue (*)(void*, void*);

extern "C" void* thread_spawn_raw(void* fn_ptr, void* env) {
    auto* h = new ThreadHandle{
        std::thread([fn_ptr, env]() { reinterpret_cast<ClosureFn>(fn_ptr)(env, nullptr); })
    };
    return h;
}

extern "C" VerbValue thread_join(VerbValue handle) {
    auto* h = static_cast<ThreadHandle*>(as_handle(handle));
    h->t.join();
    delete h;
    return verb_nil();
}

extern "C" VerbValue thread_sleep_ms(VerbValue ms) {
    std::this_thread::sleep_for(std::chrono::milliseconds(verb_as_int(ms)));
    return verb_nil();
}

extern "C" VerbValue mutex_new() {
    return verb_handle(new std::mutex());
}

extern "C" VerbValue mutex_lock(VerbValue handle) {
    static_cast<std::mutex*>(as_handle(handle))->lock();
    return verb_nil();
}

extern "C" VerbValue mutex_unlock(VerbValue handle) {
    static_cast<std::mutex*>(as_handle(handle))->unlock();
    return verb_nil();
}

extern "C" VerbValue channel_new() {
    return verb_handle(new Channel());
}

extern "C" VerbValue channel_send(VerbValue handle, VerbValue v) {
    if (!is_primitive(v)) return verb_bool(0);
    auto* c = static_cast<Channel*>(as_handle(handle));
    {
        std::lock_guard<std::mutex> lock(c->m);
        c->q.push_back(v);
    }
    c->cv.notify_one();
    return verb_bool(1);
}

extern "C" VerbValue channel_recv(VerbValue handle) {
    auto* c = static_cast<Channel*>(as_handle(handle));
    std::unique_lock<std::mutex> lock(c->m);
    c->cv.wait(lock, [c]() { return !c->q.empty(); });
    VerbValue v = c->q.front();
    c->q.pop_front();
    return v;
}
