// Compiled for both ARM and host; referenced only by Rust tests.
#include "vtCaptureApi_c.h"
#include "gm.h"
#include "halgal.h"
#include "dile_vt.h"
#include <stdexcept>

extern "C" void prysm_abi_layout(size_t* output) {
    const size_t layout[] = {
        sizeof(_LibVtCaptureProperties), offsetof(_LibVtCaptureProperties, frm),
        sizeof(_LibVtCapturePlaneInfo), offsetof(_LibVtCapturePlaneInfo, activeregion),
        sizeof(_LibVtCaptureBufferInfo), offsetof(_LibVtCaptureBufferInfo, size0),
        sizeof(GM_SURFACE), offsetof(GM_SURFACE, framebuffer), offsetof(GM_SURFACE, surfaceID),
        sizeof(HAL_GAL_SURFACE), offsetof(HAL_GAL_SURFACE, pitch), offsetof(HAL_GAL_SURFACE, vendorData),
        sizeof(DILE_OUTPUTDEVICE_STATE), offsetof(DILE_OUTPUTDEVICE_STATE, framerate),
        sizeof(DILE_VT_FRAMEBUFFER_PROPERTY), offsetof(DILE_VT_FRAMEBUFFER_PROPERTY, ptr),
        sizeof(DILE_VT_FRAMEBUFFER_CAPABILITY), sizeof(DILE_VT_RECT),
        sizeof(DILE_VT_VIDEO_FRAME_OUTPUT_DEVICE_LIMITATION),
        offsetof(DILE_VT_VIDEO_FRAME_OUTPUT_DEVICE_LIMITATION, scaleDownLimitWidth)
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
