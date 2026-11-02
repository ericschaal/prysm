# Control API and lifecycle

`prysm` owns configuration, the embedded React web UI, the REST API, and
capture → processing → rendering. UI and API share `[web].listen` and
`[web].allowed_hosts`. The `prysm-web` library supplies embedded assets; there is
one application process and one configuration file.

## Access policy

The embedded UI and API share `[web].listen` and `[web].allowed_hosts`.
Host validation, CSRF protection, security headers and request limits cover both.
Cross-site writes are rejected using `Sec-Fetch-Site` or an Origin/Host comparison;
CLI requests without browser metadata remain supported. Safe methods such as GET
are permitted after Host validation. The panel has no login; use a trusted LAN/private VPN.
Startup observes existing lights and never restores saved On intent.

See [frontend setup](frontend.md) for Vite, or [user configuration](../users/configuration.md)
for operating the panel.

## Configuration and HTTP

The backend owns one TOML file, specified by `--config`. Configuration saved in
the UI replaces this file atomically; the process needs write access to its parent
folder. No separate preferences file exists. All settings have one shared Rust
model for disk and HTTP. Processing settings update through the processing worker command channel;
input, capture resolution, and WLED output changes recreate a running session.
Processing changes apply live without recreating capture/output. Listen-address and allowed-host changes are saved but require an application
restart. The UI and API keep their original access policy until then. Settings
that require unavailable build features are rejected before they are saved.
When `restart_required` is set, the panel explains that Prysm must restart and
disables power, source, and retry actions until the restart is complete.

- `GET /api/config` returns the configuration and revision.
- `PUT /api/config` accepts `{expected_revision, config}` and rejects stale writes
  with 409. Persisted settings are applied as an operation visible in status.
- `GET /api/state` returns observed lighting, pipeline status, and operation status.
- `PUT /api/state` accepts `{power, source}` and returns an operation ID with 202.

`cargo run -p prysm-api --bin export-openapi` exports OpenAPI from the actual
`prysm-api` router. openapi-typescript and openapi-fetch provide the TypeScript
client. See [frontend tooling](frontend.md).

## Browser preview and live settings

Overview controls confirmed WLED power/source and shows input/output and operation state.
Preview & tuning edits all six processing settings in one draft with Save & apply/Discard.
Conflicts preserve the draft and require explicit review/reload. Input/output/listener forms
are outside this UI; existing backend configuration capabilities remain available.

`GET /api/preview` upgrades to a same-origin request/response WebSocket. Send `next`;
receive one paired JSON frame (JPEG/base64, source/preview dimensions, source-pixel crop,
sRGB edge strips, frame ID, applied revision), or idle/busy/error. At most four viewers,
one outstanding request each, at most 8 fps. One global encoder has one active and one
queued job. Hidden/closed previews stop requests. The last image survives capture stop
and is labeled stale. No image work occurs without demand. See [runtime ownership](architecture.md).

Existing WLED configuration representation is unchanged. Legacy egui output is rejected
with a migration message requiring a real WLED URL and LED counts; the browser replaces
the native window. There is no preview-only production output.

## Lighting transitions

Off stops and joins capture even if WLED is offline. It reapplies the WLED Off
command after the final DDP packet. Effects stops and joins the pipeline before
releasing realtime control. Screen sync enables WLED realtime and starts the
pipeline; success requires both processed frames and observed WLED live
state. A failed start restores the previous lighting state where possible.
Selecting a source while already Off does not turn on the lights.

With the [webOS TV package](../../packaging/webos/README.md), the TV sender stays
available independently of capture. Prysm keeps its webOS listener open while
the pipeline is off. Screen sync sends settings to the connected TV; Off or
Effects ends the capture session and releases TV resources. A fresh worker then
reconnects idle.
The panel does not remotely start/stop TV processes. Wake the TV before enabling
Screen sync; a start that is not confirmed within 12 seconds fails and rolls back.

Accepted operations continue if a browser disconnects. Concurrent mutations are
rejected while an operation is pending. Capture failures remain visible without
stopping HTTP. Ctrl+C or SIGTERM drains the pipeline before application exit.

The panel has no login. Restrict allowed hosts and use a trusted LAN/private VPN.
See [pipeline runtime](architecture.md) and [Debian deployment](../../packaging/debian/README.md).

If Prysm restarts before the panel observes an operation's completion, the panel
releases its pending controls and reports that completion is unknown. Retry keeps
the original requested target; it does not automatically repeat the operation.
