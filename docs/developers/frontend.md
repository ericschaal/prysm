# Frontend development

React + TypeScript, bundled with Vite. Node 24 or newer is required for building; production runs in the single `prysm` executable.

Start with a writable directory and no `settings.json` file, then complete Settings
in the browser. No configuration template is required.
From the repository root, start the Rust application (HTTP plus a processing worker on its own thread):

```sh
cargo xtask setup host
cargo run -p prysm -- --config settings.json --allowed-host localhost:8787,127.0.0.1:8787,localhost:5173
```

In a second terminal:

```sh
cd crates/web/prysm-web/control-ui
npm ci
npm run dev
```

Open http://localhost:5173. Vite proxies `/api/` to `http://127.0.0.1:8787`, preserving the browser Host and Origin for the backend's request checks. The command above permits the Vite browser address. Set the capture input and
WLED connection in Settings. Deployment options belong in CLI/environment.

## API generation

Rust owns the HTTP handlers and their router in `prysm-api`, with shared DTOs in
`prysm-contract`. The `prysm-api` binary `export-openapi` prints the actual
router’s OpenAPI document. From this frontend directory, regenerate the
checked-in schema and client after changing DTOs or routes:

```sh
cargo xtask codegen api
npm test
```

Generation runs `cargo run --locked -p prysm-api --bin export-openapi`, captures
its output in `openapi.json`, then generates `api.generated.ts`. It includes the
preview WebSocket messages as component schemas alongside HTTP contracts.
`cargo xtask codegen api --check` verifies both committed files without modifying them.
`cargo xtask codegen` generates both host and TV service contracts; add `--check`
for verification. Generators run explicitly, never inside a Cargo build script.

## Build and tests

`npm run build` type-checks the web UI and writes production HTML, JavaScript
and CSS to `dist/`. Run `cargo xtask setup host` after cloning or changing npm dependencies.
When building `prysm-web`, its `build.rs` bundles the UI into Cargo's `OUT_DIR/dist`
and embeds that bundle in both debug and release builds. It never installs npm
dependencies or writes the standalone frontend's `dist/` directory.

`npm test` type-checks, builds, and runs DOM behavior and development-proxy tests.
`cargo xtask check frontend` additionally checks generated contracts for both UIs
and runs repository-wide ESLint/Prettier and TV packaging checks. The full `cargo xtask check` also checks Debian
packaging helpers and documentation. Run `cargo xtask setup` first.
HTTP calls use `openapi-fetch` and the generated schema to check paths, methods,
requests and responses. Runtime payload checks remain necessary for WebSocket and Luna messages.

## webOS TV app

The [TV app](../../crates/web/prysm-webos/README.md) is a separate React/TypeScript/Vite
frontend in `crates/web/prysm-webos`. Run `npm ci` and `npm run dev` there, then
open http://127.0.0.1:5174. A development-only Luna simulation lets you work on the
UI without a TV or the native service; Vite provides React Fast Refresh. Production
builds target Chromium 53 and emit a classic script for the packaged TV app.
The [IPK build](packaging.md#webos) stages the generated `dist/` assets.

TV service types are generated from the Rust service's serialized contracts into
`src/service.generated.ts`, with the schema in `service.schema.json`.
Use `cargo xtask codegen webos` after changing them, or add `--check` to verify.
The generator uses the host UI's locked `openapi-typescript` installation; run
`cargo xtask setup` before generating or checking both frontends.

## Linting and formatting

From the repository root, `cargo xtask setup` installs shared tooling and both UIs.
Use `npm ci` at the root to install only the shared tooling.

```sh
npm run lint
npm run lint:fix
npm run format
npm run format:check
```

One ESLint flat config covers both apps, their tests and build scripts, and the
webOS packaging JavaScript. It uses the recommended ESLint, type-aware
TypeScript, React, React Hooks, React Refresh, and JSX accessibility presets.
TypeScript checks props; runtime PropTypes are unnecessary. Prettier owns
formatting, with `eslint-config-prettier` disabling conflicting lint rules.
ESLint 10 uses `@eslint-react/eslint-plugin` (including its type-checked preset),
React's `eslint-plugin-react-hooks`, and the ESLint Community
[`eslint-plugin-jsx-a11y-x`](https://github.com/eslint-community/eslint-plugin-jsx-a11y-x)
accessibility fork. TypeScript ESLint and Fast Refresh also support ESLint 10.

The root package temporarily overrides `minimatch@^10` with `11.0.1` to fix
[GHSA-g3hv-75wg-m249](https://github.com/advisories/GHSA-g3hv-75wg-m249).
Remove this override once the lint dependencies require a patched version.
After dependency updates, run `npm ci`, `npm audit`, and
`cargo xtask check frontend` to verify the locked tree and lint behavior.

Generated contracts, build output, dependencies, and the local `work/` research
tree are excluded. Regenerate contracts with `cargo xtask codegen`; do not format
them manually. `cargo xtask check frontend` enforces lint and formatting in CI,
with warnings treated as failures.

For the TV/phone wizard, its same-origin relay and generated contracts, see
[Visual calibration](calibration.md).
