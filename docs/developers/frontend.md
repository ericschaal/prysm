# Frontend development

`v1/frontend` is one npm workspace (Node.js 24+) with four bundles: the
desktop portal (served by `prysm-headless` and `prysm-tui`), the TV's own
portal (full package), and the TV app for each package. The Rust hosts embed
the browser bundles at compile time; the TV bundles go into the IPKs.
Reference: [frontend README](../../v1/frontend/README.md) (screens, saving
rules, limits) and [UI and host assemblies](../../v1/UI-ASSEMBLIES.md).

## Portal with hot reload

Start a host, then the dev server in a second terminal:

```sh
npm --prefix v1/frontend ci
npm --prefix v1/frontend run build
cargo run --manifest-path v1/Cargo.toml -p prysm-tui -- v1/hosts/tui/settings.example.json
```

```sh
npm --prefix v1/frontend run dev:headless
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
npm --prefix v1/frontend run codegen        # after a Rust contract change
npm --prefix v1/frontend run codegen:check  # fail on drift without writing
```

Codegen runs Rust, so it needs a Rust toolchain (and `libclang` on Linux).
Details: [contract ownership](../../v1/CONTRACTS.md).

## webOS TV app

```sh
npm --prefix v1/frontend run dev:webos        # full package's TV app
npm --prefix v1/frontend run dev:webos-slim   # slim package's TV app
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
npm --prefix v1/frontend test           # codegen, typecheck, build, contract and DOM tests
npm --prefix v1/frontend run lint
npm --prefix v1/frontend run format:check
npm --prefix v1/frontend run format     # rewrite with Prettier
```

The DOM tests use jsdom. Mobile browsers (iOS Safari, Firefox) have not been
checked against the portal.

## End-to-end tests

`v1/frontend/e2e/` drives the portal in Chromium and WebKit with
[Playwright](https://playwright.dev/). Each test starts its own
`prysm-headless` on an ephemeral loopback port with a settings file in a
temporary directory (remote capture, `null` output), and may start test
senders: `prysm-capture-remote`'s `test_sender` example stands in for the
TV's slim worker, streaming generated frames, refusing its first connections
(`--refuse N REASON`) or stopping after some frames (`--stop-after N`).
Nothing leaves loopback.

```sh
cargo build --manifest-path v1/Cargo.toml -p prysm-headless
cargo build --manifest-path v1/Cargo.toml -p prysm-capture-remote --example test_sender
npm --prefix v1/frontend run e2e:install   # Chromium and WebKit, once per Playwright version
npm --prefix v1/frontend run e2e           # add -- --project webkit, -- --headed, -- -g brightness
```

The binary embeds the portal bundle at compile time: rebuild the frontend,
then `prysm-headless`, before testing a frontend change. The tests take the
debug builds from `$CARGO_TARGET_DIR` (default `v1/target`), or
`PRYSM_HEADLESS_BIN` and `PRYSM_TEST_SENDER_BIN`. Results, traces and videos
go to `v1/frontend/.e2e/`. Find elements as a user does, by role and
accessible name, not by class names.

Playwright's WebKit is not Safari, but it shares Safari's engine bugs: the
brightness drag fails in it when the slider captures the pointer
([WebKit bug 219636](https://bugs.webkit.org/show_bug.cgi?id=219636)), which
Chromium never showed. Touch drags and iOS are not covered; check them on a
phone.

CI (`e2e` in `.github/workflows/v1.yml`) runs the suite on Linux with the
binaries built by the `rust` job, one retry per test and traces on retry; the
report, traces and videos are uploaded when it fails.
