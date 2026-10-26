#include "transport.h"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

int main(int argc, char** argv)
{
    assert(argc == 3);
    unsigned port = (unsigned)strtoul(argv[1], NULL, 10);
    prysm_sender sender;
    assert(prysm_sender_init(&sender, "invalid", port) < 0);
    assert(prysm_sender_init(&sender, "127.0.0.1", 0) < 0);
    assert(prysm_sender_init(&sender, "127.0.0.1", port) == 0);
    const uint8_t pixels[] = { 12, 34, 56, 78, 90, 123 };
    assert(prysm_sender_frame(&sender, 0, 1, pixels) < 0);
    assert(prysm_sender_frame(&sender, 1921, 1, pixels) < 0);
    assert(prysm_sender_frame(&sender, 1, 1081, pixels) < 0);
    assert(prysm_sender_frame(&sender, 1, 1, NULL) < 0);
    if (strcmp(argv[2], "blocked") == 0) {
        uint8_t* large = calloc(1920 * 1080, 3);
        assert(large);
        assert(prysm_sender_frame(&sender, 1920, 1080, large) < 0);
        free(large);
    } else {
        if (strcmp(argv[2], "retry") == 0) {
            assert(prysm_sender_frame(&sender, 2, 1, pixels) < 0);
            puts("retry");
            fflush(stdout);
        }
        unsigned sent = 0;
        for (unsigned attempt = 0; attempt < 60 && sent < 2; attempt++) {
            if (prysm_sender_frame(&sender, 2, 1, pixels) == 0)
                sent++;
            struct timespec delay = { .tv_nsec = 50000000 };
            nanosleep(&delay, NULL);
        }
        assert(sent == 2);
    }
    prysm_sender_close(&sender);
    return 0;
}
