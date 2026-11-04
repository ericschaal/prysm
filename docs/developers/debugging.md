# Debugging

1. Record commit, settings, input/output, firmware/OS, resolution and FPS.
2. Run `prysm --config PATH check` for settings validation.
3. Keep foreground stderr, or `journalctl -u prysm -f` on Debian.
4. Inspect pipeline/errors and saved settings:

```sh
curl http://127.0.0.1:8787/api/state
curl http://127.0.0.1:8787/api/settings
```

Use a known video file to isolate capture hardware; use browser preview to isolate
WLED. EOF/cancellation succeed; capture failure leaves HTTP available.

## Runtime logs

```sh
RUST_LOG=info,prysm::preview=debug prysm --config PATH
RUST_LOG=info,prysm::stats=off prysm --config PATH
```

`prysm::stats` reports active-stage windows about every 30 seconds and on exit.
`total_*` fields are lifetime counters. Timing includes input waits, not just CPU.
Pool `exhausted` means readers held every slot, requiring replacement storage.
Disabling stats skips per-frame timing. See [metrics](monitoring.md) for more.

## Hardware checks

- Capture card: `cargo run -p prysm-capture --example capture_smoke` checks three
  frames at 800×600/30 Hz; adjust the example to a supported device mode.
- Preview: check start/stop, EOF/replay and matching picture/edge colors.
- TV: run the [isolated smoke test](../../apps/prysm-webos-service/README.md#isolated-hardware-test),
  then test reconnect, absent video, standby/wake, cold boot/autostart and uninstall.

Compare release builds with identical content, dimensions, FPS and codec. Include
actual output rate and failures. Host fixtures do not qualify native drivers;
local sends and receiver gaps do not measure physical screen-to-light latency.
