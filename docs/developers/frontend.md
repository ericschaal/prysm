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

The DOM tests use jsdom. Mobile browsers (iOS Safari, Firefox) have not been
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
for every push, with the binaries built by the `rust (ubuntu-24.04)` job, one retry per test
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
