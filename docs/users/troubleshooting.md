# Troubleshooting

Start with the light card on the portal's Home: it says what is wrong and
offers the fix. **Your setup** marks the step that failed. Prysm starts with
the lights off after every restart.

## Portal and settings

| Problem | Fix |
| --- | --- |
| Portal does not open | On the computer, try `http://localhost:8787`. From a phone, use the computer's IP address and port 8787. Check the log for the address Prysm printed and for errors. |
| "origin denied" | A page from another site tried to use the portal. To allow a specific page, start Prysm with `--allow-origin ORIGIN`; the slim TV app needs `--allow-origin null`. Behind an HTTPS reverse proxy the portal's own pages work when the proxy forwards the `Host` header; a proxy that rewrites it needs `--allow-origin https://` plus the proxy's address. |
| Cannot save settings | Prysm's user must be able to write the settings directory. In Docker, mount a directory at `/data`, writable by user 10001. |
| "These settings were changed elsewhere" | Someone saved in another browser, or Prysm restarted. The form shows the current settings; make your change again. |
| Saved, but lights fail | The settings are saved; the light card shows why the start failed. Fix the device or address and turn Screen sync on again. |
| Prysm starts without settings, and the light card names the settings file | The file is damaged, from another Prysm version, or not readable by Prysm's user. Run `prysm-headless --check-settings PATH` to see the error, fix the file and restart; or set Prysm up again in the portal, which keeps the old file beside the new one (`settings.json.unreadable-…`). |

Logs:

```sh
# Debian
journalctl -u prysm-headless -n 100

# Docker
docker logs --tail 100 prysm
```

## Picture and lights

Play the [live preview](configuration.md#live-preview) on Home. No or frozen picture: check the video source.
Moving picture but wrong LEDs: check WLED and the LED counts.

| Problem | Fix |
| --- | --- |
| Capture card cannot open | Check the device path or ID and that no other app uses the card. Linux: Prysm's user needs the `video` group. macOS: the card needs camera access for the app that starts Prysm (such as Terminal). Allow it when macOS asks, or under **System Settings → Privacy & Security → Camera**, then restart that app; if macOS never asks, start Prysm from Terminal. Docker: map the device and its group. |
| Unsupported mode | Choose a size and rate the card supports. On Linux it must offer YUYV, RGB3 or BGR3; MJPEG-only cards do not work. |
| Capture device delivered unusable buffers in a row | The card sends a broken or incomplete picture. Check the cable and the source's signal, or choose another size and rate. |
| Waiting for video | Prysm runs but no picture arrives. Check the source is playing. After about 3 seconds WLED returns to its own state until video comes back. |
| No LEDs | Check the WLED address, that WLED allows DDP input, and the LED counts. Allow HTTP and UDP port 4048 from Prysm to WLED. In Docker, enter WLED by IP address. |
| Light card says WLED is off, overridden or not live | Something else changed WLED (its power button, a live override, another app sending colours). Prysm keeps sending; fix it in WLED. |
| LED counts differ | WLED's own LED count (**Config → LED Preferences**) is not the sum of Prysm's edges: with fewer in Prysm the last LEDs stay dark, with more the last edge is cut short. Correct whichever is wrong. |
| LEDs run the wrong way | The strip must start at the bottom-left corner and run clockwise (seen from the front). Check the per-edge counts. |
| Dark or washed-out colours | Check WLED's brightness and power limit. With a settings PIN in WLED, Prysm cannot read its gamma: set gamma for colour to 2.2 ([prepare WLED](configuration.md#1-prepare-wled)). |
| Dark scenes twinkle or shimmer | Turn off **Smooth dark colours** under **Advanced** on the Lights page ([lights](configuration.md#video-source-and-lights)). |
| Dim colours fade in steps or change colour as they fade | Keep **Smooth dark colours** on (the default) under **Advanced** on the Lights page. |
| Black bars still lit | Wait a second or two after the picture changes. Very dark scenes keep the last crop. |

## LG TV

| Message or problem | Fix |
| --- | --- |
| Prysm needs setting up | Choose **Set up service**. After every install or update the service must be set up again. |
| TV is asleep, Screen saver is on | Capture continues when the TV wakes. |
| Waiting for video | Play video or use an active HDMI input. The Home screen may give no picture. |
| Screen sync stopped, Capture stopped | The TV tries again by itself and counts down. If it repeats, the reason is under **Service options → View diagnostics**. Stop other capture apps. |
| Couldn’t read settings | The TV's saved settings could not be read, so Prysm uses its defaults and leaves the file as it is. Change a setting (**Start with TV**, or for slim the **Prysm address**) to save new ones. **View diagnostics** says what was wrong. |
| Slim: Waiting for Prysm | Check the **Prysm address** (the one you open the portal with), that Prysm on the computer has the **LG TV on your network** source with its lights on, and, behind a reverse proxy, that it passes WebSockets on `/api/v1/capture/stream` without a login. |
| Slim: Update Prysm or TV app | Prysm on the computer and the TV app send the picture in different ways. Update the older of the two; the TV checks again every second, so there is no need to restart it. |
| Slim: Another TV took over | Another TV sends its picture to the same Prysm, so this one stopped. It sends again when you choose **Stop capture**, then **Start capture**, when the TV wakes from standby, or when you change the Prysm address. |
| Slim: Paused by Prysm | Normal while the computer's lights are off. Turn on Screen sync. |
| Slim: Prysm stopped receiving | Prysm on the computer could not use the TV's picture (for example, no H.264 decoder starts there). Its web portal says why; choose another encoding for the video source there, or turn Screen sync on again. The TV keeps trying. |
| Slim: the TV app cannot reach or control Prysm ("doesn't take commands from the TV") | Check **Prysm address**. Start the computer's Prysm with `--allow-origin null`. |
| Slim: Encoding not supported | Choose **Software H.264** or **Uncompressed** for the video source in the portal. |
| Full: no portal address on the TV | The TV has no network address yet. Check its network connection. |

## Report a problem

Include the message from the portal or TV, the Prysm version
(`prysm-headless --version`), the computer's OS, the video source, size and
frame rate. For a TV, add the model, webOS version, package (full or slim) and
the text from **View diagnostics**. Remove private addresses from logs.
See [developer debugging](../developers/debugging.md).
