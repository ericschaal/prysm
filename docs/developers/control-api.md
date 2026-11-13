# Portal API and settings

The web portal is a surface over the engine: every API call is one engine
command or watch. The complete contract (routes, bodies, limits, status codes)
is [HTTP-CONTRACT.md](../../v1/surfaces/web-portal/HTTP-CONTRACT.md); generated
types live in `@prysm/contracts/application` ([contracts](../../v1/CONTRACTS.md)).

## Routes at a glance

| Route | Purpose |
| --- | --- |
| `GET /api/v1/state`, `GET /api/v1/state/stream` | The whole state, once or as server-sent events after every change |
| `PUT /api/v1/target` | `off`, `processed` (Screen sync) or `device_native` (the output's own effects) |
| `PUT /api/v1/settings` | Replace the whole settings document from an expected revision |
| `GET /api/v1/preview/stream` | Picture, sampled area and edge colours, at most 8 per second |
| `GET /api/v1/colors/stream` | A few LED colours per edge, for tinting Home |
| `PUT`, `DELETE /api/v1/calibration` | Show or clear a calibration pattern on the TV |
| `GET /api/v1/candidates/{section}/{kind}` | What an adapter can find: video devices, WLED lights |
| `GET /metrics` | Prometheus text, only with `--metrics` |

Commands answer once applied (`204`), not with an operation ID. A client that
loses a reply reads the state instead of resubmitting.

## Access

There is no login or token. Anyone who can reach the port can read the state
and control the lights. The portal refuses requests from web pages of other
sites by their `Origin` header, unless the host was started with
`--allow-origin ORIGIN` (`null` for the slim TV app, `*` for any). `Host` is
not checked, so DNS rebinding is not prevented. The full TV worker allows any
origin. Keep the portal on a trusted LAN, or bind it to `127.0.0.1` with
`--listen`.

## Settings ownership

- The settings file holds everything a user changes at run time: `capture`,
  `output`, `processing` (with colour profiles). Format:
  [settings-file](../../v1/adapters/settings-file/README.md).
- Process options (settings path, `--listen`, `--allow-origin`, `--metrics`,
  `--start`, `--log-file`) are command-line only and never in the file. There
  are no `PRYSM_*` environment variables.
- The lighting mode is not saved. Hosts start with the lights off unless
  started with `--start`.
- Saves replace the whole document with the revision the client edited from;
  a stale revision is `409` and is never retried automatically. The browser
  copies the latest settings and replaces one section, so it keeps fields it
  does not show ([saving settings](../../v1/frontend/README.md#saving-settings)).

See the [settings ownership decision](decisions/2026-11-03-settings-ownership.md).

## Lighting transitions

- **Off:** stop the session; the output is idled off after the last frame.
- **WLED effects** (`device_native`): stop the session; the output is idled
  to the device's own state.
- **Screen sync** (`processed`): open the source (a failure is the reply), then
  open the output at the first frame. An output that cannot open ends the
  session and shows in `failure`.

There is no confirmation window and no rollback to the previous mode after a
failed start. A failure leaves the target as requested. Without frames for 3 s
the output returns to the device's own state until frames return; WLED
problems while running (switched off, live override, another sender, no
answer) show as `output_issue`, and Prysm does not switch WLED back.

On SIGINT or SIGTERM the host stops the session (output idled off), lets open
requests finish for up to 5 s, and exits non-zero if shutdown failed.
