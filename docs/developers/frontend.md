# Frontend development

React + TypeScript, bundled with Vite. Node 24 is required for building; production runs in the single `prysm` executable.

Create `prysm-local.toml` from a [user setup guide](../users/README.md).
From the repository root, start the Rust application (HTTP plus a processing worker on its own thread):

```sh
cargo run -p prysm --no-default-features -- --config prysm-local.toml
```

In a second terminal:

```sh
cd crates/web/prysm-web/control-ui
npm ci
npm run dev
```

Open http://localhost:5173. Vite proxies `/api/` to `http://127.0.0.1:8787`, preserving the browser Host and Origin for the backend's request checks. Configure the corresponding listen address and allowed hosts in your TOML, along with your WLED address and capture input.

## API generation

Rust owns the HTTP handlers and their router in `prysm-api`, with shared DTOs in
`prysm-contract`. The `prysm-api` binary `export-openapi` prints the actual
router’s OpenAPI document. From this frontend directory, regenerate the
checked-in schema and client after changing DTOs or routes:

```sh
npm run generate-api
npm test
```

Generation runs `cargo run --locked -p prysm-api --bin export-openapi`, captures
its output in `openapi.json`, then generates `api.generated.ts`. `check-api`
checks both files against Rust without modifying them. Generation builds only the
API crate and its dependencies. It stays separate from `npm run build` so the web
application’s build script never invokes Cargo recursively. Building `prysm` also builds its `prysm-web` library dependency and requires Node.js 24 and npm.

## Build and tests

`npm run build` type-checks the web UI and writes production HTML, JavaScript
and CSS to `dist/`. When building `prysm-web`, its `build.rs` runs `npm ci` and
`npm run build` automatically when frontend inputs change. Node.js 24 and npm are
required for workspace builds. Both debug and release builds embed the assets.

`npm test` checks generated client drift, type-checks, builds, then runs DOM behavior and development-proxy tests. HTTP calls use `openapi-fetch` and the generated schema to check paths, methods, requests and responses. The UI loads coherent configuration and status revisions, and saves the complete configuration with its expected revision so concurrent changes cannot overwrite one another.

## webOS TV app

The [TV app](../../crates/web/prysm-webos/README.md) is a separate React/TypeScript/Vite
frontend in `crates/web/prysm-webos`. Run `npm ci` and `npm run dev` there, then
open http://127.0.0.1:5174. A development-only Luna simulation lets you work on the
UI without a TV or the native service; Vite provides React Fast Refresh. Production
builds target Chromium 53 and emit a classic script for the packaged TV app.
The [IPK build](packaging.md#webos) stages the generated `dist/` assets.
