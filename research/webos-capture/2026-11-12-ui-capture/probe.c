// Standalone HAL_GAL graphics-plane capture probe (research only).
//
// Measures libhalgal UI capture cost at several output sizes, its CPU share,
// the transparency/change scan, open/close cost, and the jitter a background
// UI thread adds to a simulated 30 fps video loop. It loads the TV's own
// libhalgal at runtime; the surface layout below is the public ABI also used
// by hyperion-webos (MIT) and Prysm's removed 0.1.22 backend.
//
// Usage (root, Prysm disabled so nothing else holds the capture lock):
//   probe serial W H N [dump]   N back-to-back captures, per-frame CSV on stdout
//   probe pace   W H N HZ       N captures paced at HZ on one thread
//   probe thread W H SECS UIHZ  30 fps simulated video loop + UI thread at UIHZ
//                               (UIHZ 0 = baseline without UI thread)
//   probe cycles W H N          N open/capture/close cycles
// SIGTERM/SIGINT stop every loop; surfaces are destroyed and unmapped on exit.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <time.h>
#include <unistd.h>

typedef struct {
    uint32_t offset;
    uint32_t *physicalAddress;
    uint16_t pitch, bpp, width, height, pixelFormat;
    uint32_t paletteInfo[257];
    uint32_t property;
    uint32_t vendorData;
} hal_surface;

static int (*hal_init)(void);
static int (*hal_create)(uint32_t, uint32_t, uint32_t, hal_surface *);
static int (*hal_destroy)(hal_surface *);
static int (*hal_capture)(hal_surface *);

static volatile sig_atomic_t stop_flag;
static void on_signal(int sig) { (void)sig; stop_flag = 1; }

static int64_t now_us(clockid_t clock) {
    struct timespec ts;
    clock_gettime(clock, &ts);
    return (int64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}
#define WALL() now_us(CLOCK_MONOTONIC)
#define TCPU() now_us(CLOCK_THREAD_CPUTIME_ID)

static void sleep_until(int64_t deadline_us) {
    struct timespec ts = { deadline_us / 1000000, (deadline_us % 1000000) * 1000 };
    while (clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &ts, NULL) == EINTR && !stop_flag) {}
}

static int load_hal(void) {
    void *lib = dlopen("libhalgal.so.1", RTLD_NOW); // kept loaded for the process lifetime
    if (!lib) { fprintf(stderr, "dlopen: %s\n", dlerror()); return -1; }
    *(void **)&hal_init = dlsym(lib, "HAL_GAL_Init");
    *(void **)&hal_create = dlsym(lib, "HAL_GAL_CreateSurface");
    *(void **)&hal_destroy = dlsym(lib, "HAL_GAL_DestroySurface");
    *(void **)&hal_capture = dlsym(lib, "HAL_GAL_CaptureFrameBuffer");
    if (!hal_init || !hal_create || !hal_destroy || !hal_capture) { fprintf(stderr, "dlsym failed\n"); return -1; }
    return 0;
}

typedef struct {
    hal_surface surface;
    uint8_t *map;
    size_t len;
    int fd;
    int created;
} ui_tap;

// Init, create and map must run on the thread that captures (EGL affinity).
static int tap_open(ui_tap *t, uint32_t w, uint32_t h, int64_t *open_us) {
    int64_t t0 = WALL();
    memset(t, 0, sizeof(*t));
    t->fd = -1;
    int rc = hal_init();
    if (rc) { fprintf(stderr, "HAL_GAL_Init: %d\n", rc); return -1; }
    rc = hal_create(w, h, 0, &t->surface);
    if (rc) { fprintf(stderr, "HAL_GAL_CreateSurface: %d\n", rc); return -1; }
    t->created = 1;
    t->fd = open("/dev/gfx", O_RDWR);
    if (t->fd < 0) { perror("open /dev/gfx"); return -1; }
    t->len = t->surface.property ? t->surface.property : (size_t)t->surface.pitch * t->surface.height;
    t->map = mmap(NULL, t->len, PROT_READ | PROT_WRITE, MAP_SHARED, t->fd, t->surface.offset);
    if (t->map == MAP_FAILED) { t->map = NULL; perror("mmap"); return -1; }
    if (open_us) *open_us = WALL() - t0;
    return 0;
}

static void tap_close(ui_tap *t) {
    if (t->map) munmap(t->map, t->len);
    if (t->fd >= 0) close(t->fd);
    if (t->created) hal_destroy(&t->surface);
    memset(t, 0, sizeof(*t));
    t->fd = -1;
}

// Count visible (alpha != 0) pixels and hash the visible content (FNV-1a).
static uint32_t scan(const uint8_t *buf, uint32_t w, uint32_t h, uint32_t pitch, uint32_t *visible) {
    uint32_t hash = 2166136261u, count = 0;
    for (uint32_t y = 0; y < h; y++) {
        const uint32_t *row = (const uint32_t *)(buf + (size_t)y * pitch);
        for (uint32_t x = 0; x < w; x++) {
            uint32_t px = row[x];
            if (px >> 24) count++;
            hash = (hash ^ px) * 16777619u;
        }
    }
    *visible = count;
    return hash;
}

static int cmp_i64(const void *a, const void *b) {
    int64_t x = *(const int64_t *)a, y = *(const int64_t *)b;
    return (x > y) - (x < y);
}
static void stats(const char *name, int64_t *v, int n) {
    if (n <= 0) { printf("%s n=0\n", name); return; }
    qsort(v, n, sizeof(*v), cmp_i64);
    double sum = 0;
    for (int i = 0; i < n; i++) sum += v[i];
    printf("%s n=%d mean_us=%.0f p50=%lld p95=%lld p99=%lld max=%lld\n", name, n, sum / n,
        (long long)v[n / 2], (long long)v[(n * 95) / 100], (long long)v[(n * 99) / 100], (long long)v[n - 1]);
}

static int64_t process_cpu_us(void) {
    struct rusage ru;
    getrusage(RUSAGE_SELF, &ru);
    return (int64_t)(ru.ru_utime.tv_sec + ru.ru_stime.tv_sec) * 1000000 + ru.ru_utime.tv_usec + ru.ru_stime.tv_usec;
}

// Premultiplied BGRA (over black) -> limited-range BT.709 NV12, integer math.
// Coefficients are BT.709 scaled by 219/255 (Y) and 224/255 (Cb, Cr), x256.
static void to_nv12(const uint8_t *bgra, uint32_t w, uint32_t h, uint8_t *y_plane, uint8_t *uv_plane) {
    for (uint32_t y = 0; y < h; y++) {
        const uint8_t *row = bgra + (size_t)y * w * 4;
        for (uint32_t x = 0; x < w; x++) {
            int b = row[x * 4], g = row[x * 4 + 1], r = row[x * 4 + 2];
            y_plane[(size_t)y * w + x] = (uint8_t)(16 + ((47 * r + 157 * g + 16 * b) >> 8));
        }
    }
    for (uint32_t y = 0; y < h; y += 2) {
        const uint8_t *row = bgra + (size_t)y * w * 4;
        for (uint32_t x = 0; x < w; x += 2) {
            int b = row[x * 4], g = row[x * 4 + 1], r = row[x * 4 + 2];
            uint8_t *uv = uv_plane + (size_t)(y / 2) * w + x;
            uv[0] = (uint8_t)(128 + ((-26 * r - 87 * g + 112 * b) >> 8));
            uv[1] = (uint8_t)(128 + ((112 * r - 102 * g - 10 * b) >> 8));
        }
    }
}

// ---- serial / pace ---------------------------------------------------------

static int run_captures(uint32_t w, uint32_t h, int n, int hz, int dump) {
    ui_tap t;
    int64_t open_us = 0;
    if (tap_open(&t, w, h, &open_us)) { tap_close(&t); return 1; }
    printf("# surface %ux%u pitch=%u bpp=%u fmt=%u len=%zu open_us=%lld\n", t.surface.width, t.surface.height,
        t.surface.pitch, t.surface.bpp, t.surface.pixelFormat, t.len, (long long)open_us);
    uint8_t *copy = malloc((size_t)w * h * 4);
    int64_t *cap = calloc(n, sizeof(int64_t)), *cpu = calloc(n, sizeof(int64_t)),
            *cpy = calloc(n, sizeof(int64_t)), *scn = calloc(n, sizeof(int64_t)), *cnv = calloc(n, sizeof(int64_t));
    uint8_t *nv12 = malloc((size_t)w * h * 3 / 2);
    int done = 0, changed = 0;
    uint32_t last_hash = 0;
    printf("idx,start_us,capture_us,capture_cpu_us,copy_us,scan_us,visible,hash\n");
    int64_t base = WALL(), cpu0 = process_cpu_us();
    for (int i = 0; i < n && !stop_flag; i++) {
        if (hz > 0) sleep_until(base + (int64_t)i * 1000000 / hz);
        int64_t s = WALL(), sc = TCPU();
        int rc = hal_capture(&t.surface);
        int64_t e = WALL(), ec = TCPU();
        if (rc) { fprintf(stderr, "capture %d: %d\n", i, rc); break; }
        for (uint32_t y = 0; y < h; y++) memcpy(copy + (size_t)y * w * 4, t.map + (size_t)y * t.surface.pitch, (size_t)w * 4);
        int64_t c = WALL();
        uint32_t visible;
        uint32_t hash = scan(copy, w, h, w * 4, &visible);
        int64_t d = WALL();
        to_nv12(copy, w, h, nv12, nv12 + (size_t)w * h);
        cnv[i] = WALL() - d;
        if (i > 0 && hash != last_hash) changed++;
        last_hash = hash;
        cap[i] = e - s; cpu[i] = ec - sc; cpy[i] = c - e; scn[i] = d - c;
        printf("%d,%lld,%lld,%lld,%lld,%lld,%u,%08x\n", i, (long long)(s - base), (long long)cap[i],
            (long long)cpu[i], (long long)cpy[i], (long long)scn[i], visible, hash);
        if (dump && i == n / 2) {
            char path[128];
            snprintf(path, sizeof(path), "frame-%ux%u.bgra", w, h);
            FILE *f = fopen(path, "wb");
            if (f) { fwrite(copy, (size_t)w * h * 4, 1, f); fclose(f); }
        }
        done++;
    }
    int64_t wall = WALL() - base, pcpu = process_cpu_us() - cpu0;
    printf("# %ux%u hz=%d frames=%d changed=%d wall_ms=%lld process_cpu_ms=%lld cpu_share_of_one_core=%.1f%%\n", w, h,
        hz, done, changed, (long long)(wall / 1000), (long long)(pcpu / 1000), wall ? 100.0 * pcpu / wall : 0.0);
    // Skip the first five frames as warm-up.
    int skip = done > 10 ? 5 : 0;
    stats("# capture_wall", cap + skip, done - skip);
    stats("# capture_cpu", cpu + skip, done - skip);
    stats("# copy", cpy + skip, done - skip);
    stats("# scan", scn + skip, done - skip);
    stats("# to_nv12", cnv + skip, done - skip);
    free(copy); free(cap); free(cpu); free(cpy); free(scn); free(cnv); free(nv12);
    tap_close(&t);
    return 0;
}

// ---- open/close cycles ------------------------------------------------------

static int run_cycles(uint32_t w, uint32_t h, int n) {
    int64_t *open_t = calloc(n, sizeof(int64_t)), *first = calloc(n, sizeof(int64_t)), *close_t = calloc(n, sizeof(int64_t));
    int done = 0;
    for (int i = 0; i < n && !stop_flag; i++) {
        ui_tap t;
        if (tap_open(&t, w, h, &open_t[i])) { tap_close(&t); break; }
        int64_t s = WALL();
        int rc = hal_capture(&t.surface);
        first[i] = WALL() - s;
        s = WALL();
        tap_close(&t);
        close_t[i] = WALL() - s;
        if (rc) { fprintf(stderr, "cycle %d capture: %d\n", i, rc); break; }
        done++;
    }
    printf("# cycles %ux%u done=%d\n", w, h, done);
    stats("# open", open_t, done);
    stats("# first_capture", first, done);
    stats("# close", close_t, done);
    return 0;
}

// ---- simulated video loop + UI thread --------------------------------------

typedef struct {
    uint32_t w, h;
    int hz;
    int frames, failed, opened;
    int64_t busy_us, cpu_us;
} ui_args;

static volatile int ui_running;

static void *ui_thread(void *p) {
    ui_args *a = p;
    ui_tap t;
    if (tap_open(&t, a->w, a->h, NULL)) { tap_close(&t); a->failed = 1; return NULL; }
    a->opened = 1;
    uint8_t *copy = malloc((size_t)a->w * a->h * 4);
    int64_t base = WALL(), cpu0 = TCPU();
    for (int i = 0; ui_running && !stop_flag; i++) {
        sleep_until(base + (int64_t)i * 1000000 / a->hz);
        if (!ui_running || stop_flag) break;
        int64_t s = WALL();
        if (hal_capture(&t.surface)) { a->failed = 1; break; }
        for (uint32_t y = 0; y < a->h; y++) memcpy(copy + (size_t)y * a->w * 4, t.map + (size_t)y * t.surface.pitch, (size_t)a->w * 4);
        uint32_t visible;
        (void)scan(copy, a->w, a->h, a->w * 4, &visible);
        a->busy_us += WALL() - s;
        a->frames++;
    }
    a->cpu_us = TCPU() - cpu0;
    free(copy);
    tap_close(&t);
    return NULL;
}

// The "video" loop: every 33.333 ms, copy and average a 640x360 NV12 frame,
// roughly the CPU work of the v1 adapter + sampler per frame.
static int run_thread(uint32_t w, uint32_t h, int secs, int ui_hz) {
    const size_t nv12 = 640 * 360 * 3 / 2;
    uint8_t *src = malloc(nv12), *dst = malloc(nv12);
    memset(src, 0x55, nv12);
    int n = secs * 30;
    int64_t *late = calloc(n, sizeof(int64_t)), *work = calloc(n, sizeof(int64_t)), *interval = calloc(n, sizeof(int64_t));
    pthread_t th;
    ui_args a = { w, h, ui_hz, 0, 0, 0, 0, 0 };
    if (ui_hz > 0) {
        ui_running = 1;
        pthread_create(&th, NULL, ui_thread, &a);
        usleep(300000); // let the UI tap open before the measured window
    }
    int64_t base = WALL(), cpu0 = process_cpu_us(), prev = 0;
    volatile uint32_t sink = 0;
    int done = 0;
    for (int i = 0; i < n && !stop_flag; i++) {
        int64_t deadline = base + (int64_t)i * 1000000 / 30;
        sleep_until(deadline);
        int64_t s = WALL();
        memcpy(dst, src, nv12);
        uint32_t acc = 0;
        for (size_t k = 0; k < nv12; k += 16) acc += dst[k];
        sink += acc;
        int64_t e = WALL();
        late[i] = s - deadline;
        work[i] = e - s;
        interval[i] = i ? s - prev : 33333;
        prev = s;
        done++;
    }
    int64_t wall = WALL() - base, pcpu = process_cpu_us() - cpu0;
    if (ui_hz > 0) {
        ui_running = 0;
        pthread_join(th, NULL);
    }
    printf("# thread ui=%ux%u ui_hz=%d ui_opened=%d ui_failed=%d ui_frames=%d ui_busy_mean_us=%.0f ui_cpu_ms=%lld\n", w, h,
        ui_hz, a.opened, a.failed, a.frames, a.frames ? (double)a.busy_us / a.frames : 0.0, (long long)(a.cpu_us / 1000));
    printf("# video_frames=%d wall_ms=%lld process_cpu_ms=%lld cpu_share_of_one_core=%.1f%%\n", done,
        (long long)(wall / 1000), (long long)(pcpu / 1000), wall ? 100.0 * pcpu / wall : 0.0);
    stats("# video_wake_late", late, done);
    stats("# video_work", work, done);
    stats("# video_interval", interval + 1, done - 1);
    free(src); free(dst); free(late); free(work); free(interval);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 5) { fprintf(stderr, "usage: see source\n"); return 2; }
    struct sigaction sa = { 0 };
    sa.sa_handler = on_signal;
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGINT, &sa, NULL);
    // Refuse to run while another capturer (Prysm) holds the shared lock.
    int lock = open("/tmp/prysm-capture.lock", O_RDWR | O_CREAT, 0644);
    if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB) != 0) { fprintf(stderr, "capture lock busy\n"); return 3; }
    if (load_hal()) return 4;
    uint32_t w = (uint32_t)atoi(argv[2]), h = (uint32_t)atoi(argv[3]);
    int n = atoi(argv[4]);
    int rc = 2;
    if (!strcmp(argv[1], "serial")) rc = run_captures(w, h, n, 0, argc > 5);
    else if (!strcmp(argv[1], "pace") && argc > 5) rc = run_captures(w, h, n, atoi(argv[5]), 0);
    else if (!strcmp(argv[1], "thread") && argc > 5) rc = run_thread(w, h, n, atoi(argv[5]));
    else if (!strcmp(argv[1], "cycles")) rc = run_cycles(w, h, n);
    close(lock);
    return rc;
}
