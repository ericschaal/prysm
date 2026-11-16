# Hardware acceptance runner

[`scripts/acceptance/run.py`](../../scripts/acceptance/run.py) checks Prysm on
real devices: WLED, a capture card, and both TV packages. It is run by hand,
never by CI or a build. Each check is a named step that ends PASS, FAIL or SKIP
with its evidence; a run writes `report.md`, `report.json` and its evidence
(snapshots, logs, CSV tables) to `work/acceptance/<time>-<stage>/`, which is
not tracked. Copy what matters into [research](../../research/README.md).

```sh
cp scripts/acceptance/config.example.toml scripts/acceptance/config.toml   # then fill in
python3 scripts/acceptance/run.py desktop --dry-run
python3 scripts/acceptance/run.py desktop
python3 scripts/acceptance/run.py desktop --source device --display-check
python3 scripts/acceptance/run.py tv --tv-is-free [--flavour full] [--sweep] [--wss-url https://HOST]
python3 scripts/acceptance/run.py eyes
python3 scripts/acceptance/run.py restore-wled work/acceptance/RUN/wled-snapshot.json
```

Python 3.11+ standard library only. `--dry-run` prints every step, HTTP call
and command (TV commands as they run there) and contacts nothing. `--only
NAME[,NAME]` and `--from NAME` (patterns over step names) select checks; setup
and cleanup steps always run, so a selected check finds what it needs (for
`tv`, a package is built and installed only when one of its checks is
selected). The exit
code is 0 when nothing failed (skips allowed), 1 on a failure, 2 on bad usage.
`config.toml` (ignored by git) holds the addresses (WLED, TV, receiver) and
the LED counts; no address is written in the code.

## Stages

| Stage | Needs | Person |
| --- | --- | --- |
| `desktop` | WLED; FFmpeg 9+ for the file source, or a capture card | Only for `--display-check` set-up |
| `tv` | The TV (only when its owner says it is free: `--tv-is-free`), WLED, the webOS SDK unless IPKs are given | Standby/wake, and removing the app unless `tv.ares_device` is set |
| `eyes` | A portal (the TV's, or `--portal`) | Every step |

### desktop

Builds (or uses) a release `prysm-headless`, starts it on `127.0.0.1` with a
temporary settings file (WLED output, neutral processing) and checks through
the [portal API](../../surfaces/web-portal/HTTP-CONTRACT.md) and WLED's JSON
API:

- the source: a generated clip with known edge colours (`--source file`), a
  given clip (`file:PATH`), or a capture card (`device`: the devices and
  their modes are printed, the preset resolved as the portal does);
- Screen sync starts; WLED reports live DDP from this computer; the frame
  rate is within 15 % of the source's and nothing falls behind; each edge
  shows the clip's colour (a card: not black);
- WLED effects (`device_native`), Screen sync again, off;
- WLED's brightness: 40 % reported within 5 s at `bri` 102, gone within 35 s
  with Force max brightness, `state.bri` unchanged, `/json/cfg` under 64 KiB;
- the Smooth dark colours switch, off and on; with a card, release and
  re-open;
- `--display-check`: when the card records this Mac's HDMI output, a local
  page the runner controls shows red, green, blue, white and an edge pattern;
  each edge must come out as that colour;
- SIGTERM: exit code 0 and WLED released.

WLED's gamma is not observable through Prysm's API, so it is a SKIP here and a
question in `eyes`. For a rehearsal without lights, run the stand-in
`python3 scripts/acceptance/fake_wled.py --ddp 127.0.0.1:4048` and give
`--wled 127.0.0.1:18080`.

### tv

For each package (`full`, then `slim`): build (`build-ipk.sh`) or take the
configured IPK, check it with `ipk.py verify`, install it the way that works
on webOS 10 ([packaging](packaging.md)), and check:

- registration: `getState` answers with that flavour; the app, service,
  manifests and boot hook exist;
- full: the TV's portal answers and captures with video playing; 90 s soaks
  at 640 × 360 and 1280 × 720 with the portal's frame rate and the worker's
  CPU and RSS from `/proc`; `POST /api/v1/capture/modes`;
- slim, streaming to a `prysm-headless` the runner starts here (or
  `--receiver URL`): the TV's offer is known while the lights are off; each
  encoding streams 640 × 360 at 30 fps; a 90 s soak with hardware H.264 and
  the worker's CPU and RSS; 60 fps with hardware H.264 is refused
  on `capture.fps`; pause and resume; a stream version mismatch (the receiver
  answers 426, and a local stand-in answering another version makes the TV
  report `version`); optional `wss://`;
- `--sweep`: sizes × rates actually delivered, for capture (full) and for
  each encoding (slim), as `sweep-full.csv` and `sweep-slim.csv`;
- standby and wake (guided: a person presses power, the runner watches);
- reinstall keeps preferences, the full settings and the lighting;
- uninstall: the supervisor's `uninstall` and `ares-remove` (or the person)
  leave no app, service, manifest, boot hook, staged package, process or
  Luna service; `/var/lib/io.prysm.tv` stays, as documented.

A hung capture has no documented trigger on the installed package: it is a
SKIP, covered by `native-smoke.py --supervisor` and the supervisor tests.

### eyes

One question at a time, answered `y`, `n` or `skip` with an optional note:
the lights match the picture, dark scenes, dithering off and on in a dark room
(the runner switches it), the TV app's screens and dialogs, the nine TV UI
polish items, calibration patterns (the runner shows each) and the TV's own
calibration flow, the portal on a phone, and a cold boot by unplugging.

## Safety

- **The TV.** Every command the runner can run, on the TV or here, is a
  template in [`commands.py`](../../scripts/acceptance/commands.py); its
  parameters are checked against fixed patterns and nothing from the
  configuration becomes shell text. The TV commands only read, stage the
  package as `/media/developer/temp/io.prysm.tv.ipk` (removed after each
  install), install through `com.webos.appInstallService`, run the package's
  `setup.sh` and the supervisor's `uninstall`, and call Prysm's own Luna
  methods. Nothing writes to system partitions or deletes
  `/var/lib/io.prysm.tv`. [Tests](../../scripts/tests/test_acceptance_commands.py)
  enumerate every template and check the paths it names.
- **Bounded.** Every command has a timeout; `luna-send` runs with its stdin
  held open and a remote `timeout`, as `install-ipk.sh` does.
- **Left as found.** The `tv` stage saves `getState`, the files in
  `/var/lib/io.prysm.tv` and the full portal's settings first (and stops if
  an installed Prysm does not answer, since it could not be put back), and at
  the end installs the package it found (`tv.restore_flavour`), sets the
  preferences back and puts the settings and lighting back. WLED is
  snapshotted before anything changes (`wled-snapshot.json`) and restored
  last, after Prysm has stopped, also on a failure, Ctrl-C, SIGTERM or SIGHUP;
  a second Ctrl-C does not cut the restore short. After a hard kill use
  `restore-wled`.
- **The owner decides.** The `tv` stage refuses to start without
  `--tv-is-free`.

On macOS, a Local Network refusal (`EHOSTUNREACH`) means the app running the
script needs Local Network access; the runner says so.
