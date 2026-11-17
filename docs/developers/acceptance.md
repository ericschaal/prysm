# Hardware acceptance runner

The runner, the package [`scripts/acceptance/prysm_acceptance`](../../scripts/acceptance/prysm_acceptance/run.py),
checks Prysm's artifacts on real devices: WLED, a capture card, a Linux
computer over SSH, and both TV packages. It is run by hand, never by CI or a build, and it never builds:
it tests the artifacts the configuration names (`cargo xtask dist`,
[packaging](packaging.md#artifacts-in-dist)). A run writes its report and
evidence (snapshots, logs, backups) to `work/acceptance/<time>-<stage>/`,
which is not tracked. Copy what matters into
[research](../../research/README.md). How it came to this shape is in the
[plan](acceptance-plan.md).

```sh
cp scripts/acceptance/config.example.toml scripts/acceptance/config.toml   # then fill in
cargo xtask dist mac                                                        # the artifact under test
uv run --no-dev --project scripts/acceptance acceptance host mac --dry-run
uv run --no-dev --project scripts/acceptance acceptance host mac --source file
uv run --no-dev --project scripts/acceptance acceptance host mac --source device [--display-check]
uv run --no-dev --project scripts/acceptance acceptance host linux [--source file|device]
uv run --no-dev --project scripts/acceptance acceptance host linux --delivery deb|image
uv run --no-dev --project scripts/acceptance acceptance tv full --tv-is-free
uv run --no-dev --project scripts/acceptance acceptance tv slim --receiver mac|linux --tv-is-free
uv run --no-dev --project scripts/acceptance acceptance all [--tv-is-free] [--prepare]
uv run --no-dev --project scripts/acceptance acceptance eyes
uv run --no-dev --project scripts/acceptance acceptance restore-wled work/acceptance/RUN/CELL/wled-snapshot.json
uv run --no-dev --project scripts/acceptance acceptance settings restore tv|linux work/acceptance/RUN/CELL/prysm-backup.json
```

Python 3.11+ and [uv](https://docs.astral.sh/uv/). `scripts/acceptance` is a
uv project (`pyproject.toml`, `uv.lock`); `uv run --project
scripts/acceptance` from the repository root installs it into
`scripts/acceptance/.venv` and runs its `acceptance` command. A bench run
needs no dev group (`--no-dev`). Its libraries are
[pytest](https://docs.pytest.org/), which runs the host and TV cells, and
[PyChromecast](https://github.com/home-assistant-libs/pychromecast), imported
only by a run that casts the clip (the Chromecast feed, the TV's feed). `--dry-run` prints what
would run, with every command and HTTP call (TV commands as they run there),
and contacts nothing. The exit code is 0 when nothing failed (skips allowed),
1 on a failure, 2 on bad usage, 3 when `--prepare` failed. `config.toml` (ignored by git) describes the
bench (see [`config.example.toml`](../../scripts/acceptance/config.example.toml)):
WLED and the LED counts, `[checks]` (sample length, frame-rate tolerance),
`[mac]` (`target = "local"`; an `ssh://` target is refused as not
implemented), `[linux]` (`target = "ssh://USER@HOST"` or `"local"`), `[tv]`
(the TV and its packages, below); no address is written in the code.

## Stages

| Stage | Needs | Person |
| --- | --- | --- |
| `host mac` | WLED; `[mac] binary`; FFmpeg 9+ for the file source, or a capture card (`[mac] device`; with a feed: FFmpeg, and for the Chromecast adb) | Only for `--display-check` set-up |
| `host linux` | WLED; `[linux] binary` and `target`; over SSH a key the runner can use without a prompt; FFmpeg here for the clip; FFmpeg 9+ on the target for the file source, or a capture card on it (`[linux] device`; with a feed: adb here) | No |
| `host linux --delivery deb` | As the device cell, with `[linux] deb` instead of `binary`, an SSH target with passwordless sudo and no Prysm installed (or `[linux] installed = "replace"`) | No |
| `host linux --delivery image` | As the device cell, with `[linux] image`, an SSH target with Docker and its compose plugin 2.24.4+ for the ssh user, and no `prysm-headless:local` image | No |
| `tv full`, `tv slim --receiver mac\|linux` | `--tv-is-free` (the owner says the TV may be used now); WLED; `[tv] target` and the package (`ipk_full`, `ipk_slim`); slim: the receiver's `[mac]` or `[linux] binary`; no Prysm on the TV (or `[tv] installed = "replace"`); nobody using the TV; FFmpeg and PyChromecast for the clip cast to it | Standby and wake; accepting the TV's prompt for the first cast from this computer |
| `all` | Whatever each host cell needs, and with `--tv-is-free` each TV cell's; a cell without it is skipped | As its cells |
| `eyes` | A portal (the TV's, or `--portal`) | Every step |

### host

The host cells run under pytest: a cell is a parameter (platform, delivery,
source): `mac-binary-file`, `mac-binary-device`, `linux-binary-file`,
`linux-binary-device`, `linux-deb-device` and `linux-image-device` (the
package and the image have no file source). `acceptance host mac` runs both
Mac cells, `acceptance host linux` both Linux binary cells; `--source file`
or `--source device` names one, and `--delivery deb` or `--delivery image`
names that delivery's one cell. Other arguments go to pytest, so `-k fps`,
`-m file` or `-x` select and stop as pytest does. A cell whose artifact or
tool is missing (no `binary`, no `device`, no FFmpeg, no sudo, no Docker) is
a SKIP with the reason; a named cell fails instead.

`acceptance all` runs every host cell, one at a time, each skipped with the
reason when its artifact, tool or device is missing; none is named, so none
fails for that. With `--tv-is-free` it then runs the [TV cells](#tv) the
same way (a slim cell whose receiver has no binary configured is skipped);
without it, none. `eyes` is a person's stage. With one capture card on the
bench, leave `device` out of the platform whose computer does not have it.

`--prepare` (with `host`, `tv` or `all`) first asks the build system for the
artifacts of the chosen cells, as one `cargo xtask dist TARGET...` run in
the repository (a template in `commands.py`), and waits for it. Which target
makes which artifact is a closed table in `prepare.py`: the configuration key
(`[linux] deb`, `[tv] ipk_slim` ...) and the file in `dist/` to the target that
writes it; a slim TV cell needs its package and its receiver's binary. The
architecture is taken from the configured file name, so `--prepare` refuses
an artifact configured anywhere but under its `dist/` name. A cell without a
configured artifact is not built (and is skipped as usual). A failed or
refused build ends the run with exit code 3 before anything is contacted.
With `--dry-run` it prints the build command and runs nothing. Without
`--prepare` the runner never builds. The run directory has `report.xml` (JUnit, with each check's
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
- `deb` and `image`: the package and the image cells' artifacts (below).

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

#### Run the Linux package and image cells

```sh
cargo xtask dist deb-arm64 image-arm64   # or: acceptance host linux --delivery deb --prepare
uv run --no-dev --project scripts/acceptance acceptance host linux --delivery deb --dry-run
uv run --no-dev --project scripts/acceptance acceptance host linux --delivery deb
uv run --no-dev --project scripts/acceptance acceptance host linux --delivery image
```

Both run the device cell's fixtures and checks through their own seam (an
SSH target only; with `target = "local"` they are skipped as not
implemented), and both treat the target as a test bench: nothing of Prysm
may be there before the run, and nothing is left after it.

**The package** (`linux-deb-device`, `host_deb.py`, `deb.*` templates):

- preflight: the package's `Architecture` read from its control file here,
  against `uname -m`; `sudo -n true`; then `deb.leftovers` must find nothing:
  none of the package's files, `/var/lib/prysm` or `/var/lib/private/prysm`,
  a unit of that name, the run's drop-in, a `prysm-headless` user or group, or a
  package dpkg knows (else, with `[linux] installed = "refuse"`, the default, a
  SKIP that lists what it found, or a failure when named; with `"replace"` see
  [An installed Prysm](#an-installed-prysm-installed)); the device readable;
  the listen port free;
- replace (only with `"replace"` and something found, after the WLED
  snapshot): the settings and target read from the installed unit's portal on
  its default `HOST:8787` (stops when it does not answer, or while an
  administrator's unit or drop-in is under `/etc/systemd/system`, which the
  purge would leave), then `deb.purge` and `deb.leftovers`, which must find
  nothing;
- deliver: the `.deb` copied into the run directory, then
  `sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y
  --no-install-recommends ./prysm-headless.deb` there (its output in
  `apt.log`); `dpkg-query` must then report the control file's version;
- start: a runtime drop-in, the package's documented override, in
  `/run/systemd/system/prysm-headless.service.d/` (gone at the next boot)
  that runs the unit's own command with `--listen LISTEN --metrics`;
  `daemon-reload`; `systemctl restart`; once the unit is active and the API
  answers, the settings are given through the portal (`PUT
  /api/v1/settings`), so Prysm writes them into its state directory itself;
- stop: `systemctl stop` (SIGTERM; systemd kills after the unit's 30 s); the
  exit code is `ExecMainStatus` when `ExecMainCode` says the process exited,
  otherwise the stop reports the signal or the timeout;
- logs: the unit's journal since the delivery (`journal.log`) and the
  kernel's USB lines;
- its own checks: the unit's main process does not run as root (no user name
  is assumed); a `systemctl restart` while lighting comes back idle with the
  saved settings (as the package documents: a restart leaves the lights off)
  and Screen sync lights WLED again;
- close, also after a failure: the drop-in removed, `apt-get purge -y
  prysm-headless`, then `deb.leftovers` again: anything it lists (a `prysm`
  user or group included) fails the cell and is left for a person to remove;
  the run directory is removed either way.

**The image** (`linux-image-device`, `host_image.py`, `image.*` templates):

- preflight: the archive's architecture and tags read from its
  `manifest.json` here (it must carry `prysm-headless:local`, the name
  compose.yaml runs), against `uname -m`; `docker info` and `docker compose
  version` as the ssh user, without sudo (Debian's `docker.io` and compose
  packages, the user in the `docker` group), compose 2.24.4 or newer for
  `!override`; neither the image tag nor the run's compose project may exist;
  the device's group ID; port 8787 free;
- deliver: the archive copied into the run directory and `docker load`ed;
  its image ID recorded;
- start: [`packaging/docker/compose.yaml`](../../packaging/docker/compose.yaml)
  copied as it is, with a second compose file next to it,
  `compose.acceptance.yaml`: `restart: "no"`, `command: ["--metrics"]`, and
  with `!override` the configured device in place of the file's
  `/dev/video0` (at that path in the container, where Prysm finds it) and the
  device's group in place of `44`. The project is
  `prysm-acceptance-RUN`. The settings are written into the project's volume
  as the image's user (`compose run --rm -T --entrypoint sh prysm -c 'cat >
  /data/settings.json'`, as `smoke.sh` seeds them), then `compose up -d
  --pull never --no-build`; Prysm listens on compose.yaml's published port
  8787 (`[linux] listen` does not apply);
- stop: `compose stop` (SIGTERM, KILL after the file's 30 s grace) and the
  container's exit code from `compose ps`;
- logs: `compose logs` (`container.log`) and the kernel's USB lines;
- sender: the target's own `src` on the route to WLED: compose.yaml's bridge
  network reaches the LAN through Docker's masquerade as that address (with
  `network_mode: host` it is the same);
- its own check: a new container (`compose up --force-recreate`) answers idle
  with the same settings, so they live on the volume, and lights WLED again;
- close, also after a failure: `compose down --volumes --remove-orphans` (this
  project's container, network and volume only), `docker image rm
  prysm-headless:local` when the tag exists (the preflight saw none, so it is
  the run's, also after an interrupted load), the run directory removed.
  Nothing else of Docker's is touched.

#### Add a cell

A cell is one more `Cell` in `CELLS` in
[`options.py`](../../scripts/acceptance/prysm_acceptance/options.py).
A new delivery is one more implementation of `Host` (preflight, deliver,
start, stop, logs, base URL, expected sender, close) in its own module with
its own templates, chosen in `_seam` in
[`host_cells.py`](../../scripts/acceptance/prysm_acceptance/host_cells.py);
the shared checks stay as they are. A check of what only that delivery adds
is marked `@only(Delivery.X)` and is not collected for other cells, so
dropping a delivery is dropping its module, templates, cell and those
checks. Each fixture and check carries its description (the
docstring) and what it does for the dry run (`planned`, a `Planner` of the
module's kind of cell), so there is no separate table of steps. A cell
module's `CELLS` are its parameters; a check one module takes from another
(`tv_cells.py` takes those of `tv_checks.py`) runs with the taking module's
cells and fixtures.

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
templates in `commands.py` like every other command; the rest of the
Chromecast is in `feed_chromecast.py`, on the serving and casting it shares
with the TV's feed in `feed_cast.py`, behind `feed.py`.

### tv

The TV cells run under pytest like the host cells, from
[`tv_cells.py`](../../scripts/acceptance/prysm_acceptance/tv_cells.py):
`tv-full` (`acceptance tv full`), and `tv-slim-mac` and `tv-slim-linux`
(`acceptance tv slim --receiver mac|linux`), where the slim package streams
to a receiver on that platform. Each needs `--tv-is-free`: without it `tv`
refuses (exit code 2, also with `--dry-run`) and `all` runs no TV cell. The
TV is the owner's daily device, not a bench: a run starts and ends with no
Prysm on it ([An installed Prysm](#an-installed-prysm-installed)).

The checks any TV runs are in
[`tv_checks.py`](../../scripts/acceptance/prysm_acceptance/tv_checks.py) and
reach the TV only through the `Tv` protocol in
[`tv.py`](../../scripts/acceptance/prysm_acceptance/tv.py) and Prysm's HTTP
API. The webOS TV is its implementation, `WebosTv` in
[`webos.py`](../../scripts/acceptance/prysm_acceptance/webos.py) (Luna calls,
the install service, the app ids). `tv_cells.py` holds the webOS fixtures and
the webOS package's own checks, and takes the shared checks in their order,
each with its plan for this TV. A
[test](../../scripts/acceptance/tests/test_acceptance_tv.py) checks that
`tv.py` and `tv_checks.py` import nothing of `webos.py` or of the command
templates (but the `Command` type in `tv.py`) and name nothing of Luna, the
package or Prysm's webOS app.

`[tv]` in `config.toml`:

- `target`: `"ssh://root@HOST"`, with a key that works without a prompt. HOST
  must resolve here: the wake-on-LAN packet leaves on the route to it;
- `ipk_full`, `ipk_slim`: the packages under test (by default the names
  `cargo xtask dist tv-full tv-slim` writes);
- `mac` (optional): the TV's hardware address. When the TV does not answer
  over SSH, the runner sends it one wake-on-LAN magic packet (a UDP broadcast
  to port 9 from this computer's address towards the TV, `local.wake-on-lan`)
  and tries SSH every 5 s for 90 s; a TV that still does not answer skips the
  cell (fails it when named). Without `mac` an unreachable TV is that skip at
  once. The runner never puts the TV to sleep;
- `installed`: `"refuse"` (default) or `"replace"`, below;
- `soak_seconds`: each soak's length (default 90);
- `[tv.feed.cast]` (optional): tunes the test clip cast to the TV,
  [below](#the-clip-cast-to-the-tv): `serve_port` (default 18792) and
  `settle_seconds` (default 5). The TV cells always cast it.

The slim receiver is the host cells' binary seam on that platform: `[mac]
binary` started here, or `[linux] binary` on the Linux target over SSH,
listening on the platform's `listen` port on all interfaces (`0.0.0.0`) so
the TV reaches it. The TV is pointed at it as `http://ADDRESS:PORT`, ADDRESS
being this computer's address towards the TV or the Linux target's. Its
settings are the host cells' (WLED output, neutral processing) with the
remote capture at 640 × 360 and 30 fps, hardware H.264, without the UI.

The fixtures set up, in order, and their teardowns undo it in reverse, also
after a failure, Ctrl-C, SIGTERM or SIGHUP:

- **artifact**: WLED and the LEDs configured, `[tv] target` set, the package
  (and for slim the receiver's binary) present, else a SKIP (a failure when
  named); `ipk.py verify` accepts the package (by what it contains, whatever
  its name); its hash, age, version and commit recorded as for the host cells;
- **target**: the TV reachable (woken as above) and at a known start: the
  app in front (saved as `tv-foreground.json`) is none, the home screen (also
  under the screen saver), Prysm's own or the Cast receiver; over any other
  app (an input, a streaming app) the runner brings the home screen to the
  front, and fails the cell unless the TV reports it there within 20 s.
  `--tv-is-free` is the owner's word that nobody is watching. Then the old `io.prysm.capture` app not running,
  and what of Prysm is there: the app, the service, the Luna manifests and
  boot hook, the staged package, a process under `io.prysm.*`, `getState`
  answering, or the data files in `/var/lib/io.prysm.tv` that an uninstall
  leaves (`tv-found.json`);
- **receiver** (slim): the receiver's preflight, as the host cells' (over SSH:
  reachable, `uname -m` against the binary, the port free);
- **wled**: the host cells' snapshot, refused while something else drives WLED
  (a Prysm lighting it from the TV included: the runner does not switch it
  off), restored and compared last;
- **cleared**: with `installed = "replace"`, what the target found is backed
  up and uninstalled (below);
- **package**: the package installed the way that works on webOS 10
  ([packaging](packaging.md)): verified, staged as
  `/media/developer/temp/io.prysm.tv.ipk`, `appInstallService` until
  `installed`, unstaged, `setup.sh`, `getState`. At the end it is uninstalled:
  the supervisor's `uninstall`, then the app removed through
  `appInstallService` until `removed`, then nothing
  of the above but the data files may be left (else the teardown fails, saying
  what is left);
- **prysm**: full: the TV's portal at the address `getState` reports, given
  the run's settings (WLED output, neutral processing, the webOS capture at
  640 × 360 and 30 fps without the UI) with `PUT /api/v1/settings`; slim: the
  receiver delivered and started, then the TV's `setPreferences` (`instance`,
  the receiver) and `setEnabled true`, as the app does; the receiver is
  stopped and removed at the end;
- **running**: the sender WLED should see: the TV (the host of the portal
  address `getState` reports) or the receiver's, as the host cells find it.
  A failure fetches the TV worker's recent log lines from `getState`
  (`tv-worker.log`) and the receiver's logs;
- **clip**: the host cells' clip, generated;
- **screen**: what the TV shows ([The TV's states](#the-tvs-states)). It sets
  nothing itself: each check sets the state it needs. At the end, also after
  a failure or Ctrl-C, and before the receiver stops and the package is
  uninstalled, the cast is stopped, the clip server closed and Prysm's app
  closed when the runner launched it; the app in front then is saved as
  `tv-end.json`. The app found in front at the start is not brought back.

#### The TV's states

No check measures whatever the TV happens to show. Each check that measures
declares a state; the runner sets it, confirms it from the TV, and only then
measures. A part not reached fails the check, saying which, and every check's
evidence holds the state as the TV confirmed it: `getState`'s `power` and
`video`, the app in front and the size of the frames that arrive.

| State | Set by the runner | Confirmed |
| --- | --- | --- |
| video | Prysm's app closed if the runner launched it; the clip cast with its overlay, unless it plays | power `active`, video exactly `available`, the Cast receiver (`com.webos.chromecast`) in front, all within 20 s; `settle_seconds` after `PLAYING` the clip still playing; the frames that arrive exactly the size asked |
| UI | the cast stopped; Prysm's own app launched | power `active`, video exactly `unavailable`, `io.prysm.tv` in front, all within 20 s; the frames that arrive at most 320 × 180 |

`unknown` video, the screen saver and no app named in front count as not
reached. Power, video and the app come from the TV (`getState`, which reads
`com.webos.service.tvpower` and the main sink of
`com.webos.service.videooutput`, and the application manager's
`getForegroundAppInfo`), never from the Cast receiver's own status: it still
says `PLAYING` after another app has taken the screen. Between the TV's
confirmation and the size, the capture the check needs is saved and Screen
sync must run with frames within 30 s; the size is that of the frames
Prysm's preview stream shows (`GET /api/v1/preview/stream`).

The cast takes the TV out of its screen saver by itself. The checks run in
this order, the video ones first, so the clip is cast once and stopped once;
a check selected alone (`-k`) sets its state just the same. A cast that does
not play (nobody accepted it, below, or it fails) is tried once: every check
that needs video then fails at once with that reason, and the UI checks run.

The checks, in order (`@only` marks a package's own):

- registered: `getState` answers as the cell's package with its worker; the
  app, service, manifests and boot hook exist;
- video only (video state, the capture without the UI): the frames at
  640 × 360, the frame rate within the tolerance, and the colours follow the
  clip on every edge ([The test clip and the colour check](#the-test-clip-and-the-colour-check))
  though the overlay is over the bottom of the screen: the UI is left out;
- video with the overlay (the same state, only the UI included): the same
  size and rate; the top edge and the side edges' upper halves still follow
  the clip, and both halves of the bottom edge show the overlay (white) at
  every step. Both checks judge against the clip's known colours, so the clip
  keeps playing and nothing is compared with a capture of another moment;
- WLED live and the frame rate (video state), the host cells' own: Screen
  sync lights WLED with live DDP from the expected sender, no `output_issue`;
  the frame rate within the tolerance, sent alike, never behind;
- capturing (video state): `getState` says capturing with a frame rate;
- full (video state): the TV's capture modes are known
  (`POST /api/v1/capture/modes`);
- slim (video state): with the receiver off, the TV's offer is known for each
  encoding; each encoding streams at 30 fps and `getState` names it (with the
  worker's CPU). Raw has no rate control, so its rate is what the link to the
  receiver carries; the evidence holds the size of the frames that arrived
  and the TV's power and video state, to tell a slow link from a larger
  frame; the receiver off pauses the TV's capture and on resumes it;
- the soak (video state at each size): `soak_seconds` at 640 × 360 and
  1280 × 720 (full) or 640 × 360 hardware H.264 (slim), the frames arriving
  at that size, the frame rate within the tolerance, the worker's CPU and RSS
  from `/proc` every 5 s;
- reinstall while lighting (video state): the same package again keeps the
  preferences (and the full package's settings), lighting resumes by itself,
  and the video state is confirmed again after it;
- UI only (UI state, the capture with the UI): the frames within the UI size,
  the frame rate within the tolerance, the picture not black;
- no video and no UI (the same state, the capture without the UI): for 10 s
  no frames (Prysm's session never running with frames, no preview frame)
  and nothing failing (no `failure` in Prysm's state or the TV's `getState`,
  the session not faulted), as designed; then with the UI again the frames
  return, of the UI size;
- standby and wake (UI state; a person presses power): standby seen when the
  TV answers, then active with the worker running and capturing within 90 s.
  Last, as it ends the other states; deselect it (`-k "not standby"`) when
  nobody is at the TV.

That nothing of Prysm is left after the uninstall is the `package` fixture's
teardown, so it is checked also after a failure.

Beyond Prysm's own service and the install service, the runner sends the TV
`getForegroundAppInfo` (it only reads), `launch` and `closeByAppId` of
`com.webos.service.applicationManager` with the id `io.prysm.tv`, and `launch`
with the id `com.webos.app.home`, each id fixed in its template. To the Cast receiver on port 8009 it loads the clip with its
subtitle track, switches that track on, and at the end stops the media and
quits the receiver. It sends no key or input event, no toast, and nothing to
the power, settings, input, volume or update services.

#### The clip cast to the TV

1. The runner serves `edges.mp4` and `overlay.vtt` (those files only) from
   this computer, on the address that routes to the TV and `serve_port`;
2. it connects to the host of `[tv] target` on port 8009 (no discovery, no
   ADB, nothing over SSH), plays the clip in the default media receiver and
   waits up to 30 s for `PLAYING`;
3. the TV asks a person to allow the first cast from this computer, and
   until then reports no player state and does not fetch the clip. So without
   `PLAYING` in time the runner asks on the terminal to accept the prompt on
   the TV with the remote: `y` waits another 30 s (no second cast); `skip`,
   or no terminal to ask, leaves the clip not shown, what was started is
   stopped at once, and the video state is not reached. The runner never
   accepts the prompt itself;
4. once `PLAYING`, it switches the clip's subtitle track on: the overlay.

The overlay is one WebVTT cue for the whole clip: two lines of solid blocks
at the bottom (`line:100%`), white on white at twice the size. The Cast
receiver draws subtitles over the video, where the TV draws its menus, so a
capture with the UI sees them and one without does not. On the TV it covers
the bottom fifth of the picture from about 11 % to 89 % of its width: the
bottom edge's read entries (12.5 to 87.5 %), and none of the side edges'
reach (10 % of the width with the runner's neutral processing). The
player's own controls show only for a few seconds after a pause, so they
are no overlay to test with.

The clip runs 30 minutes, longer than a cell, so it does not end or loop
while the checks run. The video state waits `settle_seconds` after `PLAYING`
and fails when the clip no longer plays. The UI state stops the cast: the
media is stopped, the receiver quit and the server stopped.

#### An installed Prysm (`installed`)

One rule for every target that installs Prysm (the TV, and the Linux package
cell with `[linux] installed`; `[mac]` refuses the key): no Prysm before a run
and none after it.

- `"refuse"` (the default): the cell stops in its preflight, before anything
  changes, and lists what it found.
- `"replace"`: after the WLED snapshot (so a Prysm lighting WLED is refused
  first), the runner reads what a user would lose through the installed
  Prysm's own API and saves it as `prysm-backup.json` in the cell's run
  directory: the settings and lighting target from its portal (the full
  package's at the address `getState` reports; the Linux package's at
  `HOST:8787`), and on the TV `getState`'s flavour, preferences (`autostart`,
  `instance`), `enabled` and lighting, and the data files `tv.json`,
  `settings.json` and `resume.json` as they are. It reads the file back and
  compares it. When the backup cannot be taken (an installed Prysm that does
  not answer) or does not compare, the cell stops and nothing is removed.
  Otherwise the end-of-run warning is registered and Prysm is uninstalled
  (the TV's uninstall above; the package purged), and must leave nothing.
  The cell then runs and cleans up as always: the target ends without Prysm.
  Data files without an installed Prysm (an earlier uninstall, or an earlier
  run, leaves them) are backed up the same way and nothing is uninstalled.

After `replace`, the end of the run (also after a failure or Ctrl-C) prints a
warning: Prysm was removed and NOT reinstalled, reinstalling the right version
is the user's, the backup's path, and the command that restores it:

```sh
uv run --no-dev --project scripts/acceptance acceptance settings restore tv work/acceptance/RUN/CELL/prysm-backup.json
```

`settings restore tv|linux BACKUP` sends the backup to the Prysm reinstalled
on that configured target through its API and refuses when no Prysm answers
there (or, on the TV, when the other package is installed): the TV's
preferences with `setPreferences` and, for the full package, the settings
with `PUT /api/v1/settings`; the Linux package's settings likewise. It does
not switch lighting on, set `enabled` or a lighting target: it says what the
lighting was, and the user switches it on. `--dry-run` prints what it would
send. The run leaves the TV's data files in `/var/lib/io.prysm.tv` with the
run's values (the runner never deletes them). Within one run they are the
run's own once a TV cell has started installing, so the next TV cell does not
count them; a later run on the default `"refuse"` stops on them, and a later
`"replace"` backs up those values: restore the first run's backup, as its
warning says.

### eyes

One question at a time, answered `y`, `n` or `skip` with an optional note:
the lights match the picture, dark scenes, dithering off and on in a dark room
(the runner switches it), the TV app's home, portal QR code, waiting for
video and navigation, that nothing re-renders visibly while idle (until its
unit test, prysm-skn), calibration patterns (the runner shows each) and the
TV's own calibration flow, the portal on a phone, and a cold boot by
unplugging. It is a plain script on the step runner `plan.py`, with
`--only NAME[,NAME]` and `--from NAME` (patterns over step names; setup and
cleanup steps always run); it reads the TV's portal address and, after a cold
boot, its state with `getState` when `[tv] target` is set.

## Safety

- **The TV.** Every command the runner can run, on the TV or here, is a
  template in [`commands.py`](../../scripts/acceptance/prysm_acceptance/commands.py); its
  parameters are checked against fixed patterns and nothing from the
  configuration becomes shell text. The TV commands only read, stage the
  package as `/media/developer/temp/io.prysm.tv.ipk` (removed after each
  install), install and remove the app through `com.webos.appInstallService`,
  run the package's `setup.sh` and the supervisor's `uninstall`, call
  Prysm's own Luna methods, ask which app is in front, launch or close
  Prysm's own app and bring the home screen to the front (no other app); here, one wake-on-LAN packet. Nothing
  writes to system partitions or deletes `/var/lib/io.prysm.tv`.
  [Tests](../../scripts/acceptance/tests/test_acceptance_commands.py)
  enumerate every template and check the paths it names, and every TV command
  the TV cells' plans name ([test](../../scripts/acceptance/tests/test_acceptance_tv.py)). On the Chromecast,
  ADB only reads its state and Wi-Fi address, wakes it and reads or sets
  `stay_on_while_plugged_in`; there is no free-form `adb shell`. To the TV's Cast receiver
  the runner only plays the clip with its subtitle track and stops it; it never answers the TV's prompt.
- **A Linux target.** The binary cells write only in the run directory under
  `~/.prysm-acceptance/` (named by a checked run name, never a path from the
  configuration), signal only their own Prysm's PID, never use sudo, and
  remove the directory at the end. The package cell also changes, with
  `sudo -n` in its `deb.*` templates only: the package's own files through
  apt-get (install, purge), the runtime drop-in under `/run/systemd/system/`,
  the unit's state (`systemctl`), and with `installed = "replace"` an installed
  package (purged after its backup); Prysm writes its settings itself. The image
  cell changes only Docker's own storage (the loaded image, the run's compose
  project with its container, network and volume), without sudo. A test
  enumerates these templates and the paths they may write.
- **Bounded.** Every command has a timeout; `luna-send` runs with its stdin
  held open and a remote `timeout`, as `install-ipk.sh` does.
- **No Prysm before or after.** A Prysm found on the TV or the package
  cell's target is refused unless `installed = "replace"`, which removes it
  only after a backup that reads back as written, and never reinstalls it
  ([An installed Prysm](#an-installed-prysm-installed)). WLED is snapshotted
  before anything changes (`wled-snapshot.json`) and restored last, after
  Prysm has stopped, also on a failure, Ctrl-C, SIGTERM or SIGHUP; a second
  signal does not cut the restore short. After a hard kill use
  `restore-wled`.
- **The owner decides.** No TV cell runs without `--tv-is-free`.

On macOS, a Local Network refusal (`EHOSTUNREACH`) means the app running the
script needs Local Network access; the runner says so.

## Develop the runner

The runner's own code is linted, formatted, type-checked and kept in layers.
`cargo xtask check scripts` runs these checks with the repository's other
Python checks, as CI's `quality` job does; it needs
[uv](https://docs.astral.sh/uv/). The rules are in
[`pyproject.toml`](../../scripts/acceptance/pyproject.toml): Ruff, Pyright in
strict mode, and Import Linter's layers (command line; the TV cells; the
checks any TV runs, which reuse the host cells' checks; the host cells;
their pytest plugin and `eyes`; the clip; the feed; the package and image
seams; the devices, the webOS TV among them; an installed Prysm's backup,
`installed.py`, and the TV the checks drive, `tv.py`; clients; the step
runner; base).
Only `transport.py` runs
commands, and only `commands.py` builds them. One at a time, from the
repository root:

```sh
uv run --locked --directory scripts/acceptance ruff check
uv run --locked --directory scripts/acceptance ruff format --check
uv run --locked --directory scripts/acceptance pyright
uv run --locked --directory scripts/acceptance lint-imports
uv run --locked --directory scripts/acceptance pytest
```

The tests use fakes and contact no device; the host and TV cells are tested
through the runner's command line (the dry run contacts, sends and starts
nothing, the skip-or-fail rule, `--tv-is-free`), and the TV's commands,
the wake, the backup and restore against recorded transcripts. A real run of the Mac file cell against
prysm-headless and the fake WLED on loopback is opt-in, as it needs a built
binary and UDP port 4048: `cargo build -p prysm-headless`, then
`PRYSM_ACCEPTANCE_BINARY=target/debug/prysm-headless` with the `pytest`
line above (from `scripts/acceptance`, give the path from there). `test_acceptance_contract.py`
checks the fields and requests the runner relies on against Prysm's generated
contracts in `frontend/packages/contracts/generated/`; after a contract
change, run it with the rest. Pyright comes with its Node.js runtime from
PyPI (`pyright[nodejs]`), so nothing else needs installing.
