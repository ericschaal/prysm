# webOS lifecycle and permissions

The [webOS capturer](../../apps/prysm-webos-capture/README.md) runs as a worker
owned by the [Luna supervisor](../../apps/prysm-webos-service/README.md).
Installation: [TV setup](../users/webos.md).

## Capture and power lifecycle

- In remote mode, run one worker only while enabled and TV power is confirmed Active. Manual Stop
  survives standby/wake; cold boot uses saved autostart. Host Prysm starts idle.
- The remote worker connects to `/api/capture`; idle connections open no capture devices. Screen sync
  supplies dimensions/FPS. Off/effects ends the session; restart with fresh native
  and codec state. The supervisor owns retries and reap-before-restart.
- Both capture modes wait for videooutput MAIN readiness. Context changes reset
  the client; valid black/muted frames do not expire.
- Native operations have a five-second watchdog; preparation has ten seconds.
  Stop/standby sends TERM, then KILL after five seconds if cleanup blocks.
- Typed environment config, stdin readiness and a separate event pipe connect the
  processes. Do not mix IPC with vendor stdout or pass settings through a shell.

## Processing location

The supervisor resolves the saved processing location into one worker. Remote mode verifies the configured receiver before handing over capture. Embedded mode launches the shared Prysm runtime with a native LG frame source and the settings file in the private service directory. It serves the existing portal on port 8787 as soon as the TV is awake. Only the configured Screen sync pipeline can open the capture device. Pausing capture or losing video releases the device and leaves HTTP available. Mode changes reap the previous worker before starting its replacement.

The same `embedded-prysm` flag includes the local executable, its Luna role, and the processing-location UI. The TV displays the portal address after the child reports a successful bind. The shared settings store owns WLED, LED layout and processing preferences. The native runtime accepts only the webOS input and has no remote video ingestion route. Preview and calibration use the existing portal protocols.

When upgrading the earlier experimental package, the supervisor copies the saved WLED configuration into a new portal settings file only if none exists. It keeps the old preferences snapshot for rollback. Switching between embedded and remote processing retains the portal settings and saved receiver address.

## Luna permissions and persistent files

Setup validates owned files, registers separate capture/supervisor roles, bounds
Luna rescan/activation and checks the reply. Only the setup app's Luna identity may
mutate settings. Never overwrite unrelated manifests/hooks or LG system manifests.

| Path | Purpose |
| --- | --- |
| `/var/luna-service2/manifests.d/io.prysm.capture.native.json` | Capture role registration |
| `/var/lib/webosbrew/init.d/prysm-capture` | Restore registration/activate at boot |
| `/var/luna-service2/manifests.d/io.prysm.capture.embedded.native.json` | Optional embedded capture role registration |
| `/var/lib/prysm-capture/config.json` | Original remote settings, retained for pre-feature releases |
| `/var/lib/prysm-capture/config-v2.json` | Previous experimental preferences, retained for rollback |
| `/var/lib/prysm-capture/config-v3.json` | Service preferences for processing location and TV capture |
| `/var/lib/prysm-capture/portal-settings.json` | WLED, processing and calibration settings owned by the portal |

Before uninstalling: **Service options → Remove service setup… → Remove setup**.
This stops capture and removes its registration; settings remain. Removing the
IPK first can leave stale registration because the package has no removal script.

## Embedded acceptance on hardware

The November 4, 2026 portal trial used an isolated app and service identity on the rooted LG TV. With no receiver, WLED output, or video, the TV displayed a LAN address and a desktop browser opened the existing setup wizard. With capture paused, the wizard saved a 162-LED layout. Closing and reopening the TV GUI left the portal and its settings available.

At 1280 × 720, screen sync delivered native frames to the portal preview and DDP packets to a loopback WLED fixture. Pausing capture stopped DDP and released the exclusive capture lock while the portal stayed live. Restarting the supervisor restored the layout with capture disabled. The TV displayed the calibration QR code from its own portal. Switching to a verified remote receiver closed the local listener, and switching back restored the layout. The production service and foreground app were restored. The trial did not exercise physical standby or a cold boot.

## TV app interaction

Arrows move focus, OK edits, Save applies, Back cancels/leaves. Preserve text-cursor
movement, visible focus and drafts across status reconnects. Closing the app leaves
the webOS capturer running. [Frontend guide](frontend.md#webos-tv-app).
