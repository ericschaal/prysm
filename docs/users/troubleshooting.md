# Troubleshooting

Start with the error shown in the panel. Prysm starts idle: select **Screen sync**
and turn the lights on, or click **Start capture** in the preview window.

| Symptom | Check / fix |
| --- | --- |
| Panel will not open | On the host try `http://localhost:8787`. On Debian run `systemctl status prysm` and `journalctl -u prysm -n 100`. For a phone, configure [LAN access](configuration.md#access-from-your-phone). |
| Request rejected / invalid Host | Add the exact browser hostname or IP with port to `[web].allowed_hosts`, then restart Prysm. |
| Capture device unavailable | Check its configured path/UID and that no other program owns it. Linux service user needs video access; macOS needs camera permission. |
| Unsupported format or rate | Choose a resolution and frame rate supported by the device; Prysm does not silently accept a different mode. |
| No LEDs, but capture runs | Check WLED's HTTP URL, DDP input, LED counts and wiring. Allow UDP 4048 from Prysm to WLED. |
| Wrong edge or direction | Counts are viewed from the front. Wiring starts bottom-left → up left → across top → down right → along bottom. |
| Dark or washed-out colors | Check [gamma and calibration](configuration.md#color-calibration), brightness, and WLED power limits. |
| File does not play | FFmpeg 9+ must be on PATH for the process running Prysm; check the absolute file path and read permissions. |
| File finished | EOF is normal. The panel stays available; start capture again to replay. |
| Settings will not save | Prysm must be able to replace settings.json and write its parent directory. Reload the panel if another client changed the revision. |
| Restart required | Restart the Prysm process/service; saved output-kind or web-policy changes cannot apply live. |

## TV sender

- **Waiting for Prysm:** open the saved panel URL from the TV network. Check the host allowlist, proxy WebSocket support and certificate trust for HTTPS. The TV retries automatically.
- **Receiver in use:** another TV is capturing. The sender retries when the session ends.
- **Connected · Capture off:** connection is ready. Start screen sync on the host.
- **Capturing but no picture:** show active video on the awake TV; both capture
  modes can wait on Home. Check Prysm for actual advancing frames.
- **Service unavailable / permission denied:** use **Service options → Set up or
  repair service** and inspect diagnostics. Stop competing capturers.
- **Start fails after about 12 seconds:** the host rolls back an unconfirmed start.
  Wake the TV, show video, resolve the reported error, and retry.
- **Invalid configuration after upgrade:** UI-only capture is removed. Follow
  the [saved-configuration reset instructions](webos.md#change-settings-or-remove).

For deeper diagnosis, use the [developer debugging guide](../developers/debugging.md)
and [monitoring setup](../developers/monitoring.md). Include the error, Prysm revision,
OS/TV firmware, input/output type, resolution and frame rate when reporting a problem.
Redact local addresses or paths if you do not want them published.
