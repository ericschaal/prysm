# Troubleshooting

Start with the light card on the portal's Home: it says what is wrong and
offers the fix. **Your setup** marks the step that failed. Prysm starts with
the lights off after every restart.

## Portal and settings

| Problem | Fix |
| --- | --- |
| Portal does not open | On the computer, try `http://localhost:8787`. From a phone, use the computer's IP address and port 8787. Check the log for the address Prysm printed and for errors. |
| "origin denied" | A page from another site tried to use the portal. To allow a specific page, start Prysm with `--allow-origin ORIGIN`; the slim TV app needs `--allow-origin null`. |
| Cannot save settings | Prysm's user must be able to write the settings directory. In Docker, mount a directory at `/data`, writable by user 10001. |
| "These settings were changed elsewhere" | Someone saved in another browser, or Prysm restarted. The form shows the current settings; make your change again. |
| Saved, but lights fail | The settings are saved; the light card shows why the start failed. Fix the device or address and turn Screen sync on again. |
| Prysm does not start | The settings file may be damaged. Run `prysm-headless --check-settings PATH` to see the error. |

Logs:

```sh
# Debian
journalctl -u prysm-headless -n 100

# Docker
docker logs --tail 100 prysm
```

## Picture and lights

Open [Live preview](preview.md). No or frozen picture: check the video source.
Moving picture but wrong LEDs: check WLED and the LED counts.

| Problem | Fix |
| --- | --- |
| Capture card cannot open | Check the device path or ID and that no other app uses the card. Linux: Prysm's user needs the `video` group. macOS: allow camera access. Docker: map the device and its group. |
| Unsupported mode | Choose a size and rate the card supports. On Linux it must offer YUYV, RGB3 or BGR3; MJPEG-only cards do not work. |
| Waiting for video | Prysm runs but no picture arrives. Check the source is playing. After about 3 seconds WLED returns to its own state until video comes back. |
| No LEDs | Check the WLED address, that WLED allows DDP input, and the LED counts. Allow HTTP and UDP port 4048 from Prysm to WLED. In Docker, enter WLED by IP address. |
| Light card says WLED is off, overridden or not live | Something else changed WLED (its power button, a live override, another app sending colours). Prysm keeps sending; fix it in WLED. |
| LEDs run the wrong way | The strip must start at the bottom-left corner and run clockwise (seen from the front). Check the per-edge counts. |
| Dark or washed-out colours | Check WLED's gamma settings ([prepare WLED](configuration.md#1-prepare-wled)), brightness and power limit. |
| Black bars still lit | Wait a second or two after the picture changes. Very dark scenes keep the last crop. |

## LG TV

| Message or problem | Fix |
| --- | --- |
| TV service unavailable | Choose **Set up service**. After every install or update the service must be set up again. |
| Waiting for TV | The TV is asleep or in standby. Capture resumes when it wakes. |
| Waiting for video | Play video or use an active HDMI input. The Home screen may give no picture. |
| Capture needs attention, retrying | The TV shows the reason and a countdown. If it repeats, see **Service options → View diagnostics**. Stop other capture apps. |
| Slim: Waiting for Prysm | Check **Frames go to** (address and port), that Prysm on the computer has the **Capture-only TV (remote)** source with that port, and that the port is reachable. |
| Slim: Paused by Prysm | Normal while the computer's lights are off. Turn on Screen sync. |
| Slim: the TV app cannot reach or control Prysm | Check **Prysm address**. Start the computer's Prysm with `--allow-origin null`. |
| Slim: Screen sync stops with an encoder error | Choose **Software H.264** or **Uncompressed** for the video source in the portal. |
| Full: no portal address on the TV | The TV has no network address yet. Check its network connection. |

## Report a problem

Include the message from the portal or TV, the Prysm version
(`prysm-headless --version`), the computer's OS, the video source, size and
frame rate. For a TV, add the model, webOS version, package (full or slim) and
the text from **View diagnostics**. Remove private addresses from logs.
See [developer debugging](../developers/debugging.md).
