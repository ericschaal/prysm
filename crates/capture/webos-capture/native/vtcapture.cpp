// Typed exception boundary for LG's C API. No capture or transport logic lives here.
#include "vtCaptureApi_c.h"
#include <cstdio>
#include <exception>

template <typename Call>
static int guarded(char* error, Call call) noexcept {
    try { return call(); }
    catch (const std::exception& e) { std::snprintf(error, 512, "%s", e.what()); }
    catch (...) { std::snprintf(error, 512, "unknown native exception"); }
    return -1000;
}

extern "C" int prysm_vt_create(VT_DRIVER* (*function)(), VT_DRIVER** driver, char* error) noexcept {
    return guarded(error, [&] { *driver = function(); return *driver ? 0 : -1; });
}
extern "C" int prysm_vt_init(int (*function)(VT_DRIVER*, const char*, char*),
    VT_DRIVER* driver, const char* caller, char* client, char* error) noexcept {
    return guarded(error, [&] { return function(driver, caller, client); });
}
extern "C" int prysm_vt_preprocess(int (*function)(VT_DRIVER*, char*, _LibVtCaptureProperties*),
    VT_DRIVER* driver, char* client, _LibVtCaptureProperties* properties, char* error) noexcept {
    return guarded(error, [&] { return function(driver, client, properties); });
}
extern "C" int prysm_vt_plane(int (*function)(VT_DRIVER*, char*, _LibVtCapturePlaneInfo*),
    VT_DRIVER* driver, char* client, _LibVtCapturePlaneInfo* plane, char* error) noexcept {
    return guarded(error, [&] { return function(driver, client, plane); });
}
extern "C" int prysm_vt_buffer(int (*function)(VT_DRIVER*, _LibVtCaptureBufferInfo*),
    VT_DRIVER* driver, _LibVtCaptureBufferInfo* buffer, char* error) noexcept {
    return guarded(error, [&] { return function(driver, buffer); });
}
extern "C" int prysm_vt_client(int (*function)(VT_DRIVER*, char*),
    VT_DRIVER* driver, char* client, char* error) noexcept {
    return guarded(error, [&] { return function(driver, client); });
}
extern "C" int prysm_vt_release(int (*function)(VT_DRIVER*), VT_DRIVER* driver, char* error) noexcept {
    return guarded(error, [&] { return function(driver); });
}
