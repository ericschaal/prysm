# Settings and daily use

Install the [Prysm application](README.md#1-install-the-prysm-application) and
connect your capture device first. This page covers the common setup for both
capture cards and the webOS capturer.

## 1. Prepare WLED

Open your WLED controller in a browser and check that it can light the strip.
In WLED:

- Set the physical LED count in **Config → LED Preferences**.
- Enable DDP input in **Config → Sync Interfaces**.
- Enable **Use Gamma for color** in **LED Preferences** and leave
  **Disable realtime gamma correction** unchecked in **Sync Interfaces**.

Keep the WLED address handy, for example `http://192.168.1.30/`.
This is a different address from the Prysm control panel.

## 2. Complete setup in Prysm

Open `http://localhost:8787` on the Prysm host, or use the LAN address from your
installation guide. `localhost` always means the device you are using: on a
phone, use the Prysm host's LAN address instead.

The first-run wizard has three steps:

1. **Video source:** under **Capture from**, choose **USB capture device** or
   **webOS capturer**. Enter the capture details from the
   [capture card](quickstart.md) or [webOS capturer](webos.md) guide.
2. **Your lights:** enter the **WLED address** and the LED count for each edge.
   Count the LEDs as seen from the front of the screen. The strip must start at
   bottom-left, go up the left edge, across the top, down the right, then along
   the bottom. Use zero for an edge without LEDs.
3. **Review & finish:** keep the picture defaults, choose **Screen sync**, and
   select **Finish setup**. You can save while the capture device or WLED is offline.

Use WLED's base address, without a settings path or login details. Prysm sends
colors to the same host on UDP port 4048; an explicit HTTP port does not change
that port. The four LED counts must total between 1 and 65535.

## 3. Turn on Screen sync

In **Overview**, select **Screen sync** and turn the lights on. The LEDs should
now follow the picture. If they do not, open [Preview](preview.md) to check
whether Prysm is receiving video, then use [troubleshooting](troubleshooting.md).

For everyday use:

- **Screen sync** follows the captured video.
- **WLED effects** lets WLED run its own effects and presets. **Open WLED** opens
  the controller's interface.
- The power control turns the lights on or off. Choosing a color source while
  the lights are off keeps them off.

Prysm starts idle after every restart. Select Screen sync and turn the lights on
to resume. Closing the browser leaves Prysm running.

## Change settings

Open **Settings**, make changes, and choose **Save & apply**. **Discard** returns
to saved values. Picture changes apply during screen sync; changing the capture
device, capture mode, WLED address, or LED layout restarts active capture.

If another browser saved settings while you were editing, reload the saved
settings before editing again. If applying a save fails, the panel shows the
error; saving settings does not prove the hardware is connected.

### Processing

Start with the defaults. Change one setting at a time while watching the
[preview and lights](preview.md).

| Setting | Default | What it changes |
| --- | --- | --- |
| Brightness | 100% | Overall LED brightness. |
| Shadow lift | 0% | Makes dark colors brighter while keeping black off. |
| Smoothing duration | 0.1 seconds | Softens color changes. Increase for gentler transitions; use 0 for immediate changes. |
| Sample density | 128 | How many picture samples Prysm uses per 1000 edge pixels. Higher values use more CPU and memory. |
| Spatial reach | 10% | How far colors inside the picture influence the edges. |
| Remove black bars | On | Follows the picture inside letterbox or pillarbox bars. Allow about 1.5 seconds to settle; needs at least 5 fps. |

### Video files

For testing, choose **Video file** under **Capture from** and enter a full path
visible to Prysm, plus resolution and frame rate. Install FFmpeg 9+ on the Prysm
host; the standard Docker image does not include it. In Docker, mount the video
and use its path inside the container.

Playback runs once, at no more than the source or configured frame rate. At the
end, Preview keeps the last image. Restart screen sync to replay. WLED remains
required; there is no preview-only output. Device and file capture are limited
to 3840 × 2160.

## Access from your phone

Allow the address you will open in the browser when starting Prysm. For example,
if the Prysm host's LAN address is `192.168.1.20`:

```sh
PRYSM_ALLOWED_HOSTS=localhost:8787,127.0.0.1:8787,192.168.1.20:8787 prysm --config settings.json
```

Open `http://192.168.1.20:8787` on your phone. The default listener already accepts
LAN connections. For the Debian service, [edit the service environment](debian.md#2-open-the-control-panel).
For containers, also [publish the LAN port](docker.md#network-and-deployment-options).

The allowed list contains exact hostnames or IP addresses with the browser port,
without `http://` or a path. It does not provide a login. Use a trusted LAN or
private VPN and do not expose Prysm to the public internet.

## Color calibration

Prysm sends sRGB colors. Start with WLED gamma near 2.2 and the gamma options
above, then judge the result with your LEDs and wall. WLED brightness and power
limits also affect the result.

With the webOS capturer, use the [Calibrate Colors wizard](calibration.md) to save
color profiles. TV capture follows the TV's rendered picture; HDR video files
are tone-mapped before processing.

## Settings and storage

Prysm saves browser settings when you finish setup or save changes:

| Installation | Settings file |
| --- | --- |
| Run `prysm` directly | `settings.json` in the working directory |
| Debian package | `/etc/prysm/settings.json` |
| Docker / TrueNAS | `/config/settings.json` inside the mounted volume or dataset |

Back up the settings directory before changing your installation. Prysm needs
permission to write the directory as well as the file. Mount the whole directory
in containers and use one Prysm instance per settings file. An invalid or
unreadable file produces an error; Prysm will not replace it with defaults.
Stop Prysm before manual edits.

### Startup options

These options belong in the command or service/container environment. Video,
lights, and picture settings belong in the browser.

| Command option | Environment variable | Default |
| --- | --- | --- |
| `--config PATH` | `PRYSM_CONFIG` | `settings.json` (packages and containers set their own path) |
| `--listen IP:PORT` | `PRYSM_LISTEN` | `0.0.0.0:8787` |
| `--allowed-host HOST:PORT,...` | `PRYSM_ALLOWED_HOSTS` | `localhost:8787,127.0.0.1:8787` |
| `--metrics-listen IP:PORT` | `PRYSM_METRICS_LISTEN` | Off; see [monitoring](../developers/monitoring.md) |
| — | `RUST_LOG` | `info` |

Command options take priority over environment variables. Restart Prysm after
changing them. To check an existing settings file without starting capture:

```sh
prysm --config settings.json check
```

This checks saved values, not hardware connections. Finish browser setup before
using it.
