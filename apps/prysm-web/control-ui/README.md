# Web UI

React + TypeScript, bundled with Vite. Node 24 is required for building; production needs `prysm` and the independent `prysm-web` executable.

From the repository root, start the Rust application (HTTP plus an actor pipeline on its own thread):

```sh
cargo run -p prysm --no-default-features -- --config prysm.toml
```

In a second terminal:

```sh
cd apps/prysm-web/control-ui
npm ci
npm run dev
```

Open http://localhost:5173. Vite proxies `/api/` to `http://127.0.0.1:8787`, preserving the browser Host and Origin for the backend's request checks. Configure the corresponding listen address and allowed hosts in `prysm.toml`, along with your WLED address and capture input.

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
application’s build script never invokes Cargo recursively. Building the engine
alone requires no Node or frontend assets.

`npm run build` type-checks the web UI and writes production HTML, JavaScript
and CSS to `dist/`. When building `prysm-web`, its `build.rs` runs `npm ci` and
`npm run build` automatically when frontend inputs change. Node.js 24 and npm are
required for workspace builds. Both debug and release builds embed the assets.

`npm test` checks generated client drift, type-checks, builds, then runs DOM behavior and development-proxy tests. HTTP calls use `openapi-fetch` and the generated schema to check paths, methods, requests and responses. The UI loads coherent configuration and status revisions, and saves the complete configuration with its expected revision so concurrent changes cannot overwrite one another.

For a standalone production web UI, run `cargo run -p prysm-web
-- --backend http://127.0.0.1:8787` from the repository root. Open http://localhost:8788. This process forwards API calls; Prysm alone
owns and persists configuration. Vite can also connect directly to Prysm during
development; allow its Host in Prysm's configuration.
