# Strict settings, tolerant wire messages

Date: 2026-11-22

Settings refuse what they do not know; messages between separately
installed peers ignore it. The two never share a type.

- **Settings** (the settings file, `PUT /api/v1/settings`, adapter settings
  such as the `remote` capture's `RemoteSettings`) keep
  `deny_unknown_fields`, ranges and field paths. A misspelt field is a
  mistake to report, and the file's `format` number already says which
  Prysm can read it ([settings-file](../../../crates/adapters/settings-file/README.md)).
- **Wire messages** between a slim TV and a desktop Prysm (the capture
  stream's request, keyframe request, refusal and offer) ignore unknown
  fields and message types. An unknown value in a known field has one
  stated reading per field (an unknown encoding is refused as an invalid
  request; an unknown refusal cause is read as `request`; an unknown offer
  encoding is skipped). Anything else malformed ends the connection. The
  wire types are their own (`RequestMessage`, the offer's read types), so
  loosening them never loosens a setting.
- What a peer must not ignore is sent only when that peer asked for it:
  the receiver asks for something new only when the sender's offer
  announces it, and the sender sends something new only when the request
  asks for it. The protocol version rises only when a peer of the previous
  version can no longer work with this one
  ([capture-remote](../../../crates/adapters/capture-remote/README.md#wire-protocol),
  [releasing](../releasing.md#what-the-version-promises)).

Why: the TV app and the desktop Prysm are installed and updated separately.
With strict messages every added field needed a new protocol version, so a
TV and a desktop one minor release apart refused each other until both were
updated. With one type for settings and request, the request could not
become tolerant without the settings file accepting typos.

Considered:

- One strict type with a version bump per addition: breaks minor-release
  pairings, the case this decision exists for.
- One type, tolerant everywhere: settings would silently drop misspelt
  fields.
- A version field inside the request: the upgrade's `426` already reports a
  version mismatch before any message.
