# How fast can the sampler's cell averages get? (2026-11-15)

**Status: measured on the TV, nothing shipped.** The sampler is fast enough
today; this records how much headroom exists if a later processing step
needs it, and what each route costs.

## Setting

The sampler's "cell averages" stage reads every pixel of the picture once:
YUV to 8-bit RGB (`prysm_core::YuvToRgb`), sRGB to linear light by table,
summed per grid cell (`FrameView::average_linear_every`). On the dev TV
(armv7, Cortex-A9 class, NEON with a 64-bit datapath) it is the largest
stage. All numbers are the processor benchmark
(`crates/processor/examples/bench.rs`) on the TV, NV12 limited BT.709
640x360, 60 frames, colour focus off, in ms per frame.

## Results

| Version | Cell averages, 1 thread | Whole frame, 1 thread | Cell averages, 2 threads | Whole frame, 2 threads |
| --- | --- | --- | --- | --- |
| Current code (plain loop, f32 table, f64 sums) | 7.1 | 11.2 | 3.8 | 6.5 |
| Integer sums, scalar, a grid row at a time (commit 953dd9e, reverted in 6313e0d) | 6.3 | 10.3 | – | – |
| Same integer sums, one cell at a time | 7.5–7.8 | 11.6 | – | – |
| Compiler-vectorised conversion pass (32-bit lanes) | 8.3 | – | – | – |
| NEON intrinsics in C, table lookups, a grid row at a time ([patch](neon-c.patch)) | 3.2 | 7.2 | 1.8 | 4.5 |
| The same in Rust, nightly intrinsics ([patch](neon-rust.patch)) | 3.35 | 7.5 | 2.0 | 4.7 |
| Rust without the `black_box` workaround | 3.55 | 7.6 | 2.3 | 5.0 |
| C, sRGB curve as a NEON polynomial instead of the table | 8.2 | 12.3 | – | – |
| C kernel called one cell at a time | 10.4 | 14.5 | – | – |
| Current code reading a quarter of the pixels (`PIXEL_BUDGET` 320x180) | 1.9 | 5.2 | 1.3 | 4.1 |

The current-code row and the three NEON table rows (C, Rust, Rust without
the workaround) were run with Screen sync stopped on the TV; the others
with it running, which changed the figures by under 3 % where both were
measured.

## Findings

- **Walk order matters more than arithmetic on this CPU.** Every version
  that gained does a whole row band of cells per picture row. The same
  kernels called cell by cell were slower than the current loop.
- **Integer sums alone are not a gain**, and the 12 % they gave in the
  row-band shape was not worth the harder code.
- **Auto-vectorisation is a dead end here**: LLVM produced 32-bit lanes,
  which the A9 runs in two halves.
- **Hand-written NEON for the conversion halves the stage** (16 pixels at a
  time, widening 16-bit multiplies, bit-exact with `YuvToRgb` in an
  exhaustive check of the C version with representative coefficients). The
  table lookups and sums stay scalar and are then the bottleneck; computing
  the curve in NEON instead was slower.
- **Rust and C are within 5 %.** LLVM misses the widening multiply
  (`vmlal`) unless a `std::hint::black_box` hides the coefficients once per
  block; that workaround is worth 6 % and may break with another compiler.
- **Reading fewer pixels beats all of it** for one line of code. Its cost on
  synthetic panning scenes, against the full read, in 8-bit steps at gamma
  2.2: natural-like texture mean 0.5, max 2.3; heavy grain mean 0.5, max
  2.1; fine stripes and sharp shapes alias (max 74, frame-to-frame change
  doubles). Capturing at 320x180 instead lets the TV's scaler filter first:
  no flicker, fine high-contrast detail read darker (see
  [Capture resolution and LED detail](../2026-11-12-resolution-detail/README.md)
  for real content).

## What shipping NEON would need

- **Rust route** (preferred of the two): the `nightly-neon` Cargo feature on
  `prysm-processor`, gates `stdarch_arm_neon_intrinsics`
  (rust-lang/rust#111800) and `arm_target_feature`, and the whole TV binary
  built by a pinned nightly. `packaging/webos/env.sh` works unchanged with
  `cargo +nightly`. The kernel is one file chosen by a single `cfg`; on
  aarch64 the intrinsics are stable, so its tests run on stable there.
  About 350 lines, four small `unsafe` blocks.
- **C route**: the `cc` crate, a build script and a C compiler in every
  builder; about 400 lines.
- Both cover NV12 at full resolution only; other formats and skipped pixels
  use the plain loop. CI cannot run the 32-bit ARM path.

## Not tried

- Reading the middle of the picture more sparsely than the band near the
  edges (the kernel weighs the middle only through its broad tail):
  estimated 35–45 % off the stage at the default edge depth, unmeasured.
- Shifting the skip lattice each frame so the smoother averages the
  aliasing.
- Lower-precision 16-bit conversion, per-column sums, hand-written assembly.

## Not verified

- Bit-exactness of the Rust kernel beyond the 1e-6 accuracy test.
- The quality figures are from synthetic scenes; the harness was not kept.
