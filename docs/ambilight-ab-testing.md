# Prysm sampling A/B testing protocol

Date: 2026-10-10. Status: protocol; experiments have not been run.

## Decision to make

Determine whether higher spatial sampling density improves faithful edge extension, and whether a stable dominant-color estimator improves perceived color matching without unacceptable flicker, brightness amplification, or latency.

Run these as separate experiments. A change in sampling density must not be bundled with a change in color selection or temporal smoothing. The numerical thresholds below are engineering acceptance criteria chosen for this experiment, not published perceptual thresholds.

## Current implementation and prerequisites

The current processor uses quadratic inward weighting, linear-light region averaging, black-bar detection, and elapsed-time exponential smoothing. `Config::sample_density` defaults to `SampleDensity(60)`, producing 38 horizontal and 21 vertical samples per edge at 640×360. `Config::smoothing_seconds` defaults to 0.1 seconds to complete 95% of a transition. The desktop renderer defaults to 300 displayed LEDs and interpolates the edge colors.

Relevant code:

- [Region averaging](../prysm-processor/src/frames/view_frame.rs)
- [Sampling geometry](../prysm-processor/src/nodes/edge_sampler.rs)
- [Density and color strip interpolation](../prysm-core/src/color_strip.rs)
- [Processor configuration](../prysm-core/src/config.rs)
- [Temporal smoothing](../prysm-processor/src/nodes/temporal_smoothing.rs)
- [Video playback](../prysm/src/bin/video.rs)
- [Desktop rendering](../renderers/desktop-renderer/src/lib.rs)
- [Existing pipeline regression tests](../prysm-processor/tests/pipeline.rs)

The repository currently has no A/B mode selector, per-frame measurement exporter, or implemented histogram candidate. The [LED renderer](../renderers/led-renderer/src/lib.rs) supports WLED output over DDP through the headless camera binary (`cargo run --release -p prysm --bin led -- wled.local:4048 96 54 96 54`); the desktop/video demo does not call it. Implement only the candidate and measurement support needed for the experiment being run. Real-wall conclusions require a functioning LED output path; desktop preview results must be labeled as preview results.

The desktop video player and LED binary (`--video [PATH]`) share FFmpeg 9+ input delivering 640×360 sRGB RGB24 frames, playing once and exiting at EOF. They can support visual inspection, but their playback and scheduling are not a deterministic measurement harness.

## 1. Freeze the experiment

Create a result directory outside the source tree or in an ignored local directory. Keep a manifest with:

| Field | Required value |
| --- | --- |
| Run ID | Date, experiment ID, and repeat number |
| Source | Commit SHA plus working-tree patch; include untracked source files used by the build |
| Build | Release profile, Rust version, enabled features, OS, CPU, power mode |
| Input | Clip names, SHA-256 hashes, frame counts, frame rates, dimensions, pixel format, color metadata |
| Decode | FFmpeg version, complete filter string, tone-map settings when applicable |
| Candidate | Algorithm ID, all parameters, random seeds, output mapping |
| Pipeline | Brightness, smoothing, edge depth, crop policy |
| Display | TV mode, brightness, refresh rate, scaling, dynamic processing settings |
| LEDs | Type, per-edge counts, orientation, controller, update rate, calibration, power limit |
| Room | Wall color, screen-to-wall distance, LED placement, viewing position, ambient-light setting |

Copy inputs once and decode them once into a shared frame sequence. Give A and B the same decoded bytes and frame indices. Preserve source presentation timestamps in a sidecar when testing real video. Do not independently tone-map, resize, auto-expose, or white-balance each candidate's input.

Use synthetic fixtures and clips 01–06 below for tuning. Freeze parameters before opening the held-out natural clips or collecting preference votes. Changes after that point create a new experiment ID and require a fresh evaluation.

## 2. Candidate comparisons

| Experiment | A | B | Primary question |
| --- | --- | --- | --- |
| S1: density | Mean with explicit baseline `SampleDensity(30)` | Same mean with `SampleDensity(150)` | Does a finer edge signal preserve position and detail? |
| C1: representative color | Mean at the density selected in S1 | Stable weighted histogram at exactly the same density | Does representative hue improve perceived matching? |
| T1: temporal behavior, optional later | Current elapsed-time smoother | Adaptive smoother | Does adaptive smoothing improve stability without excess lag? |

At 640×360 without cropping, S1 B produces 96 horizontal and 54 vertical samples per edge. Changing density partitions the same edge bands more finely; it does not intentionally skip input pixels. Preserve quadratic depth weighting, linear RGB arithmetic, brightness, and output positions in both variants.

For C1, freeze the following reference candidate before evaluation:

- 24 circular hue bins, using hue and saturation from decoded sRGB. Each eligible pixel votes with `spatial_weight × saturation`; hue wraparound must be handled.
- Pixels with linear luminance below 0.005 or saturation below 0.10 do not vote for hue. They still contribute to the whole-region luminance and spatial-weight denominator.
- Use the actual weighted linear RGB mean of the selected bin, not its bin-center color.
- Fall back to the ordinary mean when eligible pixels occupy less than 10% of the region's total spatial weight. When initially choosing a bin, also fall back if the strongest bin has less than 1.20 times the support of the runner-up.
- Once a bin is selected, keep it until a challenger has more than 1.20 times its current support. If the selected bin has zero support, clear it and repeat the initial selection rule. If eligibility falls below 10%, clear it and use the mean.
- Scale the selected color to the ordinary mean's whole-region linear luminance. If the selected color has zero luminance, use the ordinary mean. If scaling would take any channel above 1, scale the complete RGB vector down to fit, preserving its ratios; record this gamut limit and resulting luminance shortfall.
- Clear bin history on a new clip, frame-layout change, or viewport change. Retain the same downstream smoother as A.

These are starting parameters, not claims of an optimal algorithm. Tune them on the tuning set only, record any changes, and then freeze them. K-means or mean shift can be a later C2 comparison if C1 exposes histogram artifacts; they are not required for this protocol.

## 3. Fixed pipeline settings

Use two passes for S1 and C1:

| Setting | Numerical sampler pass | Viewing / full-pipeline pass |
| --- | --- | --- |
| Input | 640×360, full-range sRGB RGB24 | Same decoded sequence, at recorded playback rate |
| Brightness | 100% | 80%, identical for A and B |
| Edge depth | 15% of viewport height | 15% |
| Smoothing | 0 seconds | Elapsed-time smoother at 0.1 seconds |
| Black-bar removal | Off; explicit known viewport for crop fixtures | On |
| Output grid | Top 96, right 54, bottom 96, left 54 | Same 300 positions or one frozen physical layout |

Evaluate the numerical pass at the renderer's actual normalized positions: `i/(N-1)` for an edge with N outputs. Keep the same positions and ordering in A, B, and the numerical reference. Avoid interpreting a change in output layout as a sampling improvement.

For the numerical crop fixtures, bypass detection and supply the known viewport. For the full-pipeline pass, use the same detector and log its viewport on every frame. A and B must produce identical viewport histories.

For each clip, create fresh processor and estimator state. Before the scored interval, feed two seconds of that clip's first frame to both candidates to establish initial smoothing and crop state. Preserve history through intentional cuts, fades, and aspect-ratio changes within a clip.

For deterministic replay, call `process_frame_at` with a shared monotonic clock origin plus each source timestamp for both candidates, including pre-roll. Unpaced calls to `process_frame` use processing time and cannot reproduce the clip's smoothing response. These duration settings replace the earlier fixed-frame baseline; comparisons against that baseline must pin the earlier source revision and record the change as a separate experiment.

Do not change exposure, gamma, saturation, calibration, power limits, or per-candidate brightness to make one candidate look better. Warm physical LEDs for ten minutes and keep room/display settings fixed. Record any power limiting; a trial that triggers different limiting in A and B needs an explicit explanation.

## 4. Synthetic fixture suite

Generate fixtures directly as lossless RGB24 frames; do not introduce video compression. Use 30 fps, a fixed seed of 20261010, and black background unless specified. Each test has two seconds of initial-state warm-up followed by ten scored seconds. Run all four edges, with spatial probes in the central half of the edge to avoid corner overlap.

| ID | Exact stimulus | Required observation |
| --- | --- | --- |
| F1 uniform / neutral | Separate constant black, white, R, G, B, cyan, magenta, yellow, and gray inputs; gray encoded values 16, 64, 128, 192 | Uniform output; black remains black; neutral inputs remain neutral |
| F2 moving patch | White and red rectangles, 4, 8, 16, and 32 pixels wide along an edge, 54 pixels deep. Move their leading edge by one pixel per frame, wrapping within the central half | Position, footprint width, peak, integrated brightness, and wrap discontinuity |
| F3 split boundary | Red/cyan split through the 54-pixel edge band. Move the split from 25% to 75% of edge length over ten seconds | Transition width and position; whether C1's selected hue jumps |
| F4 competing colors | Tile red and cyan in the band. Alternate red coverage between 49% and 51% each frame; then separate static 45%, 50%, and 55% cases | Color switching, confidence fallback, and selection persistence |
| F5 sparse highlight | Fill a centrally located baseline sampling segment with red occupying 1%, 5%, 10%, and 25% of its along-edge width, through the full 54-pixel depth; round widths to pixels and record actual coverage | Highlight brightness stays proportional to weighted coverage; no full-segment amplification |
| F6 depth response | Red rectangle 32 pixels along the edge and 4 pixels deep, at inward offsets 0, 10, 27, 45, and 55 pixels | Contribution decreases inward; no contribution beyond the band |
| F7 cuts / fades | Whole-frame gray128→white, red→blue, and white→black cuts at scored second 5; separate linear-light white→black fade lasting two seconds | Time to settle, overshoot, hue path, and residual light |
| F8 bars | Gray128 picture: full frame for 3 s, rows 48–311 active for 3 s, dark20 picture with those bars for 3 s, then full frame. Repeat with columns 80–559 active | Confirmation/removal time, crop preservation in darkness, and remapping artifacts |
| F9 text / logo | Gray64 background, white lower text-like blocks blinking on/off once per second, plus a static red 20×20 corner logo. Keep a second copy without overlays | Local contamination and unwanted light pulsing; assess separately from faithful extension |
| F10 noise | Constant gray16 and gray128; add independent per-channel integer noise uniformly in [-2,2], clamp to [0,255] | Jitter, false chroma, histogram switching; use identical noisy frames for A and B |

For F4, equal pixel coverage does not guarantee equal histogram support when spatial weights differ. Match depth distributions and log actual weighted support. For F5, calculate expected luminance from actual pixels and weights, not nominal coverage or the encoded RGB mean.

## 5. Held-out viewing corpus

Select twelve legally available natural-video excerpts before testing. Use distinct scenes, 15 seconds each, at least two per category:

1. Dark scenes with small highlights.
2. Competing saturated colors / animation.
3. Natural neutral colors, faces, sky, and foliage.
4. Rapid movement or camera pans.
5. Subtitles or persistent logos.
6. Cuts, fades, or changing aspect ratios.

Record source timestamps and clip hashes. Reserve separate clips 01–06 for tuning; use held-out clips 07–18 for the twelve scored excerpts. Do not move failed evaluation clips into the tuning set. Synthetic pattern videos and the Philips test video can supplement testing but cannot replace natural content.

Start with SDR. If HDR material matters, conduct a separately labeled pass with one frozen upstream HDR-to-sRGB conversion shared by both candidates. This evaluates sampling after tone mapping, not native HDR handling.

## 6. Measurements and numerical reference

Log frame index, source timestamp, input arrival, processing start/end, viewport, native sample counts, and final linear RGB at every fixed output position. For C1, also log selected bin, support, fallback reason, and gamut-limited outputs. Store run metadata separately from measurements.

Use linear luminance `Y = 0.2126R + 0.7152G + 0.0722B`. Measure raw sampler output before brightness/smoothing for fidelity, and final output separately for temporal and viewing behavior.

For S1, create an independent dense reference by averaging each column/row through the edge depth with the same quadratic pixel-center weights. Integrate that dense signal over the physical output footprints centered at the frozen output positions; clip footprints at the edge endpoints and normalize by their actual support. Adjacent output centers define footprint boundaries. This reference represents the declared edge-extension objective, not human preference or a measured wall light field.

| Metric | Definition / reporting |
| --- | --- |
| Spatial fidelity | Mean absolute linear RGB error against the dense reference, averaged per fixture first; report each fixture and equal-weight aggregate |
| Moving-patch position | Absolute luminance-centroid error in pixels along the edge; skip frames with zero output mass and report misses |
| Spatial spread | Luminance-weighted RMS distance around the centroid; compare with reference and report peak luminance separately |
| Brightness | Mean and summed Y on the fixed output grid; for sparse highlights, report ratio to the dense reference when reference Y is nonzero |
| Neutrality | `max(R,G,B) - min(R,G,B)` for each gray/white output; report maximum |
| Static jitter | Per-output standard deviation of linear RGB over scored static intervals; report median and p95 across outputs |
| Color-selection stability | Bin switches per output per second, fallback rate, and mean absolute frame-to-frame RGB change on F4/F10 |
| Cut response | Elapsed time to reach and remain within 10% of step amplitude from the final steady output for at least 200 ms; measure RGB and Y, skip zero-amplitude channels |
| Crop behavior | Time from known bar change to correct viewport; number of incorrect viewport changes; A/B viewport histories must match |
| CPU cost | Median, p95, p99, and maximum processor duration; distinguish sampler-only from complete processor |
| Playback delivery | Input/output counts, dropped/missed frame indices, update intervals, and source-to-output delivery age where clocks are comparable |

Do not score C1 by agreement with mean RGB alone: selecting a representative hue deliberately changes that quantity. Still report luminance differences, gamut limits, and spatial contamination so preference cannot hide regressions.

For CPU measurement, preload input frames, warm code paths for 300 frames, and measure at least 3,000 calls per variant per repeat. Start from fresh state after warm-up and replay a defined sequence; retain output with a black-box consumer. Exclude decoding, disk I/O, UI rendering, and CSV writing from the processor timer. Run three paired repeats, alternating A-first/B-first, on the intended deployment CPU.

For full-pipeline replay, include all frames from the scored intervals and measure delivery separately. An unpaced processor benchmark cannot establish playback latency. Recheck CPU cost on representative YUYV input with fixed range/matrix metadata if YUYV is the deployment path; RGB24 timing is not a substitute.

## 7. Blinded viewing procedure

Use one physical setup and sequential playback of the same excerpt. Two adjacent light installations would introduce wall/geometry differences. A facilitator or concealed runner maps A/B to anonymous X/Y labels. The viewer must not see the algorithm, parameters, or performance counters.

For each comparison, run 24 trials: the twelve held-out excerpts twice. Present each excerpt once in X→Y order and once in Y→X order, spread across two blocks of twelve. Randomize excerpt order with recorded seed 20261010; do not place repeated excerpts consecutively. Keep the concealed mapping fixed within the session.

Each trial:

1. Hold neutral gray128 screen content and identical neutral light for five seconds.
2. Play the first variant for 15 scored seconds, with the specified pre-roll before scoring.
3. Repeat the five-second neutral interval.
4. Reset candidate state, apply the same pre-roll, and play the second variant for 15 scored seconds.
5. Record a forced choice with a tie option: **X / Y / no meaningful difference**.
6. Record color match and comfort on separate 1–5 scales, plus observed blur, lag, flicker, or exaggerated highlights.

Prompt: “Which lighting better extends this scene while staying comfortable to watch?” Do not ask which is brighter or more saturated. Do not show results or reveal mappings between blocks. Take a five-minute break between blocks. Log incomplete or interrupted trials; repeat them with the same order after the break, preserving the exclusion reason.

For a personal decision, one viewer is sufficient, but label the result personal preference. For broader evidence, recruit at least twelve viewers and run the same locked corpus. Report preferences per viewer and per clip. Repeated votes from one viewer are not independent participants.

If reporting uncertainty for the multi-viewer study, bootstrap viewer-level preference scores with 10,000 resamples and seed 20261010. Report a 95% interval and state that it describes this fixed clip corpus. Do not claim population-level evidence from a single-viewer session or treat all repeated trials as independent samples.

## 8. Physical latency check

After a candidate passes numerical checks, record at least twenty screen/LED transitions in one high-frame-rate camera view with exposure and white balance locked. Use the F7 gray-to-white cut with smoothing off first, then at the viewing setting. If testing the camera capture path, include that capture camera in the real pipeline.

For screen and LEDs, determine first sustained crossing of 10% of each region's own black-to-white measured range; require the crossing to persist for two camera frames. Report LED crossing time minus screen crossing time, median/p95, camera frame period, and rolling-shutter uncertainty. At 240 fps, one frame is about 4.17 ms; slower recording may not resolve the adoption bound below.

Keep screen and LED measurement regions near the same camera scanline where practical. Avoid clipped highlights. Camera measurements estimate relative onset timing; they do not replace calibrated LED color or luminance measurements. If no hardware output or suitable camera is available, mark this check unavailable and limit conclusions to processor/preview behavior.

## 9. Adoption rules

Freeze these before evaluating held-out content. Change a bound only under a new experiment ID.

Common gates:

- F1 raw uniform output differs from the decoded linear input by no more than 0.001 per channel, and neutral channel spread is at most 0.001. Black raw input remains black within 0.000001 per channel.
- F5's mean output luminance is no more than 1.10 times reference luminance plus 0.001. Include black pixels in brightness estimation; disclose any gamut-caused reduction.
- F10 p95 static jitter is no more than A's p95 plus 0.002 linear RGB units. C1 must have zero selected-bin switches during exact repeated uniform input after warm-up.
- Full-pipeline viewport histories match exactly; no new crashes, malformed outputs, or nonfinite/out-of-range channels. Preserve existing regression-test expectations unless a changed sampling assertion requires a justified update.
- At 640×360 / 30 fps, complete processor p95 is at most 5 ms, p99 at most 10 ms, and no more than twice A's median. These bounds apply on the deployment CPU.
- No missing outputs in deterministic replay and no additional missed frames in paced paired runs. Physical onset p95 increases by no more than 10 ms when measurable. With identical smoothing, cut-response p95 increases by at most one input-frame period.

Adopt S1 B only if it passes the common gates, reduces equal-weight F2/F3 reference RGB error by at least 20%, and worsens no tested edge's mean centroid error by more than one pixel. Viewing votes must show no consistent comfort regression. If the finer output is preferred but fails the numerical target, record that discrepancy instead of calling it a fidelity improvement.

Adopt C1 B for personal use only if it passes the common gates, wins at least 18 of 24 preference trials counting ties as non-wins, and wins both presentations of at least eight of the twelve clips. It must not lower mean comfort by more than 0.25 points on the 1–5 scale. Otherwise keep the mean as default; category-specific preference can motivate a separate experiment, not an untested automatic mode.

For the twelve-viewer study, require median viewer B-preference of at least 60%, a viewer-bootstrap lower bound above 50%, and mean comfort decline no greater than 0.25 points, in addition to the common gates. Count a tie as half a preference vote for this study and disclose that scoring rule. This is an adoption rule for the chosen corpus, not proof that B is universally superior.

If a result misses a gate, retain A and record the failure. Do not compensate by adjusting only B's brightness or adding smoothing after the evaluation.

## 10. Result record and existing commands

Existing checks and inspection entry points, run from the repository root:

```sh
cargo test -p prysm-core -p prysm-processor
cargo run --release -p prysm-processor --example bench
cargo run --release -p prysm --bin video -- "/absolute/path/to/clip.mp4"
```

The existing benchmark prints rough mean pipeline cost. It does not produce the required percentiles, candidate comparisons, timestamps, or viewing randomization. The existing video command does not accept algorithm-selection flags; do not assume a nonexistent CLI.

Save `manifest.md`, the frozen candidate specifications, input hashes/fixture definitions, per-frame measurements, `trials.csv`, the concealed mapping, and `results.md`. Suggested viewing columns:

```text
viewer_id,experiment_id,block,trial,clip_id,order,choice,color_x,color_y,comfort_x,comfort_y,notes,exclusion_reason
```

Use this result summary:

| Item | A | B | Pass / interpretation |
| --- | --- | --- | --- |
| Spatial reference error | | | |
| Centroid error / spread | | | |
| Sparse-highlight luminance | | | |
| Neutrality / jitter | | | |
| Selection switches / fallbacks | | | |
| Cut response / crop behavior | | | |
| Processor median / p95 / p99 | | | |
| Missing frames / delivery age | | | |
| Physical onset median / p95 | | | |
| Preference / comfort | | | |
| Gamut limits / other failures | | | |

Conclude with **adopt B**, **retain A**, or **inconclusive**, list failed/unmeasured gates, and state whether the evidence came from desktop preview or real LEDs. Keep improvements in faithful edge reconstruction separate from improvements in viewing preference.

## Research basis

The protocol preserves linear-light averaging as the baseline, tests representative color separately, and explicitly measures temporal stability and luminance:

- [HyperHDR 22 linear-light region averaging](https://raw.githubusercontent.com/awawa-dev/HyperHDR/v22.0.0.0/sources/base/ImageColorAveraging.cpp)
- [Hyperion mean, mode, and k-means estimators](https://raw.githubusercontent.com/hyperion-project/hyperion.ng/master/include/hyperion/ImageToLedsMap.h)
- [Philips perceptual dominant-color extraction patent](https://patents.google.com/patent/US20070242162A1/en)
- [Sekulovski, Studies in ambient intelligent lighting, 2013](https://pure.tue.nl/ws/files/3681859/752369.pdf)

The fixture definitions, candidate parameters, session design, and acceptance bounds are proposed for Prysm. They have not been validated by those sources.
