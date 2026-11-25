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
   and the latency; `device.issue` says why WLED is not showing the colours,
   and `device.brightness_percent` how far WLED's own brightness dims them.

   ```sh
   curl http://127.0.0.1:8787/api/v1/state
   curl -N http://127.0.0.1:8787/api/v1/state/stream
   ```

## Isolate the problem

- **Capture or lights?** Use the [live preview](../users/configuration.md#live-preview), or run
  `prysm-tui` with the `null` output: its terminal shows the picture and edge
  colours without any LEDs.
- **Capture hardware?** Replace the device with a known video file (`file`
  source, FFmpeg 9+; see [play a video file](README.md#play-a-video-file)).
- **TV stream?** Watch a slim TV's stream on a desktop without lights: run
  `prysm-headless` (or `prysm-tui`) with the `remote` capture and the `null`
  output, turn lighting on and give the TV the desktop's portal address. The
  portal shows the received rate and the preview; a TV of another stream
  version reports its link as `version_differs` (the TV app says "Update
  Prysm or TV app"; the reason in its diagnostics names both versions) and
  retries every second.

## Logs

Log lines are `tracing` events, including those of the shared capture and
processing crates, one per line as `<process>: <level> <target>: <message>
<field>=<value>...`, filtered by `RUST_LOG` (default `warn,prysm=info`)
([prysm-logging](../../crates/engine/logging/src/lib.rs)):

```sh
RUST_LOG=debug cargo run -p prysm-headless -- settings.json
```

While `prysm-tui` shows its preview on the terminal stderr writes to, logs go
only to `--log-file PATH`; a panic's message is printed once the terminal is
restored. On the TV, the last 20 lines of the supervisor and its worker,
oldest first, show in the TV app's **View diagnostics** (the state's `log`;
[how](webos.md#supervisor-and-worker)). The TV starts the supervisor with
stderr on `/dev/null`, so that is the only place its lines show under Luna
activation. The boot hook's `setup` runs without the service and writes to
`/tmp/io.github.ericschaal.prysm-setup.log`; a failed setup from the app
shows its stderr as the failure. Each new line is published to `getState`
subscribers, so nothing the supervisor logs at `info` or above repeats on
its own: its subscription renewals are `debug`.

For rates and timing over time, enable [metrics](monitoring.md).

## Memory safety and fuzzing

CI checks the unsafe, threaded and parsing code with tools that need a
nightly Rust: the one [ci.yml](../../.github/workflows/ci.yml) pins for Miri.
Install it once, with cargo-fuzz for the fuzz targets:

```sh
rustup toolchain install nightly-2026-11-01 --component miri,rust-src
cargo install cargo-fuzz --locked --version 0.13.1
```

### Miri

The `miri` job's steps name the tests Miri can run: Miri cannot call C, so a
test that reaches `dlopen`, the C++ bridge, a socket or a file-backed `mmap`
stops the whole run. For example:

```sh
cargo +nightly-2026-11-01 miri test -p prysm-core --lib --test frame_limits -- frame:: expected_size_matches
cargo +nightly-2026-11-01 miri test -p prysm-core --test frame_limits --target armv7-unknown-linux-gnueabi
```

The second checks the frame sizes with the TV's 32-bit `usize`.

### Sanitizers

The `sanitizer` jobs run the tests of the unsafe and threaded crates under
AddressSanitizer and ThreadSanitizer. Use your host's triple
(`x86_64-unknown-linux-gnu` in CI; `aarch64-apple-darwin` on a Mac, which has
no LeakSanitizer). The job's step lists every crate; these are the unsafe
ones:

```sh
RUSTFLAGS=-Zsanitizer=address cargo +nightly-2026-11-01 test -Zbuild-std \
  --target x86_64-unknown-linux-gnu --lib --tests \
  -p prysm-capture-webos-sys -p prysm-webos-luna-sys -p prysm-capture-device-sys
```

`-Zsanitizer=thread` for ThreadSanitizer. Code built by `cc` (the C++
bridge, OpenH264, ring) is not instrumented.

### Fuzzing

[fuzz/](../../fuzz/Cargo.toml) is its own workspace with one target per
decoder or size check; the nightly workflow runs each for ten minutes. Each
target asserts what the code promises, not only that it does not panic. Run
one for a minute, bounded in memory:

```sh
cd fuzz
cargo +nightly-2026-11-01 fuzz list
mkdir -p corpus/frame_header
cargo +nightly-2026-11-01 fuzz run frame_header corpus/frame_header seeds/frame_header -- -max_total_time=60 -rss_limit_mb=2048
```

A crash is saved under `fuzz/artifacts/TARGET/`; CI prints its bytes and
keeps it as an artifact. Replay it with
`cargo +nightly-2026-11-01 fuzz run TARGET PATH`.

## Hardware checks

No run of these against Prysm 1.x is recorded yet (the only recorded 1.x
measurement on a TV is the processor's benchmark,
[sampler speed](../../research/processing/2026-11-15-sampler-speed/README.md));
results belong in [research](../../research/README.md), TV acceptance in its
[records](../../research/tv-acceptance/README.md). The
[acceptance runner](acceptance.md) automates most of them (desktop, both TV
packages) and asks for the rest.

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
  (`check-encoder`; see [packaging](../../packaging/README.md#checking-the-slim-h264-encoder-on-a-tv)).
- **TV acceptance:** install, set up, capture with video and without,
  standby and wake, cold boot with **Start with TV**, a hung capture being
  killed, reconnecting the slim stream, calibration, and removal.
  `tv-acceptance.sh` runs the three smoke levels and the install, then asks
  for each manual check and writes the record to
  [research/tv-acceptance](../../research/tv-acceptance/README.md). Read its
  plan first:

  ```sh
  packaging/webos/tests/tv-acceptance.sh --dry-run --flavour full root@TV path/to/io.github.ericschaal.prysm-full_VERSION_all.ipk
  packaging/webos/tests/tv-acceptance.sh --dry-run --flavour slim --prysm http://HOST:8787 root@TV path/to/io.github.ericschaal.prysm-slim_VERSION_all.ipk
  ```

Compare release builds with the same content, size, rate and encoding.
Include the actual output rate and failures. Prysm's latency figures stop at
the output write; they do not include the lights or the screen.
