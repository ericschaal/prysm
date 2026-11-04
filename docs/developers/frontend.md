# Frontend development

Requires Node.js 24+/npm. Host panel: React/TypeScript/Vite embedded by
`prysm-web/build.rs`. TV UI: separate bundle in the IPK.

## Host panel

```sh
cargo xtask setup host
cargo run -p prysm -- --config settings.json --allowed-host localhost:8787,127.0.0.1:8787,localhost:5173
```

Second terminal:

```sh
npm run dev --prefix crates/web/prysm-web/control-ui
```

Open `http://localhost:5173`. Vite proxies `/api/`, preserving Host/Origin.
Use disposable settings; capture/WLED remain real unless using a fixture.

## API generation

Rust owns HTTP/Luna types; generate schemas, TypeScript and TV runtime validators:

```sh
cargo xtask setup
cargo xtask codegen
cargo xtask check frontend
```

Use `codegen api`/`codegen webos` for one contract; `--check` verifies without writing.
Both need the host UI's tooling. Never hand-edit generated files. WebSocket/Luna
payloads still need runtime validation.

## webOS TV app

```sh
cargo xtask setup webos
npm run dev --prefix crates/web/prysm-webos
```

Open `http://127.0.0.1:5174`. The development Luna simulation resets on reload;
it never contacts TV hardware. Production targets Chromium 53 syntax with a classic
script for `file:` loading. See [TV source](../../crates/web/prysm-webos/README.md).

## Tests and formatting

`cargo xtask check frontend` checks generation, both UI test suites, ESLint,
Prettier and TV packaging helpers. Use `npm run lint:fix`/`npm run format` to fix.
[Calibration](calibration.md#local-checks) has the browser/demo commands.
