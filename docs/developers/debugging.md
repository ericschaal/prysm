# Debugging

For installation problems, start with [user troubleshooting](../users/troubleshooting.md).
Use a disposable settings file when testing changes.

## Reproduce and inspect

1. Record the commit, settings, input/output, OS or TV firmware, resolution and FPS.
2. Run `prysm --config PATH check`. It validates settings, not hardware availability.
3. Run in the foreground and keep stderr; for Debian use `journalctl -u prysm -f`.
4. Inspect settings, pipeline phase, errors and operation status:

```sh
curl http://127.0.0.1:8787/api/settings
curl http://127.0.0.1:8787/api/state
```

Use a known video file to remove capture hardware from the problem. Use browser
preview to check processing before investigating WLED. Keep content and settings
fixed when comparing runs. EOF and cancellation are normal completion; capture
failure should leave HTTP available. Pending operations explain disabled controls;
do not bypass the controller to force a second operation.

## Runtime logs

The host logs lifecycle events to stderr at INFO. Select targets with `RUST_LOG`:

```sh
RUST_LOG=info,prysm::preview=debug prysm --config PATH
RUST_LOG=info,prysm::stats=off prysm --config PATH
```

`prysm::stats` reports active-stage summaries roughly every 30 seconds and on exit
(`final_report=true`). Reporting follows activity; idle workers do not heartbeat.
`frames`, `fps`, `bytes`, `failures` and `cached` cover the current window;
`total_*` counters cover the worker lifetime. Preview cache hits avoid encoding.

`mean_ms`/`max_ms` measure successful wall time, including input waits for capture
and receive stages. They are not CPU usage. WLED bytes exclude packet headers,
and a write means local socket acceptance, not delivery to LEDs.

Pool counters distinguish `created`, `reused`, `grown` and `exhausted` buffers.
Exhaustion means consumers held all slots, requiring replacement storage; it is
not an allocation failure. Retained bytes exclude displaced buffers still owned
by readers. Counts describe storage buffers, not allocator calls.

Filters accept comma-separated levels and `target=level`, not span expressions.
Invalid levels fail startup. Disabling stats skips per-frame timing. Repeated
capture/encode warnings are limited to one per reporting window.

## Metrics and performance

[Enable monitoring](monitoring.md), then use the
[metric catalog](../../observability/CATALOG.md) to identify the stage measured.
Compare the same release build, content, resolution, FPS and codec, including
actual throughput and failures. Receiver gaps and local send timings do not
measure physical screen-to-light latency.

## Hardware checks

- **Capture card:** `cargo run -p prysm-capture --example capture_smoke` checks
  three frames from the default device at 800×600/30 Hz. Change the example's
  requested mode if the device does not support it.
- **Preview:** check start/stop, EOF, replay and matching picture/edge colors using
  the [preview guide](../users/preview.md).
- **TV:** follow the supervisor's [isolated smoke test](../../apps/prysm-webos-service/README.md#isolated-hardware-test).
  Keep its isolation, watchdog and cleanup. Capture probes require competing
  capture to be stopped.
- **TV lifecycle:** check repeated start/stop, reconnect, absent video,
  standby/wake, cold boot/autostart and uninstall on the target firmware.

Host fixtures cannot qualify LG drivers or another firmware. For ABI problems,
read the [supervisor boundary](../../apps/prysm-webos-service/README.md#unsafe-boundary-contract)
and [worker contract](../../apps/prysm-webos-capture/README.md).
