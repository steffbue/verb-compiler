// Built-in bindings for `import std net;` -- blocking UDP datagram
// sockets over POSIX. Compiled and linked in automatically by `verb
// build`/`compile` whenever a program uses `import std net;`; unlike the
// generic `import mod` mechanism, the user never writes or links this
// file themselves.
//
// Every function returns verb_nil() on failure -- no C++ exception ever
// crosses the extern "C" boundary. Socket handles reuse the existing
// VERB_INT tag (a POSIX fd is already an integer), exactly as std io's
// TCP helpers do.
#include "verb.h"

// Defined by Verb's own generated LLVM module (src/codegen.rs).
extern "C" void* verb_alloc(int64_t n);

#include <cstdint>
#include <cstring>
#include <string>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

static VerbValue verb_string_from(const std::string& s) {
    char* out = static_cast<char*>(verb_alloc(static_cast<int64_t>(s.size() + 1)));
    if (!out) return verb_nil();
    std::memcpy(out, s.data(), s.size());
    out[s.size()] = '\0';
    return verb_string(out);
}

// Creates an unbound IPv4 UDP socket, returning its fd. Returns nil on
// failure.
extern "C" VerbValue udp_socket() {
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd == -1) return verb_nil();
    return verb_int(fd);
}

// Binds an existing UDP socket to the given local port on all interfaces.
// Returns true on success, nil on failure.
extern "C" VerbValue udp_bind(VerbValue fd, VerbValue port) {
    int sock = static_cast<int>(verb_as_int(fd));
    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    addr.sin_port = htons(static_cast<uint16_t>(verb_as_int(port)));
    if (bind(sock, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        return verb_nil();
    }
    return verb_bool(1);
}

// Sends `data` as a single datagram to host:port. `host` must be a
// numeric IPv4 dotted-quad (e.g. "127.0.0.1"). Returns true on success,
// nil on failure or short write.
extern "C" VerbValue udp_send(VerbValue fd, VerbValue host, VerbValue port, VerbValue data) {
    int sock = static_cast<int>(verb_as_int(fd));
    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_port = htons(static_cast<uint16_t>(verb_as_int(port)));
    if (inet_pton(AF_INET, verb_as_string(host), &addr.sin_addr) != 1) {
        return verb_nil();
    }
    const char* s = verb_as_string(data);
    size_t len = std::strlen(s);
    ssize_t n = sendto(sock, s, len, 0, reinterpret_cast<sockaddr*>(&addr), sizeof(addr));
    if (n < 0 || static_cast<size_t>(n) != len) return verb_nil();
    return verb_bool(1);
}

// Blocks for a single incoming datagram and returns its payload as a
// string. Returns nil on failure.
extern "C" VerbValue udp_recv(VerbValue fd) {
    int sock = static_cast<int>(verb_as_int(fd));
    char buf[65536];
    ssize_t n = recvfrom(sock, buf, sizeof(buf), 0, nullptr, nullptr);
    if (n < 0) return verb_nil();
    return verb_string_from(std::string(buf, static_cast<size_t>(n)));
}
