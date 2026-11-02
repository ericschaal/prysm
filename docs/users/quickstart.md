# Capture device → WLED

You need a Linux or macOS host, a USB capture device supplying your video,
and a working WLED installation. Have the `prysm` executable ready:
[Debian package](debian.md) or [build from source](../developers/README.md#build).

## 1. Configure WLED

In WLED, set the physical LED count and enable DDP input under **Config → Sync Interfaces**.
Enable **Use Gamma for color** in **LED Preferences** and leave
**Disable realtime gamma correction** unchecked in **Sync Interfaces**.

## 2. Create your configuration

Save the [example configuration](examples/device-wled.toml) below as
`prysm-local.toml`. Replace the WLED address, device, and four LED counts.
Counts are viewed from the front: wiring starts at bottom-left, goes up the left,
across the top, down the right, then along the bottom. Use zero for an absent edge.

```toml
revision = "0"
source = "prysm"

[web]
listen = "127.0.0.1:8787"
allowed_hosts = ["localhost:8787", "127.0.0.1:8787"]

[input]
type = "device"
device = "/dev/video0"
capture_resolution = [640, 360]
refresh_rate_hz = 30

[output]
type = "wled"
url = "http://wled.local/"

[output.leds]
top = 51
right = 31
bottom = 49
left = 31

[processing]
brightness_percent = 80
shadow_lift_percent = 0
smoothing_seconds = 0.1
sample_density = 128
spatial_reach_percent = 10
remove_black_bars = true
```

On Linux, prefer a stable `/dev/v4l/by-id/...` path when available. On macOS,
`device` must be the AVFoundation unique ID, not `/dev/video0`; find it in
**System Information → Hardware → Camera**, and grant camera access when prompted.
Your device must support the chosen resolution and frame rate. Change those values
if capture reports an unsupported format or rate.

## 3. Start Prysm

```sh
prysm --config prysm-local.toml check
prysm --config prysm-local.toml
```

`check` validates settings; it does not test the capture device or WLED connection.
Open `http://localhost:8787`, select **Screen sync**, and turn the lights on.
**Success:** the panel reports active screen sync and the LEDs follow the picture.

To control from your phone, [enable LAN access](configuration.md#access-from-your-phone).
If anything fails, use [troubleshooting](troubleshooting.md).
