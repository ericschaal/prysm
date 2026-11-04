// Eight NV12 pixels at a time, using the balanced limited-range BT.709 Q13 matrix.
#include <arm_neon.h>
#include <stddef.h>

static int16x8_t offset(uint8x8_t bytes, int bias) {
    return vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(bytes)), vdupq_n_s16(bias));
}

struct Chroma {
    int32x4_t r, g, b;
};

static Chroma chroma(int16x4_t u, int16x4_t v) {
    return {vmull_n_s16(v, 14686), vmlsl_n_s16(vmull_n_s16(v, -4366), u, 1747),
            vmull_n_s16(u, 17305)};
}

static uint8x8_t channel(int32x4_t lo, int32x4_t hi) {
    return vqmovun_s16(vcombine_s16(vrshrn_n_s32(lo, 13), vrshrn_n_s32(hi, 13)));
}

static void convert8(const unsigned char *luma, unsigned char *rgb,
                     const Chroma &low, const Chroma &high) {
    const int16x8_t y = offset(vld1_u8(luma), 16);
    const int32x4_t lo = vmull_n_s16(vget_low_s16(y), 9539);
    const int32x4_t hi = vmull_n_s16(vget_high_s16(y), 9539);
    const uint8x8x3_t pixels = {{
        channel(vaddq_s32(lo, low.r), vaddq_s32(hi, high.r)),
        channel(vaddq_s32(lo, low.g), vaddq_s32(hi, high.g)),
        channel(vaddq_s32(lo, low.b), vaddq_s32(hi, high.b))
    }};
    vst3_u8(rgb, pixels);
}

// The caller supplies complete row pairs and a width divisible by eight.
extern "C" void prysm_nv12_rows(const unsigned char *y, const unsigned char *uv,
                                unsigned char *rgb, size_t width, size_t pairs) {
    for (size_t row = 0; row < pairs; ++row) {
        for (size_t x = 0; x < width; x += 8) {
            const uint8x8_t packed = vld1_u8(uv + x);
            const uint8x8x2_t split = vuzp_u8(packed, packed);
            const int16x8_t u = offset(vzip_u8(split.val[0], split.val[0]).val[0], 128);
            const int16x8_t v = offset(vzip_u8(split.val[1], split.val[1]).val[0], 128);
            const Chroma low = chroma(vget_low_s16(u), vget_low_s16(v));
            const Chroma high = chroma(vget_high_s16(u), vget_high_s16(v));
            convert8(y + x, rgb + 3 * x, low, high);
            convert8(y + width + x, rgb + 3 * (width + x), low, high);
        }
        y += 2 * width;
        uv += width;
        rgb += 6 * width;
    }
}
