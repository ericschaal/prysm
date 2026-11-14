# Debugging

1. Record the version (`prysm-headless --version`), OS, video source, size,
   frame rate and the settings file.
2. Validate the settings without opening devices:

   ```sh
   prysm-headless --check-settings PATH
   ```

3. Read the log: stderr in the foreground, `journalctl -u prysm-headless -f`
   for the Debian package, `docker logs -f prysm` for Docker.
4. Read the state. `failure` has the last lighting failure with its `kind`,
   `section` and `field`; `input` has the measured rate, whether frames arrive
   and the latency; `output_issue` says why WLED is not showing the colours.

   ```sh
   curl http://127.0.0.1:8787/api/v1/state
   curl -N http://127.0.0.1:8787/api/v1/state/stream
   ```

## Isolate the problem

- **Capture or lights?** Use the [live preview](../users/preview.md), or run
  `prysm-tui` with the `null` output: its terminal shows the picture and edge
  colours without any LEDs.
- **Capture hardware?** Replace the device with a known video file (`file`
  source, FFmpeg 9+; see [play a video file](README.md#play-a-video-file)).
- **TV stream?** Watch a slim TV's stream on a desktop without lights: run
  `prysm-headless` (or `prysm-tui`) with the `remote` capture and the `null`
  output, turn lighting on and give the TV the desktop's portal address. The
  portal shows the received rate and the preview; a TV of another stream
  version reports the failure kind `version` (the TV app says "Update Prysm
  and TV app"; the reason in its diagnostics names both versions) and
  retries every 10 s.

## Logs

Log lines are `tracing` events, including those of the shared capture and
processing crates, filtered by `RUST_LOG` (default `warn,prysm=info`):

```sh
RUST_LOG=debug cargo run -p prysm-headless -- settings.json
```

While `prysm-tui` shows its preview, logs go only to `--log-file PATH`. The TV
workers log to stderr, which the supervisor keeps (the last 20 lines show in
the TV app's **View diagnostics**).

For rates and timing over time, enable [metrics](monitoring.md).

## Hardware checks

None of these have been run against Prysm 1.x yet; results belong in
[research](../../research/README.md).

- **Capture card:** start `prysm-tui` with the `device` source and the `null`
  output; check the picture, `input.fps` and the colours. Then the same with
  WLED: on, off, WLED effects, unplugging the card, and stopping Prysm (the
  lights must go off).
- **Docker:** the same with the device mapped into the container.
- **TV, without capture:** the on-TV smoke test runs the built binaries under
  a separate probe identity in a private temporary directory and removes only
  what it created. Read its plan first with `--dry-run`:

  ```sh
  python3 packaging/webos/tests/native-smoke.py --host root@TV --flavour full --dry-run
  ```

  `--supervisor` adds setup, the app-only API and uninstall; `--capture` runs
  the real worker.
- **TV, slim encoder:** with the Prysm service disabled, the installed slim
  worker can check one encoder without capture or a receiver
  (`check-encoder`; see [v1 packaging](../../packaging/README.md#checking-the-slim-h264-encoder-on-a-tv)).
- **TV acceptance:** install, set up, capture with video and without,
  standby and wake, cold boot with **Start with TV**, a hung capture being
  killed, reconnecting the slim stream, calibration, and removal.
  `tv-acceptance.sh` runs the three smoke levels and the install, then asks
  for each manual check and writes the record to
  [research/tv-acceptance](../../research/tv-acceptance/README.md). Read its
  plan first:

  ```sh
  packaging/webos/tests/tv-acceptance.sh --dry-run --flavour full root@TV path/to/io.prysm.tv-full_VERSION_all.ipk
  packaging/webos/tests/tv-acceptance.sh --dry-run --flavour slim --prysm http://HOST:8787 root@TV path/to/io.prysm.tv-slim_VERSION_all.ipk
  ```

Compare release builds with the same content, size, rate and encoding.
Include the actual output rate and failures. Prysm's latency figures stop at
the output write; they do not include the lights or the screen.
