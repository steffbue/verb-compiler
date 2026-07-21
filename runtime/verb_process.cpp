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

namespace {

// Mirrors the array header src/codegen.rs's Expr::ArrayLit builds:
// 24-byte header { i64 len, i64 cap, ptr elems }, each element a
// 16-byte VerbValue (matches verb.h's VerbValue layout exactly).
struct VerbArrayLayout {
    int64_t len;
    int64_t cap;
    VerbValue* elems;
};

// Builds argv[0] = cmd, argv[1..] = each string element of args (in
// order), argv[N] = nullptr. Returns false (leaving argv untouched) if
// cmd isn't a string or args isn't an array of strings.
bool build_argv(VerbValue cmd, VerbValue args, std::vector<std::string>& storage,
                 std::vector<char*>& argv) {
    if (cmd.tag != VERB_STRING) return false;
    storage.push_back(verb_as_string(cmd));
    if (args.tag != VERB_ARRAY) return false;
    auto* arr = reinterpret_cast<VerbArrayLayout*>(verb_as_map(args));
    for (int64_t i = 0; i < arr->len; ++i) {
        VerbValue elem = arr->elems[i];
        if (elem.tag != VERB_STRING) return false;
        storage.push_back(verb_as_string(elem));
    }
    argv.reserve(storage.size() + 1);
    for (auto& s : storage) argv.push_back(const_cast<char*>(s.c_str()));
    argv.push_back(nullptr);
    return true;
}

} // namespace

#ifdef _WIN32

#include <unordered_map>

namespace {
std::unordered_map<int64_t, HANDLE>& spawned_handles() {
    static std::unordered_map<int64_t, HANDLE> handles;
    return handles;
}
} // namespace

extern "C" VerbValue spawn(VerbValue cmd, VerbValue args) {
    std::vector<std::string> storage;
    std::vector<char*> argv;
    if (!build_argv(cmd, args, storage, argv)) return verb_nil();

    std::string cmdline;
    for (size_t i = 0; i + 1 < argv.size(); ++i) {
        if (i > 0) cmdline.push_back(' ');
        cmdline.push_back('"');
        cmdline += argv[i];
        cmdline.push_back('"');
    }

    STARTUPINFOA si{};
    si.cb = sizeof(si);
    PROCESS_INFORMATION pi{};
    BOOL ok = CreateProcessA(
        nullptr, cmdline.data(), nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi);
    if (!ok) return verb_nil();
    CloseHandle(pi.hThread);
    spawned_handles()[static_cast<int64_t>(pi.dwProcessId)] = pi.hProcess;
    return verb_int(static_cast<int64_t>(pi.dwProcessId));
}

extern "C" VerbValue wait(VerbValue pid) {
    auto& handles = spawned_handles();
    auto it = handles.find(verb_as_int(pid));
    if (it == handles.end()) return verb_nil();
    HANDLE h = it->second;
    handles.erase(it);
    if (WaitForSingleObject(h, INFINITE) != WAIT_OBJECT_0) {
        CloseHandle(h);
        return verb_nil();
    }
    DWORD code = 0;
    BOOL ok = GetExitCodeProcess(h, &code);
    CloseHandle(h);
    if (!ok) return verb_nil();
    return verb_int(static_cast<int64_t>(code));
}

#else

extern "C" VerbValue spawn(VerbValue cmd, VerbValue args) {
    std::vector<std::string> storage;
    std::vector<char*> argv;
    if (!build_argv(cmd, args, storage, argv)) return verb_nil();

    pid_t pid = fork();
    if (pid < 0) return verb_nil();
    if (pid == 0) {
        execvp(argv[0], argv.data());
        _exit(127); // execvp only returns on failure
    }
    return verb_int(static_cast<int64_t>(pid));
}

// The exported symbol MUST be `wait` (codegen's PROCESS_FUNCS declares the
// extern by that exact name). But POSIX's <sys/wait.h> -- which we need for
// waitpid/WIFEXITED/WEXITSTATUS -- already declares `int wait(int*)` with C
// linkage, so a plain `extern "C" VerbValue wait(VerbValue)` is a hard
// redeclaration conflict. Name the C++ function differently and pin its
// emitted symbol to `wait` via an asm label. __USER_LABEL_PREFIX__ supplies
// the target's symbol prefix ("_" on Mach-O/Darwin, empty on ELF), so the
// label matches codegen's reference on every platform without hardcoding it.
#define VERB_SYM_STR2(x) #x
#define VERB_SYM_STR(x) VERB_SYM_STR2(x)
#define VERB_SYM(name) VERB_SYM_STR(__USER_LABEL_PREFIX__) name

extern "C" VerbValue verb_process_wait(VerbValue pid) asm(VERB_SYM("wait"));
VerbValue verb_process_wait(VerbValue pid) {
    int status = 0;
    pid_t result = waitpid(static_cast<pid_t>(verb_as_int(pid)), &status, 0);
    if (result < 0) return verb_nil();
    if (!WIFEXITED(status)) return verb_nil();
    return verb_int(static_cast<int64_t>(WEXITSTATUS(status)));
}

#endif
