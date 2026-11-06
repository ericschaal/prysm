# Frontend composition proof

An isolated npm workspace with its own lockfile. The Rust workspace and existing
frontends keep their build systems. Node.js 24+ and Rust are required.

```sh
# From the repository root:
npm --prefix v1/frontend ci
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
cargo test --manifest-path v1/Cargo.toml --workspace --all-targets --locked --offline
```

`npm test` regenerates contracts, type-checks the workspace, builds the four frontend entry points and
the browser bundles' Rust asset lists, then runs schema/drift and DOM interaction tests.
`npm run build` regenerates and builds without running tests. CI checks drift
before the build so regeneration cannot hide uncommitted contract changes.
Run the frontend build before compiling the Rust host examples: they use
`include!` to embed generated assets, so a missing build fails compilation.
Cargo never launches npm. Headless Rust libraries do not require frontend assets.

For interactive inspection:

```sh
npm --prefix v1/frontend run dev:server
# Or, separately:
npm --prefix v1/frontend run dev:webos-portal
npm --prefix v1/frontend run dev:webos
npm --prefix v1/frontend run dev:webos-slim
```

All pages visibly identify themselves as in-memory demos. They connect to no
hardware, network service or persisted settings.

Review the [UI/host assembly guide](../UI-ASSEMBLIES.md) for the full topology.
The embedded webOS assembly includes both the TV UI and browser portal. Frontend
state uses Redux Toolkit; RTK Query remains deferred.

## Read the composition

- [Server browser surface](hosts/server/web-portal/app.tsx): shared portal + host-owned device/remote capture selection + WLED panels.
- [webOS browser surface](hosts/webos/web-portal/app.tsx): shared portal + webOS capture + WLED panels.
- [Shared portal](packages/web-portal/index.tsx): typed browser panels and command feedback.
- [TV UI](packages/tv-ui/index.tsx), [embedded TV assembly](hosts/webos/tv-ui/app.tsx),
  and [remote TV assembly](hosts/webos-slim/tv-ui/app.tsx): always-present TV surface.
- [Application client](packages/application-client/index.ts): transport-independent ports and generated payloads.
- [RTK state assembly](packages/application-state/index.ts) and
  [React bindings](packages/application-react/index.tsx): observation state, typed
  commands and hooks, separate from either UI. [State wiring](STATE.md) shows the
  slice, listener, command and host boundaries. [Platform/presenter ports](packages/webos-platform/index.ts)
  handle television-specific responsibilities.
- [Video device capture](packages/capture-device-ui/index.tsx),
  [remote capture](packages/remote-capture-ui/index.tsx),
  [webOS capture](packages/capture-webos-ui/index.tsx), and
  [WLED output](packages/output-wled-ui/index.tsx): independent companion packages.
- [Application fixture](testkit/application.ts): revision checks and in-memory
  snapshot updates, explicitly outside the portal package.
- [Build](scripts/build.mjs): produces two browser bundles with Rust asset lists,
  plus separate `dist/webos/tv-ui` and `dist/webos-slim/tv-ui` application bundles.

The shared portal imports no adapters. Hosts import real components instead of
selecting an enum or schema identifier. Each panel receives its current settings,
a disabled state and a submission callback. Settings are nullable to support
initial setup. The panel constructs its typed settings; the portal calls the
application client with the observed revision and displays acceptance/errors.
Acceptance is distinct from application completion.

The frontend client is a UI-facing facade, **not a defined wire protocol**. A real
transport must translate its methods into application commands, preserving common
processing/calibration settings, use revision checks, validate responses and
implement snapshot resynchronization. Backend adapter validation is authoritative.
Adapter settings types are generated from Rust; panels and hosts import them
from `@prysm/contracts/device`, `/remote-capture`, `/webos` and `/wled`. Client requests, private presentation messages, operation outcomes and the current
snapshot projection are also generated from Rust. Complete application state/events
and endpoint mappings remain outside this milestone.

## Rust is the settings source of truth

[Contract ownership and generation](../CONTRACTS.md) is the review guide.
Adapter settings now live in independent Rust crates: [device](../adapters/capture-device/src/lib.rs),
[webOS](../adapters/capture-webos/src/lib.rs), and [WLED](../adapters/output-wled/src/lib.rs).
Each library owns a schema document and optional `wire`/`schema` features.
Shared public wire types are registered by [prysm-protocol](../crates/protocol/src/schema.rs).
Neither feature is required by headless application traits. The existing core
frame and resolution types stay unchanged; resolution decoding still validates
through the core constructor.

The [standalone exporter](../tools/export-contracts/src/main.rs) compiles without
frontend bundles, HTTP servers or native capture. The build sequence is:

```text
Rust settings → utoipa OpenAPI → openapi-typescript → frontend bundles → Rust hosts
```

[Host contract composition](../assemblies/src/contracts.rs) selects library
schemas and assigns module names. [Codegen](scripts/codegen.mjs) mechanically
translates these documents into independent OpenAPI and TypeScript files, deriving
all root-type exports from the schemas. It has no adapter names or type mappings.
The contracts package uses wildcard exports. Actual Rust-serialized regression
fixtures are exported separately into the same `packages/contracts/generated` directory. These files belong in source control;
never edit them or run a formatter over the generated TypeScript. The `.generated.ts`
suffix follows the repository's formatter/linter exclusions. Stable package subpaths
hide generator structure from panels; they do not depend on any frontend host.
Host entry points use the combined `host-server` or `host-webos` document.
The shared `application` module already demonstrates generation of non-settings
contracts. New public types are registered in their owning Rust library.

```sh
npm --prefix v1/frontend run codegen        # Refresh after Rust contract changes.
npm --prefix v1/frontend run codegen:check  # Compare without writing; fail on drift.
```

Build and dev commands regenerate automatically. The isolated
[v1 CI workflow](../../.github/workflows/v1.yml) checks committed artifacts first,
then runs the frontend and Rust checks. The exporter uses the locked Cargo workspace.

The device settings contract still contains the provisional `device`/`file` tagged
union, both carrying `path`; the runtime rejects `file` until its separate adapter
is implemented. Remote capture
owns a separate `{ peer }` connection contract, without resolution/rate. The server
owns `{ adapter: 'device' | 'remote', settings: ... }` and composes the respective
panels in [its selector](hosts/server/web-portal/capture-settings.tsx). Selecting a
panel edits the form; only submitting it sends a command. Runtime adapter switching
and configuration of the producing source over the network remain deferred. Resolutions serialize as `[width, height]`. WLED LED counts use four `u32`
values, giving a stable range across host architectures and exact JavaScript
representation. TypeScript records shapes; Rust remains responsible for semantic
validation, including positive rates, valid IP/socket addresses and resolution
bounds. The generated schema tests do not claim to replace these validators.

## What is verified

- Both hosts compile and bundle with their selected panels and generated settings types.
- Rust roundtrips all source variants and rejects invalid settings. Schema checks
  validate actual Rust serialization and reject incorrect tags, bounds and tuple lengths.
- Drift tests cover missing, modified and obsolete files and confirm check mode does not write.
- Negative type tests reject an incompatible capture panel and an output panel
  with the wrong settings type. `NoInfer` makes the application client determine
  settings types, rather than letting an incompatible panel widen them.
- DOM tests mount both hosts, check the selected panels, edit and submit settings,
  check revisions/snapshot updates and preservation of the other adapter settings,
  and display submission errors. Fixture tests check stale revisions and unsubscribe.
- Build assertions check HTML asset references. Rust host compilation checks that
  every generated asset can be embedded without knowing any adapter panel names.

## Deliberately incomplete

These panels expose only enough fields to prove composition. Backend validation,
the full processing/calibration/status UI, production operation retention, HTTP,
Luna, native playback and preview remain deferred. The fixture updates snapshots immediately; it does not simulate
coordinator scheduling or hardware completion. Forms reset when their own saved
settings change; conflict-aware draft preservation is deferred to the real editor.

The builds target ES2020 as a development proof. Actual TV browser compatibility,
polyfills, remote navigation and accessibility beyond these basic labeled forms
still need validation on the target webOS version. Rust portal execution remains
an explicit `Unavailable` stub; embedding assets does not start an HTTP server.
