# Settings and daily use

## 1. Prepare WLED

Open WLED in a browser. Check that it lights the strip, then set:

| WLED page | Setting |
| --- | --- |
| **Config → LED Preferences** | Physical LED count; **Use Gamma for color** enabled. |
| **Config → Sync Interfaces** | DDP input enabled; **Disable realtime gamma correction** unchecked. |

## 2. Complete setup in Prysm

Open `http://localhost:8787` on the Prysm host. From your phone, use the
[host's LAN address](#access-from-your-phone).

1. **Video source:** choose **USB capture device** or **webOS capturer**.
   Enter the [card](quickstart.md) or [TV](webos.md) capture settings.
2. **Your lights:** enter WLED's base address, such as `http://192.168.1.30/`,
   and the LED count for each edge. Use zero for an absent edge.
3. **Review & finish:** keep the picture defaults, select **Screen sync**,
   and choose **Finish setup**.

Viewed from the front, the strip must run:
**bottom-left → up left → across top → down right → along bottom**.

## 3. Turn on Screen sync

In **Overview**, select **Screen sync** and turn the lights on.
The LEDs should follow the picture. If they do not, see [troubleshooting](troubleshooting.md).

- **WLED effects** returns control to WLED. **Open WLED** opens its settings.
- Choosing a source while the lights are off keeps them off.
- Prysm starts idle after every restart. Turn Screen sync on again to resume.
- Closing the browser leaves Prysm running.

## Change settings

Open **Settings**, edit, then **Save & apply**. **Discard** cancels unsaved edits.
Picture changes apply live. Capture and WLED changes restart active capture.

### Processing

| Setting | Default | Effect |
| --- | --- | --- |
| Brightness | 100% | Overall LED brightness. |
| Shadow lift | 0% | Brightens dark colors; black stays off. |
| Smoothing duration | 0.1 s | Softens transitions. Use 0 for immediate changes. |
| Sample density | 128 | More samples use more CPU and memory. Leave at 128 to start. |
| Spatial reach | 10% | How far into the picture colors influence each edge. |
| Remove black bars | On | Ignores letterbox/pillarbox bars. Needs at least 5 fps and about 1.5 s to settle. |

Use [Preview](preview.md) to check changes.

### Video files

Install FFmpeg 9+, choose **Video file**, and enter the full file path,
resolution, and frame rate. Playback runs once; restart Screen sync to replay.
WLED is still required.

The Docker image does not include FFmpeg. A custom image must include it and
mount the video file.

## Access from your phone

For a service, follow the [Debian commands](debian.md#2-open-the-control-panel)
or [Docker port mapping](docker.md#network-and-deployment-options).

For a direct run, stop Prysm and restart with your host's LAN IP instead of
`192.168.1.20`:

```sh
PRYSM_ALLOWED_HOSTS=localhost:8787,127.0.0.1:8787,192.168.1.20:8787 prysm
```

Open `http://192.168.1.20:8787` on your phone.
Keep Prysm on your trusted LAN; it has no login.

## Color calibration

Start with WLED gamma near 2.2. Check WLED brightness and power limits if the
lights are too dark. With the webOS capturer, use [Calibrate Colors](calibration.md)
to match the wall light's tint to the TV.

## Settings and storage

| Installation | Settings file |
| --- | --- |
| Direct run | `settings.json` in the working directory |
| Debian | `/etc/prysm/settings.json` |
| Docker / TrueNAS | `/config/settings.json` in the volume/dataset |

Back up the settings file. Prysm needs write access to the directory, not just the
file. Use one instance per settings file; stop Prysm before manual edits.

For startup options, run `prysm --help`. To validate saved settings:

```sh
prysm --config settings.json check
```

This checks values, not device connections.
