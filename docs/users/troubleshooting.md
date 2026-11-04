# Troubleshooting

Read the error in Prysm's browser panel first. After starting or restarting
Prysm, choose **Screen sync** in **Overview** and turn the lights on.

## Panel and settings

| Problem | What to check |
| --- | --- |
| Panel will not open | On the Prysm host, try `http://localhost:8787`. For a phone, use the host's LAN address and [allow LAN access](configuration.md#access-from-your-phone). Check the service or container logs. |
| Request rejected / invalid Host | Add the exact browser hostname or IP, including port, to `PRYSM_ALLOWED_HOSTS` in the service/container environment. Restart or recreate Prysm. |
| Settings will not save | Prysm needs write access to the settings directory, not just the file. In containers, mount the whole directory. If another browser saved changes, reload saved settings before editing again. |
| Settings saved, but not applied | Read the application error in the panel. Check the capture device and WLED connection, then retry Screen sync. |

For Debian service logs:

```sh
sudo systemctl status prysm
sudo journalctl -u prysm -n 100
```

For Docker logs, run `docker compose logs --tail 100 prysm` from the Compose directory.

## Picture and lights

Open [Preview](preview.md). If its picture is missing or frozen, check capture.
If it advances correctly, check WLED and the LED layout.

| Problem | What to check |
| --- | --- |
| Capture card unavailable | Confirm the path/ID and that another program is not using it. Linux needs video-device permissions; macOS needs camera permission. Docker needs the device and group mappings. |
| Unsupported capture mode | Choose a resolution, frame rate, and format the card supports. Linux accepts YUYV, RGB3, or BGR3, not MJPEG-only modes. |
| Preview works, LEDs do not | Check the WLED address, DDP input, LED counts, and wiring. Allow HTTP/WebSocket and UDP 4048 from the Prysm host or container to WLED. |
| Colors follow the wrong edge | Viewed from the front, wiring starts bottom-left → up left → across top → down right → along bottom. Check each edge's count. |
| Dark or washed-out colors | Check [WLED gamma](configuration.md#color-calibration), brightness, and WLED power limits. |
| Black bars are still sampled | Allow about 1.5 seconds for detection. Black-bar removal needs at least 5 incoming fps. |
| Video file will not play | FFmpeg 9+ must be on the running process's PATH. Check the file path and read access; in a container, mount the file. The standard image does not include FFmpeg. |
| File finished / last image remains | Playback runs once. Restart Screen sync to replay. |

## webOS capturer

| TV message or problem | What to do |
| --- | --- |
| Waiting for Prysm | Verify the saved host panel address is reachable from the TV's network. Check `PRYSM_ALLOWED_HOSTS`. For HTTPS, check certificate trust and proxy WebSocket support. |
| Receiver in use | Another TV has the capture session. The webOS capturer retries when it is free. |
| Connected · Capture off / Ready to capture | Choose Screen sync and turn the lights on in the host panel. |
| Waiting for video / Capturing but no picture | Wake the TV and play video or select an active HDMI input. Check that Preview actually advances; the TV Home screen may provide no frames. |
| Service unavailable / permission denied | Use **Service options → Set up or repair service**. Stop PicCap or other capturers. |
| Start fails after about 12 seconds | Wake the TV, show active video, fix the reported error, and retry. Prysm cancels a start that never confirms. |

## Report a problem

Include the error, code revision, host OS, capture device, WLED address type
(IP or hostname), resolution, and frame rate. For LG capture, include the TV
model and firmware. Redact private addresses and paths before posting logs.

For more detail, see [developer debugging](../developers/debugging.md) and
[monitoring](../developers/monitoring.md).
