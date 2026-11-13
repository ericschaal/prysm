# Dive deeper

Experiments, measurements, alternatives and deployment history live here.
For current instructions, use [Run Prysm](../docs/users/README.md) or
[Develop Prysm](../docs/developers/README.md).

## Color, sampling and calibration

| Investigation | Finding / status |
| --- | --- |
| [Stable black-bar detection](black-bar-detection-2026-11-02/README.md) | Live crop diagnosis, literature review, implemented temporal boundary estimator and offline replay validation; not deployed |
| [Capture resolution and LED detail](processing/2026-11-12-resolution-detail/README.md) | Offline: 1280×720 adds ≤ 2 steps (p95) over 640×360 on real content; the bounded lattice costs less than dropping to 640; capture size changes black-bar crops in dark scenes |
| [Sampling A/B protocol](sampling-ab-testing.md) | Historical proposal; not run, and based on a superseded edge-band sampler |
| [webOS colorimetry](webos-capture/2026-11-01-colorimetry/README.md) | Target-TV evidence for limited-range BT.709 capture decoding |
| [HDR and brightness](webos-capture/2026-11-01-hdr-brightness/README.md) | Capture reflects TV tone/gamut mapping; transfer calibration remains separate |
| [Transfer calibration](webos-capture/2026-11-01-transfer-calibration/README.md) | Target-specific transfer investigation; consult its result and limits before reuse |

## TV lifecycle and reliability

| Investigation | Finding / status |
| --- | --- |
| [Lifecycle investigation](webos-capture/2026-11-01-lifecycle/README.md) | Historical native ownership, constructor failure and cleanup analysis |
| [Cleanup diagnosis](webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md) | Isolated HAL unload/resource growth; native release hangs still justify process isolation |
| [Readiness](webos-capture/2026-11-01-readiness/README.md) | Readiness-driven recovery and dated validation evidence |
| [Supervisor validation](webos-capture/supervisor-validation.md) | Staged and installed checks across earlier versions |
| [Deployment history](webos-capture/deployment-history.md) | Version-specific installations and observations; not installation instructions |
| [TV acceptance records](tv-acceptance/README.md) | One record per `tv-acceptance.sh` run (smoke, install, manual checks); none yet |

## Encoding and latency

| Investigation | Finding / status |
| --- | --- |
| [On-TV Prysm pipeline](webos-capture/2026-11-04-on-tv-pipeline/README.md) | Integrated native capture and lighting at 1280×720 with staged profiles, exact replay evidence and the tested ARM binary |
| [Direct WebSocket capture](webos-capture/2026-11-04-websocket-transport/README.md) | On-TV TCP, LAN WebSocket and Caddy WSS benchmark with RP5 packet and WLED pixel observations |
| [Initial hardware encoding](webos-capture/2026-11-01-hardware-encoding/README.md) | Feasibility probes and vendor encoder investigation |
| [Encoder latency](webos-capture/2026-11-02-encoder-latency/README.md) | Static hypotheses followed by live tests and integration; read follow-ups in order |
| [Hardware qualification](webos-capture/hardware-h264-qualification.md) | Measurements from multiple historical implementations, with workload limits |
| [Single-tap cleanup](webos-capture/2026-11-02-encoder-latency/single-tap-cleanup.md) | Current reduction to Combined/Video-only capture; older UI fallback paths removed |
| [TV UI capture revisited](webos-capture/2026-11-12-ui-capture/README.md) | HAL graphics capture at 320×180 costs about 4.3 ms per frame; recommended only as a tap used while there is no video; probe-measured, not in v1 |
| [Open encoder driver](webos-capture/2026-11-02-open-encoder-driver/report.md) | Public-source analysis and native V4L2 experiments; distinguish static evidence from live results |

## Reading and adding evidence

Reports describe the version, hardware and workload tested at the time. An early
"unverified" statement may be resolved in a dated follow-up; a successful short
probe does not establish sustained performance or compatibility with another TV.

Keep scripts, manifests and committed evidence beside their report. Some historical
runs used ignored local `work/` directories or raw captures that are not distributed;
those locations are marked as unavailable rather than linked as public evidence.
Start new reports with date, status, finding, platform, limits and a link to the
maintained guide. Follow the [documentation rules](../docs/developers/guidelines.md#documentation).
