# Prysm v1 frontend

An isolated npm workspace with its own lockfile. Node.js 24+ and Rust are
required; codegen runs the Rust contract exporter.

```sh
# From the repository root:
npm --prefix v1/frontend ci
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
```

`npm test` regenerates contracts, type-checks, builds the four entry points
(and the browser bundles' Rust asset lists), then runs the schema/drift and DOM
tests. `npm run build` does the same without tests. Build the frontend before
compiling a Rust host that `include!`s the asset list; Cargo never launches npm.

```sh
npm --prefix v1/frontend run dev:headless      # desktop portal, no API behind Vite
npm --prefix v1/frontend run dev:webos-portal
npm --prefix v1/frontend run dev:webos         # TV UI on a fixture
npm --prefix v1/frontend run dev:webos-slim
```

The browser portals connect at once to the same-origin
[portal API](../surfaces/web-portal/HTTP-CONTRACT.md), which needs no
credentials. The TV entry points run on
in-memory fixtures until the Luna client for the TV supervisor exists. See the
[UI/host assembly guide](../UI-ASSEMBLIES.md) and [state wiring](STATE.md).

## Read the composition

- [Shared portal](packages/web-portal/index.tsx): lighting controls, state,
  failures and the host's settings panels. It imports no adapters.
- [Desktop host](hosts/headless/web-portal/app.tsx) (served by `prysm-headless` and `prysm-tui`) and
  [webOS browser host](hosts/webos/web-portal/app.tsx) choose the panels.
- Panels: [video device](packages/capture-device-ui/index.tsx),
  [remote capture](packages/capture-remote-ui/index.tsx),
  [webOS capture](packages/capture-webos-ui/index.tsx),
  [WLED](packages/output-wled-ui/index.tsx). Each receives its section
  (`{ kind, ... }` or `null`), a disabled flag and `onSubmit`.
- [TV UI](packages/tv-ui/index.tsx) with the
  [TV platform port](packages/webos-platform/index.ts).
- [Application client](packages/application-client/index.ts),
  [HTTP transport](packages/application-client/http.ts),
  [RTK state](packages/application-state/index.ts) and
  [React bindings](packages/application-react/index.tsx).
- [Application fixture](testkit/application.ts): revision checks, immediate
  state updates and scripted failures, outside the shipped packages.

## Contracts

[Contract ownership](../CONTRACTS.md) is the review guide. The exporter prints
the `application` document (state, session, failures, requests, settings), one
`host-*` document per set of hosts with their adapter sections typed
(`host-desktop`, `host-webos`), the TV supervisor's `luna` document, and Rust
serialization fixtures; [codegen](scripts/codegen.mjs)
turns each document into `packages/contracts/generated/<name>.generated.ts`.
Generated files are checked in; never edit them.

```sh
npm --prefix v1/frontend run codegen        # refresh after Rust contract changes
npm --prefix v1/frontend run codegen:check  # fail on drift without writing
```

Panels use their adapter's generated section type from
`@prysm/contracts/host-desktop` (`DeviceSettings`, `RemoteReceiverSettings`,
`WledSettings`) or `@prysm/contracts/host-webos` (`WebosSettings`). Rust
remains the validator.

## Limits

Panels edit only enough fields to prove composition; processing controls,
preview and calibration are not in the UI. The bundles target ES2020; real webOS
browser compatibility and remote navigation still need validation.
