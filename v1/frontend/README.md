# Prysm v1 frontend

An isolated npm workspace with its own lockfile. Node.js 24+ and Rust are
required; codegen runs the Rust contract exporter.

```sh
# From the repository root:
npm --prefix v1/frontend ci
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
```

`npm test` regenerates contracts, type-checks, builds the four entry points
(and the browser bundles' Rust asset lists), then runs the schema/drift and DOM
tests. `npm run build` does the same without tests. Build the frontend before
compiling a Rust host that `include!`s the asset list; Cargo never launches npm.

```sh
npm --prefix v1/frontend run dev:headless      # desktop portal, no API behind Vite
npm --prefix v1/frontend run dev:webos-portal
npm --prefix v1/frontend run dev:webos         # TV UI: open /demo.html (in-memory fakes)
npm --prefix v1/frontend run dev:webos-slim
```

The browser portals connect at once to the same-origin
[portal API](../surfaces/web-portal/HTTP-CONTRACT.md), which needs no
credentials. The TV entry points use the Luna client of the TV supervisor;
their `demo.html` pages run the same UI on an
[in-memory supervisor](testkit/luna.ts) and portal (flags `?offline`,
`?refused`, `?unreachable`). See the
[UI/host assembly guide](../UI-ASSEMBLIES.md) and [state wiring](STATE.md).

## Read the composition

- [Shared portal](packages/web-portal/index.tsx): lighting modes, live picture
  controls, one Save form per adapter section, connection and failures. It
  imports no adapters.
  - [Lighting](packages/web-portal/lighting.tsx): Screen sync (`processed`),
    the output's own effects (`device_native`, "WLED effects") and Off; modes
    that cannot work are disabled with the reason; "Open WLED" for a WLED output.
  - [Picture](packages/web-portal/processing.tsx): the six `Processing`
    controls and the [colour profiles](packages/web-portal/profiles.tsx)
    (choose the active one; new, rename, duplicate, delete; correction and hue
    shift), applied live (debounced 400 ms) because processing-only changes
    reach a running session without restarting it.
  - [Preview](packages/web-portal/preview.tsx): the captured picture drawn on
    a canvas with each edge's colours around it and the sampled area outlined
    when black bars are removed. Shown only on request; the stream is open only
    while the panel is shown, the page is visible and the portal is connected
    (the portal counts viewers). Hosts pass `httpPreviewClient()`; without a
    preview client there is no panel.
  - [Section form](packages/web-portal/section-form.tsx) with its
    [draft](packages/web-portal/draft.ts): capture and output kind selector,
    explicit Save, Reset, dirty badge, field errors, stale reload.
  - [Setup](packages/web-portal/setup.tsx): shown instead of the page while
    capture or output is missing, and on "Run setup again": video source, lights
    (the same kind selector and editors as the forms), review. Nothing is saved
    until the end, which saves both sections with one request and can start
    screen sync; a refused field returns to its step.
  - [Fields](packages/web-portal/fields.tsx): labelled inputs that show the
    server's `Failure.field` errors and their own range checks.
- [Desktop host](hosts/headless/web-portal/app.tsx) (served by `prysm-headless`
  and `prysm-tui`) and [webOS browser host](hosts/webos/web-portal/app.tsx)
  register their kinds as `AdapterUis` maps; the types are exhaustive over the
  host's generated unions.
- Panels, each an `AdapterUi { label, create, Editor, normalize?, lighting? }`:
  [video device](packages/capture-device-ui/index.tsx),
  [video file](packages/capture-file-ui/index.tsx),
  [remote capture](packages/capture-remote-ui/index.tsx),
  [webOS capture](packages/capture-webos-ui/index.tsx),
  [WLED](packages/output-wled-ui/index.tsx) with the
  [LED layout](packages/output-wled-ui/layout.tsx), and the
  [null output](packages/output-null-ui/index.tsx).
- [TV UI](packages/tv-ui/index.tsx) (both packages): status card and primary
  action from the supervisor's state ([rules](packages/tv-ui/model.ts)),
  settings rows with an explicit Save ([service and draft](packages/tv-ui/service.ts)),
  lighting modes on the Prysm that drives the lights ([portal](packages/tv-ui/portal.tsx));
  capture, lights and the slim stream encoding are set up only in the web
  portal, whose address the TV shows (and names in the status card while no
  lights are set up). Service options
  (set up or repair, diagnostics, remove setup) and remote control
  ([navigation](packages/tv-ui/navigation.ts): arrows move focus between two
  columns and within rows, OK chooses, Back closes one dialog level and then
  asks before closing). Its [Luna client](packages/webos-platform/index.ts)
  checks replies against the generated `luna` contract, times out after 30 s
  and resubscribes after 3 s; the UI holds the subscriptions only while the
  page is visible.
- [Application client](packages/application-client/index.ts),
  [HTTP transport](packages/application-client/http.ts),
  [RTK state](packages/application-state/index.ts) and
  [React bindings](packages/application-react/index.tsx).
- [Application fixture](testkit/application.ts): revision checks, immediate
  state updates, engine-like target results and scripted failures, outside the
  shipped packages. DOM tests run with [setup-dom](tests/setup-dom.js) loaded
  first, so React sees an `input` event.

## Saving settings

| Section | When it is sent | Conflict check |
| --- | --- | --- |
| Picture (`processing`) | 400 ms after the last change (a profile name when it is committed with Enter or by leaving the field); a refused value is not retried until edited | The six controls, the profiles and the active profile must still match what the panel started from |
| Capture, Lights (`capture`, `output`) | Save; Reset discards the draft | The section must still match what the form started from |

Every save copies the latest observed settings, replaces one section and sends
the current revision, so fields the UI does not show (new settings, unknown
adapter fields) are kept, and saving one section never
conflicts with another. If the section changed elsewhere, or the server answers
`stale_revision` (409), the form reloads the current values and says so.
`invalid` failures mark the control named by `field` (`capture.width`,
`output.leds` for the LED group) and move focus to it; a field the form does not
show is reported in the form's alert. Limits come from the generated schemas
and are checked against them in [tests](tests/portal.test.jsx); where a schema
has no maximum the panel uses the Rust type bound (`sample_density` `u16`,
file `fps` `u32`, device `fps` `i32` for AVFoundation). Profile limits (32
profiles, 64-character names, -100 to 100 per control) are checked the same way.

## Contracts

[Contract ownership](../CONTRACTS.md) is the review guide. The exporter prints
the `application` document (state, session, failures, requests, settings), one
`host-*` document per set of hosts with their adapter sections typed
(`host-desktop`, `host-webos`), the TV supervisor's `luna` document, and Rust
serialization fixtures; [codegen](scripts/codegen.mjs)
turns each document into `packages/contracts/generated/<name>.generated.ts`.
Generated files are checked in; never edit them.

```sh
npm --prefix v1/frontend run codegen        # refresh after Rust contract changes
npm --prefix v1/frontend run codegen:check  # fail on drift without writing
```

Panels use their adapter's generated section type from
`@prysm/contracts/host-desktop` (`DeviceSettings`, `FileSettings`,
`RemoteReceiverSettings`, `WledSettings`, `NullSettings`) or
`@prysm/contracts/host-webos` (`WebosSettings`). Rust remains the validator.

## Limits

Calibration is not in the portal yet. The browser
bundles target ES2020, the TV bundles Chromium 53 (classic scripts, like the
root TV app); each is styled by one stylesheet (the portal CSP allows no inline
styles, and no `data:` or `blob:` images, so the preview decodes its JPEG
with `createImageBitmap` and draws it on a canvas); real webOS browser compatibility and
remote navigation still need validation.
