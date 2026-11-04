# Control API and lifecycle

Prysm owns the embedded panel, settings, HTTP API and capture → processing → WLED
pipeline. The panel and API share one listener and access policy.

## Access policy

Host validation, CSRF checks, security headers and request limits cover the panel
and API. Cross-site writes are rejected using Fetch Metadata or Origin/Host;
CLI requests without browser metadata remain supported. GET still requires an
allowed Host. There is no login: use a trusted LAN or private VPN.

## Configuration and HTTP

One JSON document stores browser settings. `--config` or `PRYSM_CONFIG` selects
its path; its directory must allow atomic replacement and syncing. A missing file
opens setup with defaults. A malformed or unreadable file fails rather than being
reset. CLI/environment configure process options separately: bindings, allowed
hosts, metrics, logging and storage do not belong in saved settings.

| Route | Contract |
| --- | --- |
| `GET /api/settings` | `{revision, settings, missing_required}` |
| `PUT /api/settings` | Complete `{expected_revision, settings}`; stale revision returns 409; invalid or unknown fields are rejected. |
| `GET /api/state` | Observed lights, pipeline and operation status. |
| `PUT /api/state` | `{power, source}`; accepted work returns an operation ID with 202. |

Input/output may be absent during setup. Complete settings enable capture but do
not start it or prove hardware availability. All startup remains idle.

The settings store validates and atomically persists before publishing a revision.
Failed writes preserve prior settings. Processing changes apply between frames;
input/output changes recreate a running session. Saved and processor-applied
revisions are distinct. The controller serializes accepted mutations and drains
them on shutdown, even if callers disconnect. Concurrent mutations conflict.
See [generated clients](frontend.md#api-generation) for schema updates.

## Browser preview and live settings

The setup wizard and Settings share fields and the same save operation. Settings
keeps one draft; conflicts preserve it for explicit review/reload. Preview observes
the picture without editing settings or starting capture. WLED is the only output;
there is no preview-only output type.

`GET /api/preview` upgrades to a same-origin WebSocket. Send `next` to receive a
paired frame: JPEG/base64, source/preview dimensions, source-pixel crop, sRGB edge
strips, frame ID and applied revision. Other replies are idle/busy/error.
Limits are four viewers, one outstanding request per viewer, up to 8 fps, and one
global encoder with one active and one queued job. Hidden views stop requests;
stopped capture retains the last image marked stale. See [preview ownership](architecture.md#preview-observation).

## Lighting transitions

- **Off:** stop and join capture/output, then send WLED Off after the final DDP
  packet. Local capture stops even if WLED is offline.
- **WLED effects:** stop and join the pipeline before releasing realtime control.
- **Screen sync:** enable realtime and start capture. Success needs processed
  frames and observed WLED live state; failure restores the previous state where possible.

Selecting a source while Off does not turn on lights. Ctrl+C/SIGTERM drains
accepted work and the pipeline. Capture failure keeps HTTP available.

The webOS receiver stays available while lighting is off. Screen sync sends
settings to a connected TV worker; Off/effects ends its capture session. The
supervisor starts a fresh idle worker afterward. The panel controls capture,
not TV processes. Wake the TV first: startup must be confirmed within 12 seconds
or it rolls back. See [TV lifecycle](webos.md).

After a Prysm restart, an unobserved operation result is unknown. The panel releases
pending controls and keeps the requested target for an explicit retry.
