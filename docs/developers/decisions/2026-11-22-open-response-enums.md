# Open response enums, closed request enums

Date: 2026-11-22

Enum values Prysm sends may grow in a minor release; values it receives
may not.

- **Sent values are open:** HTTP responses and events, `State`, the Luna
  service's replies and the capture stream's messages. A client accepts a
  value it does not know and reads it as the contract states for that
  field, never as off or stopped. It sends a value it read back unchanged.
  The one exception is a tag that selects a whole shape (Luna `flavor`): its
  stated reading is "this state cannot be shown".
- **Each field has one reading,** listed in the
  [portal contract](../../../crates/surfaces/web-portal/HTTP-CONTRACT.md#values-that-may-grow)
  and the [Luna API](../webos.md#luna-api). Examples:
  - a newer `target` or `session.state` is the situation `unknown`, with no
    mode selected, and a failure or message in it is still shown;
  - a newer Luna worker `phase` shows "Updating…" and offers no start or
    stop;
  - adapter and style kinds in the settings are kept as sent and sent back
    unchanged.
- **Received values are closed:** request bodies, the settings file, Luna
  requests and the worker link. Prysm refuses a value it does not know
  (`400`, or `422` with `field`; a Luna error; a settings error), so a new value there
  is a minor change that only newer clients send. The worker link is not
  covered by the version: both of its ends ship in one IPK.
- The OpenAPI schemas keep `enum` with the values this version sends, so the
  generated unions stay exact; the openness lives in the contract text and in
  each client's reading (`known(values, raw, 'unknown')`).

Why: the slim TV app and a desktop Prysm are installed and updated
separately, so the one cross-version client already meets values newer than
itself. With frozen response enums every new device issue, pattern or
failure kind would be a major release; reading an unknown value as off or
stopped would say something false about the lights.

Considered:

- Frozen response enums, each addition a major release: every new condition
  waits for 2.0, or is not reported.
- Open request enums as well: a value Prysm cannot carry out would be
  accepted and silently do nothing.
- Unknown values read as a known one (off, stopped): an older app would show
  lights it cannot name as off.
