# Live encoder experiments — 2026-11-02

Follow-up: the user accepted continuing with source integration. The direct path
and caller metadata correction are now implemented and passed staged TV tests;
see [integration results](integration-results.md). The integration recommendations
below describe the state at the end of the initial experiments.

**Removing `h264parse` saved approximately 34 ms of frame latency on this TV at 640×360, with inputs paced at 30 fps.** Direct native V4L2 encoding also succeeded without LG's userspace encoder libraries, but has not shown an advantage over direct `rvenc`. These are bounded synthetic experiments, not a deployed production change or a sustained throughput qualification.

The target is the existing webOS TV with RTD2875Q hardware and `/dev/video24` (`rtk-vea`). The user explicitly authorized both live probes. Tests ran serially under the capture lock with external deadlines, temporary staging and cleanup. After the user disabled the sender, subsequent tests required it to remain disabled; final [sanitized status](live/evidence/service-state-sanitized.json) confirms disabled, no service error and unchanged configuration. Production source and the installed application were not changed by these experiments.

## Single-frame experiment

The [C probe](live/probe.c) submits one synthetic NV12 frame and waits 250 ms before submitting the second. Subsequent inputs are paced approximately 33.33 ms apart. It records encoder input, encoder output, parser output and appsink collection, using the same persistent DMA-BUF ownership and synchronization protections as the existing implementation. Each run submits 45 frames; EOS drains the final parser output. The sequence was parsed → direct → parsed, at 640×360, stride 768, 2 Mbit/s.

| Pipeline | Output during first isolated-frame window | First submission → collection | Mean subsequent submission → collection |
| --- | ---: | ---: | ---: |
| `rvenc ! h264parse ! appsink`, first run | 0 | 264.16 ms | 45.57 ms |
| `rvenc ! appsink` | 1 | 24.55 ms | 11.81 ms |
| `rvenc ! h264parse ! appsink`, repeat | 0 | 264.20 ms | 45.83 ms |

Subsequent means use frame indices 5–43, excluding startup and the EOS-drained final frame. Encoder-input → encoder-output averages remain 11.54, 11.53 and 11.81 ms respectively. Thus the large improvement is removal of a downstream frame wait, not faster hardware encoding. First-frame timing includes startup work; the parsed first-frame delay includes the deliberate 250 ms input gap. Raw [parsed](live/evidence/0-parsed/stdout.txt), [direct](live/evidence/1-direct/stdout.txt), [repeat](live/evidence/2-parsed/stdout.txt) traces and [summary](live/evidence/timing-summary.json) retain the measurements.

Every one of the 135 captured access units passed the unchanged Prysm `normalize_access_unit` and `Decoder`. Every decoded RGB pixel matched the intended changing frame pattern within four channel values. [Checker](live/check/src/main.rs), [results](live/evidence/receiver-check.txt), and individual `.au` files are retained. The first direct frame decoded before a second input existed, establishing correct immediate output rather than an off-by-one picture association.

This **supersedes the static report's F-006 uncertainty**. The upstream parser source and negotiated AU caps were insufficient to predict this vendor pipeline's behavior. The actual pad traces locate the wait after encoder output. They do not establish which internal parser branch causes it.

## Rust backend comparison

A scratch copy of the existing backend removes the parser/output caps filter and the first-submission skip: [exact candidate diff](live/candidate.diff). Stable DMA identities, CPU/device synchronization, output validation and H.264 normalization remain. The test checks the candidate's output against the current input and the baseline's output against the previous input. Forced Annex B output caps fail to link because `rvenc` advertises AVC statically; the working direct graph omits that caps filter and validates the actual Annex B output.

The TV ran baseline → candidate → baseline, each with NV12, RGB and NV16 input at 640×360, 30 fps requested, 2 Mbit/s. Each case had five warmup submissions and 40 measured outputs. All 360 measured outputs decoded and passed changing-color patch association checks; all measured cases reported zero skips. Input checksums match across runs. This patterned-image check complements the exhaustive uniform-frame check above; it is not a full-image fidelity metric for the lossy textured inputs.

| Input | Baseline mean encode-call wall time, two runs | Candidate | Baseline mean CPU per call, two runs | Candidate |
| --- | ---: | ---: | ---: | ---: |
| NV12 | 12.43–12.69 ms | 12.28 ms | 2.08–2.24 ms | 1.92 ms |
| RGB | 16.01–16.10 ms | 15.57 ms | 5.71–5.75 ms | 5.37 ms |
| NV16 | 13.72–13.91 ms | 13.80 ms | 3.44–3.46 ms | 3.37 ms |

The CPU timer is process CPU accumulated **during the encode call**, excluding decoding, pacing and CPU spent outside that interval. These short runs suggest modest CPU savings, not a qualified whole-sender utilization reduction. Encode-call wall time does not include the baseline output's previous-frame age; the single-frame experiment above measures that latency directly. Cold plugin discovery makes startup comparisons unsuitable for a speedup claim. [Raw measurements](live/evidence/benchmark-summary.json), [benchmark and checks](live/bench/src/encoder/mod.rs), and both source snapshots are retained.

## Native V4L2 result

The delegated agent built a standalone C harness; the parent ran it on the TV. It uses standard V4L2 MMAP with two NV12M input planes and one H.264 capture plane, with no GStreamer or LG encoder userspace library dependency. Both the one-frame and 15-frame runs succeeded, including dequeue, stream shutdown and buffer release. Every visible decoded YUV sample matched its intended input exactly; the unchanged Prysm normalization/decoder also passed every RGB pixel (maximum channel error one).

Median subsequent-frame QBUF → CAPTURE DQBUF was **13.87 ms**. This harness includes polling and different input generation, and achieved about 26.43 submitted frames/s in its short steady sample; it is not a fair proof of native performance being better or worse. First isolated-frame output required no following frame or EOS. See the [native report and evidence](../2026-11-02-open-encoder-driver/report.md).

The native stream still has crop/color/timing metadata quirks and requires normalization. Successful V4L2 control readback does not guarantee matching emitted SPS metadata. A replacement userspace encoder is now demonstrated as feasible while retaining the TV's kernel driver and firmware. A matching open-source driver implementation was not found in the bounded source search.

## Integration and remaining limits

The smallest supported next production change is the direct `rvenc` path. It must also update the H.264 caller in [encoder.rs](../../../crates/capture/webos-capture/src/encoder.rs): it currently pairs hardware output with saved previous-frame header/content. Removing only the parser or only the first-frame skip would leave incorrect metadata association or a missing-first-metadata failure. Full integration must preserve software fallback behavior, DMA ownership, validation and normalization, and qualify capture, restart and recovery.

The tested size was **640×360**, not the user's current 640×280 live capture. We did not test sustained live capture, standby/recovery, resolution changes, simultaneous capture/encode, 60 fps, 4K encoding, or decoder throughput. The known wrapper dimensions stop at 1920×1088 and its rate mapping programs nominal 30 fps; these are userspace restrictions, not a measured silicon ceiling. 4K60 decoding capability alone does not establish 4K60 encoding capability. The results support a substantial latency improvement, not a claim that the full 30 fps sender CPU budget is solved.

## Reproduction and provenance

From the repository root, host replay of the saved 135 access units uses the existing decoder:

```sh
cargo run --offline --manifest-path research/webos-capture/2026-11-02-encoder-latency/live/check/Cargo.toml --target-dir /tmp/prysm-encoder-receiver-check-target -- research/webos-capture/2026-11-02-encoder-latency/live/evidence
```

[SHA256SUMS](live/SHA256SUMS) covers all retained live artifacts. Build scripts are exact local SDK/work-directory snapshots; adapt their paths to rebuild elsewhere. Cargo manifests in the archive have only their repository-relative dependency paths adjusted. Hardware execution scripts and raw service status remain in ignored `work/` rather than publishing connection details. The original failed caps-negotiation benchmark and initial native format failure were preserved in `work/`; the native report additionally retains its sanitized failure trace. No broad workspace test suite or persistent deployment was needed for these standalone experiments.
