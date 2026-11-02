# Realtek webOS hardware H.264 encoding

On webOS 10.3.2-34 (ARM32 EABI5 userland, AArch64 kernel), a synthetic NV12
DMA-BUF stream encoded successfully through `rvenc`: fifteen frames, EOS,
normal stop/close/finalize and exit 0. Luma pitch must be a multiple of 192;
the working 640x360 probe uses pitch 768 and chroma offset 276480.

The encoder's original SPS advertises uncropped 640x368 and omits required
video-signal color metadata. An offline SPS-only crop/BT.709 correction passes
all fifteen frames through the existing Prysm receiver without changing its
validation or any non-SPS NAL bytes. This proves an isolated synthetic path,
not production capture coexistence, color accuracy or performance.

- [Reusable PoC and offline validation](poc/README.md)
- [Bounded TV reproduction](poc/RUN-ON-TV.md)
- [Detailed findings, ABI and lifecycle map](report.md)
- [Pinned upstream source identities](upstream-sources.json)

`report.txt`, the original DMA probe and the initial logs preserve the earlier
failed experiments. Their conclusions are historical and superseded by this
README and `report.md`. Do not repeat them without a new hypothesis.

Successful-run evidence is in `evidence/`; fixture metadata and receiver
results are in `poc/evidence/`. Static excerpts use ELF virtual addresses,
with mixed ARM/Thumb PLT targets resolved through GOT relocations. The library
hash manifest identifies the exact samples; vendor binaries and bulk generated
analysis are deliberately not archived here. Upstream code is referenced by
pinned URL and hashes rather than copied into this repository.

Network destinations are redacted. Live health data describes the time of the
experiment only; obtain current status and an idle capture window before any
new device test. No production configuration changes are part of this archive.
