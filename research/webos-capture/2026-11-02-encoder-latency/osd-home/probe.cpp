// Bounded research probe; uses the same headers, capture lock and lifecycle as Prysm.
#include "vtCaptureApi_c.h"
#include <dlfcn.h>
#include <sys/file.h>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <chrono>
#include <stdexcept>
#include <vector>
#include <string>

using Clock = std::chrono::steady_clock;
static double ms(Clock::time_point a, Clock::time_point b) {
    return std::chrono::duration<double, std::milli>(b-a).count();
}
static void check(const char *name, int code) {
    fprintf(stderr, "CALL %s %d\n", name, code);
    if (code) throw std::runtime_error(name);
}
template<class T> static T sym(void *lib, const char *name) {
    auto p = reinterpret_cast<T>(dlsym(lib, name));
    if (!p) throw std::runtime_error(name);
    return p;
}
static void save(const std::string &path, const std::vector<unsigned char> &data) {
    FILE *f = fopen(path.c_str(), "wb");
    if (!f) throw std::runtime_error("open output");
    bool ok = fwrite(data.data(), 1, data.size(), f) == data.size();
    ok = fclose(f) == 0 && ok;
    if (!ok) throw std::runtime_error("write output");
}
int main(int argc, char **argv) {
    if (argc != 3 || strlen(argv[1]) != 1 || argv[1][0] < '2' || argv[1][0] > '4') return 2;
    int mode = argv[1][0]-'0';
    int lock = open("/tmp/prysm-capture.lock", O_CREAT|O_RDWR|O_NOFOLLOW, 0600);
    if (lock < 0 || flock(lock, LOCK_EX|LOCK_NB)) return 3;
    VT_DRIVER *driver = nullptr;
    char client[128] = {'0', '0'};
    int stage = 0, result = 0;
    int (*stop)(VT_DRIVER*,char*) = nullptr, (*post)(VT_DRIVER*,char*) = nullptr;
    int (*finalize)(VT_DRIVER*,char*) = nullptr;
    void (*release)(VT_DRIVER*) = nullptr;
    try {
        void *lib = dlopen("libvtcapture.so.1", RTLD_NOW|RTLD_GLOBAL);
        if (!lib) throw std::runtime_error(dlerror());
        auto create = sym<VT_DRIVER*(*)()>(lib, "vtCapture_create");
        auto init = sym<int(*)(VT_DRIVER*,const char*,char*)>(lib, "vtCapture_init");
        auto prepare = sym<int(*)(VT_DRIVER*,char*,_LibVtCaptureProperties*)>(lib, "vtCapture_preprocess");
        auto plane = sym<int(*)(VT_DRIVER*,char*,_LibVtCapturePlaneInfo*)>(lib, "vtCapture_planeInfo");
        auto process = sym<int(*)(VT_DRIVER*,char*)>(lib, "vtCapture_process");
        auto buffer = sym<int(*)(VT_DRIVER*,_LibVtCaptureBufferInfo*)>(lib, "vtCapture_currentCaptureBuffInfo");
        stop = sym<decltype(stop)>(lib, "vtCapture_stop");
        post = sym<decltype(post)>(lib, "vtCapture_postprocess");
        finalize = sym<decltype(finalize)>(lib, "vtCapture_finalize");
        release = sym<decltype(release)>(lib, "vtCapture_release");
        driver = create();
        if (!driver) throw std::runtime_error("null driver");
        check("init", init(driver, "prysm-webos-capture", client)); stage = 1;
        _LibVtCaptureProperties props = {};
        props.dump = mode; props.reg.w = 640; props.reg.h = 360; props.buf_cnt = 3; props.frm = 30;
        int prepared = 1;
        for (int attempt = 0; attempt < 11; ++attempt) {
            prepared = prepare(driver, client, &props);
            fprintf(stderr,"PREPARE attempt=%d code=%d\n", attempt+1, prepared);
            if (prepared != 1 || attempt == 10) break;
            usleep(1000000);
        }
        check("preprocess", prepared); stage = 2;
        _LibVtCapturePlaneInfo info = {};
        check("plane", plane(driver, client, &info));
        fprintf(stderr, "PLANE mode=%d stride=%d region=%d,%d,%d,%d\n", mode, info.stride,
                info.planeregion.a, info.planeregion.b, info.planeregion.c, info.planeregion.d);
        if (info.planeregion.c != 640 || info.planeregion.d != 360 || info.stride < 640 || info.stride > 16384)
            throw std::runtime_error("invalid plane geometry");
        stage = 3; check("process", process(driver, client));
        std::vector<unsigned char> nv12(640*360*3/2);
        FILE *csv = fopen((std::string(argv[2])+".csv").c_str(), "w");
        if (!csv) throw std::runtime_error("open CSV");
        fprintf(csv,"frame,video_ms\n");
        auto started = Clock::now();
        int frames = 0;
        while (frames < 90 && ms(started,Clock::now()) < 6000) {
            auto a = Clock::now();
            _LibVtCaptureBufferInfo b = {};
            int code = buffer(driver,&b);
            if (code == 2) { usleep(5000); continue; }
            if (code) check("buffer",code);
            if (!b.start_addr0 || !b.start_addr1 || b.size0 < info.stride*359+640 ||
                b.size1 < info.stride*179+640 || b.size0 > 64*1024*1024 || b.size1 > 64*1024*1024)
                throw std::runtime_error("invalid NV12-compatible buffer sizes");
            for (int y=0;y<360;y++) memcpy(nv12.data()+640*y,b.start_addr0+info.stride*y,640);
            for (int y=0;y<180;y++) memcpy(nv12.data()+640*360+640*y,b.start_addr1+info.stride*y,640);
            auto btime = Clock::now();
            fprintf(csv,"%d,%.6f\n",frames,ms(a,btime));
            if (frames == 15 || frames == 45 || frames == 75) {
                auto base = std::string(argv[2])+"-"+std::to_string(frames);
                save(base+".nv12",nv12);
            }
            ++frames;
            double left = 1000.0/30-ms(a,Clock::now());
            if (left > 0) usleep(static_cast<useconds_t>(left*1000));
        }
        fclose(csv);
        fprintf(stderr,"RESULT mode=%d frames=%d elapsed_ms=%.3f\n",mode,frames,ms(started,Clock::now()));
        if (frames != 90) throw std::runtime_error("frame deadline");
    } catch (const std::exception &e) {
        fprintf(stderr,"ERROR mode=%d %s\n",mode,e.what()); result = 10;
    }
    try {
        if (stage >= 3) check("stop",stop(driver,client));
        if (stage >= 2) check("postprocess",post(driver,client));
        if (stage >= 1) check("finalize",finalize(driver,client));
        if (driver) release(driver);
    } catch (const std::exception &e) { fprintf(stderr,"CLEANUP %s\n",e.what()); result = 11; }
    // Vendor libraries stay loaded: their globals contain live process resources.
    return result;
}
