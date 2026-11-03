# Settings and daily use

Open `http://localhost:8787` on the Prysm host. Select **Screen sync** for captured
colors, **WLED effects** for WLED's presets/effects, or turn the lights off.
**Customize in WLED** opens WLED's interface. Selecting a source while off does
not turn on the lights. Prysm always starts idle after a restart.

## Configuration file

Prysm reads `prysm.toml` unless you pass `--config PATH`. The Debian service uses
`/etc/prysm/prysm.toml`. Settings saved in the panel replace that file; its parent
directory must be writable by Prysm. Stop Prysm before editing the file manually.
Use the [complete portable example](quickstart.md#2-create-your-configuration)
as a starting point; repository-root samples contain local device IDs and paths.

| Section | What to set |
| --- | --- |
| `[web]` | Panel/API listen address and allowed `host:port` values |
| `[input]` | Device, file, or webOS input; resolution and frame rate |
| `[output]` | `wled` with its HTTP base `url` |
| `[output.leds]` | Top, right, bottom, left LED counts |
| `[processing]` | Brightness, shadow lift, smoothing, sampling, black-bar removal |
| `source` | `prysm` for screen sync, `wled` for effects (WLED output only) |
| `revision` | Managed by Prysm for concurrent saves; start a new file with `"0"` |

## Input and output

All three inputs work with WLED and the optional browser preview:

| Input type | Required fields | Notes |
| --- | --- | --- |
| `device` | `device`, `capture_resolution`, `refresh_rate_hz` | Linux V4L2 path or macOS AVFoundation unique ID; hardware must support the requested mode |
| `file` | `path`, `capture_resolution`, `refresh_rate_hz` | FFmpeg 9+ on PATH; plays once, retains the last preview frame, and leaves the panel available |
| `webos` | `listen`, `capture_resolution`, `refresh_rate_hz` | [TV setup](webos.md); even dimensions within 2–1920 × 2–1080, rate 1–60 Hz; hardware may support less |

File output rate is capped at the lower of the source and configured rate.
Device and file capture resolutions are limited to 3840 × 2160 (4K UHD).
WLED DDP uses the URL's host on UDP port 4048; an explicit URL port applies only
to HTTP control. Use an HTTP(S) base URL without credentials, path, query, or fragment.
LED counts total 1–65535; zero is allowed for an absent edge.
[Wiring orientation](quickstart.md#2-create-your-configuration) is fixed.

## Processing

These are library defaults; your saved configuration may override them.

| Setting | Default | Meaning |
| --- | --- | --- |
| `brightness_percent` | 100 | Output brightness, 0–100 |
| `shadow_lift_percent` | 0 | Black-preserving shadow boost, 0–100 |
| `smoothing_seconds` | 0.1 | Seconds to complete 95% of a color transition; 0 disables smoothing |
| `sample_density` | 128 | Samples per 1000 pixels of cropped edge length; positive integer |
| `spatial_reach_percent` | 10 | Influence radius as a percentage of cropped picture height, 1–100 |
| `remove_black_bars` | true | Follow stable picture boundaries; allow about 1.5 seconds to acquire new bars |

Higher sample density increases CPU and memory use. Start at **128** and increase
only if it visibly improves the lights. Density controls samples per 1000 pixels
of cropped edge length, capped at one sample per pixel; it is independent of LED
count. Higher capture resolutions also increase processing cost. Lower density
reduces spatial detail, but every pixel in the active picture still contributes.

Processing changes apply live. Input, resolution, rate, or WLED settings changes
recreate active capture. Panel listen address or allowed hosts require restarting
Prysm; the panel indicates when this is necessary. Only the six live processing
settings are editable in Preview & tuning; input/output/service settings remain
read-only context. Save & apply and Discard operate on one shared draft.

Black-bar removal needs at least 5 incoming frames per second. Below that rate,
the crop does not update; increase the capture rate or disable black-bar removal.
The tuning panel warns when the configured rate is below 5 fps. Video files can
also fall below this threshold when their source frame rate is lower.

## Access from your phone

Set the following in your configuration, replacing `YOUR_PI` with the hostname or
LAN IP you will type in your browser. Add each actual `host:port` you need.

```toml
[web]
listen = "0.0.0.0:8787"
allowed_hosts = ["YOUR_PI:8787", "localhost:8787", "127.0.0.1:8787"]
```

Restart Prysm (`sudo systemctl restart prysm` on Debian), then open
`http://YOUR_PI:8787` on the same network. The panel has no login; use a trusted
LAN/private VPN. Allowed hosts protect request handling; they are not authentication.

## Color calibration

Prysm outputs sRGB. Enable WLED's color gamma and realtime gamma correction.
Use gamma near 2.2 as a starting point, then calibrate for your LEDs and wall;
steeper curves darken shadows and midtones. File HDR is tone-mapped before
processing. TV capture reflects the TV's own rendering and is not independently
HDR-calibrated. See the [color investigation](../../research/webos-capture/2026-11-01-hdr-brightness/README.md)
for measured limits.
