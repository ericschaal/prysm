# Frontend development

`frontend` is one npm workspace (Node.js 24+) with four bundles: the
desktop portal (served by `prysm-headless` and `prysm-tui`), the TV's own
portal (full package), and the TV app for each package. The Rust hosts embed
the browser bundles at compile time; the TV bundles go into the IPKs.
Reference: [frontend README](../../frontend/README.md) (screens, saving
rules, limits) and [UI and host assemblies](ui-assemblies.md).

## Portal with hot reload

Start a host, then the dev server in a second terminal:

```sh
npm --prefix frontend ci
npm --prefix frontend run build
cargo run -p prysm-tui -- hosts/tui/settings.example.json
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
npm --prefix frontend run codegen        # after a Rust contract change
npm --prefix frontend run codegen:check  # fail on drift without writing
```

Codegen runs Rust, so it needs a Rust toolchain (and `libclang` on Linux).
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
npm --prefix frontend test           # codegen, typecheck, build, contract and DOM tests
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
TV's slim worker, streaming generated frames, refusing its first connections
(`--refuse N REASON`) or stopping after some frames (`--stop-after N`).
Nothing leaves loopback.

```sh
cargo build -p prysm-headless
cargo build -p prysm-capture-remote --example test_sender
npm --prefix frontend run e2e:install   # Chromium and WebKit, once per Playwright version
npm --prefix frontend run e2e           # Chromium and WebKit; add -- --headed, -- -g brightness
```

One engine, or Firefox (installed with `npx playwright install firefox`), from
`frontend`: `npx playwright test -c e2e --project firefox`.

The binary embeds the portal bundle at compile time: rebuild the frontend,
then `prysm-headless`, before testing a frontend change; the suite stops at
once if the binary serves another bundle than `dist`. The tests take the
debug builds from `$CARGO_TARGET_DIR` (default `target`), or
`PRYSM_HEADLESS_BIN` and `PRYSM_TEST_SENDER_BIN`. Results, traces and videos
go to `frontend/.e2e/`. Find elements as a user does, by role and
accessible name, not by class names.

Playwright's WebKit is not Safari, but it shares Safari's engine bugs: the
brightness drag fails in it when the slider captures the pointer
([WebKit bug 219636](https://bugs.webkit.org/show_bug.cgi?id=219636)), which
Chromium never showed. Touch drags and iOS are not covered: see
[manual browser checks](#manual-browser-checks).

CI (`e2e` in `.github/workflows/ci.yml`) runs Chromium and WebKit on Linux
for every push, with the binaries built by the `rust` job, one retry per test
and traces on retry; the report, traces and videos are uploaded when it
fails. `.github/workflows/e2e-nightly.yml` runs WebKit (closest to Safari on
macOS) and Firefox on a macOS runner every night and on demand.

The system behind the portal (the TV stream over `ws://` and `wss://`, a fake
WLED on 127.0.0.1, settings files across restarts) is tested without a
browser in [`hosts/headless/tests/system.rs`](../../hosts/headless/tests/system.rs).

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
