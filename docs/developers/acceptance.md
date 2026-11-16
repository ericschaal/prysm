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
uv run --project scripts/acceptance scripts/acceptance/run.py desktop --source device
python3 scripts/acceptance/run.py tv --tv-is-free [--flavour full] [--sweep] [--wss-url https://HOST]
python3 scripts/acceptance/run.py eyes
python3 scripts/acceptance/run.py restore-wled work/acceptance/RUN/wled-snapshot.json
```

Python 3.11+. Every stage runs with the standard library alone, except a
capture card run with the Chromecast feed, which needs
[PyChromecast](https://github.com/home-assistant-libs/pychromecast).
`scripts/acceptance` is a [uv](https://docs.astral.sh/uv/) project for that
(`pyproject.toml`, `uv.lock`); `uv run --project scripts/acceptance` from the
repository root installs it into `scripts/acceptance/.venv` and runs the
runner with it. The tests need nothing installed. `--dry-run` prints every step, HTTP call
and command (TV commands as they run there) and contacts nothing. `--only
NAME[,NAME]` and `--from NAME` (patterns over step names) select checks; setup
and cleanup steps always run, so a selected check finds what it needs (for
`tv`, a package is built and installed only when one of its checks is
selected). The exit
code is 0 when nothing failed (skips allowed), 1 on a failure, 2 on bad usage.
`config.toml` (ignored by git) describes the bench: the addresses (WLED,
TV, receiver), the feed into the capture card and the LED counts; no
address is written in the code.

## Stages

| Stage | Needs | Person |
| --- | --- | --- |
| `desktop` | WLED; FFmpeg 9+ for the file source, or a capture card (with a feed: FFmpeg, and for the Chromecast adb and uv) | Only for `--display-check` set-up |
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
- with a card and a feed: the generated clip on the card's input (see
  below);
- Screen sync starts; WLED reports live DDP from this computer; the frame
  rate is within 15 % of the source's and nothing falls behind; each edge
  shows the clip's colour (a card without a feed: not black);
- WLED effects (`device_native`), Screen sync again, off;
- WLED's brightness: 40 % reported within 5 s at `bri` 102, gone within 35 s
  with Force max brightness, `state.bri` unchanged, `/json/cfg` under 64 KiB;
- WLED's gamma: `/json/cfg` has the fields Prysm reads, as it parses them
  (`if.live.no-gc` true or false, `light.gc.col` and `light.gc.bri` numbers
  within 0.1–3), read only;
- the Smooth dark colours switch, off and on; with a card, release and
  re-open;
- `--display-check`: when the card records this Mac's HDMI output, a local
  page the runner controls shows red, green, blue, white and an edge pattern;
  each edge must come out as that colour. The card then records this Mac,
  so the feed is not started for that run;
- SIGTERM: exit code 0 and WLED released.

#### The feed into the capture card

A capture card shows whatever is plugged into it, or a "no input" splash.
To compare the edges with a known picture, the bench needs a feed the runner
controls: something on the card's HDMI input that can show the test clip and
stop again. The configuration names it. With a feed, the `desktop.feed` step
("Picture into the capture card") shows the generated clip on it and the
colour check compares each edge with the clip, as with the file source.
Without one, that step is a SKIP that says so, and the colour check can only
say "not black". The file source needs no feed and never starts it.

The supported feed is a Chromecast with Google TV, `[feed.chromecast]`:

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
overlay to clear, checks that the clip is still playing and compares each
edge with the clip's colour by its nearest named colour, which absorbs the
shift through HDMI and the card. The capture re-open check also needs the clip
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

`[feed.chromecast]` in `config.toml`: `adb_serial` (required),
`serve_port` (default 18792) and `settle_seconds` (default 5; the
Chromecast's player overlay, so it belongs to this feed). Remove the section
when nothing the runner controls feeds the card. The ADB commands are
templates in `commands.py` like every other command; everything else about
the Chromecast is in `feed_chromecast.py`, behind `feed.py`.

The gamma check proves only that this WLED has the configuration layout
Prysm reads. Prysm does not expose the gamma it applies, so whether it
applied it stays with the `output-wled` unit tests, and whether dark scenes
look right is a question in `eyes`. For a rehearsal without lights, run the stand-in
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
  enumerate every template and check the paths it names. On the Chromecast,
  ADB only reads its state and Wi-Fi address, wakes it and reads or sets
  `stay_on_while_plugged_in`; there is no free-form `adb shell`.
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
