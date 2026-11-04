# Control API and lifecycle

`prysm` owns configuration, the embedded React web UI, the REST API, and
capture → processing → rendering. UI and API share the process HTTP listener and allowed-host policy. The `prysm-web` library supplies embedded assets; there is
one application process and one configuration file.

## Access policy

The embedded UI and API share the process HTTP listener and allowed-host policy.
Host validation, CSRF protection, security headers and request limits cover both.
Cross-site writes are rejected using `Sec-Fetch-Site` or an Origin/Host comparison;
CLI requests without browser metadata remain supported. Safe methods such as GET
are permitted after Host validation. The panel has no login; use a trusted LAN/private VPN.
Startup observes existing lights and never restores saved On intent.

See [frontend setup](frontend.md) for Vite, or [user configuration](../users/configuration.md)
for operating the panel.

## Configuration and HTTP

The backend owns one UI-saved JSON file selected by `--config` or `PRYSM_CONFIG`.
The directory must be writable for atomic replacement and directory syncing.
Missing files use application defaults with no input or output selected. Process
options (HTTP/metrics bindings, allowed hosts, logging and storage path)
come only from CLI/environment on every startup, never from the saved JSON.
A malformed or unreadable existing file is reported rather than reset.

`input` and `output` may be missing while the panel serves Settings. Required
values are reported through `missing_required`; capture is unavailable until
settings resolve to a complete validated configuration. The application stays
idle after setup. Runtime availability of capture hardware or WLED is distinct
from settings validity.

- `GET /api/settings` returns `{revision, settings, missing_required}`.
- `PUT /api/settings` accepts `{expected_revision, settings}` with a complete typed
  document. Stale revisions return 409; unknown fields and invalid values are rejected.
- `GET /api/state` returns observed lighting, pipeline status, and operation status.
- `PUT /api/state` accepts `{power, source}` and returns an operation ID with 202.

The settings store validates and atomically writes the document before changing
memory or revision. Failed writes retain the previous in-memory settings.
Processing changes apply live; input/output changes recreate a running session.
Process options cannot be changed through this API. The controller serializes
all accepted mutations, including persisted source selection, and drains them
on shutdown even if the HTTP caller disconnects.

`cargo run -p prysm-api --bin export-openapi` exports OpenAPI from the actual
`prysm-api` router. openapi-typescript and openapi-fetch provide the TypeScript
client. See [frontend tooling](frontend.md).

## Browser preview and live settings

Overview controls confirmed WLED power/source and shows input/output and operation state.
Preview inspects the picture without editing settings. Settings edits input,
output, source and processing in one draft with Save & apply/Discard.
Conflicts preserve the draft and require explicit review/reload. All saves use
the same validated API. The
initial setup flow uses these same fields and save operation; no separate setup
endpoint or completion marker exists.

`GET /api/preview` upgrades to a same-origin request/response WebSocket. Send `next`;
receive one paired JSON frame (JPEG/base64, source/preview dimensions, source-pixel crop,
sRGB edge strips, frame ID, applied revision), or idle/busy/error. At most four viewers,
one outstanding request each, at most 8 fps. One global encoder has one active and one
queued job. Hidden/closed previews stop requests. The last image survives capture stop
and is labeled stale. No image work occurs without demand. See [runtime ownership](architecture.md).

Configure a WLED output with its HTTP URL and LED counts. Unsupported output types
are rejected. There is no preview-only production output.

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
