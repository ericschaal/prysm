// Typed exception boundary for LG's C API. No capture or transport logic lives here.
// Each entry writes an exception's message, terminated, into the `error_length`
// bytes at `error` and returns -1000.
#include "vtCaptureApi_c.h"
#include <cstddef>
#include <cstdio>
#include <exception>

template <typename Call>
static int guarded(char* error, size_t error_length, Call call) noexcept {
    try { return call(); }
    catch (const std::exception& e) { std::snprintf(error, error_length, "%s", e.what()); }
    catch (...) { std::snprintf(error, error_length, "unknown native exception"); }
    return -1000;
}

extern "C" int prysm_vt_create(decltype(&vtCapture_create) function, VT_DRIVER** driver,
    char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { *driver = function(); return *driver ? 0 : -1; });
}
extern "C" int prysm_vt_init(decltype(&vtCapture_init) function,
    VT_DRIVER* driver, const char* caller, char* client, char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { return function(driver, caller, client); });
}
extern "C" int prysm_vt_preprocess(decltype(&vtCapture_preprocess) function,
    VT_DRIVER* driver, char* client, _LibVtCaptureProperties* properties,
    char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { return function(driver, client, properties); });
}
extern "C" int prysm_vt_plane(decltype(&vtCapture_planeInfo) function,
    VT_DRIVER* driver, char* client, _LibVtCapturePlaneInfo* plane,
    char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { return function(driver, client, plane); });
}
extern "C" int prysm_vt_buffer(decltype(&vtCapture_currentCaptureBuffInfo) function,
    VT_DRIVER* driver, _LibVtCaptureBufferInfo* buffer, char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { return function(driver, buffer); });
}
extern "C" int prysm_vt_client(decltype(&vtCapture_process) function,
    VT_DRIVER* driver, char* client, char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { return function(driver, client); });
}
extern "C" int prysm_vt_release(decltype(&vtCapture_release) function, VT_DRIVER* driver,
    char* error, size_t error_length) noexcept {
    return guarded(error, error_length, [&] { function(driver); return 0; });
}
