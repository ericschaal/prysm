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

## Processing location

Saved preferences retain both profiles. The Rust service resolves the selected location to a typed capture target before verifying or saving it. A remote target contains the receiver and codec configuration. The embedded target contains the WLED output and capture controls. A failed receiver probe or settings write leaves the running target in place. The process adapter launches the selected executable after the previous child exits. Both workers share the capture lock, supervisor video controls, event channel and power policy.

`cargo xtask package webos --embedded-prysm` enables the additional GUI, service capability, executable and capture role. The app reads the installed capability before exposing embedded setup. The service snapshots local WLED settings into the child configuration at launch. Embedded processing has no capture transport or encoder. The prior standalone settings-file entry remains available for research replay.

The first extended save creates `config-v2.json` in the private service directory. Feature-aware builds prefer that file. A standard build falls back to remote mode while retaining the embedded preferences. A pre-feature release reads its last `config.json` snapshot, or starts in receiver setup if there was none. Changes made by a pre-feature release during a rollback affect only its legacy snapshot.

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
| `/var/lib/prysm-capture/config-v2.json` | Atomic preferences file after the first extended save |

Before uninstalling: **Service options → Remove service setup… → Remove setup**.
This stops capture and removes its registration; settings remain. Removing the
IPK first can leave stale registration because the package has no removal script.

## Embedded acceptance on hardware

On November 4, 2026 the experimental IPK’s UI and native workers were exercised on the rooted test TV under an isolated app/service identity. With no receiver URL, saving the WLED setup and enabling capture produced DDP from the TV. Selecting an unreachable receiver kept the local worker’s PID, settings and output. The selection survived reopening the GUI. Selecting a reachable test receiver handed off capture and delivered three native frames; selecting embedded again resumed DDP. The standard service then loaded the same preferences, returned to remote mode and rejected embedded selection through Luna. The installed production app, service configuration and foreground were restored. Physical standby/wake was not part of this trial.

## TV app interaction

Arrows move focus, OK edits, Save applies, Back cancels/leaves. Preserve text-cursor
movement, visible focus and drafts across status reconnects. Closing the app leaves
the webOS capturer running. [Frontend guide](frontend.md#webos-tv-app).
