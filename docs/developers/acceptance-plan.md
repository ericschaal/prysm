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
| Linux, binary, file | FFmpeg 9 or newer on the Linux machine (Debian 13 ships none that new; the bench Pi has one installed by hand) |
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
   Decided on 2026-11-17, after the audit and a spike that passed on the bench:
   - **pytest runs the host cells.** The host and the WLED snapshot are
     fixtures, each with its teardown; a cell is a parameter; selection and
     skips are pytest's. The checks take typed fixtures. `--dry-run` is a
     pytest option that prints the commands and actions and runs nothing.
     pytest is a main dependency, so a bench run installs no dev group.
     `eyes` stays a plain script, and the `tv` stage keeps the old step
     runner (`plan.py`) until phase 4 moves it.
   - **Dropped**, because other tests prove them: the build steps, the
     settings check, the first `processed` step, the 60 fps refusal, the
     version-mismatch check, the hung-capture steps and eleven `eyes` items
     about the TV app's dialogs and options.
   - **Moved** to the product's own tests: reading WLED's gamma from a real
     firmware configuration (output-wled), changing dither while lighting
     runs (the headless host's system tests), and four TV layout and
     re-render checks (the TV end-to-end and unit tests).
   - **Copies removed:** preset resolution (the configuration names an exact
     device and size), the gamma parser, the stream version constants, the
     TV portal port and the IPK name patterns. The TV paths stay explicit as
     the safety allow-list, with a contract test.

   **Landed** ([acceptance.md](acceptance.md#host)): `host mac` on pytest
   with the local-binary seam, the configuration above (`[mac]`, `[linux]`
   read but unused), artifacts from the configuration with their manifest,
   `--dry-run`, and the drops and removals. The `eyes` item about idle
   re-renders stays until its unit test exists (prysm-skn). What remains of
   it: the Mac device cell has not run on the bench (its device and mode
   check is tested against the generated examples only); the TV paths have
   no generated source to test the allow-list against, so that contract
   test waits for one; the `tv` stage still reads its old `[tv]` keys
   (`address`, `ssh`) until phase 4. `--prepare` is phase 3.
2. **The Linux binary cell.** Copied to a Raspberry Pi 5 over SSH and run
   through the feed.

   **Landed** ([acceptance.md](acceptance.md#run-the-linux-binary-cells)):
   `host linux` with the cells `linux-binary-device` and `linux-binary-file`,
   the SSH seam (`SshBinary`: a run directory under `~/.prysm-acceptance/`,
   a detached start that records its PID and exit status, SIGTERM and KILL
   only to that PID, the log and the kernel's USB lines fetched, the
   directory removed), `[linux]` used (`target`, `binary`, `device`,
   `listen`, `capture`, `feed`), the target preflight (ssh without a prompt,
   `uname` against the binary's ELF machine, FFmpeg 9+ for the file source,
   the device readable, the port free) and a `linux.*` template per remote
   operation. Both cells passed on a Raspberry Pi 5 on 2026-11-17: the
   device cell through the capture card and the Chromecast feed, the file
   cell with FFmpeg 9 installed on the Pi (Debian 13 ships none that new).
   The capture size is always sent, and the card must offer it as a mode.
   What remains: `target = "local"` on a Linux runner, which uses the same
   code as the Mac but has not run.
3. **The Linux package and image cells and `all`**, with the skip rules. Both
   deliveries stay thin; the package cell is not extended until the package's
   future is decided. `--prepare` is added here.

   **Landed** ([acceptance.md](acceptance.md#run-the-linux-package-and-image-cells)):
   the cells `linux-deb-device` (`SshDeb`: apt-get
   install, a runtime drop-in for `--listen` and `--metrics`, the settings
   through the portal, systemd's exit status, the journal; its checks: not
   root, a restart keeps the settings and lights again; purge and a check
   that nothing is left) and `linux-image-device` (`SshImage`: `docker load`,
   compose.yaml with a second compose file of the runner's values, the
   settings seeded into the volume, the container's exit code, `compose
   logs`; its check: a recreated container keeps the settings; `down
   --volumes` and the image removed). The bench must start without Prysm
   installed and without the image; both are refused otherwise. Each
   delivery is one module with its templates, its cell and its `@only`
   checks. `acceptance all` runs every host cell; the `tv` stage joins it
   with phase 4. `--prepare` builds the chosen cells' configured `dist/`
   artifacts with one `cargo xtask dist` before anything is contacted (exit
   code 3 when it fails). Both cells passed on a Raspberry Pi 5 on 2026-11-17
   (Debian 13, Docker 26.1.5, compose 2.26.1) through the capture card and
   the Chromecast feed, and left nothing behind. What remains: `--prepare`
   has not run a real build, and `target = "local"` for these cells is not
   implemented.
4. **The TV stages on the new configuration**, slim with a Mac or a Linux
   receiver.

   **Landed** ([acceptance.md](acceptance.md#tv)) and run on the TV:
   the cells `tv-full`, `tv-slim-mac` and `tv-slim-linux` on pytest
   (`acceptance tv full`, `acceptance tv slim --receiver mac|linux`, and
   `acceptance all --tv-is-free`), each only with `--tv-is-free`; the WLED
   snapshot, the TV and the installed package as fixtures with teardowns;
   the host cells' WLED and frame-rate checks shared; the slim receiver on
   the host cells' binary seam (`LocalBinary`, `SshBinary`); `[tv]` with
   `target`, `ipk_full`, `ipk_slim`, `mac` (wake-on-LAN, one allow-listed
   packet), `installed` and `soak_seconds`, and `[receiver]`,
   `[sweep]`, `address`, `ssh`, `flavours` and `restore_flavour` removed;
   `installed = "refuse" | "replace"` for the TV and the Linux package cell,
   written once (`installed.py`), with the backup, the end-of-run warning
   and `acceptance settings restore`. `plan.py` stays for `eyes`. Dropped:
   `--sweep` (a measurement table, not acceptance), the `wss://` check (it
   needed a reverse proxy in `[receiver]`), and restoring the package found
   at the end (`restore_flavour`; the runner no longer reinstalls).
   On the bench TV (webOS 10, showing its screen saver, so the capture
   includes the TV's own picture): `replace` backed up and removed an
   installed full package, and the backup matched a copy taken by hand;
   `tv-full` passed (30 fps in both soaks, the reinstall kept the settings,
   the uninstall left nothing); `tv-slim-mac` passed with all three
   encodings at 30 fps; `tv-slim-linux` passed but for the raw encoding,
   4 fps to a Raspberry Pi 5 on Wi-Fi where the two H.264 encodings held 30;
   a second run of that check alone held raw at 30, and the cause is open
   (the check now records the size that arrives and the TV's state). Those
   runs proved less than they seemed: in the screen saver the TV reports no
   video, so every frame was the UI capture, at most 320 × 180 whatever was
   asked. The "1280 × 720" soak and the encodings delivered UI-sized frames,
   and the video path was not tested at all. The app is removed through
   `appInstallService`, as it is installed. What remains unverified: the
   wake-on-LAN packet (the TV answered), `acceptance settings restore`
   against a reinstalled Prysm (only its dry run on the real backup),
   standby and wake (a person presses power), and `replace` on the Linux
   package cell.

   Since then every TV check that measures runs in a state the runner sets
   and confirms from the TV ([The TV's states](acceptance.md#the-tvs-states)):
   video (the test clip cast to the TV's own Cast receiver with a subtitle
   overlay over its bottom) or the UI (the cast stopped, Prysm's app in
   front). On the bench TV on 2026-11-17, `tv-full` with the states: video
   available at 640 × 360 and 30 fps; the colours followed the clip with the
   capture leaving the UI out, and with it included the bottom edge showed
   the overlay at every step while the rest followed the clip; WLED live,
   the frame rate, capturing, the capture modes, the reinstall, and both
   soaks passed with the frames at the size asked (1280 × 720 at 30 fps, the
   worker at 51 % CPU). In a first run the UI checks passed too (320 × 180;
   no frames and no failure without the UI, frames again with it); in every
   run after it the worker's UI capture failed to open (`HAL_GAL_Init failed
   with status -1`), so the UI checks failed, also alone on a fresh install.
   The slim cells have not run with the states.


A first package stage exists outside the tree (written before this plan, a
separate `linux` stage that builds its own package). It passed on a Raspberry
Pi 5 on 2026-11-16 and is the source for the package cell's commands and
checks.
