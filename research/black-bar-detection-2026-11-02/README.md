# Stable black bar detection

Date: 2026-11-02. Status: replacement implemented locally; live deployment pending.
Platform: live webOS capture observed through the local Mac preview, 640×360 at
configured 30 Hz. Source inspected at commit `6b6d2406c706140fb311ba2705836f57eef7a30e`.
Current runtime architecture is described in [the maintained guide](../../docs/developers/architecture.md).

The original instability had a concrete cause: a discontinuous symmetry heuristic
turns one pixel of bottom-boundary noise into ten pixels of top-boundary movement.
Threshold-only observations and 200 ms confirmation introduced additional failures.
The replacement uses four independently estimated boundaries, explicit uncertainty,
overlay rejection, and conservative temporal confirmation. This is a literature-informed
design; superiority over other systems still needs controlled replay evaluation.

The user selected stability first, approximately 1–2 seconds before cropping deeper,
0.3–0.5 seconds to restore clearly visible picture, and ignoring small subtitles/logos
inside the bars.

## Implementation and validation

The replacement preserves the detector's interface, the sampling/smoothing pipeline,
and existing configuration. It estimates each boundary with twelve transverse sectors,
eight samples per sector, a calibrated black ceiling and chroma checks. A provisional
pass restricts the other axis before requiring distributed support, preserving narrow
windowboxed pictures. Weak near-black detail can veto cutting picture away without
being sufficient to establish a boundary. Stronger median/chroma evidence anchors
geometry. An uninformative frame preserves the last confirmed crop.
An outward candidate must connect through the previously excluded bar into known
picture. This prevents a tall caption island from becoming a new boundary. Strong
verification scales with available picture extent; uncertain startup overlays can
leave an edge uncropped until clear evidence arrives. Large or connected overlays
are not guaranteed to be distinguishable from picture.

Each edge retains a committed coordinate and a median challenger. Fresh observations
confirm inward changes after 1500 ms and outward changes after 400 ms. A two-source-pixel
deadband retains exact committed coordinates. Unknown observations and gaps over 200 ms
invalidate tentative evidence; low-rate inputs with gaps this large may not acquire a
crop. The detector retains the existing minimum active-picture extent of 10% per axis.
Black-level noise margins are bounded by calibrated full/limited-range ceilings; there
is no unrestricted learning from dark scenes, neural model, or motion-vector dependency.

The updated [synthetic replay](repro.rs) asserts the intended behavior. Its
[new output](implemented-replay.txt) holds 20/11 through one-pixel bottom noise, retains
20/20 through dim picture edges, and retains 48/48 through a bottom subtitle. The
[original output](repro-output.txt) is preserved as historical evidence. Processor-level
tests additionally verify identical LED colors when the changed pixels lie outside the
locked viewport, preserved smoothing during acquisition, and resets on layout/new runs.

A second 44.96-second recording supplied 325 consecutive 480×270 JPEG previews at
about 7.2 Hz, with unchanged configuration revision 60. Both detector versions processed
the same decoded RGB pixels at the observed timestamps. The old detector made seven
transitions including acquisition, or six subsequent changes. The new detector acquired
bottom=8 at 1.54 seconds and top=15 at 2.93 seconds, then held the 480×247 active rectangle
with **zero subsequent crop changes**. The acquisition is slower than the minimum
1500 ms window because ambiguous observations do not count. See the
[paired coordinates](preview-comparison.csv).

This preview replay is secondary robustness evidence: JPEG ringing, RGB conversion,
downscaling and lower observation rate differ from raw capture. Visual inspection
supports the resulting boundaries, but this is one short clip rather than an annotated
multi-show accuracy benchmark. The temporary JPEG/raw frames and comparison runner
remain in `/tmp/prysm-blackbars/replay` and `/tmp/prysm-blackbars/compare.rs`, unavailable
in a fresh checkout. No broadcast images are distributed with the report.

The final release-mode benchmark on the development Mac measured approximately
1.20 ms/frame at 640×360 and 9.70 ms/frame at 1920×1080 after the change, versus 1.22 ms
and 10.05 ms before. These are rough complete-pipeline means over 120 synthetic frames,
not detector-only timings or evidence of a statistically significant speed difference.
Repeat with `cargo run --offline --release -p prysm-processor --example bench` on target
hardware. The TV service and its saved settings were not changed by this implementation.

Final checks:

- `cargo test --offline -p prysm-processor -p prysm`: 98 passed, four existing tests
  ignored (two measurement tests and two FFmpeg-dependent tests). Application tests
  needed loopback socket access outside the filesystem/network sandbox.
- `cargo clippy --offline -p prysm-processor --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Synthetic replay assertions and the paired preview replay passed after the final
  tall-caption correction. The replay's fixed hypothetical crop comparisons remain
  intentionally unchanged to document geometry's effect on LED colors.
- Documentation link checks and their five unit tests passed.

## Observed instability

[Live crop observations](live-crops.csv) contain 650 paired preview responses over
89.94 seconds. Every response had configuration revision 60 and source size 640×360.
There were 29 observed crop changes across six rectangles. Preview observations were
about 7.2 Hz, so changes between observations may be missed. This is a lower bound,
not a count of every detector update.

| Top margin | Bottom margin | Observed responses |
| --- | --- | --- |
| 20 | 11 | 354 |
| 10 | 10 | 209 |
| 8 | 8 | 36 |
| 17 | 8 | 32 |
| 9 | 9 | 14 |
| 17 | 10 | 5 |

All observations had zero left/right crop. The first transition was at 0.42 seconds,
from 20/11 to 10/10; inspected JPEGs show it occurring within the same shot.
Settings were sample density 256, spatial reach 20%, smoothing 0.3 seconds, shadow
lift 30%, brightness 100%, black-bar removal enabled. These came from the running
API; the working-tree TOML differed and was not used as runtime evidence.

The preview exposes committed geometry, not the raw detector proposals or its black
threshold. Thus the live trace plus the exact source replay strongly implicate the
symmetry discontinuity, but do not establish every live proposal or the exact running
binary revision. JPEG previews are downscaled and lossy; they cannot calibrate raw
luma thresholds. Original observations, occasional images, and the one-shot observer
remain locally in `/tmp/prysm-blackbars`; those temporary files are unavailable in a
fresh checkout. Only crop metadata and synthetic reproduction are retained here.

## Why the original detector failed

At the baseline commit above, the detector scanned at most every 50 ms, built P98
row/column brightness projections, found continuous dark runs from the image edges,
required opposing bars, applied an asymmetry correction, then confirmed a changed
pair for 200 ms.

1. **A discontinuous geometry correction.** In
   [detect_axis](../../crates/processing/prysm-processor/src/nodes/black_bar_detector.rs),
   opposing margins differing by more than `height / 40` are both replaced by their
   minimum. At height 360 this tolerance is nine pixels: `(20,11)` stays `(20,11)`,
   but `(20,10)` becomes `(10,10)`. The two-pixel temporal deadband operates after
   this amplification and cannot suppress it. Legitimate asymmetric padding is lost.
2. **Sparse P98 is nearly a maximum.** With stride eight, a 640-pixel row has 80
   samples; its P98 is the second-largest sample. Two sampled bright pixels can break
   the row. A 360-pixel column has 45 samples; its P98 is the maximum. One sampled
   bright pixel can break it. Subtitle strokes, logos, and noise are influential.
3. **Darkness is mistaken for matte.** Limited-range Y at or below 23 is classified
   as black. The interior-brightness guard only requires one sufficiently bright
   projection entry somewhere. Bright central content surrounded by dim picture
   edges therefore passes the guard and loses real content. Uniformly dark frames
   are correctly treated as unknown; partially dark frames remain problematic.
4. **Opposing edges are coupled.** A subtitle on the bottom can change the top
   through the minimum rule. A missing bar on either side releases the entire axis.
   Independent horizontal/vertical tracking does not isolate top from bottom.
5. **Confirmation measures persistence of a possibly wrong observation.** It stores
   the first candidate, not a temporal estimate, and treats inward/outward movement
   equally. A scene can easily sustain an error for 200 ms. Two matching observations
   across a long capture gap can also confirm because elapsed silence counts.
6. **Geometry changes affect the whole color sampler.**
   [SpatialSampler](../../crates/processing/prysm-processor/src/nodes/spatial_sampler.rs)
   rebuilds its grid, edge sample counts, and weighting when cropped dimensions change.
   Its influence radius depends on cropped height. Changing top/bottom can therefore
   alter all four output strips. This is expected sampling behavior, not proof of a
   sampler bug. Existing smoothing retains history, but reaches 95% of a step in the
   configured 0.3 seconds; it cannot make repeated wrong geometry harmless.

## Reproduction and limits

Run from the repository root:

```sh
research/black-bar-detection-2026-11-02/run-repro.sh
cargo test --offline -p prysm-processor --lib black_bar_detector
```

The [replay](repro.rs) imports the actual detector, frame view and sampler and uses
limited-range BT.709 synthetic frames. It now asserts corrected behavior; the
results below describe its original diagnostic assertions against the baseline commit.
The shell runner creates and removes a temporary Cargo project; it needs cached
dependencies for offline compilation. [Recorded results](repro-output.txt):

| Controlled input | Existing result |
| --- | --- |
| Top/bottom 20/11 → 20/10 → 20/11 | 20/11 → 10/10 → 20/11 |
| True 20/20 bars, dim picture extending to depth 40 on both sides | Crops 40/40, discarding dim content |
| True 48/48 bars with a bottom subtitle | Crops 23/23, moving both edges |
| Entirely black frame after acquisition | Correctly retains the prior crop |

On an unchanged synthetic frame, applying the two observed crop rectangles changes
the mean top-strip linear red from 0.527115 to 0.356726, a 32.3% decrease. An optional
480×270 RGB24 file argument repeats the comparison on a decoded preview. For the
first observed JPEG, scaled versions of those two crops produced a maximum top-LED
sRGB channel difference of 30/255 and mean absolute channel difference of 10.405/255
over 51 LEDs. These comparisons exclude smoothing and shadow lift; they isolate
geometry sensitivity and are not measurements of physical LED brightness or exact
full-resolution production output. The JPEG input is not distributed.

All 18 original detector tests passed. They did not cover the asymmetry threshold
crossing. The subtitle test explicitly tolerates a reduced bottom bar, and the
alternating subtitle test tests side-bar acquisition rather than stable top/bottom
framing through sustained subtitles. The initial investigation changed no production
files or runtime settings; the implementation described above followed it.

## What other implementations do

| System | Actual approach | Useful lesson and limit |
| --- | --- | --- |
| Hyperion | RGB black threshold; default probes three lines across the sides. Classic, OSD, and letterbox modes change probe placement. Letterbox avoids the bottom-center caption area. The processor defaults to 50 consistent detections, tolerates 10 inconsistent ones, and uses a separate 600-detection unknown threshold. | Stronger temporal inertia than Prysm. Sparse probes and symmetric geometry still have limitations; counts depend on detector invocation rate. [Detector](https://github.com/hyperion-project/hyperion.ng/blob/master/include/blackborder/BlackBorderDetector.h), [processor](https://github.com/hyperion-project/hyperion.ng/blob/master/libsrc/blackborder/BlackBorderProcessor.cpp). |
| HyperHDR | The inspected implementation retains the same families of sparse RGB probes and 50/10/600 temporal defaults. | It is not evidence of a fundamentally different learned or semantic detector. [Detector](https://github.com/awawa-dev/HyperHDR/blob/master/sources/blackborder/BlackBorderDetector.cpp), [processor](https://github.com/awawa-dev/HyperHDR/blob/master/sources/blackborder/BlackBorderProcessor.cpp). |
| Firefly Luciferin | Samples distributed locations at preset border depths and the center; checks RGB darkness and interior brightness; chooses fullscreen, letterbox or pillarbox LED matrices. | Preset states reduce continuous-coordinate jitter but sacrifice arbitrary/asymmetric geometry. The inspected helper also mutates state inside detection, so its outer confirmation counter alone is not proof that every switch is debounced. [Source](https://github.com/sblantipodi/firefly_luciferin/blob/master/src/main/java/org/dpsoftware/grabber/ImageProcessor.java). |
| FFmpeg cropdetect | Black mode can retain the largest observed active area until reset. `mvedges` combines motion vectors and Canny edges to locate the picture. | Borrow positive content history and boundary evidence. A permanent largest-area estimate cannot adapt to newly introduced bars; motion vectors are not presently part of Prysm's processor input. [Documentation](https://github.com/FFmpeg/FFmpeg/blob/master/doc/filters.texi#L10337), [implementation](https://github.com/FFmpeg/FFmpeg/blob/master/libavfilter/vf_cropdetect.c). |

Sources were inspected on the report date; upstream branches can change. These are
source comparisons, not comparative execution benchmarks.

## Scientific and industrial evidence

**Carreira and Queluz, SIGMAP 2014**, *Automatic Letter/Pillarbox Detection for Optimized
Display of Digital TV*, DOI 10.5220/0005064202810288, is directly relevant. It combines
near-black luma with neutral chroma. Its overlay-tolerant mode inspects the central
half, permits some nonblack pixels and short interruptions, and uses temporal voting.
On its difficult 352.1-second S4 clip, five-second filtering improves bottom-boundary
success from 73.76% to 84.88%; the other edges reach 99.20–99.99%. Success permits
1% aspect-ratio error. Reported case-two processing is 3.09–3.45 ms/frame on an
i7-4770, on a small selected corpus mostly around 640×360. This supports temporal
evidence and overlay tolerance, but demonstrates that voting alone leaves errors.
[Paper](https://www.scitepress.org/Papers/2014/50642/50642.pdf),
[institutional record](https://www.it.pt/Publications/PaperConference/15213).

**Liu, Liu and Shahraray, AT&T TRECVID 2009**, section 3.2.1, accumulate directional
Canny edges, remove short segments, merge nearby segments, and locate projection
peaks near frame boundaries. Temporal pixel-intensity variance provides a fallback.
The method processes an entire query video; its copy-detection results do not isolate
real-time crop accuracy. Persistent boundaries and temporal activity are useful cues,
but static picture content also has low variance.
[Primary report](https://www-nlpir.nist.gov/projects/tvpubs/tv9.papers/att.pdf).

**Yeh, Hsu and Lu, NTNU–Academia Sinica TRECVID 2010**, section 2.1, sample one frame
per second, apply Sobel to a mean frame, and combine projected edge peaks with low
border variance and geometric checks. Again, evaluation concerns a complete
copy-detection system. This motivates combining uniform exterior and boundary
support, but whole-clip averaging cannot directly track live aspect changes.
[Primary report](https://www-nlpir.nist.gov/projects/tvpubs/tv10.papers/ntnu-academia-sinica.pdf).

**Esteve Brotons, doctoral thesis, 2023**, section 6.4.2.2, describes shot-center
frames, Scharr derivatives, adaptive thresholds and accumulated directional edges.
Its paired symmetric edge criterion should not be copied into Prysm: real asymmetric
padding is the central observed failure here. The university-indexed section and
date were checked; the complete thesis evaluation was inaccessible and no independent
accuracy claim is made here.
[Thesis](https://rua.ua.es/dspace/bitstream/10045/137347/1/tesis_miguel_jose_esteve_brotons.pdf),
[university date record](https://web.ua.es/es/dtic/investigacion/memorias-de-investigacion/b148-2023.pdf).

**Park, Kim and Jang, 2026**, *An Input Video Normalization Method Based on Valid Regions
and Frames for Illegal Streaming Video Identification*, uses a brightness-valid
bounding rectangle and rejects uninformative frames using brightness entropy and
edge density. Its evaluation measures video identification rather than crop jitter.
The useful idea is abstention on uninformative input; recency does not make its
brightness rectangle a demonstrated improvement for this task.
[Journal paper](https://journal.kci.go.kr/ksavs/archive/articlePdf?artiId=ART003348214).

Industrial disclosures provide additional ideas, not peer-reviewed comparisons.
Dolby's [US20130071034A1](https://patents.google.com/patent/US20130071034A1/en)
uses transitions in row/column variation and historical information.
Signify's [EP4609607A1](https://patents.google.com/patent/EP4609607A1), published in
2025, directly addresses video lighting: move a few analysis regions to locate bars
and reposition lighting samples. It emphasizes resource cost, and does not establish
the algorithm in a currently shipping Hue product or comparative accuracy.

The reviewed material supports a hybrid spatial/temporal design. It does not establish
a shared contemporary benchmark or a universal best low-latency Ambilight detector.
Neither a neural network nor dense optical flow is justified by the evidence gathered.

## Original design proposal

Use a **spatiotemporal active-picture estimator with a stable committed viewport**.
This combines published ideas with the following Prysm-specific design decisions.

1. **Keep native geometry and calibrated black levels.** Read raw frame data and
   honor its range/matrix. Start with the current calibrated full/limited black
   distinction. Check chroma neutrality near the black threshold to protect dim
   colored detail. If real fixtures require pedestal/noise adaptation, bound it and
   learn only from confidently established bar pixels. Never learn black from the
   whole current frame. RGB and YUV observations need equivalent tests.
2. **Measure a boundary in distributed sectors.** Prototype 8–12 sectors per edge,
   each using a strip rather than one point. Seek a uniform, near-black exterior
   followed by a sustained transition into picture. Combine black occupancy,
   robust variation and perpendicular edge evidence. Temporal change can strengthen
   picture evidence, but must not be required for a static shot. Dark or conflicting
   sectors return unknown, not a deeper boundary.
3. **Reject overlays before pooling evidence.** Small spatially localized detections
   are outliers. Also inspect several rows/columns on the picture side: a subtitle
   can span many sectors, but often forms a bright island followed by more black.
   Do not treat that island as the start of sustained picture. Large banners that
   occlude most evidence produce unknown rather than arbitrary recropping. No OCR
   or fixed exclusion of the whole middle of the frame is needed initially.
4. **Cluster compatible boundary locations.** Require substantial, spatially
   distributed support and use the robust center of that cluster. Preserve an
   uncertainty interval for blurred/noisy edges. Do not average incompatible
   candidates, snap to standard aspect ratios, or force opposing margins equal.
   Track each edge separately and validate the combined rectangle before publishing.
   Opposing-edge agreement is supporting evidence, not permission to overwrite it.
5. **Retain recent positive picture evidence.** Keep a short, bounded history of
   trustworthy content near each boundary, after rejecting overlays. Evidence that
   a strip recently contained picture vetoes immediately cropping it away. Avoid a
   raw temporal maximum: one bright subtitle or permanent logo could poison it.
6. **Separate observations from decisions.** Each edge has a committed location
   and one competing cluster with confidence, fresh-observation support and expiry.
   A candidate within the committed uncertainty/deadband keeps the exact old
   coordinate. Unknown frames preserve the committed crop and do not accrue support.
   Cap time credited to each observation; long gaps expire candidates rather than
   confirming them. Use the median of the supported cluster, not its first sample.
7. **Make transition evidence directional.** Cropping farther inward requires about
   1.5–2 seconds of fresh, high-confidence matte/boundary evidence. Restoring picture
   requires about 0.3–0.5 seconds of distributed positive picture evidence in the old
   bar. Local subtitles/logos cannot trigger the fast path. These are proposed tuning
   ranges, not measured performance guarantees or timeouts that force a decision.
8. **Preserve framing across cuts and fades.** A cut is not an aspect-ratio change.
   Reset on explicit source/session or frame-layout changes; otherwise use the same
   evidence rules. At startup, use full frame until confident. In prolonged ambiguity,
   retain the last established crop. Same-resolution source switches without a signal
   can only be recognized once the pixels supply enough evidence.

Keep the existing LED temporal smoother for the first implementation. A smoothly
moving crop would still sweep sampling weights through the picture, and an extra
crossfade can conceal detector errors. Add a separate transition treatment only if
measurements of genuine aspect changes justify it after geometry is stable.

The intended cost is a bounded set of sector statistics and short temporal state,
without video buffering, a model dependency, or decoder motion-vector plumbing.
Retain a roughly 20 Hz observation budget initially and measure actual processing
time. Sector counts, thresholds, history length and deadlines require validation.

The fundamental ambiguity remains: a static black region inside the intended picture
can be pixel-identical to padding. Without trustworthy source metadata, no pixel-only
method can guarantee the intended crop. Conservative abstention is the selected tradeoff.

## Qualification and implementation scope

Compare current Prysm, a symmetry-only fix, a simple temporal-consensus baseline,
Hyperion-style stabilization, and the proposed estimator on the same annotated raw
replays. Keep tuning and validation shows/sources separate. Measure:

- False committed crop changes per minute on fixed-format clips, including dark
  scenes, fades, subtitles, corner logos and wide captions. Initial target: zero
  after acquisition on the regression corpus, not a universal guarantee.
- Per-edge error and time spent excluding real picture. Do not rely on mean IoU:
  small edge movement can have a large lighting effect.
- Genuine inward/outward format-change delay, separately from ordinary scene cuts.
  Apply the requested timing ranges only where enough visual evidence exists.
- Same-frame LED error against an annotated fixed crop, with the same downstream
  settings, to isolate detection from real changes in the scene.
- Runtime and allocation cost at the actual capture size and rate.

Fixtures must include the exact 20/11 versus 20/10 cliff, thin/asymmetric bars,
letterbox, pillarbox, windowbox, dim saturated detail, noisy/blurred boundaries,
full/limited-range inputs, explicit source/layout resets, long capture gaps, and
irregular timestamps. The current all-black preservation behavior must survive.

The production change should primarily reach `black_bar_detector.rs`, its tests,
and raw pixel-statistic helpers if chroma is added. Processor reset/configuration
callers and end-to-end sampling tests need verification. Diagnostic candidate/commit
reasons should be available for replay. Preserve `FrameView`/`Viewport` output and
the existing `remove_black_bars` setting; no public UI/config additions are required
by this proposal. Updating tests that currently permit subtitle-driven geometry is
part of adopting the new behavior. The sampler and smoother remain unless controlled
evidence identifies a separate defect.
