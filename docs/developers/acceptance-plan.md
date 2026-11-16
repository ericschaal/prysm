# Plan: the acceptance runner's next shape

This is a plan, not a description of what exists. The
[acceptance runner](acceptance.md) today has the stages `desktop`, `tv` and
`eyes`, runs Prysm only on the computer it runs on, and builds what it tests.
This page is what it is to become. Delete it when the last phase has landed and
[acceptance.md](acceptance.md) describes the result.

## Principles

1. **The runner tests artifacts given in the configuration.** It never builds
   on its own. The build system prepares the artifacts; the runner calls it only
   when asked to with `--prepare`.
2. **Capture devices and feeds are per platform**, each device given by its
   exact identifier.
3. **A remote Mac is not implemented.** `[mac]` with an `ssh://` target is
   refused when the configuration loads, saying so.
4. **The checks do not depend on how Prysm is delivered.** The binary is the
   main Linux cell, because every delivery runs it. Whether the Debian package
   stays is undecided, and Docker may become the usual way to run Prysm on
   Linux, so a delivery is a thin layer that is cheap to add or drop.

## Layout

- **The build system** makes the artifacts. It is its own work, apart from this
  plan (bead `prysm-9ho`): one entry point for code generation, the checks and
  every artifact, cross-built where needed, written to `dist/` with a manifest
  (version, commit, hash). Until it exists the artifacts are made by hand:
  `cargo build --release -p prysm-headless`, the `binary` and `deb` targets of
  [`packaging/docker/Dockerfile`](../../packaging/docker/Dockerfile), and
  [`packaging/webos/build-ipk.sh`](../../packaging/webos/build-ipk.sh).
- `scripts/acceptance/`: the runner. It shares no code with the build system.

### `--prepare`

Without the option the runner uses the configured artifacts as they are and
fails in preflight when one is missing. With `--prepare` it first asks the
build system for each artifact the chosen cells need, as a separate process,
then tests what the configuration names. Which command makes which artifact is
a closed table in the runner; the command is one template in the allow-list. A
failed build ends the run before any device is touched, as a preparation
failure and not a test failure. The option waits for the build system.

## Configuration

```toml
[wled]
address = "192.168.1.40"

[mac]
target = "local"                       # "ssh://…" is refused: not implemented
binary = "dist/prysm-headless-macos-arm64"
device = "0x1100000345f2131"           # AVFoundation unique ID; omit without a card

[mac.feed.chromecast]
adb_serial = "0123456789ABCD"

[linux]
target = "ssh://pi@192.168.1.70"       # or "local"
binary = "dist/prysm-headless-linux-arm64"
deb    = "dist/prysm-headless-linux-arm64.deb"  # optional
device = "/dev/v4l/by-id/usb-EXAMPLE-video-index0"

[linux.feed.chromecast]
adb_serial = "0123456789ABCD"

[tv]
target   = "ssh://root@192.168.1.50"
ipk_full = "dist/prysm-tv-full.ipk"
ipk_slim = "dist/prysm-tv-slim.ipk"
```

- The runner compares `uname` on each target with the table's name and refuses
  a mismatch.
- A platform without `device` skips its capture cells. A `device` without a
  feed runs with the colour check reduced to "not black".
- `device` is what Prysm's own device setting takes. On Linux that is a path,
  and a stable `/dev/v4l/by-id/…` link is preferred over `/dev/videoN`, which
  can change between boots. On macOS it is the AVFoundation unique ID.
- A device identifier that is not found fails with the devices that are
  present and their identifiers. The runner matches no names.
- ADB runs on the computer that runs the runner.

## Commands

- `run.py host mac|linux [--delivery binary|deb] [--source file|device]`
- `run.py tv full` and `run.py tv slim --receiver mac|linux`
- `run.py eyes`, `run.py all`, `run.py restore-wled`
- `--prepare`, `--dry-run`, `--only` and `--from`. `--no-build` goes.

## Cells

| Cell | Needs |
| --- | --- |
| Mac, binary, file | FFmpeg 9 or newer on the Mac |
| Mac, binary, device | a card on the Mac; run from Terminal (camera permission) |
| Linux, binary, device | a card on the Linux machine |
| Linux, binary, file | FFmpeg 9 or newer on the Linux machine (Debian 13 ships an older one, so this cell is a SKIP on the bench Pi) |
| Linux, deb, device | a card on the Linux machine, `sudo` |
| TV full | the TV free |
| TV slim, receiver Mac or Linux | the TV free |

The Debian package has no `file` source, so that cell does not exist.

A Docker cell is not planned yet. The image and its compose file exist
([`packaging/docker/`](../../packaging/docker/compose.yaml)); the cell would
test what a container adds: the capture device passed in, the settings on a
volume, and the sender address WLED sees through the container's network. It
needs Docker on the Linux machine.

A cell whose artifact or tool is missing is a SKIP with the reason; a cell
asked for by name fails instead. Cells run one at a time, and the runner
refuses while something else drives WLED. With one card and one feed on the
bench, the capture cells follow where the card is plugged in: one `all` run
does not cover capture on both platforms.

## One host seam

Every host cell shares the same checks through one interface: deliver, start,
stop, logs, base URL, and the sender address WLED should report.

| | Deliver | Start and stop | Logs |
| --- | --- | --- | --- |
| Binary, local (Mac or Linux) | none | child process, SIGTERM | file |
| Binary, SSH (Linux) | copy to a run directory | over SSH; listens on the LAN | fetched file |
| Package (Linux, local or SSH) | copy, `apt-get install` | systemd unit | `journalctl` |

A remote `file` source also copies the clip to the target.

## Allow-list

`commands.py` stays the only module that builds a command line. Every remote
operation is its own template with checked arguments. There is no template that
runs a caller's string over SSH.

## Preflight

- **The runner's computer:** SSH; FFmpeg for the clip; ADB and the Chromecast
  library when the cell has a feed.
- **The target:** the architecture matches the artifact, `sudo` for the
  package, FFmpeg 9 or newer for the `file` source, the device identifier
  present and readable.
- **Artifacts:** the report records each one's version, hash and age (and its
  commit, once the build system writes a manifest), and warns when one is older
  than the checkout's last commit. After the start the
  running version (`prysm_build_info` in `/metrics`) is compared with the
  artifact's. Prysm reports the product version there (`1.0.0-alpha.0`; the
  package's own file name writes it the Debian way, `1.0.0~alpha.0-1`) and not
  its commit, so two builds of one version differ only by hash.

## Limits

- Capture on the Mac needs the run started from Terminal: macOS gives a
  process started elsewhere no camera prompt.
- The runner on Linux, and `[linux]` with `target = "local"`, use the same
  code as the tested paths but are not verified on the bench.
- With the runner on Linux the Mac cells are unavailable, because a remote Mac
  is refused.
- Windows as the runner's computer is not a goal.

## Phases

Each phase lands with its tests and CI green before the next starts.

1. **Host seam and configuration.** `host mac` replaces `desktop`. Verified
   with the Mac file cell.
2. **The Linux binary cell.** Copied to a Raspberry Pi 5 over SSH and run
   through the feed.
3. **The Linux package cell and `all`**, with the skip rules. The package cell
   is kept as it is and not extended until the package's future is decided.
   `--prepare` follows when the build system exists.
4. **The TV stages on the new configuration**, slim with a Mac or a Linux
   receiver.

The Docker cell has no phase yet: where it goes is to be decided.

A first package stage exists outside the tree (written before this plan, a
separate `linux` stage that builds its own package). It passed on a Raspberry
Pi 5 on 2026-11-16 and is the source for the package cell's commands and
checks.
