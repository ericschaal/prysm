# Direct rvenc integration — 2026-11-02

Deployment follow-up: at the user's explicit request, version 0.1.21 was installed
after these checks. All 21 installed files match the IPK; the sender is enabled
and connected to the current computer, with capture active and no service error.
The host receiver confirmed decoded H.264 frames. The sections below retain the
earlier staged-test context. Deployment evidence (local evidence at `work/webos-encoder-latency/deployment/evidence/deployment.json`; not distributed).

The user accepted continuing with the proposed production integration after the
[controlled latency experiment](live-results.md). The source now connects
`rvenc` directly to `appsink`, requires one complete output per hardware input,
and associates output with the current input header/content. The hardware
`encode` result no longer permits a silent first-input skip. The outer software
encoder retains its rate-control skip behavior. Existing DMA allocation,
synchronization, output validation, SPS normalization and fallback protections
remain. No dependencies, configuration choices or wire changes were added.

The [integration diff](integration/integration.diff) is relative to the existing
uncommitted source at the start of this step, preserving the prior DMA work.
Only the pipeline, its hardware return contract, the caller and the existing
TV qualification tests needed behavioral changes. Hardware and package docs
were updated. The installed application was not replaced.

## Validation

- Default host tests: capture 48 plus one startup integration test, protocol 17,
  service 42; all passed. The hardware fixture replay test remains opt-in.
- Hardware-enabled host tests: 51 plus one startup integration test passed; the
  TV-only qualification was excluded on the host and then explicitly run on TV.
- Default and hardware-enabled host strict Clippy, ARM hardware strict Clippy
  including tests, formatting, ARM hardware test build and worker build passed.
- The updated ignored ARM qualification passed in 15.66 seconds. It requires
  an immediately decodable first hardware IDR, checks each changing input's
  current picture, alternates NV12 video and RGB UI with current content labels,
  and verifies geometry failure requires a fresh software stream. It covers
  640×360 RGB/NV16, 320×180 NV12 and 640×280 NV12 at 30 FPS, plus repeated teardown.
  [Test output](integration/qualification.stdout), [timings](integration/qualification.stderr).
- Four staged production worker sessions yielded 405 wire frames. Every frame
  decoded through the unchanged host `Decoder`, without additional normalization
  or receiver changes. Each connection's first picture also decoded with a fresh
  decoder. [Replay results](integration/wire-check.log), [checker](integration/wire-check.rs).

The initial sandboxed host tests could not bind local sockets. The same tests
passed with authorized loopback access; no implementation relaxation was made.

## Live capture at the user's resolution

The staged worker used source `both`, 640×280, 30 FPS, 2 Mbit/s, metrics enabled
and a TV-local receiver. It sent **300 hardware frames**, with no software
fallback. Native video capture started normally; metrics identify **299 video
outputs and one startup UI output**, while both video and UI capture continued.

| Measurement | Observed |
| --- | ---: |
| Steady output interval | 33.32 ms (~30 FPS) |
| Mean capture/encode/send iteration, including startup | 23.99 ms |
| Mean UI acquisition | 11.78 ms |
| Mean encode call | 10.42 ms |
| Mean hardware output wait, included in encode time | 9.78 ms |
| Steady worker CPU, one logical CPU = 100% | 24.82% |
| First received frame after settings | 243.33 ms |
| Disconnect shutdown | 165.92 ms |

The mean iteration uses about 72% of the 33.33 ms frame budget for this content.
This is a roughly ten-second observation, not a tail-latency guarantee or proof
of sustained headroom under other content. CPU includes capture, encode, metrics,
vendor debug logging and TV-local send, but not host decoding. It is distinct
from the earlier synthetic encode-call CPU timer. First-frame timing includes
capture and pipeline startup and is not steady frame latency.

The other sessions sent 35 frames each: hardware 320×180/15 FPS, software
640×280/30 FPS and software 320×180/15 FPS. All passed disconnect or SIGTERM
cleanup. Their durations and content differ, so this run does not establish
a controlled hardware-versus-software speedup. The prior parsed/direct/parsed
experiment remains the evidence for approximately one frame less latency.

[Structured results](integration/results.json) and
[hardware metrics](integration/hardware-640x280.metrics) retain the calculations.
All timing is measured at the worker/local receiver boundaries, not display-to-LED
latency. Live image recordings remain in ignored `work/` rather than the research
archive.

## Cleanup and limits

TV probes were serialized under the capture lock, with a 30-second qualification
deadline and a 25-second worker-probe deadline, each with a two-second kill grace.
A temporary Luna role authorized only the staged worker path using the existing
capture permission template. It and the temporary staging directory were removed.
Final status confirms sender disabled, unchanged configuration and no service
error. No firmware, driver, installed package or power/input state was changed.

Standby/resume, prolonged stress, other firmware, 60 FPS and 4K were not tested.
Native V4L2 remains a separately validated research option, with no performance
advantage established. The current production source keeps the simpler `rvenc`
backend.

## Reproduction and provenance

The updated TV test is
`encoder::hardware::qualification::hardware_capture_qualification`; build it
with `--features hardware-h264` for ARM and run with `--ignored --exact` under
the capture lock and an external deadline. The test acquires the lock itself.
Local build/test/staging scripts, binaries and raw recordings are in
`work/webos-encoder-latency/integration/`.

Replay the four saved wire streams from the repository root:

```sh
cargo run --offline --locked --manifest-path work/webos-encoder-latency/integration/wire-check/Cargo.toml --target-dir /tmp/prysm-encoder-receiver-check-target -- work/webos-encoder-latency/integration/evidence
```

[Build provenance](integration/build-provenance.json) hashes the local binaries
and execution scripts; they are retained in `work/`, not duplicated in the
archive. [SHA256SUMS](integration/SHA256SUMS) covers the archived evidence.
