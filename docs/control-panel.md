# Controller

`prysm` owns configuration, the REST API, and capture → processing → rendering.
`prysm-web` serves the React controller and forwards requests to Prysm. They run
independently and can live on separate machines. The controller never reads or
writes Prysm's TOML file or starts a pipeline in its own process.

## Run locally

Edit `prysm.toml` for your device and WLED addresses. From the repository root:

```sh
cargo run -p prysm -- --config prysm.toml
```

In another terminal:

```sh
cd apps/prysm-web/control-ui
npm ci
npm run dev
```

Open http://localhost:5173. Vite refreshes React and proxies `/api/` to port 8787.
For production, run `cargo run -p prysm-web --
--backend http://127.0.0.1:8787`. The Rust build installs npm dependencies and builds/embeds the UI; Node.js 24
and npm are required. Open http://localhost:8788.
Use `--listen` and `--allowed-host` to expose the controller on your LAN.
Prysm must allow the Host from the backend URL; the controller validates browser
Host and uses Tower’s CSRF middleware before forwarding. Cross-site writes are
rejected using `Sec-Fetch-Site` or an Origin/Host comparison; requests without
browser metadata remain supported for CLI clients. Safe methods such as GET are
permitted after Host validation. The JSON proxy forwards only content-type,
uses a 64 KiB request limit and a 65-second timeout, and does not follow redirects.
Tower also applies the controller’s security and cache headers to error responses.
Startup observes existing lights and never restores saved On intent.

## Configuration and HTTP

The backend owns one TOML file, specified by `--config`. Configuration saved in
the UI replaces this file atomically; the process needs write access to its parent
folder. No separate preferences file exists. All settings have one shared Rust
model for disk and HTTP. Processing settings update through actor messages;
input, capture resolution, and WLED output changes recreate a running session.
Changing output kind or the egui refresh rate stops capture and requires
a restart. Processing changes apply live for either renderer. Listen-address and allowed-host changes are saved but require an application
restart. The current API keeps its original access policy until then. Settings
that require unavailable build features are rejected before they are saved.

- `GET /api/config` returns the configuration and revision.
- `PUT /api/config` accepts `{expected_revision, config}` and rejects stale writes
  with 409. Persisted settings are applied as an operation visible in status.
- `GET /api/state` returns observed lighting, pipeline status, and operation status.
- `PUT /api/state` accepts `{power, source}` and returns an operation ID with 202.

`cargo run -p prysm-api --bin export-openapi` exports OpenAPI from the actual
`prysm-api` router. openapi-typescript and openapi-fetch provide the TypeScript
client. See [frontend tooling](../apps/prysm-web/control-ui/README.md).

## egui output

With `prysm --features desktop` and `[output] type = "egui"`, Prysm renders to a
native window. Device, file, and webOS input all work with either egui or WLED.
No `[wled]` HTTP endpoint is required for rendering. Without WLED HTTP control,
Start output and Stop output control the pipeline; WLED effects are unavailable.
Startup is idle: click **Start capture** in the native toolbar to begin capture or playback,
and **Stop capture** to stop it, or press **Space** to toggle. Status and errors appear beside the button; at EOF, **Start capture**
replays the file. These controls call `PUT /api/state` with
`{"power":true,"source":"prysm"}` through the typed `prysm-api` client and poll
`GET /api/state` every 250 ms. Requests use `prysm-contract` types and run off the
UI thread, with a three-second timeout and automatic state polling recovery.
At file EOF the last frame stays visible and the REST API remains available;
Start capture replays it. Closing the window stops Prysm but leaves `prysm-web`
running. The same pipeline controls work with DDP output without HTTP management.

## Lighting transitions

Off stops and joins capture even if WLED is offline. It reapplies the WLED Off
command after the final DDP packet. Effects stops and joins the pipeline before
releasing realtime control. Screen sync enables WLED realtime and starts the
actor pipeline; success requires both processed frames and observed WLED live
state. A failed start restores the previous lighting state where possible.
Selecting a source while already Off does not turn on the lights.

Accepted operations continue if a browser disconnects. Concurrent mutations are
rejected while an operation is pending. Capture failures remain visible without
stopping HTTP. Ctrl+C or SIGTERM drains the pipeline before application exit.

The panel has no login. Restrict allowed hosts and use a trusted LAN/private VPN.
See [actor runtime](actor-runtime.md) and [Debian deployment](../apps/prysm-web/debian/README.md).
