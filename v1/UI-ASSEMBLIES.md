# UI and host assemblies

The webOS application always has a TV UI. The embedded-processing composition
also serves the browser portal. The capture-only TV composition has no local
engine, output or portal.

On the TV, Luna belongs to the supervisor (`prysm-webos-service`, Luna
`io.prysm.tv.service`, both packages), not to the workers: it owns power and
video, the TV preferences and the worker process, and the TV UI calls it for
state, preferences and enable/disable ([Luna API](../docs/plans/prysm-v1-webos-supervisor.md#6-luna-api),
generated types `@prysm/contracts/luna`). The workers talk only to the
supervisor over stdio and have no Luna surface.

| Composition | TV UI talks to | Browser portal | Processing |
| --- | --- | --- | --- |
| `prysm-headless`, `prysm-tui` | — (`prysm-tui` adds the terminal surface) | `WebPortal` surface | Local engine |
| `prysm-webos` worker | supervisor over Luna; its own portal over HTTP for settings | `WebPortal` surface | Local engine |
| `prysm-webos-slim` worker (a `Forwarder`) | supervisor over Luna; the remote headless portal over HTTP | None on the TV | Remote engine |

On the Rust side every UI is a runtime surface over one `EngineHandle`; see the
[simplified runtime](../docs/plans/prysm-v1-simplified-runtime.md) and the
[portal HTTP contract](surfaces/web-portal/HTTP-CONTRACT.md). The TV UI's Luna
client does not exist yet, so the TV entry points run on in-memory fixtures.

## Frontend wiring

- [Desktop browser host](frontend/hosts/headless/web-portal/app.tsx), shared by
  `prysm-headless` and `prysm-tui`: shared portal,
  a [capture selector](frontend/hosts/headless/web-portal/capture-settings.tsx) for
  the `device` and `remote` kinds, and the WLED panel.
- [webOS browser host](frontend/hosts/webos/web-portal/app.tsx): shared portal,
  webOS capture and WLED panels, typed by `@prysm/contracts/host-webos`.
- [Embedded TV](frontend/hosts/webos/tv-ui/app.tsx) and
  [capture-only TV](frontend/hosts/webos-slim/tv-ui/app.tsx): the shared
  [TV UI](frontend/packages/tv-ui/index.tsx) with an injected application
  client and [TV platform client](frontend/packages/webos-platform/index.ts)
  (still a provisional local shape; the Luna client will replace it with the
  generated `luna` types).

```mermaid
flowchart LR
    Browser[Browser portal UI] --> HTTP[HTTP client] --> Portal[WebPortal surface]
    TV[TV UI, both packages] --> Luna[Luna client, not built yet] --> Supervisor[prysm-webos-service]
    Supervisor -->|stdio| Worker[prysm-webos or prysm-webos-slim]
    TV -->|HTTP client: own portal| Portal
    Portal --> Engine
    TV -.->|slim: HTTP client with baseUrl, not wired yet| Remote[Remote WebPortal]
```

[application-client](frontend/packages/application-client/index.ts) is the
transport-independent port: `observe` (a stream of whole states), `setTarget`
and `replaceSettings(expected, settings)`, each resolving with the engine's own
result or rejecting with a typed `Failure`. The
[HTTP client](frontend/packages/application-client/http.ts) implements it:
`httpApplicationClient()` talks to the page's own portal, and
`httpApplicationClient({ baseUrl: 'http://192.168.1.20:8787' })` to a remote one
that allows the page's origin (`--allow-origin`; a `file://` app needs `'*'`, see
[portal access](surfaces/web-portal/HTTP-CONTRACT.md#access)). The portal needs
no credentials.
[application-state](frontend/STATE.md) holds the latest state and command
feedback in one RTK store per frontend.

Settings panels receive their section (or `null`), a disabled flag and an
`onSubmit`. The command layer copies the observed settings, replaces only that
section and sends the observed revision, so concurrent edits conflict instead of
overwriting each other. UI teardown releases the state stream only; it never
turns lighting off.

## Build and inspect

```sh
npm --prefix v1/frontend ci
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
npm --prefix v1/frontend run dev:webos
```

The build creates `dist/headless/web-portal` (both desktop hosts), `dist/webos/web-portal`,
`dist/webos/tv-ui` and `dist/webos-slim/tv-ui`. Browser bundles also produce a
Rust `assets.rs` list for `WebPortal`. TV bundles are packaged with the app.

Not implemented: the TV UI's Luna client, wiring the slim TV UI to the HTTP client, TV
calibration (not in the runtime yet), preview, IPK packaging and validation on
a real webOS browser.
