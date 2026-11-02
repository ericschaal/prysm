// Compiled for both ARM and host; referenced only by Rust tests.
#include "vtCaptureApi_c.h"
#include <stdexcept>

extern "C" void prysm_abi_layout(size_t* output) {
    const size_t layout[] = {
        sizeof(_LibVtCaptureProperties), offsetof(_LibVtCaptureProperties, frm),
        sizeof(_LibVtCapturePlaneInfo), offsetof(_LibVtCapturePlaneInfo, activeregion),
        sizeof(_LibVtCaptureBufferInfo), offsetof(_LibVtCaptureBufferInfo, size0),
    };
    for (size_t i = 0; i < sizeof(layout) / sizeof(layout[0]); ++i) output[i] = layout[i];
}

extern "C" VT_DRIVER* prysm_test_throw_create() {
    throw std::runtime_error("LSRegisterPubPriv FAILED");
}

extern "C" int prysm_test_throw_call(VT_DRIVER*, char*) {
    throw std::runtime_error("native call failed");
}

extern "C" void prysm_test_throw_release(VT_DRIVER*) {
    throw std::runtime_error("native release failed");
}
