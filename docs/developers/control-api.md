# Control API

## Access policy

Panel/API share Host validation, CSRF checks, security headers and request limits.
Cross-site writes fail; CLI requests without browser metadata work. There is no
login: use a trusted LAN/private VPN.

## Configuration and HTTP

| Route | Contract |
| --- | --- |
| `GET /api/settings` | `{revision, settings, missing_required}` |
| `PUT /api/settings` | Complete `{expected_revision, settings}`; stale revision → 409; reject invalid/unknown fields. |
| `GET /api/state` | Observed lights, pipeline and operation status. |
| `PUT /api/state` | `{power, source}` → 202 with operation ID. |

One controller serializes mutations, including those whose callers disconnect.
Settings persist atomically before publication; failed writes retain prior state.
Saved and processor-applied revisions differ. Processing changes apply live;
input/output changes restart capture. CLI/environment process options stay outside
saved JSON. Missing settings open setup; malformed files fail. Startup stays idle.

## Browser preview and live settings

Setup/Settings share the same save API. Conflicts preserve the draft for review.
`/api/preview` is a same-origin WebSocket: send `next`, receive a paired frame
(JPEG/base64, dimensions, source-pixel crop, sRGB edge strips, frame ID, revision)
or idle/busy/error. Limits: four viewers, one request each, 8 fps; one encoder with
one active/one queued job. Hidden views stop requests; stopped capture retains a
stale image. Preview never starts capture. WLED is the only output type.

## Lighting transitions

- **Off:** stop/join capture and output, then send Off after the final DDP packet.
- **WLED effects:** stop/join before releasing realtime control.
- **Screen sync:** enable realtime and capture; confirm frames plus WLED live state
  within 12 seconds, otherwise compensate to the previous state where possible.

Selecting a source while Off keeps lights off. WLED failure must not prevent local
capture stopping. Ctrl+C/SIGTERM drains accepted work. A restart makes unfinished
operation results unknown; retries remain explicit.

The webOS receiver remains available while lighting is off. Screen sync configures
the webOS capturer; Off/effects ends its session. Wake the TV before starting.
See [TV lifecycle](webos.md) and [generated clients](frontend.md#api-generation).
