# webOS lifecycle and permissions

The [webOS capturer](../../apps/prysm-webos-capture/README.md) runs as a worker
owned by the [Luna supervisor](../../apps/prysm-webos-service/README.md).
Installation: [TV setup](../users/webos.md).

## Capture and power lifecycle

- Run one worker only while enabled and TV power is confirmed Active. Manual Stop
  survives standby/wake; cold boot uses saved autostart. Host Prysm starts idle.
- Connect to `/api/capture`; idle connections open no capture devices. Screen sync
  supplies dimensions/FPS. Off/effects ends the session; restart with fresh native
  and codec state. The supervisor owns retries and reap-before-restart.
- Both capture modes wait for videooutput MAIN readiness. Context changes reset
  the client; valid black/muted frames do not expire.
- Native operations have a five-second watchdog; preparation has ten seconds.
  Stop/standby sends TERM, then KILL after five seconds if cleanup blocks.
- Typed environment config, stdin readiness and a separate event pipe connect the
  processes. Do not mix IPC with vendor stdout or pass settings through a shell.

## Luna permissions and persistent files

Setup validates owned files, registers separate capture/supervisor roles, bounds
Luna rescan/activation and checks the reply. Only the setup app's Luna identity may
mutate settings. Never overwrite unrelated manifests/hooks or LG system manifests.

| Path | Purpose |
| --- | --- |
| `/var/luna-service2/manifests.d/io.prysm.capture.native.json` | Capture role registration |
| `/var/lib/webosbrew/init.d/prysm-capture` | Restore registration/activate at boot |
| `/var/lib/prysm-capture/config.json` | Validated TV settings, atomically saved in a root-only directory |

Before uninstalling: **Service options → Remove service setup… → Remove setup**.
This stops capture and removes its registration; settings remain. Removing the
IPK first can leave stale registration because the package has no removal script.

## TV app interaction

Arrows move focus, OK edits, Save applies, Back cancels/leaves. Preserve text-cursor
movement, visible focus and drafts across status reconnects. Closing the app leaves
the webOS capturer running. [Frontend guide](frontend.md#webos-tv-app).
