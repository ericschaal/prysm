# Frontend development

Both frontends use React, TypeScript and Vite. Node.js 24+/npm is needed to build
them. The host panel is embedded in Prysm; the TV UI is packaged in the IPK.

## Host panel

From the repository root:

```sh
cargo xtask setup host
cargo run -p prysm -- --config settings.json --allowed-host localhost:8787,127.0.0.1:8787,localhost:5173
```

In a second terminal:

```sh
npm run dev --prefix crates/web/prysm-web/control-ui
```

Open `http://localhost:5173`. Vite proxies `/api/` to Prysm while preserving Host
and Origin for request checks. Complete browser setup using a disposable settings
file. Capture and WLED still use real hardware unless you choose a test fixture.

`prysm-web/build.rs` bundles the panel into Cargo's `OUT_DIR` in debug and release
builds. It does not install npm dependencies. Standalone UI builds write to `dist/`.

## API generation

Rust owns contracts: `prysm-api` exports OpenAPI from its router, with DTOs in
`prysm-contract`; `prysm-webos-service` exports Luna service schemas. Regenerate
after changing these contracts:

```sh
cargo xtask setup
cargo xtask codegen
cargo xtask check frontend
```

Use `codegen api` or `codegen webos` for one side; add `--check` to verify without
writing. Generation is explicit, never part of a Cargo build script.
The host schema includes preview WebSocket messages. The TV schema generates both
TypeScript types and runtime reply validators. Both generators use the host UI's
locked tooling, so even TV-only generation needs `setup host`.

HTTP uses `openapi-fetch` and generated types. WebSocket and Luna messages still
need runtime validation. Do not edit or format generated files manually.

## webOS TV app

```sh
cargo xtask setup webos
npm run dev --prefix crates/web/prysm-webos
```

Open `http://127.0.0.1:5174`. Without `PalmServiceBridge`, a development-only Luna
simulation provides settings and status. It resets on reload and never captures
video or contacts a TV.

Production emits a classic script targeting Chromium 53, with relative asset
URLs for packaged `file:` loading. This is a syntax target, not a guarantee that
a TV supports every UI API or capture driver. Avoid raising browser
requirements through UI dependencies. See the
[TV source guide](../../crates/web/prysm-webos/README.md) and
[IPK build](packaging.md#webos).

## Tests and formatting

```sh
npm test --prefix crates/web/prysm-web/control-ui
npm test --prefix crates/web/prysm-webos
npm run lint
npm run format:check
```

UI tests type-check, build and exercise DOM behavior; host tests also check the
development proxy. `cargo xtask check frontend` includes both generated contracts,
shared lint/format checks and TV packaging helpers. Run `cargo xtask setup` first.
Use `npm run lint:fix` or `npm run format` for fixes.

Root ESLint covers both UIs and tooling, including accessibility and React hooks.
Prettier owns formatting. After dependency changes, run `npm ci`, `npm audit` and
`cargo xtask check frontend`. The root `minimatch` override can be removed once
upstream lint dependencies no longer need it for GHSA-g3hv-75wg-m249.

For the TV/phone wizard and browser tests, see [calibration](calibration.md).
