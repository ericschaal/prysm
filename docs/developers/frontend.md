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
