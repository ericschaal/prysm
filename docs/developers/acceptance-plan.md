# Plan: the acceptance runner's next shape

This is a plan, not a description of what exists. The
[acceptance runner](acceptance.md) today has the stages `desktop`, `tv` and
`eyes`, runs Prysm only on the computer it runs on, and builds what it tests.
This page is what it is to become. Delete it when the last phase has landed and
[acceptance.md](acceptance.md) describes the result.

## Goal

The runner tests what unit and end-to-end tests cannot: real devices (a
capture card, WLED, the TV, the feed), the artifacts as they are delivered
(installed, started, restarted and removed on the real system), the whole
chain from a colour on the screen to that colour on the right LEDs, and what
takes time or a person (soaks, memory growth, the `eyes` stage). A check that
a fake could prove belongs in the unit or end-to-end tests, which are faster
and run in CI; the runner does not repeat it.

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
5. **The runner does not redo what Rust or TypeScript already define.** It
   has no copy of Prysm's API: it types only the fields its checks read, keeps
   settings as opaque JSON, and a test feeds the generated examples
   (`frontend/packages/contracts/generated/fixtures.json`) through its parsers,
   so a field it depends on cannot change unnoticed. A closed set it must name
   (targets, encodings) is checked against the generated contracts by a test.
   It re-implements no product logic: where a check would need some, the check
   asks Prysm or uses explicit values.

## Layout

- **The build system** makes the artifacts: `cargo xtask dist TARGET...`
  ([packaging](packaging.md#artifacts-in-dist)) writes them to `dist/` under
  fixed names, with `manifest.json` (version, commit, hash, build time). It is
  its own work, apart from this plan (bead `prysm-9ho`).
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
image  = "dist/prysm-headless-linux-arm64.tar"  # optional
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

- `acceptance host mac|linux [--delivery binary|deb|image] [--source file|device]`
- `acceptance tv full` and `acceptance tv slim --receiver mac|linux`
- `acceptance eyes`, `acceptance all`, `acceptance restore-wled`
- `--prepare`, `--dry-run`, `--only` and `--from`. `--no-build` goes.

## Cells

| Cell | Needs |
| --- | --- |
| Mac, binary, file | FFmpeg 9 or newer on the Mac |
| Mac, binary, device | a card on the Mac; run from Terminal (camera permission) |
| Linux, binary, device | a card on the Linux machine |
| Linux, binary, file | FFmpeg 9 or newer on the Linux machine (Debian 13 ships an older one, so this cell is a SKIP on the bench Pi) |
| Linux, deb, device | a card on the Linux machine, `sudo` |
| Linux, image, device | a card and Docker on the Linux machine |
| TV full | the TV free |
| TV slim, receiver Mac or Linux | the TV free |

The Debian package and the image have no `file` source, so those cells do
not exist.

The image cell runs the image with its compose file
([`packaging/docker/`](../../packaging/docker/compose.yaml)) and tests what a
container adds: the capture device passed in, the settings on a volume, and
the sender address WLED sees through the container's network. Like the
package cell it is a thin layer over the shared checks.

A capture cell that fails is a plain failure: the runner does not retry. The
report includes the target's kernel lines about USB from the run, so a card
that reset (bead `prysm-9jt`) is recognisable.

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
| Image (Linux, local or SSH) | copy, `docker load` | `docker compose` | `docker logs` |

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
- **Artifacts:** the report records each one's hash and age. When the build
  system's `manifest.json` is next to the artifact and its hash matches, the
  report also records the version and commit from it. The runner warns when an
  artifact is older than the checkout's last commit. After the start the
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
   with the Mac file cell. Before it starts, every existing check is held
   against the goal: one a fake could prove moves to the unit or end-to-end
   tests, or is dropped where they already cover it. The copies named in
   principle 5 go the same way.
   The phase opens with a spike: the Mac file cell on pytest (the host and
   the WLED snapshot as fixtures, cells as parameters, skips and selection
   from pytest) next to the current version. If the spike is clearer, pytest
   replaces the runner's own step runner (`plan.py`) and its unittest tests;
   `--dry-run` stays the runner's own, and `eyes` may stay a plain script.
2. **The Linux binary cell.** Copied to a Raspberry Pi 5 over SSH and run
   through the feed.
3. **The Linux package and image cells and `all`**, with the skip rules. Both
   deliveries stay thin; the package cell is not extended until the package's
   future is decided. `--prepare` is added here.
4. **The TV stages on the new configuration**, slim with a Mac or a Linux
   receiver.

Phase 1 waits for the build system.

A first package stage exists outside the tree (written before this plan, a
separate `linux` stage that builds its own package). It passed on a Raspberry
Pi 5 on 2026-11-16 and is the source for the package cell's commands and
checks.
