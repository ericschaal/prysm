# Prysm v1 frontend

An isolated npm workspace with its own lockfile. Node.js 24+ and Rust are
required; codegen runs the Rust contract exporter.

```sh
# From the repository root:
npm --prefix v1/frontend ci
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
npm --prefix v1/frontend run lint          # ESLint, the root rule set
npm --prefix v1/frontend run format:check  # Prettier; `format` rewrites
```

`npm test` regenerates contracts, type-checks, builds the four entry points
(and the browser bundles' Rust asset lists), then runs the schema/drift and DOM
tests. `npm run build` does the same without tests. Build the frontend before
compiling a Rust host that `include!`s the asset list; Cargo never launches npm.

```sh
npm --prefix v1/frontend run dev:headless      # desktop portal, with hot reload
npm --prefix v1/frontend run dev:webos-portal  # TV portal: PRYSM=http://<tv>:8787
npm --prefix v1/frontend run dev:webos         # TV UI: open /demo.html (in-memory fakes)
npm --prefix v1/frontend run dev:webos-slim
```

The browser portals connect at once to the same-origin
[portal API](../surfaces/web-portal/HTTP-CONTRACT.md), which needs no
credentials. The two portal dev servers forward `/api` to a running
Prysm ([dev.config.mjs](dev.config.mjs)): `http://127.0.0.1:8787` unless
`PRYSM` names another one. Start a host first
(`cargo run --manifest-path v1/Cargo.toml -p prysm-headless -- v1/hosts/headless/settings.example.json`);
without one the page says "Invalid state stream response." The TV entry points use the Luna client of the TV supervisor;
their `demo.html` pages run the same UI on an
[in-memory supervisor](testkit/luna.ts) and portal (flags `?offline`,
`?refused`, `?unreachable`, `?old`; selects at the bottom right put the
supervisor in each of its states and play the phone's calibration part). See the
[UI/host assembly guide](../UI-ASSEMBLIES.md) and [state wiring](STATE.md).

## Read the composition

- [Shared portal](packages/web-portal/portal.tsx): three pages and two
  flows. It imports no adapters.
  - Home: the light card (the state in one word, the power button, the two
    modes, brightness and the colour profiles as chips) and
    [Your setup](packages/web-portal/setup-card.tsx) (the video source and
    lights from the adapters' `summary`, and the LEDs per edge from an
    output's `edges`). Problems show in the light card in plain words with
    the action that fixes them ("Set up Prysm").
  - Settings (`#/settings`, the icon in the header): Picture (brightness,
    black bars, colour profiles, and under Advanced shadow lift, spatial
    reach, smoothing and sample density), Calibrate colours, Run setup again,
    Live preview, the Video source and Lights forms (one disclosure each), the theme
    (System, Light, Dark; kept per browser in `localStorage`, never sent to
    Prysm) and the technical details (with the capture rate).
  - [Lighting](packages/web-portal/lighting.tsx): the power button turns the
    lights `off`, or back on in the last mode (kept per browser in
    `localStorage`; screen sync without one); the tiles choose Screen sync
    (`processed`) or the output's own effects (`device_native`, named by the
    output's `lighting().effects`, e.g. "WLED effects"), and a tile chosen
    while off turns that mode on; after a failure a tile retries. Modes that
    cannot work are disabled with the reason. Brightness and the profiles
    apply to screen sync only, so the effects tile shows a note and
    "Open WLED" in their place. The card's light (CSS only, transform and
    opacity) drifts while the session runs; it grows out of the power button
    when the session lights up and draws back in when it stops. All colour
    stays inside the card; outside it has the same neutral shadow as the
    other cards. While screen sync runs and a
    `colors` client is given, the LEDs' colours (`/api/v1/colors/stream`,
    opened only while Home is shown, visible and online) become the card's
    four light fields: written as CSS custom properties on the Home grid,
    eased, kept in an OKLCH window and blended with the designed palette for
    dull content. WLED effects keep the designed effects palette and open no
    stream. A failed or faulted session shows "Stopped" in a calm red light,
    whatever the session still reports (after a failed start from effects
    the engine leaves it `native`), with the failure said plainly from its
    typed `kind` and `field` and the message under Details. The state word
    follows the session, not the request. Words appear only when they inform
    (a failure, faulted, waiting for video, an output issue, or starting
    after a second). Your setup is the signal path: Video, Prysm, Lights,
    named (via discovery, when it finds the saved device) with one fact each
    from the adapters' typed `facts`, joined by hairlines that light up while
    frames flow. Above each hairline its transport (the adapters' closed
    `Transport`: Local, File, Built in, H.264, Raw, DDP; said in full in a
    tooltip), below it what that link adds to the lag, its mean delay from
    `Input.latency` ("+88 ms", "+<1 ms"): on the first, capture to Prysm
    receiving the frame (`capture_ms`, only when the source stamps capture
    times); on the second, received to colours sent (`output_ms`). When both
    are known the Lights stop's line says their sum instead of the LED count
    ("90 ms behind", summed in the portal); the count stays in that stop's
    tooltip and screen-reader text. The frame rate (`Input.fps`, the frames Prysm takes
    and sends) is said once, on the Prysm stop after the active profile;
    `sent_fps` stays in the API and `/metrics` but is not shown.
    When `Input.behind` names the step that keeps Prysm from keeping up
    (`processing` or `output`), that stop or link turns orange (`--behind`),
    with plain words in its tooltip; a failure shows no flowing path, so red
    always wins. A failure marks its stop (by the
    failure's `section`, else `field`) in red. On a wide card the TV drawing with the
    LED band follows in the light's colours.
  - [Picture](packages/web-portal/processing.tsx): the six `Processing`
    controls and the [colour profiles](packages/web-portal/profiles.tsx)
    (choose the active one; new, rename, duplicate, delete; correction and hue
    shift), applied live (debounced 400 ms) because processing-only changes
    reach a running session without restarting it. Home's brightness changes
    the lights while it is dragged (pointer or keys): each value is tried
    unsaved (`PUT /api/v1/processing/trial`), throttled to one every 100 ms
    with one request in flight and the latest value winning, and the value
    where it is let go is saved once; a failed step shows nothing, only the
    save's failure. A profile chip saves at once.
  - [Preview](packages/web-portal/preview.tsx): the captured picture drawn on
    a canvas with each edge's colours around it and the sampled area outlined
    when black bars are removed, on its own page (`#/preview`, from
    Settings); the stream is open only while that page is shown, the
    browser tab is visible and the portal is connected (the portal counts
    viewers). Hosts pass `httpPreviewClient()`; without a preview client
    there is none.
  - [Section form](packages/web-portal/section-form.tsx) with its
    [draft](packages/web-portal/draft.ts): capture and output kind selector,
    explicit Save, Reset, dirty badge, field errors, stale reload.
  - [Setup](packages/web-portal/setup.tsx): shown instead of the page while
    capture or output is missing, and on "Run setup again": video source, lights
    (the same kind selector and editors as the forms), review. Nothing is saved
    until the end, which saves both sections with one request and can start
    screen sync; a refused field returns to its step.
  - [Calibrate](packages/web-portal/calibrate.tsx): the page at
    `#/calibrate` (entry "Calibrate colours" in Settings, which it returns
    to; the TV's QR code points there, and leaving returns Home). Start creates a draft profile (new, or a copy of
    one) and makes it active; white balance (`gray-192`), one hue per colour,
    intensity (`red-pair`, `green-pair`, `blue-pair`), mixed colours and a
    review each show their pattern on the TV (`setCalibration`); sliders
    save live through the Picture panel's `useLiveProcessing`. Finish names
    the profile, Cancel deletes it and restores the previous active one;
    both end calibration. Without screen sync it says to choose Calibrate on
    the TV; if calibration ends elsewhere it offers Resume and keeps the draft.
  - [Find devices](packages/web-portal/discovery.tsx): "Find devices" in the
    video device editor and "Find lights" in the WLED editor ask the portal
    (`GET /api/v1/candidates/{section}/{kind}`) what is present and list it;
    picking one fills the path or address, and the typed field stays the
    fallback (nothing found: "No devices found. Enter the path below.").
    A button rather than a lookup when the editor opens: the editors are in
    Settings too, and WLED discovery takes over a second and asks the
    network. Hosts pass `httpDiscoveryClient()`; without one there is no button.
  - [Fields](packages/web-portal/fields.tsx): labelled inputs that show the
    server's `Failure.field` errors and their own range checks.
- [Desktop host](hosts/headless/web-portal/app.tsx) (served by `prysm-headless`
  and `prysm-tui`) and [webOS browser host](hosts/webos/web-portal/app.tsx)
  register their kinds as `AdapterUis` maps; the types are exhaustive over the
  host's generated unions.
- Panels, each an `AdapterUi { label, create, Editor, normalize?, lighting? }`:
  [video device](packages/capture-device-ui/index.ts),
  [video file](packages/capture-file-ui/index.ts),
  [remote capture](packages/capture-remote-ui/index.ts),
  [webOS capture](packages/capture-webos-ui/index.ts),
  [WLED](packages/output-wled-ui/index.ts) with the
  [LED layout](packages/output-wled-ui/layout.tsx), and the
  [null output](packages/output-null-ui/index.tsx).
- [TV UI](packages/tv-ui/app.tsx) (both packages): status card and primary
  action from the supervisor's state ([rules](packages/tv-ui/model.ts)),
  with the capture rate and a countdown to a pending restart (counted from
  when the state arrived),
  settings rows with an explicit Save ([service and draft](packages/tv-ui/service.ts)),
  lighting modes on the Prysm that drives the lights ([portal](packages/tv-ui/portal.tsx));
  capture, lights and the slim stream encoding are set up only in the web
  portal, whose address the TV shows with a QR code (and names in the status card while no
  lights are set up). [Calibration](packages/tv-ui/calibration.tsx): a QR
  code of the portal's `#/calibrate` page, then the pattern the phone chose as
  full-screen video ([MSE player](packages/tv-ui/patterns.ts),
  [QR encoder](packages/tv-ui/qr.ts)). Service options
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
- [Discovery fixture](testkit/discovery.ts): scripted candidates per
  `section/kind`, recorded calls and a scriptable refusal.
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

The browser
bundles target ES2020, the TV bundles Chromium 53 (classic scripts, like the
root TV app); each is styled by one stylesheet (the portal CSP allows no inline
styles, and no `data:` or `blob:` images, so the preview decodes its JPEG
with `createImageBitmap` and draws it on a canvas); real webOS browser compatibility and
remote navigation still need validation.
