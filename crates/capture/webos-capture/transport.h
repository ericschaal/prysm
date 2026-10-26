#pragma once
#include <netinet/in.h>
#include <stdint.h>

typedef struct {
    int fd;
    struct sockaddr_in destination;
    uint64_t retry_after_us;
} prysm_sender;

int prysm_sender_init(prysm_sender* sender, const char* address, unsigned port);
int prysm_sender_frame(prysm_sender* sender, unsigned width, unsigned height, const uint8_t* rgb);
void prysm_sender_close(prysm_sender* sender);
