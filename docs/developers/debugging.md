# Debugging

For common installation problems, start with [user troubleshooting](../users/troubleshooting.md).
Use a disposable configuration when investigating changes; avoid overwriting a live installation.

## Reproduce and inspect

1. Record commit, configuration, input/output, OS or TV firmware, resolution and rate.
2. Validate with `prysm --config PATH check`. This checks configuration, not hardware availability.
3. Run Prysm in the foreground and retain stderr. On Debian use `journalctl -u prysm -f`.
4. Inspect `GET /api/state` for pipeline phase, errors and operation status; inspect
   `GET /api/settings` for the configuration/revision. See [HTTP contracts](control-api.md).
5. Isolate the boundary: a known video file removes capture hardware; browser preview isolates
   WLED transport. Keep resolution, source content and processing settings fixed when comparing.

```sh
curl http://127.0.0.1:8787/api/state
curl http://127.0.0.1:8787/api/settings
```

Capture failures should leave HTTP available. EOF and cancellation are normal
completion. A pending operation or restart requirement can explain disabled controls;
do not bypass the controller to force another lifecycle operation.

## Runtime logs

The host application logs to stderr at INFO by default. Lifecycle events include
pipeline start/ready/stop/failure, configuration changes, operations, webOS sender
connections, input format changes and signal loss/recovery.

The `prysm::stats` target summarizes each active stage roughly every 30 seconds
and once when its worker exits. Reporting happens on activity, so idle workers do
not emit heartbeats. `final_report=true` identifies the closing summary.

- `frames`, `fps`, `bytes`, `failures` and `cached` describe the current window;
  `total_frames`, `total_failures` and `total_cached` cover the worker's lifetime.
  Preview cache hits avoid an encode and are counted separately from frames.
- `mean_ms` and `max_ms` measure successful operations' wall time. `file_read`,
  `webos_receive` and `v4l_capture` include input waits; `avfoundation_copy` measures
  copying a delivered frame. These are not CPU utilization measurements.
- Processing bytes describe input pixels, preview bytes describe encoded JSON,
  and WLED bytes describe RGB payloads, excluding packet headers. A successful
  WLED write means local socket acceptance, not confirmed delivery to the lights.
- `pool` is the buffer snapshot from the last successful frame. Its cumulative
  `created`, `reused` and `grown` counters distinguish new storage, reuse without
  growth and capacity growth. `exhausted` counts acquisitions when every slot was
  held by a consumer, requiring replacement storage; it is not an allocation error.
  `retained_buffers` and `retained_bytes` cover the pool's current slots, excluding
  displaced buffers still held by consumers. Counts describe buffers, not allocator
  calls, and include abandoned writes.

Use `RUST_LOG` to select levels by target prefix:

```sh
# Keep lifecycle events, silence periodic statistics.
RUST_LOG=info,prysm::stats=off prysm --config PATH
# Also show preview viewer connections, response counts and busy responses.
RUST_LOG=info,prysm::preview=debug prysm --config PATH
```

The filter accepts comma-separated levels and `target=level` directives, not span
expressions. Invalid level directives fail startup. Disabling `prysm::stats`
skips per-frame timing; producer-local pool counters remain available. Repeated
capture/encode warnings are limited to the first failure per reporting window.

## Metrics and performance

Follow [monitoring](monitoring.md) to enable the host exporter and optional TV metrics.
Use the [catalog](../../observability/CATALOG.md) to distinguish captured, decoded,
processed and transmitted frames. A local socket write does not prove remote delivery,
and receiver frame gaps do not measure screen-to-light latency.

Compare the same optimized build, resolution, frame rate, source and codec. Include
actual output rate and failures when comparing CPU or compression. Research results
are scoped to their stated hardware and workload; they are not universal performance targets.

## Hardware checks

- **Capture device:** use `cargo run -p prysm-capture --example capture_smoke`
  for the default local device. The example requests 800×600 at 30 Hz and checks
  three frames; choose a supported mode in the example if your device rejects it.
  This is a diagnostic, not an installed application interface.
- **Browser preview:** follow the [preview guide](../users/preview.md);
  check start/stop, EOF/replay, window close and agreement with the web panel.
- **webOS:** use the supervisor's [isolated smoke procedure](../../apps/prysm-webos-service/README.md#isolated-hardware-test).
  The optional capture probe requires root SSH, changes temporary Luna registrations
  and must run with competing capture stopped. Keep its watchdog and cleanup.
- **TV lifecycle:** verify repeated start/stop, disconnect/reconnect, video absence,
  standby/wake, cold boot/autostart and uninstall on the target firmware. Earlier
  successful tests do not qualify new firmware or capture/encoder revisions.

For native ownership and ABI failures, read the [supervisor boundary contract](../../apps/prysm-webos-service/README.md#unsafe-boundary-contract)
and [worker contract](../../apps/prysm-webos-capture/README.md).
