# Hardware acceptance runner

The runner, the package [`scripts/acceptance/prysm_acceptance`](../../scripts/acceptance/prysm_acceptance/run.py),
checks Prysm's artifacts on real devices: WLED, a capture card, a Linux
computer over SSH, and both TV packages. It is run by hand, never by CI or a build, and it never builds:
it tests the artifacts the configuration names (`cargo xtask dist`,
[packaging](packaging.md#artifacts-in-dist)). A run writes its report and
evidence (snapshots, logs, CSV tables) to `work/acceptance/<time>-<stage>/`,
which is not tracked. Copy what matters into
[research](../../research/README.md). Where the runner is going next (Linux,
the package and the image, the TV on the new configuration) is in the
[plan](acceptance-plan.md).

```sh
cp scripts/acceptance/config.example.toml scripts/acceptance/config.toml   # then fill in
cargo xtask dist mac                                                        # the artifact under test
uv run --no-dev --project scripts/acceptance acceptance host mac --dry-run
uv run --no-dev --project scripts/acceptance acceptance host mac --source file
uv run --no-dev --project scripts/acceptance acceptance host mac --source device [--display-check]
uv run --no-dev --project scripts/acceptance acceptance host linux [--source file|device]
uv run --no-dev --project scripts/acceptance acceptance tv --tv-is-free [--flavour full] [--sweep] [--wss-url https://HOST]
uv run --no-dev --project scripts/acceptance acceptance eyes
uv run --no-dev --project scripts/acceptance acceptance restore-wled work/acceptance/RUN/CELL/wled-snapshot.json
```

Python 3.11+ and [uv](https://docs.astral.sh/uv/). `scripts/acceptance` is a
uv project (`pyproject.toml`, `uv.lock`); `uv run --project
scripts/acceptance` from the repository root installs it into
`scripts/acceptance/.venv` and runs its `acceptance` command. A bench run
needs no dev group (`--no-dev`). Its libraries are
[pytest](https://docs.pytest.org/), which runs the host cells, and
[PyChromecast](https://github.com/home-assistant-libs/pychromecast), imported
only by a capture card run with the Chromecast feed. `--dry-run` prints what
would run, with every command and HTTP call (TV commands as they run there),
and contacts nothing. The exit code is 0 when nothing failed (skips allowed),
1 on a failure, 2 on bad usage. `config.toml` (ignored by git) describes the
bench (see [`config.example.toml`](../../scripts/acceptance/config.example.toml)):
WLED and the LED counts, `[checks]` (sample length, frame-rate tolerance),
`[mac]` (`target = "local"`; an `ssh://` target is refused as not
implemented), `[linux]` (`target = "ssh://USER@HOST"` or `"local"`), the TV and its packages, the receiver; no address is written
in the code.

## Stages

| Stage | Needs | Person |
| --- | --- | --- |
| `host mac` | WLED; `[mac] binary`; FFmpeg 9+ for the file source, or a capture card (`[mac] device`; with a feed: FFmpeg, and for the Chromecast adb) | Only for `--display-check` set-up |
| `host linux` | WLED; `[linux] binary` and `target`; over SSH a key the runner can use without a prompt; FFmpeg here for the clip; FFmpeg 9+ on the target for the file source, or a capture card on it (`[linux] device`; with a feed: adb here) | No |
| `tv` | The TV (only when its owner says it is free: `--tv-is-free`), WLED, the IPKs `[tv] ipk_full`, `ipk_slim`; slim with the local receiver: `[mac] binary` | Standby/wake, and removing the app unless `tv.ares_device` is set |
| `eyes` | A portal (the TV's, or `--portal`) | Every step |

### host

The host cells run under pytest: a cell is a parameter (platform, delivery,
source), so `mac-binary-file`, `mac-binary-device`, `linux-binary-file` and
`linux-binary-device` today, and the Debian package and the image in a later
phase. `acceptance host mac` runs both Mac cells, `acceptance host linux`
both Linux cells; `--source file` or `--source device` names one. Other
arguments go to pytest, so `-k fps`, `-m file` or `-x` select and stop as
pytest does. A cell whose artifact or tool is missing (no `binary`, no
`device`, no FFmpeg) is a SKIP with the reason; a cell named with `--source`
fails instead. The run directory has `report.xml` (JUnit, with each check's
detail), each cell's `commands.log`, `headless.log`, `artifact.json`,
`wled-snapshot.json` and `wled-cfg.json` (WLED's configuration, as evidence),
and `evidence/CHECK.json`; a Linux target over SSH adds `kernel-usb.log`.
A failure prints its evidence, the log paths and the end of the host's log
(and of `kernel-usb.log`), fetched from the target first.

The fixtures set up, in order, and their teardowns undo it in reverse, also
after a failure, Ctrl-C, SIGTERM or SIGHUP:

- **artifact**: the configured binary exists; its SHA-256 and age are
  recorded, and the version and commit from `manifest.json` next to it when
  its hash matches; a warning when it is older than the checkout's last
  commit (one `git log` template);
- **target**: the host seam (`host.py`) for the cell, and whether the target
  can run it (below); a target that cannot skips the cell, or fails it when
  named;
- **wled**: `/json/si` and `/json/cfg` saved before anything changes; the
  runner refuses while something else drives WLED; restored last and
  compared (`on`, `bri`, `lor`, `transition`, the preset, `if.live.maxbri`);
- **clip**: the generated test clip (see below), or `--source file:PATH`
  (the colour check is then "not black");
- **feed_shown**: with a capture card and a feed, the clip on the card's
  input (see below);
- **running**: the seam delivers Prysm, writes temporary settings (WLED
  output, neutral processing, the cell's capture) and starts it; stopped with
  SIGTERM (KILL after 30 s). A local target runs the binary on this computer
  as a child process, its output in `headless.log`; an SSH target is
  described below. The seam also says which sender address WLED should
  report;
- **prysm**: the portal API. With a capture card, the configured `device`
  (Prysm's own device setting: an AVFoundation unique ID on macOS) must be
  among Prysm's candidates (a Linux `/dev/v4l/by-id/` link as the node it
  resolves to on the target, which is what Prysm lists), or the cell fails
  listing the devices present with their IDs; a mode of `/capture/modes` must take the capture size and
  rate (`[mac.capture]`; 640×360 at 30 fps without it). The runner matches no names
  and resolves no presets.

The checks, in order, through the
[portal API](../../surfaces/web-portal/HTTP-CONTRACT.md) and WLED's JSON API:

- the running version (`prysm_build_info` in `/metrics`) is the artifact's
  from `manifest.json` (a skip without a matching manifest);
- Screen sync starts: running with frames; WLED reports live DDP from the
  sender the seam expects; no `output_issue`;
- the frame rate is within the tolerance of the source's, sent alike,
  nothing falls behind;
- the colours follow the test clip (a card without a feed: not black);
- WLED effects (`device_native`), Screen sync again, off;
- WLED's brightness: 40 % reported within 5 s at `bri` 102, gone within 35 s
  with Force max brightness, `state.bri` unchanged, `/json/cfg` under 64 KiB;
- with a card, capture release and re-open;
- `--display-check`: when the card records this Mac's HDMI output, a local
  page the runner controls shows red, green, blue, white and an edge pattern;
  each edge must come out as that colour. The card then records this Mac, so
  the feed is not started for that run;
- SIGTERM: exit code 0 and WLED released.

Whether Prysm applies WLED's gamma, and dithering while lighting runs, are
covered by the product's own tests (`output-wled`, the headless host's system
tests); dark scenes are a question in `eyes`.

#### Run the Mac file cell

```sh
cargo xtask dist mac          # dist/prysm-headless-macos-arm64 and dist/manifest.json
uv run --no-dev --project scripts/acceptance acceptance host mac --source file
```

with `[wled] address`, the `[leds]` counts and `[mac] binary` set. For a
rehearsal without lights, run the stand-in
`uv run --project scripts/acceptance fake-wled --ddp 127.0.0.1:4048` and
give `--wled 127.0.0.1:18080`; any build of prysm-headless works there
(`binary = "target/debug/prysm-headless"`, without a manifest the version
check is a skip).

#### Run the Linux binary cells

```sh
cargo xtask dist linux-arm64   # dist/prysm-headless-linux-arm64 (linux-amd64 for x86_64)
uv run --no-dev --project scripts/acceptance acceptance host linux --dry-run
uv run --no-dev --project scripts/acceptance acceptance host linux --source device
```

`[linux]` in `config.toml`:

- `target`: `"ssh://USER@HOST"`, or `"local"` to run the binary on this
  computer as the Mac cells do. HOST is also the address the runner reaches
  Prysm at over HTTP, so it must resolve here (an ssh config alias does not).
  ssh runs with `BatchMode=yes`: the key must work without a prompt;
- `binary`: the Linux artifact. The runner reads its ELF header's machine and
  compares it with `uname -m` on the target;
- `device`: the capture card on the target, a V4L2 path; the stable
  `/dev/v4l/by-id/…-video-index0` link is preferred over `/dev/videoN`, which
  can change between boots. Omit it without a card;
- `listen`: where Prysm listens on the target, by default `0.0.0.0:18787`
  with an ssh target, so the runner reaches it over the LAN;
- `[linux.capture]` and `[linux.feed.chromecast]`: as for `[mac]`. The feed
  runs on this computer (ADB, PyChromecast and the clip server) while the
  card is on the target;
- `deb` and `image` are read and checked, not used yet.

Before WLED is touched, the target is checked, each with one command:
reachable over ssh without a prompt; `uname -sm` is Linux on the binary's
machine; for the file source FFmpeg 9 or newer in the non-interactive ssh
environment Prysm runs in (absent or older is a SKIP, so `linux-binary-file`
is skipped on Debian 13's FFmpeg 7); for the device source the device is
readable by the ssh user (through the link); the listen port is free (`ss`).

On the target the runner writes only in `~/.prysm-acceptance/RUN`, named
after the run and the cell (`RUN` is `TIME-host-CELL`): the binary, the
settings, for the file source the clip, and Prysm's `headless.log`,
`headless.pid` and `exit-code`. A small `sh` wrapper started with `nohup`
runs Prysm detached with `--metrics` on `listen` and records its PID and,
when it exits, its exit status, which is how the runner gets the exit code.
The start counts only when the API answers while that PID is the run's own
binary. SIGTERM and KILL go only to that PID, and only while
`/proc/PID/exe` is the run's binary. The sender WLED should see is the
`src` of `ip -4 route get WLED` on the target. At the end (also after a
failed start or copy) the runner fetches `headless.log` and the kernel's USB
lines since the delivery (`journalctl -k`, no sudo; as `kernel-usb.log`, so
a card that reset shows in the report; a refused `journalctl` leaves a note
there instead) and removes the run directory. A capture cell that fails is a
plain failure: the runner does not retry. After a hard kill of the runner
Prysm and the run directory stay on the target, and the next run reports
the port in use: there, stop it with
`kill $(cat ~/.prysm-acceptance/RUN/headless.pid)`, remove
`~/.prysm-acceptance/RUN` and use `restore-wled`. Every remote command is a
`linux.*` template in `commands.py`; `--dry-run` prints each as it would
run.

#### Add a cell

A cell is one more `Cell` in `CELLS` in
[`host_cells.py`](../../scripts/acceptance/prysm_acceptance/host_cells.py).
A new delivery is one more implementation of `Host` in `host.py` (preflight,
deliver, start, stop, logs, base URL, expected sender, close), chosen in
`_seam`; the checks stay as they are. Each fixture and check carries its description (the
docstring) and what it does for the dry run (`planned`), so there is no
separate table of steps.

#### The test clip and the colour check

The clip (`edges_clip.py`, made by one FFmpeg template) is gray with a band
along each edge. Each band is split in the middle, and each corner's colour
fills the two half-bands that meet there: red top-left, green top-right, blue
bottom-right, yellow bottom-left. Every 2 seconds the colours move one corner
on, clockwise; nothing moves within a step. Read clockwise, each edge shows
two colours and its second is the next edge's first. It is H.264 (yuv420p,
faststart), 30 minutes long and about 2 MB.

The colour check listens to the colours stream for 10 s. The stream reports
`top` and `bottom` left to right and `left` and `right` top to bottom, so
each edge's halves are read in that order; only the entries between 10 and
40 % and between 60 and 90 % of an edge count, away from the corners and the
middle. Each half is named by its nearest colour among nine. The check
requires:

- every sample shows one step of the clip on all four edges, or is no step
  for at most 0.3 s at a change (two of the stream's 100 ms looks);
- at least two steps, in rotation order;
- changes 2 s apart (±0.3 s), and no step held longer than 2.3 s at the
  start or the end of the 10 s.

So it fails, saying which and with what each edge showed, the steps and their
times, on a wrong colour, a reversed (mirrored) edge, swapped edges, a frozen
picture or steps at the wrong pace. It does not measure the delay from the
screen to the LEDs. A picture turned by a quarter or a half turn looks like
another step, so it would pass. The stream carries Prysm's edge colours
before they are put in WLED's order (clockwise from the bottom-left corner),
so the check proves the direction up to Prysm's edges, not the LEDs' order on
the strip. The `output-wled` unit tests cover that order; on the bench only
a look at the lights does.

#### The feed into the capture card

A capture card shows whatever is plugged into it, or a "no input" splash.
To compare the edges with a known picture, the bench needs a feed the runner
controls: something on the card's HDMI input that can show the test clip and
stop again. The configuration names it per platform. With a feed, the
`feed_shown` fixture shows the generated clip on it and the colour check
compares each edge with the clip, as with the file source. Without one the
colour check can only say "not black". The file source needs no feed and
never starts it.

The supported feed is a Chromecast with Google TV, `[mac.feed.chromecast]`:

1. ADB (`adb_serial`) must answer `device`; the runner reads
   `stay_on_while_plugged_in`, sets it to 7 (screen stays on while powered),
   wakes the device, waits until it reports `mWakefulness=Awake` and reads
   its Wi-Fi (`wlan0`) address;
2. it serves the generated `edges.mp4` (30 minutes, longer than the stage, as
   the receiver does not loop) from this computer, on the address that routes
   to the Chromecast and `serve_port`; only that file, with byte ranges;
3. it connects to that address (no discovery), plays the clip in the
   default media receiver and waits up to 30 s for `PLAYING`.

The colour check waits `settle_seconds` after `PLAYING` for the player's
overlay to clear, checks that the clip is still playing and then reads the
colours as above; naming the nearest colour absorbs the shift through HDMI
and the card. The capture re-open check also needs the clip
still playing. At the end, also after a failure or SIGTERM, the media is
stopped, the receiver quit, the server stopped and
`stay_on_while_plugged_in` put back as it was.

One-time set-up of the Chromecast (with Google TV):

- set it up with the Google Home app on the same Wi-Fi network as this
  computer;
- enable Developer options (Settings > System > About, select Android TV OS
  build seven times), then USB debugging; connect it to this computer by USB
  (recommended: that also powers it), run `adb devices` and allow this
  computer on the TV; set `adb_serial` to the serial `adb devices` lists;
- wireless instead of USB: enable Wireless debugging, pair once with
  `adb pair HOST:PAIRPORT CODE` (Settings > System > Developer options >
  Wireless debugging > Pair device), then before each run
  `adb connect HOST:PORT` with the port that screen shows (it changes) and
  set `adb_serial = "HOST:PORT"`. The runner does not pair or connect.

`[mac.feed.chromecast]` in `config.toml`: `adb_serial` (required),
`serve_port` (default 18792) and `settle_seconds` (default 5; the
Chromecast's player overlay, so it belongs to this feed). Remove the section
when nothing the runner controls feeds the card. The ADB commands are
templates in `commands.py` like every other command; everything else about
the Chromecast is in `feed_chromecast.py`, behind `feed.py`.

### tv

The `tv` stage keeps its own step runner (`plan.py`) until the plan's phase
4 moves it to the new configuration; `--only NAME[,NAME]` and `--from NAME`
(patterns over step names) select checks, and setup and cleanup steps always
run. For each package (`full`, then `slim`): take the configured IPK
(`tv.ipk_full`, `tv.ipk_slim`, by default the names `cargo xtask dist`
writes), record its hash and its manifest's version and commit, check it with
`ipk.py verify` before every install (whatever its name: what it contains
decides), install it the way that works on webOS 10 ([packaging](packaging.md)),
and check:

- registration: `getState` answers with that flavour; the app, service,
  manifests and boot hook exist;
- full: the TV's portal (at the address `getState` reports) answers and
  captures with video playing; 90 s soaks at 640 × 360 and 1280 × 720 with
  the portal's frame rate and the worker's CPU and RSS from `/proc`;
  `POST /api/v1/capture/modes`;
- slim, streaming to `[mac] binary` the runner starts here (or
  `--receiver URL`): the TV's offer is known while the lights are off; each
  encoding streams 640 × 360 at 30 fps; a 90 s soak with hardware H.264 and
  the worker's CPU and RSS; pause and resume; optional `wss://`;
- `--sweep`: sizes × rates actually delivered, for capture (full) and for
  each encoding (slim), as `sweep-full.csv` and `sweep-slim.csv`;
- standby and wake (guided: a person presses power, the runner watches);
- reinstall keeps preferences, the full settings and the lighting;
- uninstall: the supervisor's `uninstall` and `ares-remove` (or the person)
  leave no app, service, manifest, boot hook, staged package, process or
  Luna service; `/var/lib/io.prysm.tv` stays, as documented.

### eyes

One question at a time, answered `y`, `n` or `skip` with an optional note:
the lights match the picture, dark scenes, dithering off and on in a dark room
(the runner switches it), the TV app's home, portal QR code, waiting for
video and navigation, that nothing re-renders visibly while idle (until its
unit test, prysm-skn), calibration patterns (the runner shows each) and the
TV's own calibration flow, the portal on a phone, and a cold boot by
unplugging. Like `tv`, it keeps the step runner and its `--only`/`--from`.

## Safety

- **The TV.** Every command the runner can run, on the TV or here, is a
  template in [`commands.py`](../../scripts/acceptance/prysm_acceptance/commands.py); its
  parameters are checked against fixed patterns and nothing from the
  configuration becomes shell text. The TV commands only read, stage the
  package as `/media/developer/temp/io.prysm.tv.ipk` (removed after each
  install), install through `com.webos.appInstallService`, run the package's
  `setup.sh` and the supervisor's `uninstall`, and call Prysm's own Luna
  methods. Nothing writes to system partitions or deletes
  `/var/lib/io.prysm.tv`. [Tests](../../scripts/acceptance/tests/test_acceptance_commands.py)
  enumerate every template and check the paths it names. On the Chromecast,
  ADB only reads its state and Wi-Fi address, wakes it and reads or sets
  `stay_on_while_plugged_in`; there is no free-form `adb shell`.
- **A Linux target.** The runner writes only in its run directory under
  `~/.prysm-acceptance/` (named by a checked run name, never a path from the
  configuration), signals only its own Prysm's PID, never uses sudo, and
  removes the directory at the end.
- **Bounded.** Every command has a timeout; `luna-send` runs with its stdin
  held open and a remote `timeout`, as `install-ipk.sh` does.
- **Left as found.** The `tv` stage saves `getState`, the files in
  `/var/lib/io.prysm.tv` and the full portal's settings first (and stops if
  an installed Prysm does not answer, since it could not be put back), and at
  the end installs the package it found (`tv.restore_flavour`), sets the
  preferences back and puts the settings and lighting back. WLED is
  snapshotted before anything changes (`wled-snapshot.json`) and restored
  last, after Prysm has stopped, also on a failure, Ctrl-C, SIGTERM or SIGHUP;
  a second signal does not cut the restore short. After a hard kill use
  `restore-wled`.
- **The owner decides.** The `tv` stage refuses to start without
  `--tv-is-free`.

On macOS, a Local Network refusal (`EHOSTUNREACH`) means the app running the
script needs Local Network access; the runner says so.

## Develop the runner

The runner's own code is linted, formatted, type-checked and kept in layers.
`cargo xtask check scripts` runs these checks with the repository's other
Python checks, as CI's `quality` job does; it needs
[uv](https://docs.astral.sh/uv/). The rules are in
[`pyproject.toml`](../../scripts/acceptance/pyproject.toml): Ruff, Pyright in
strict mode, and Import Linter's layers (command line, the host cells and
their pytest plugin, the stages, their cells, checks, the feed, devices,
clients, base; stages never import each other). Only `transport.py` runs
commands, and only `commands.py` builds them. One at a time, from the
repository root:

```sh
uv run --locked --directory scripts/acceptance ruff check
uv run --locked --directory scripts/acceptance ruff format --check
uv run --locked --directory scripts/acceptance pyright
uv run --locked --directory scripts/acceptance lint-imports
uv run --locked --directory scripts/acceptance pytest
```

The tests use fakes and contact no device; the host cells are tested
through the runner's command line (the dry run contacts and starts nothing,
the skip-or-fail rule). A real run of the Mac file cell against
prysm-headless and the fake WLED on loopback is opt-in, as it needs a built
binary and UDP port 4048: `cargo build -p prysm-headless`, then
`PRYSM_ACCEPTANCE_BINARY=target/debug/prysm-headless` with the `pytest`
line above (from `scripts/acceptance`, give the path from there). `test_acceptance_contract.py`
checks the fields and requests the runner relies on against Prysm's generated
contracts in `frontend/packages/contracts/generated/`; after a contract
change, run it with the rest. Pyright comes with its Node.js runtime from
PyPI (`pyright[nodejs]`), so nothing else needs installing.
