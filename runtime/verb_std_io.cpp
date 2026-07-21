// Built-in bindings for `import std io;` -- stdin, whole-file
// read/write, and blocking TCP sockets. Compiled and linked in
// automatically by `verb build`/`compile` whenever a program uses
// `import std io;`; unlike the generic `import mod` mechanism, the
// user never writes or links this file themselves.
//
// Every function returns verb_nil() on failure -- no C++ exception
// ever crosses the extern "C" boundary. File/socket handles reuse the
// existing VERB_INT tag (a POSIX fd is already an integer).
#include "verb.h"

// Defined by Verb's own generated LLVM module (src/codegen.rs).
extern "C" void* verb_alloc(int64_t n);

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#include <netdb.h>
#include <sys/socket.h>
#include <unistd.h>

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

extern "C" VerbValue read_line() {
    std::string line;
    int c = std::getchar();
    if (c == EOF) return verb_nil();
    while (c != EOF && c != '\n') {
        line.push_back(static_cast<char>(c));
        c = std::getchar();
    }
    return verb_string_from(line);
}

extern "C" VerbValue file_read(VerbValue path) {
    FILE* f = std::fopen(verb_as_string(path), "rb");
    if (!f) return verb_nil();
    std::fseek(f, 0, SEEK_END);
    long size = std::ftell(f);
    if (size < 0) { std::fclose(f); return verb_nil(); }
    std::fseek(f, 0, SEEK_SET);
    char* buf = static_cast<char*>(verb_alloc(static_cast<int64_t>(size) + 1));
    if (!buf) { std::fclose(f); return verb_nil(); }
    size_t got = std::fread(buf, 1, static_cast<size_t>(size), f);
    std::fclose(f);
    buf[got] = '\0';
    return verb_string(buf);
}

static VerbValue write_file(const char* path, const char* mode, VerbValue contents) {
    FILE* f = std::fopen(path, mode);
    if (!f) return verb_nil();
    const char* s = verb_as_string(contents);
    size_t len = std::strlen(s);
    size_t written = std::fwrite(s, 1, len, f);
    std::fclose(f);
    if (written != len) return verb_nil();
    return verb_bool(1);
}

extern "C" VerbValue file_write(VerbValue path, VerbValue contents) {
    return write_file(verb_as_string(path), "wb", contents);
}

extern "C" VerbValue file_append(VerbValue path, VerbValue contents) {
    return write_file(verb_as_string(path), "ab", contents);
}

// Tries each candidate address in turn, handing the caller a bound socket fd
// to attempt (connect, or bind+listen); closes and moves on if the attempt
// fails. Returns the first fd the callback accepts, or -1 if none work.
template <typename Attempt>
static int connect_first_working(addrinfo* res, Attempt attempt) {
    for (addrinfo* p = res; p != nullptr; p = p->ai_next) {
        int fd = socket(p->ai_family, p->ai_socktype, p->ai_protocol);
        if (fd == -1) continue;
        if (attempt(fd, p)) return fd;
        close(fd);
    }
    return -1;
}

extern "C" VerbValue tcp_connect(VerbValue host, VerbValue port) {
    std::string port_str = std::to_string(verb_as_int(port));
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* res = nullptr;
    if (getaddrinfo(verb_as_string(host), port_str.c_str(), &hints, &res) != 0) {
        return verb_nil();
    }
    int fd = connect_first_working(res, [](int fd, addrinfo* p) {
        return connect(fd, p->ai_addr, p->ai_addrlen) == 0;
    });
    freeaddrinfo(res);
    if (fd == -1) return verb_nil();
    return verb_int(fd);
}

extern "C" VerbValue tcp_listen(VerbValue port) {
    std::string port_str = std::to_string(verb_as_int(port));
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;
    addrinfo* res = nullptr;
    if (getaddrinfo(nullptr, port_str.c_str(), &hints, &res) != 0) {
        return verb_nil();
    }
    int fd = connect_first_working(res, [](int fd, addrinfo* p) {
        int yes = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));
        return bind(fd, p->ai_addr, p->ai_addrlen) == 0;
    });
    freeaddrinfo(res);
    if (fd == -1) return verb_nil();
    if (listen(fd, 16) != 0) {
        close(fd);
        return verb_nil();
    }
    return verb_int(fd);
}

extern "C" VerbValue tcp_accept(VerbValue fd) {
    int client = accept(static_cast<int>(verb_as_int(fd)), nullptr, nullptr);
    if (client == -1) return verb_nil();
    return verb_int(client);
}

extern "C" VerbValue send_line(VerbValue fd, VerbValue s) {
    std::string line = verb_as_string(s);
    line.push_back('\n');
    int sock = static_cast<int>(verb_as_int(fd));
    size_t sent_total = 0;
    while (sent_total < line.size()) {
        ssize_t n = send(sock, line.data() + sent_total, line.size() - sent_total, 0);
        if (n <= 0) return verb_nil();
        sent_total += static_cast<size_t>(n);
    }
    return verb_bool(1);
}

extern "C" VerbValue recv_line(VerbValue fd) {
    int sock = static_cast<int>(verb_as_int(fd));
    std::string line;
    char c;
    while (true) {
        ssize_t n = recv(sock, &c, 1, 0);
        if (n <= 0) {
            if (line.empty()) return verb_nil();
            break;
        }
        if (c == '\n') break;
        line.push_back(c);
    }
    return verb_string_from(line);
}

extern "C" VerbValue close_conn(VerbValue fd) {
    close(static_cast<int>(verb_as_int(fd)));
    return verb_nil();
}
