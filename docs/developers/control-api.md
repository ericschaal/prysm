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
| `GET /api/v1/preview/stream` | Picture and sampled area, at most 8 per second |
| `GET /api/v1/colors/stream` | Up to 16 LED colours per edge, for Home's light and LED band |
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

- The settings file holds the adapter and picture configuration: `capture`,
  `output`, `processing` (with the style and calibrations). Format:
  [settings-file](../../crates/adapters/settings-file/README.md).
- Process options (settings path, `--listen`, `--allow-origin`, `--metrics`,
  `--start`, `--log-file`) are command-line only and never in the file. There
  are no `PRYSM_*` environment variables.
- The settings store persists the last selected mode beside its settings file.
  Startup reconciles that mode with the device observation. An explicit
  `--start` requests Screen sync.
- Saves replace the whole document with the revision the client edited from;
  a stale revision is `409` and is never retried automatically. The browser
  copies the latest settings and replaces one section, so it keeps fields it
  does not show ([saving settings](../../frontend/README.md#saving-settings)).
- The file's `format` number belongs to the file alone: the portal's
  `settings` never carry it, and a `PUT /api/v1/settings` body with it is
  `422 invalid` with `field` `format` (an unknown setting).
- A problem inside `settings`, a `processing` body or a capture section is
  `422 invalid` with its `field`; `400 bad_request` is only for a body that
  is not a JSON object or a top-level field unknown, missing or of the wrong
  type.
- Settings sections and request bodies are JSON objects, never arrays. On
  input, `processing` fields and calibration controls may be omitted and take
  their defaults, and a custom style lists all seven values; the generated
  schema describes the response shape.

See the [settings ownership decision](decisions/2026-11-03-settings-ownership.md).

## Lighting transitions

A target command updates intent. Observe the state stream for the result.
`control.remembered` is the mode chosen by Power on. With
`control.external` true, an existing stream was detected at startup and
neither mode is selected. `control.power` is the latest known physical
power, omitted when unknown. `control.busy` marks an in-flight power
operation. `control.waiting` names a required adapter awaiting recovery.

- Off cancels pending starts and attempts power-off once if needed.
- Effects requires the output. It powers the device if needed and delegates
  the display to it. A detected explicit override during Screen sync also
  selects Effects.
- Screen sync requires both adapters. Capture opens before output. Losing
  either stops both and attempts to turn the lights off. A recoverable
  request resumes after cleanup when both adapters are ready. A blocker or
  unsafe local release prevents further starts.

`device.brightness_percent` (`40`) is WLED's own brightness while it scales
Prysm's colours; Prysm reports it and never changes it. `device.led_count`
(`120`) is how many LEDs WLED itself says it has; Prysm keeps sending the
configured edges' total, and the portal warns while the two differ.

On SIGINT or SIGTERM the host stops the session (output idled off), lets open
requests finish for up to 5 s, and exits non-zero if shutdown failed. A
second SIGINT or SIGTERM meanwhile exits at once with code 130.
