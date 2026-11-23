# Frontend development

`frontend` is one npm workspace (Node.js 24+) with four bundles: the
desktop portal (served by `prysm-headless` and `prysm-tui`), the TV's own
portal (full package), and the TV app for each package. The Rust hosts embed
the browser bundles at compile time
([`prysm_host::assets`](../../crates/hosts/common/src/assets.rs)); a debug build
without them serves a "not built" page and a release build fails. The TV
bundles go into the IPKs. `cargo xtask check frontend` runs this workspace's
checks as CI's `frontend` job does.
Reference: [frontend README](../../frontend/README.md) (screens, saving
rules, limits) and [UI and host assemblies](ui-assemblies.md).

## Portal with hot reload

Start a host, then the dev server in a second terminal:

```sh
npm --prefix frontend ci
npm --prefix frontend run bundle
cargo run -p prysm-tui -- crates/hosts/tui/settings.example.json
```

```sh
npm --prefix frontend run dev:headless
```

The dev server forwards `/api` to `http://127.0.0.1:8787`, or to the Prysm
named by `PRYSM`. Use disposable settings: capture and WLED are real unless the
settings use the `null` output.

## API generation

Rust owns every value the frontend exchanges with Prysm. The exporter writes
OpenAPI documents and serialisation fixtures; codegen turns them into
TypeScript under `packages/contracts/generated`, which is checked in. Never
edit generated files.

```sh
cargo xtask codegen          # after a Rust contract change
cargo xtask codegen --check  # fail on drift without writing
```

xtask runs the Rust exporter, so codegen needs a Rust toolchain (and
`libclang` on Linux), and passes its output to `npm run codegen`, which only
translates it; run on its own, that script stops and names the xtask command.
Details: [contract ownership](contracts.md).

## Reading the state

The portal page and the TV app may be older than the Prysm or TV service
they talk to, so they read what they are sent as
[wire messages](decisions/2026-11-22-strict-settings-tolerant-wire.md):
the structure is checked, and a value newer than the page in a known field
has one reading.

| Where | Field | Newer value read as |
| --- | --- | --- |
| State stream (`decodeState` in `application-client/http.ts`) | `target` | `unknown`: no mode is selected; a known one can still be chosen |
| | `session.state` | `unknown`, keeping a `failure` or `message` it carries (a malformed failure is dropped) |
| | `session.failure.kind` | said by its side: an `adapter` failure of its `section`, or else of its `field`'s section (`capture`, `output`); without one, `internal` |
| | `session.failure.section` | absent, unless `capture` or `output` |
| Preview and colors streams | event `type` | skipped |
| Light card, TV lights card | `device.issue` | "The lights aren’t showing screen sync." |
| TV service (`decodeState` in `webos-platform`) | `power`, `video` | `unknown` |
| | `failure.kind` | `other` |
| | full `target` | `unknown` |
| | worker `phase` | `unknown`: "Updating…", with no start or stop |
| | slim link `state` | starting |
| | slim `encoding` | kept, shown as sent |
| TV calibration screen | `calibration.pattern` | the waiting pattern |

No value is read as off or stopped
([decision](decisions/2026-11-22-open-response-enums.md)). Anything else
malformed ends the stream or the reply (`Invalid state event.`,
`invalid_reply`). What the lights are doing is decided once, by
`lightingSituation` (`application-client`) from the target and the session
(with the failure it carries), in this order: faulted, unknown (a target or
session state newer than the page), off, reached (`session.state` equals the
target: screen sync or effects), failed (the session carries a failure),
starting. Every pair lands on exactly one. Screen sync never carries a
failure. An `effects` session carries one after a start of screen sync
that failed from the effects (the situation is then failed, headed "Screen
sync couldn’t start", and the cards add that the effects are still on,
except when the lights themselves failed, `section` `output`), or, with
the effects as the target, for saved settings that do not validate (a
note, "Prysm can’t use these settings."; another kind is said in its own
words). Off keeps a stopped session's failure (settings that could not be
applied, a switch off that failed): the light card and the TV's lights
card say it, and the full TV's "Lights off" headline gives its words with
attention. The setup path marks a failure that names a side
(`failureSide`), or settings that do not validate (Prysm's stop without a
side); Change setup opens that side's page, whose status says it ("Can’t
use these settings" for settings that do not validate), the Settings row
says it, and the other section stays "Not in use". Any other failure with
the lights off is said by the light cards alone. No "Try again" is
offered: trying could only switch off again. Unknown keeps what it
carries: the note says Prysm is newer, the failure's words follow it, and
its message goes under Details; no mode tile is chosen, and Power switches
the lights off. A faulted session's `message` (what did not finish) is under
Details, on the light card and the section pages. All of them read that
one situation, and `describeSituation` gives its words.

The full TV package's status follows the portal's situation once it is
known: the worker reports its session's failure to the supervisor, and the
situation says what it means (off is off, effects showing are on). A
faulted or failed session comes before the TV's power and video, since it
needs the user and waking the TV would not end it; lights that are fine let
power and video speak first. Try
again is offered only for a failed mode (`screen_sync` or `effects`) and
sends `setTarget off`, then that target again. Before the portal answers,
or while the supervisor retries a worker, the supervisor's failure is the
headline (before the portal answers, only for a mode: lights that are off
are said as off). A worker phase newer than the app shows "Updating…" unless a
failure is reported.

## Saving from a page

`useLiveProcessing` saves the processing section from the Picture panel, the
light card, Calibrations and Calibrate. Saves never overlap: each waits for
the one before it and is sent against the revision that one returned, so
`commit(value)` resolves only once `value` itself is saved, with what was saved (or `null`). A save
refused as stale is moved onto the newer settings, each calibration on its
own: one changed here keeps this page's version, one changed elsewhere the
other's, one deleted on either side is gone.

The Calibrate page is one reducer (`calibrate-wizard.ts`, every transition
in one function and tested as a table) and one hook (`use-calibration.ts`)
that sends the TV's patterns and the saves; `calibrate.tsx` only renders.
Start shows the first pattern, then saves the draft calibration; when that
save fails it ends the pattern and puts the draft back. A save whose answer
never came may have run: the wizard keeps the draft's id, and when the state
shows that calibration it goes on with it, shown as ended (Resume or
Leave). A second Start saves the same draft. A failed live save is
answered the same way, once a newer state shows its value.

## webOS TV app

```sh
npm --prefix frontend run dev:webos        # full package's TV app
npm --prefix frontend run dev:webos-slim   # slim package's TV app
```

Open `/demo.html` on the dev server. It runs the TV app on an in-memory
supervisor and portal and never contacts a TV; selects at the bottom right
switch the supervisor's states. `dev:webos-portal` serves the TV's portal
against a running TV (`PRYSM=http://TV_IP:8787`).

TV bundles target Chromium 53 (one classic script, relative URLs for
`file://`). Controlling a portal over HTTP needs webOS 5 or newer. Real webOS
browsers and remote-control navigation have not yet been verified on a TV.

## Tests and formatting

```sh
npm --prefix frontend run typecheck
npm --prefix frontend run bundle     # frontend/dist; the TV tests read it
npm --prefix frontend test           # contract and DOM tests
npm --prefix frontend run lint
npm --prefix frontend run format:check
npm --prefix frontend run format     # rewrite with Prettier
```

The DOM tests use jsdom. `npm test` bundles each `tests/*.test.jsx` on its
own ([tests/vite.config.mjs](../../frontend/tests/vite.config.mjs)) and runs
the bundles with `node --test`: each file in its own process with a 2 GB heap,
four at a time, each test bounded to 60 s.
[tests/watchdog.js](../../frontend/tests/watchdog.js) kills a file's process
after 10 minutes or at 3 GB resident, even while its main thread is blocked,
so that file fails and the others still run. Assertions about elements pass `assert` a
boolean, through `assertFocused` and `assertSame` in
[tests/dom.jsx](../../frontend/tests/dom.jsx): a failed `assert.equal` prints
its values in full, and a DOM node in full is the whole document. ESLint
rejects `document.activeElement` in an assertion. Mobile browsers (iOS Safari, Firefox) have not been
checked against the portal.

## End-to-end tests

`frontend/e2e/` drives the portal in Chromium and WebKit with
[Playwright](https://playwright.dev/). Each test starts its own
`prysm-headless` on an ephemeral loopback port with a settings file in a
temporary directory (remote capture, `null` output), and may start test
senders: `prysm-capture-remote`'s `test_sender` example stands in for the
TV's slim worker, streaming generated frames, refusing every connection as a
busy TV does (`--refuse REASON`), stopping after some frames
(`--stop-after N`) or announcing fewer encodings, sizes and rates
(`--offer JSON`). A test that
sets `devices` starts `prysm-headless`'s `fake_devices` example instead:
the production host with stand-in video devices (`PRYSM_FAKE_DEVICES`, each
with its name, path and modes) that discovery finds and that never capture.
Nothing leaves loopback.

```sh
cargo build -p prysm-headless
cargo build -p prysm-headless --example fake_devices
cargo build -p prysm-capture-remote --example test_sender
npm --prefix frontend run e2e:install   # Chromium and WebKit, once per Playwright version
npm --prefix frontend run e2e           # Chromium and WebKit; add -- --headed, -- -g brightness
```

One engine, or Firefox (installed with `npx playwright install firefox`), from
`frontend`: `npx playwright test -c e2e --project firefox`.

The binaries embed the portal bundle at compile time: rebuild the frontend,
then `prysm-headless` and `fake_devices`, before testing a frontend change;
the suite stops at once if either serves another bundle than `dist`, or the
"not built" page. The
tests take the debug builds from `$CARGO_TARGET_DIR` (default `target`), or
`PRYSM_HEADLESS_BIN`, `PRYSM_FAKE_DEVICES_BIN` and `PRYSM_TEST_SENDER_BIN`. Results, traces and videos
go to `frontend/.e2e/`. Find elements as a user does, by role and
accessible name, not by class names.

Playwright's WebKit is not Safari, but it shares Safari's engine bugs: the
brightness drag fails in it when the slider captures the pointer
([WebKit bug 219636](https://bugs.webkit.org/show_bug.cgi?id=219636)), which
Chromium never showed. Touch drags and iOS are not covered: see
[manual browser checks](#manual-browser-checks).

CI (`e2e` in `.github/workflows/ci.yml`) runs Chromium and WebKit on Linux
for every push in Playwright's container image of the `@playwright/test`
version in `frontend/package.json` (browsers and their system packages
included), with the binaries built by the `rust (ubuntu-24.04)` job, one retry per test
and traces on retry; the report, traces and videos are uploaded when it
fails. `.github/workflows/e2e-nightly.yml` runs WebKit (closest to Safari on
macOS) and Firefox on a macOS runner every night and on demand.

### TV app

`frontend/e2e-tv/` is a separate Playwright project for the TV app of both
packages: Chromium only, 1920 × 1080, against the in-memory demo
(`testkit/tv-demo.tsx`) that Vite serves on loopback. It needs no
`prysm-headless` and no build. It drives the app only with remote keys
(arrows, OK as Enter, Back as keyCode 461) and checks what jsdom cannot: after
every key one control has focus, visible, inside the viewport and the 5 %
safe area of `tv.css`, with a focus ring; arrows reach every enabled control
on each screen and dialog; Back closes one dialog level and returns focus to
its opener, and on Home asks before closing; controls are at least `--hit`
(78 px) in their smaller dimension and text at least 20 px; and journeys
(settings, a long diagnostics log, calibration, first run of each package).

```sh
npm --prefix frontend run e2e:install   # once per Playwright version
npm --prefix frontend run e2e:tv        # add -- --headed, -- -g "first run"
```

Limits: desktop Chromium is not the TV's browser (webOS 4 runs Chromium 53),
fonts and rendering differ from the TV's, the service is the demo's fake
supervisor rather than Luna, the portal is an in-memory fake, and text entry
fills the field instead of using the TV's on-screen keyboard. Remote
navigation on a real TV still needs a manual check. CI runs it in the `e2e`
job.

The system behind the portal (the TV stream over `ws://` and `wss://`, a fake
WLED on 127.0.0.1, settings files across restarts) is tested without a
browser in [`crates/hosts/headless/tests/system.rs`](../../crates/hosts/headless/tests/system.rs).

### Manual browser checks

Before a release, and after changing a control's pointer or touch handling,
on an iPhone (Safari) and on a Mac (Safari), against a Prysm on the LAN:

- [ ] Home: Power turns screen sync on and off; the state words follow.
- [ ] Home: drag the brightness thumb with a finger (iPhone) and the mouse
      (Mac); the lights follow while dragging and the value stays after
      letting go, also when the finger leaves the slider.
- [ ] Home: tap the brightness track; the thumb jumps there and stays.
- [ ] A failed start (unplug the source or stop the TV): the problem shows,
      Try again works, and changing brightness does not leave the card busy.
- [ ] Setup: complete the wizard once with the on-screen keyboard; nothing is
      hidden behind it.
- [ ] Settings: change a value, reload, and it is still there.
- [ ] Rotate the phone on Home and in Settings: nothing is cut off.

Note the device, iOS or macOS version and Prysm version with the result. A
simulator check with [Maestro](https://maestro.dev/) could automate the iPhone
steps later.
