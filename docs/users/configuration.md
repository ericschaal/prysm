# Settings and daily use

Open the Prysm panel at `http://localhost:8787` or your configured LAN address.
On first use, Settings asks for the missing input and output. Choose your video
source, WLED HTTP address and LED counts, then save. The same Settings page edits
all application properties later; internal revisions are managed automatically.

Select **Screen sync** for captured colors, **WLED effects** for WLED's effects,
or turn the lights off. **Open WLED** opens WLED's interface. Selecting a source
while off does not turn on the lights. Prysm always starts idle after a restart.

## Settings and storage

Browser settings are stored in `settings.json`, or the file selected with
`--config PATH` / `PRYSM_CONFIG`. Docker uses `/config/settings.json`; the Debian
service uses `/etc/prysm/settings.json`. Missing storage starts the setup page.
There is no template to copy, and no TOML migration. Existing malformed or
unreadable JSON is reported as an error and is never silently replaced.

Settings files created before color profiles existed need a top-level
`"calibration": {"profiles": [], "active": null}` entry before Prysm can load them.

Saves atomically replace the settings file; the process needs write and traversal
access to its parent directory. Back up that directory before updates and stop
Prysm before editing the file manually. Use one Prysm process per settings file.

| Settings group | What you can edit |
| --- | --- |
| Input | Device, file, or webOS source; resolution and frame rate |
| Output | WLED HTTP base URL and top/right/bottom/left LED counts |
| Processing | Brightness, shadow lift, smoothing, sampling, and black-bar removal |
| Source | Screen sync or WLED effects; changing it while off keeps the lights off |
| Calibration | Saved color profiles and the active profile |

Process options are read on every startup: **CLI arguments → environment →
explicit defaults**. They never enter the JSON document or browser form. Application
settings have no override relationship with process options.

| CLI argument | Environment | Default / purpose |
| --- | --- | --- |
| `--config PATH` | `PRYSM_CONFIG` | `settings.json`; application settings location |
| `--listen IP:PORT` | `PRYSM_LISTEN` | `0.0.0.0:8787`; HTTP listener |
| `--allowed-host HOST:PORT,...` | `PRYSM_ALLOWED_HOSTS` | `localhost:8787,127.0.0.1:8787`; exact browser authorities |
| `--metrics-listen IP:PORT` | `PRYSM_METRICS_LISTEN` | Disabled; optional metrics endpoint |
| — | `RUST_LOG` | `info`; logging filter |

`prysm --config PATH check` validates effective settings without starting the
application or writing a file. Incomplete settings fail this check; start Prysm
normally to configure them in the browser. Host permissions, Docker mounts,
port mappings, and USB device access are deployment settings, not browser fields.

## Input and output

| Input | Required settings | Notes |
| --- | --- | --- |
| Device | Device path/ID, capture resolution, frame rate | Linux V4L2 path or macOS AVFoundation unique ID; hardware must support the selected mode |
| File | Absolute video path, capture resolution, frame rate | FFmpeg 9+ on PATH; plays once and retains the last preview frame |
| webOS | Capture resolution, frame rate | Even dimensions within 2–1920 × 2–1080, rate 1–60 Hz; hardware may support less |

Device and file resolutions are limited to 3840 × 2160. File frame rate is capped
at the lower of the source and configured rate. Container paths refer to files
and devices inside the container, so mount them first.

WLED uses its HTTP URL for control and the same host on UDP 4048 for DDP. Use an
HTTP(S) base URL without credentials, a path, query, or fragment. An explicit URL
port affects HTTP only. LED counts must total 1–65535; zero marks an absent edge.
[Wiring orientation](quickstart.md#2-configure-prysm) is fixed.

Processing changes apply live. Input, capture mode and WLED changes recreate
active capture. Change network bindings and allowed hosts in your deployment,
then restart Prysm. Keep Docker port mappings consistent with the bindings.

## Processing

| Setting | Default | Meaning |
| --- | --- | --- |
| Brightness | 100% | Output brightness, 0–100 |
| Shadow lift | 0% | Black-preserving shadow boost, 0–100 |
| Smoothing | 0.1 seconds | Time to complete 95% of a transition; 0 disables smoothing |
| Sample density | 128 | Samples per 1000 pixels of cropped edge length; positive integer |
| Spatial reach | 10% | Influence radius as a percentage of cropped picture height, 1–100 |
| Remove black bars | On | Follow stable picture boundaries; allow about 1.5 seconds to acquire bars |

Higher sample density and capture resolution increase CPU/memory use. Start at
128 samples and increase only if the lights improve. Samples are capped at one
per pixel, independently of LED count. Every active-picture pixel contributes.
Black-bar removal needs at least 5 incoming frames per second; below that rate,
the crop does not update. The tuning panel warns about low configured rates.

## Access from your phone

For first access from another device, set the bind and allowed browser addresses
through the deployment. For example:

```sh
PRYSM_LISTEN=0.0.0.0:8787 \
PRYSM_ALLOWED_HOSTS=192.168.1.20:8787,prysm.example.lan:8787 \
prysm --config settings.json
```

Open `http://192.168.1.20:8787`. These environment values stay authoritative every
startup and are never browser settings. On Debian, configure service environment
variables with a systemd override. See [Docker](docker.md) and [TrueNAS](truenas.md).

The panel has no login; use a trusted LAN/private VPN. Allowed-host checks protect
request handling and do not provide authentication.

## Color calibration

Prysm outputs sRGB. Enable WLED's color gamma and realtime gamma correction.
Use gamma near 2.2 as a starting point, then calibrate for your LEDs and wall;
steeper curves darken shadows and midtones. File HDR is tone-mapped before
processing. TV capture reflects the TV's own rendering and is not independently
HDR-calibrated. See the [color investigation](../../research/webos-capture/2026-11-01-hdr-brightness/README.md).
