# Portal API and settings

The web portal is a surface over the engine: every API call is one engine
command or watch. The complete contract (routes, bodies, limits, status codes)
is [HTTP-CONTRACT.md](../../crates/surfaces/web-portal/HTTP-CONTRACT.md); generated
types live in `@prysm/contracts/application` ([contracts](contracts.md)).

## Routes at a glance

| Route | Purpose |
| --- | --- |
| `GET /api/v1/state`, `GET /api/v1/state/stream` | The whole state, once or as server-sent events after every change |
| `PUT /api/v1/target` | `off`, `screen_sync` or `effects` (the output's own effects) |
| `PUT /api/v1/settings` | Replace the whole settings document from an expected revision |
| `PUT /api/v1/processing/trial` | Light with a processing section without saving it, while a slider is dragged |
| `GET /api/v1/preview/stream` | Picture, sampled area and edge colours, at most 8 per second |
| `GET /api/v1/colors/stream` | A few LED colours per edge, for tinting Home |
| `PUT`, `DELETE /api/v1/calibration` | Show or clear a calibration pattern on the TV |
| `GET /api/v1/candidates/{section}/{kind}` | What an adapter can find: video devices, WLED lights |
| `POST /api/v1/capture/modes` | What a capture section's hardware delivers: sizes and rates, or unknown |
| `GET /metrics` | Prometheus text, only with `--metrics` |

Commands answer once applied (`204`), not with an operation ID. A client that
loses a reply reads the state instead of resubmitting.

## Access

There is no login or token. Anyone who can reach the port can read the state
and control the lights. The portal refuses requests from web pages of other
sites by their `Origin` header, unless the host was started with
`--allow-origin ORIGIN` (`null` for the slim TV app, `*` for any). The
portal's own pages are those whose `Origin` is `http://` or `https://` plus
the request's `Host`, so a reverse proxy that serves the portal over HTTPS
and forwards `Host` needs no option; one that rewrites `Host` needs
`--allow-origin https://prysm.example.com`. `Host` is not checked, so DNS
rebinding is not prevented. The full TV worker allows `null`, the origin of
its `file://` TV app. Allowing `null` stops ordinary pages of other sites,
but not a deliberate attacker: any site can put its script in a sandboxed
frame, whose origin is `null` too. Keep the portal on a trusted LAN, or bind
it to `127.0.0.1` with `--listen`.

Candidates and capture modes are lookups the portal runs off the engine's
mailbox, each once at a time: who asks the same while it runs gets its
answer, so no client can stack up blocking lookups. The
portal waits for every command without a deadline of its own, since each
ends (see [architecture](architecture.md#shutdown-ordering)).

## Settings ownership

- The settings file holds everything a user changes at run time: `capture`,
  `output`, `processing` (with the style and calibrations). Format:
  [settings-file](../../crates/adapters/settings-file/README.md).
- Process options (settings path, `--listen`, `--allow-origin`, `--metrics`,
  `--start`, `--log-file`) are command-line only and never in the file. There
  are no `PRYSM_*` environment variables.
- The lighting mode is not saved. Hosts start with the lights off unless
  started with `--start`.
- Saves replace the whole document with the revision the client edited from;
  a stale revision is `409` and is never retried automatically. The browser
  copies the latest settings and replaces one section, so it keeps fields it
  does not show ([saving settings](../../frontend/README.md#saving-settings)).
- The file's `format` number belongs to the file alone: the portal's
  `settings` never carry it, and a `PUT /api/v1/settings` body with it is
  `422 invalid` with `field` `format` (an unknown setting).
- A problem inside `settings`, a `processing` body or a capture section is
  `422 invalid` with its `field`; `400 bad_request` is only for a body that
  is not JSON or a top-level field unknown, missing or of the wrong type.
- Settings sections and request bodies are JSON objects, never arrays. On
  input, `processing` fields and calibration controls may be omitted and take
  their defaults, and a custom style lists all seven values; the generated
  schema describes the response shape.

See the [settings ownership decision](decisions/2026-11-03-settings-ownership.md).

## Lighting transitions

- **Off:** stop the session; the output is idled off after the last frame.
- **WLED effects** (`effects`): stop the session; the output is idled
  to the device's own state.
- **Screen sync** (`screen_sync`): open the source (a failure is the reply),
  then open the output at the first frame. An output that cannot open ends
  the session and shows in `session.failure`.

The session reached the target when `session.state` equals it (`stopped` for
`off`). A failure lives in the session it explains (`stopped` or `effects`;
`screen_sync` has none), and a failure of what a command replaced is only
logged. While stopped with an output failure (the lights may not be off,
whichever target failed), `off` asked switches them off again; with the
target `off`, a save that does nothing about the output keeps the failure,
and saving other lights ends it.

There is no confirmation window and no rollback to the previous mode after a
failed start. A failure leaves the target as requested. Without frames for 3 s
the output returns to the device's own state until frames return; WLED
problems while running (switched off, live override, another sender, no
answer) show as `device.issue`, and Prysm does not switch WLED back.
`device.brightness_percent` (`40`) is WLED's own brightness while it scales
Prysm's colours; Prysm reports it and never changes it. `device.led_count`
(`120`) is how many LEDs WLED itself says it has; Prysm keeps sending the
configured edges' total, and the portal warns while the two differ.

On SIGINT or SIGTERM the host stops the session (output idled off), lets open
requests finish for up to 5 s, and exits non-zero if shutdown failed. A
second SIGINT or SIGTERM meanwhile exits at once with code 130.
