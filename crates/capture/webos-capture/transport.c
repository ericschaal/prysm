#include "transport.h"
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <stddef.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0
#endif

static uint64_t now_us(void)
{
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000000 + (uint64_t)now.tv_nsec / 1000;
}

static int writable(int fd, uint64_t deadline)
{
    for (;;) {
        uint64_t now = now_us();
        if (now >= deadline) {
            errno = ETIMEDOUT;
            return -1;
        }
        struct pollfd descriptor = { .fd = fd, .events = POLLOUT };
        int result = poll(&descriptor, 1, (int)((deadline - now + 999) / 1000));
        if (result > 0)
            return 0;
        if (result < 0 && errno != EINTR)
            return -1;
    }
}

void prysm_sender_close(prysm_sender* sender)
{
    if (sender->fd >= 0)
        close(sender->fd);
    sender->fd = -1;
}

int prysm_sender_init(prysm_sender* sender, const char* address, unsigned port)
{
    *sender = (prysm_sender){ .fd = -1 };
    if (port == 0 || port > 65535 || inet_pton(AF_INET, address, &sender->destination.sin_addr) != 1) {
        errno = EINVAL;
        return -1;
    }
    sender->destination.sin_family = AF_INET;
    sender->destination.sin_port = htons((uint16_t)port);
    return 0;
}

static int connect_receiver(prysm_sender* sender)
{
    sender->fd = socket(AF_INET, SOCK_STREAM, 0);
    if (sender->fd < 0)
        return -1;
    int enabled = 1;
#ifdef SO_NOSIGPIPE
    if (setsockopt(sender->fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled)) < 0)
        return -1;
#endif
    if (setsockopt(sender->fd, IPPROTO_TCP, TCP_NODELAY, &enabled, sizeof(enabled)) < 0
        || fcntl(sender->fd, F_SETFL, O_NONBLOCK) < 0)
        return -1;
    if (connect(sender->fd, (struct sockaddr*)&sender->destination, sizeof(sender->destination)) < 0) {
        if (errno != EINPROGRESS || writable(sender->fd, now_us() + 100000) < 0)
            return -1;
        int error = 0;
        socklen_t length = sizeof(error);
        if (getsockopt(sender->fd, SOL_SOCKET, SO_ERROR, &error, &length) < 0)
            return -1;
        if (error) {
            errno = error;
            return -1;
        }
    }
    return 0;
}

static int send_all(int fd, const uint8_t* bytes, size_t size, uint64_t deadline)
{
    while (size) {
        if (writable(fd, deadline) < 0)
            return -1;
        ssize_t sent = send(fd, bytes, size, MSG_NOSIGNAL);
        if (sent < 0 && (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK))
            continue;
        if (sent <= 0)
            return -1;
        bytes += sent;
        size -= (size_t)sent;
    }
    return 0;
}

int prysm_sender_frame(prysm_sender* sender, unsigned width, unsigned height, const uint8_t* rgb)
{
    if (!rgb || width == 0 || width > 1920 || height == 0 || height > 1080) {
        errno = EINVAL;
        return -1;
    }
    if (sender->fd < 0 && now_us() < sender->retry_after_us) {
        errno = EAGAIN;
        return -1;
    }
    if (sender->fd < 0 && connect_receiver(sender) < 0)
        goto disconnected;
    uint32_t header[] = { htonl(0x50525931), htonl(width), htonl(height), htonl(width * height * 3) };
    uint64_t deadline = now_us() + 100000;
    if (send_all(sender->fd, (const uint8_t*)header, sizeof(header), deadline) < 0
        || send_all(sender->fd, rgb, width * height * 3, deadline) < 0)
        goto disconnected;
    return 0;

disconnected: {
    int error = errno;
    prysm_sender_close(sender);
    sender->retry_after_us = now_us() + 1000000;
    errno = error;
    return -1;
}
}
