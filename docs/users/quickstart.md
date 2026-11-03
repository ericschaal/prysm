# Capture device → WLED

You need a Linux or macOS host, a USB capture device supplying your video, and
WLED with LED strips. Have the `prysm` executable ready: [Debian package](debian.md)
or [build from source](../developers/README.md#build). Containers use the
[Docker guide](docker.md).

## 1. Configure WLED

In WLED, set the physical LED count and enable DDP input under **Config → Sync
Interfaces**. Enable **Use Gamma for color** in **LED Preferences** and leave
**Disable realtime gamma correction** unchecked in **Sync Interfaces**.

## 2. Configure Prysm

Start Prysm with an empty writable settings location:

```sh
prysm --config settings.json
```

Open `http://localhost:8787`. In Settings choose Device, then enter the capture
path/ID, resolution, frame rate, WLED HTTP base URL, and four LED counts. A common
starting mode is 640 × 360 at 30 Hz; your capture device must support it. Save.

Counts are viewed from the front: wiring starts at bottom-left, goes up the left,
across the top, down the right, then along the bottom. Use zero for an absent edge.
On Linux, prefer a stable `/dev/v4l/by-id/...` device path. On macOS, use the
AVFoundation unique ID from **System Information → Hardware → Camera** and grant
camera access when prompted. A container's device path must match its mapping.

## 3. Start the lights

Select **Screen sync** and turn the lights on. **Success:** the panel reports
active screen sync and the LEDs follow the picture. Configuration remains saved
in `settings.json`, and subsequent restarts start idle.

To validate an existing configuration without starting anything:

```sh
prysm --config settings.json check
```

This validates effective settings, not the capture device or WLED connection.
To control from your phone, [enable LAN access](configuration.md#access-from-your-phone).
See [troubleshooting](troubleshooting.md) for connection errors.
