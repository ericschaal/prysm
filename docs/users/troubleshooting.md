# Troubleshooting

After restarting Prysm, select **Screen sync** and turn the lights on.
Read the error in the panel.

## Panel and settings

| Problem | Fix |
| --- | --- |
| Panel will not open | On the host, try `http://localhost:8787`. On a phone, use the host's LAN IP and follow [these commands](configuration.md#access-from-your-phone). |
| Invalid Host / request rejected | Add the browser's `hostname:port` or `IP:port` to `PRYSM_ALLOWED_HOSTS`, then restart Prysm. |
| Cannot save | Give Prysm write access to the settings directory. In Docker, mount the whole directory at `/config`. |
| Settings changed elsewhere | Reload saved settings before editing again. |
| Saved but not applied | Check the panel's error, capture device, and WLED connection; retry Screen sync. |

Logs:

```sh
# Debian
sudo systemctl status prysm
sudo journalctl -u prysm -n 100

# Docker, from the Compose directory
docker compose logs --tail 100 prysm
```

## Picture and lights

Open [Preview](preview.md). Missing/frozen picture: check capture.
Moving picture but wrong LEDs: check WLED and wiring.

| Problem | Fix |
| --- | --- |
| Capture card unavailable | Check path/ID, permissions, and other apps using the card. macOS needs camera access; Docker needs the device mapping. |
| Unsupported mode | Use a supported resolution/rate. Linux needs YUYV, RGB3, or BGR3; MJPEG-only modes do not work. |
| No LEDs | Check WLED address, DDP input, and LED counts. Allow HTTP/WebSocket and UDP 4048 to WLED. |
| Wrong edge | From the front: bottom-left → up left → across top → down right → along bottom. Check edge counts. |
| Dark/washed-out colors | Check [gamma](configuration.md#color-calibration), brightness, and WLED power limits. |
| Black bars still sampled | Wait about 1.5 s. Detection needs at least 5 incoming fps. |
| File will not play | Install FFmpeg 9+ and check file access. Docker needs FFmpeg in the image and a file mount. |
| File finished | Restart Screen sync to replay. |

## webOS capturer

| Message / problem | Fix |
| --- | --- |
| Waiting for Prysm | Check the saved LAN URL and `PRYSM_ALLOWED_HOSTS`. HTTPS also needs a trusted certificate and WebSocket proxying. |
| Receiver in use | Another TV has the session. Wait for it to disconnect. |
| Connected · Capture off / Ready to capture | Turn on Screen sync in the host panel. |
| Waiting for video / frozen picture | Wake the TV and play video or use an active HDMI input. Home may provide no frames. |
| Service unavailable / permission denied | **Service options → Set up or repair service**. Stop other capturers. |
| Start fails after ~12 s | Wake the TV, play video, fix the panel's error, then retry. |

## Report a problem

Include the error, commit, host OS, capture device, resolution, and frame rate.
For webOS, add TV model and firmware. Redact private addresses from logs.
[Developer debugging](../developers/debugging.md).
