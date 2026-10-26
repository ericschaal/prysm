#include "transport.h"
#include "unicapture.h"
#include "log.h"
#include <errno.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int number(const char* value, unsigned max)
{
    char* end;
    errno = 0;
    unsigned long parsed = strtoul(value, &end, 0);
    if (errno || !*value || *end || *value == '-' || parsed > max) {
        fprintf(stderr, "Invalid numeric argument: %s\n", value);
        exit(EXIT_FAILURE);
    }
    return (int)parsed;
}

static int feed(void* data, int width, int height, uint8_t* pixels)
{
    prysm_sender* sender = data;
    int previous_fd = sender->fd;
    int result = prysm_sender_frame(sender, (unsigned)width, (unsigned)height, pixels);
    if (result == 0 && previous_fd < 0)
        INFO("Connected to Prysm");
    else if (result < 0 && (previous_fd >= 0 || errno != EAGAIN))
        WARN("Prysm unavailable: %s", strerror(errno));
    return 0;
}

static int backend(cap_backend_config_t* config, capture_backend_t* capture,
                   const char* selected, const char* const* candidates)
{
    if (strcmp(selected, "auto") == 0)
        return unicapture_try_backends(config, capture, candidates);
    return unicapture_init_backend(config, capture, selected);
}

int main(int argc, char** argv)
{
    const char* address = NULL;
    const char* video = "auto";
    const char* ui = "auto";
    unsigned port = 19400;
    cap_backend_config_t config = { .resolution_width = 320, .resolution_height = 180, .fps = 30 };
    int option;
    while ((option = getopt(argc, argv, "a:p:w:h:f:b:u:q:")) != -1) {
        switch (option) {
        case 'a': address = optarg; break;
        case 'p': port = (unsigned)number(optarg, 65535); break;
        case 'w': config.resolution_width = number(optarg, 1920); break;
        case 'h': config.resolution_height = number(optarg, 1080); break;
        case 'f': config.fps = number(optarg, 60); break;
        case 'b': video = optarg; break;
        case 'u': ui = optarg; break;
        case 'q': config.quirks = number(optarg, INT_MAX); break;
        default: return EXIT_FAILURE;
        }
    }
    prysm_sender sender;
    if (!address || optind != argc || config.fps < 1
        || config.resolution_width < 2 || config.resolution_width % 2
        || config.resolution_height < 2 || config.resolution_height % 2
        || prysm_sender_init(&sender, address, port) < 0) {
        fprintf(stderr, "Usage: %s -a PRYSM_IPV4 [-p 19400] [-w 320] [-h 180] [-f 30]\n"
                        "       [-b auto|VIDEO_BACKEND.so] [-u auto|none|UI_BACKEND.so] [-q QUIRKS]\n", argv[0]);
        return EXIT_FAILURE;
    }
    if (geteuid() != 0) {
        fprintf(stderr, "TV capture requires root.\n");
        return EXIT_FAILURE;
    }
    // Block before creating workers so only the main thread handles shutdown.
    sigset_t signals;
    sigemptyset(&signals);
    sigaddset(&signals, SIGINT);
    sigaddset(&signals, SIGTERM);
    if (pthread_sigmask(SIG_BLOCK, &signals, NULL) != 0)
        return EXIT_FAILURE;

    capture_backend_t video_capture = {0}, ui_capture = {0};
    const char* const video_candidates[] = { "libvtcapture_backend.so", "libdile_vt_backend.so", NULL };
    const char* const ui_candidates[] = { "libgm_backend.so", "libhalgal_backend.so", NULL };
    log_init();
    if (backend(&config, &video_capture, video, video_candidates) != 0) {
        ERR("No usable video backend; check TV compatibility and quirks");
        return EXIT_FAILURE;
    }
    unicapture_state_t capture;
    unicapture_init(&capture);
    capture.video_capture = &video_capture;
    if (strcmp(ui, "none") != 0) {
        if (backend(&config, &ui_capture, ui, ui_candidates) == 0)
            capture.ui_capture = &ui_capture;
        else
            WARN("UI capture unavailable; using video only");
    }
    capture.fps = (uint32_t)config.fps;
    capture.target_format = PIXFMT_RGB;
    capture.callback = feed;
    capture.callback_data = &sender;
    int result = unicapture_start(&capture);
    if (result == 0) {
        int received;
        INFO("Capturing %dx%d at %d fps for %s:%u", config.resolution_width,
             config.resolution_height, config.fps, address, port);
        result = sigwait(&signals, &received);
        unicapture_stop(&capture);
    }
    prysm_sender_close(&sender);
    if (capture.ui_capture) {
        ui_capture.cleanup(ui_capture.state);
        free(ui_capture.name);
    }
    video_capture.cleanup(video_capture.state);
    free(video_capture.name);
    return result == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
