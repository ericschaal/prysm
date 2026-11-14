# Prysm frontend

An isolated npm workspace with its own lockfile. Node.js 24+ and Rust are
required; codegen runs the Rust contract exporter.

```sh
# From the repository root:
npm --prefix frontend ci
npm --prefix frontend run codegen:check
npm --prefix frontend test
npm --prefix frontend run lint          # ESLint, the root rule set
npm --prefix frontend run format:check  # Prettier; `format` rewrites
```

`npm test` regenerates contracts, type-checks, builds the four entry points
(and the browser bundles' Rust asset lists), then runs the schema/drift and DOM
tests. `npm run build` does the same without tests. Build the frontend before
compiling a Rust host that `include!`s the asset list; Cargo never launches npm.

```sh
npm --prefix frontend run dev:headless      # desktop portal, with hot reload
npm --prefix frontend run dev:webos-portal  # TV portal: PRYSM=http://<tv>:8787
npm --prefix frontend run dev:webos         # TV UI: open /demo.html (in-memory fakes)
npm --prefix frontend run dev:webos-slim
```

The browser portals connect at once to the same-origin
[portal API](../surfaces/web-portal/HTTP-CONTRACT.md), which needs no
credentials. The two portal dev servers forward `/api` to a running
Prysm ([dev.config.mjs](dev.config.mjs)): `http://127.0.0.1:8787` unless
`PRYSM` names another one. Start a host first
(`cargo run -p prysm-headless -- hosts/headless/settings.example.json`);
without one the page says "Invalid state stream response." The TV entry points use the Luna client of the TV supervisor;
their `demo.html` pages run the same UI on an
[in-memory supervisor](testkit/luna.ts) and portal (flags `?offline`,
`?refused`, `?unreachable`, `?old`; selects at the bottom right put the
supervisor in each of its states and play the phone's calibration part). See the
[UI/host assembly guide](../docs/developers/ui-assemblies.md) and [state wiring](STATE.md).

## Read the composition

- [Shared portal](packages/web-portal/portal.tsx): three pages and two
  flows. It imports no adapters.
  - Home: two cards of one width, 960 px at most (Settings is 640): the
    light card (the state in one word as the only headline, the power button,
    one kept line for a problem with its fix, else what is happening: off,
    what Power turns on; on, "Following the screen." or "WLED effects are
    showing." or, for the lights' own effects, "WLED controls its own
    effects." with Open WLED; a start or a wait for the picture once it
    lasts), then the mode, Brightness and the colour profiles as chips, 32
    px apart. The mode is a radio group ("Mode") of two option cards about
    72 px tall: an icon, the name ("Screen sync", the output's effects name)
    and one line written to fit a phone's half card, never cut ("Follows the
    screen", "Patterns from WLED", else "Its own patterns"). Selected speaks
    the chips' language, a light fill with dark ink, lit in the light's own
    colours; off, the chosen card is only outlined. Arrow keys move the
    choice; no transitions with reduced motion) and [Your setup](packages/web-portal/setup-card.tsx)
    (the video source and lights from the adapters' `summary`, a kind named
    without its qualifier such as "(development only)", Prysm by its active
    profile's name beside the frame rate, the LEDs per edge
    from an output's `edges` in a drawing that fills the card, and a
    "Change setup" row at its foot). Problems show in the light card in plain
    words with the action that fixes them ("Set up Prysm"); the card keeps
    their space in every state, so nothing moves. The header names
    "Settings" beside its icon.
  - [Settings](packages/web-portal/settings-page.tsx) (`#/settings`, the
    icon in the header): one save status under the title
    ([save status](packages/web-portal/save-status.tsx)), then one column
    of [rows](packages/web-portal/rows.tsx) in groups with a small heading:
    Setup (Video source, Lights: name and facts, or the live problem in the
    same line in red), Picture (Colour profiles, Fine-tuning, the Remove
    black bars switch) and This device (Appearance, About this Prysm, Start
    over with setup). A row is a label, its value beneath and a chevron; the
    whole row is the link; a switch only for plain on/off. The pages under
    Settings (editors, profiles, fine-tuning, appearance, about, preview)
    have the same width, heading and back link, and one line under the
    heading in the same place: the save status where things save live, else
    how the page saves ("Saved when you choose …", "Kept in this browser").
    A page that ends with actions puts them under a divider, from the left,
    the main one first and a destructive one last and quiet; an editor's
    status sits beside them. Sliders follow one rhythm: label, track and help
    close together, 32 px between sliders. Appearance
    (`#/settings/appearance`) is System, Light or Dark as rows (kept per
    browser in `localStorage`, never sent to Prysm); About this Prysm
    (`#/settings/about`) shows lights, status, frames per second and settings
    version, and holds Developer options. Brightness and choosing the profile
    are on Home only; Open WLED stays on Home.
  - Developer options ([developer](packages/web-portal/developer.ts)): an
    adapter marked `developer` (the video file, no lights) is offered only
    with them on, or while it is the saved kind. On in About this Prysm, kept
    per browser (`localStorage` `prysm.developer` = `on`; without storage,
    for the page). Tests turn it on with `saveDeveloper(true)` or that key.
  - [Lighting](packages/web-portal/lighting.tsx): the power button turns the
    lights `off`, or back on in the last mode (kept per browser in
    `localStorage`; screen sync without one); the mode cards choose Screen
    sync (`processed`) or the output's own effects
    (`device_native`, named by the output's `lighting().effects`, e.g. "WLED
    effects"), and a choice made while off turns that mode on; after a
    failure it retries. A mode that cannot work is disabled with the reason.
    Brightness and the profiles are Prysm's processing, which only a running
    session applies (the engine stops the session and idles the output for
    `device_native`), so with the lights' own effects they stay in place,
    dimmed and inert, and the card's line says "WLED controls its own
    effects." with "Open WLED". The card's light (CSS only, transform and
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
  - [Picture](packages/web-portal/processing.tsx): black bars as a switch
    row, and Picture fine-tuning (`#/settings/picture`: shadow lift, edge
    depth, smoothing 0 to 2 s, detail 1 to 512, all sliders in Home's one
    slider style, widened to fit a saved value beyond), applied live (debounced 400 ms, sliders on
    release) because processing-only changes reach a running session without
    restarting it. [Colour profiles](packages/web-portal/profiles.tsx)
    (`#/settings/profiles`): one row each ("In use" or "Not in use") opening
    the profile's page (`#/settings/profiles/<id>`: name, an "In use" switch,
    correction and hue shift, Reset, Delete). A deleted profile keeps its row
    in the list with Undo for 5 s (kept by the portal across the two pages);
    the "New profile (guided)" row opens Calibrate. Home's brightness changes
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
  - [Section editor](packages/web-portal/section-editor.tsx) with its
    [draft](packages/web-portal/draft.ts): the video source
    (`#/settings/video`) or the lights (`#/settings/lights`) on a page of
    their own, opened from their row in Settings. The same step as in setup
    ([section step](packages/web-portal/section-step.tsx), words in
    [step copy](packages/web-portal/step-copy.ts)), one button that saves
    ("Use this video source", "Use these lights"), Cancel. Saving restarts
    screen sync, so the editor then checks it: with screen sync on it waits
    up to 10 s for the new source or lights to work (flowing for 1.5 s) or
    fail (a new failure on that side), and says which; with it off it says
    nothing was tried and offers "Turn on screen sync to try". While it is
    not known to work, "Go back to the previous …" saves what was there
    before. A change on another device asks: Use theirs, or Keep mine.
    Each editor opens the live preview from a row ("See the picture",
    "Check placement"). The rows in Settings show their side's live problem, and
    Home's "Change setup" opens the editor of a failing side (else
    Settings). After a failed check the buttons are "Go back to the previous
    …" and "Keep this video source" / "Keep these lights".
  - [Setup](packages/web-portal/setup.tsx): shown instead of the page while
    capture or output is missing, and on "Start over with setup": video source, lights
    (the same step as the editors), review. Nothing is saved
    until the end, which saves both sections with one request and can start
    screen sync; a refused field returns to its step.
  - [Calibrate](packages/web-portal/calibrate.tsx): the page at
    `#/calibrate` ("New profile (guided)" in Colour profiles, which it
    returns to; Home's chip while there are no profiles; the TV's QR code
    points there, and leaving returns Home). Start creates a draft profile (new, or a copy of
    one) and makes it active; white balance (`gray-192`), one hue per colour,
    intensity (`red-pair`, `green-pair`, `blue-pair`), mixed colours and a
    review each show their pattern on the TV (`setCalibration`); sliders
    save live through `useLiveProcessing`, as the profiles page does. Finish names
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
| Picture (`processing`) | 400 ms after the last change; a slider where it is let go (tried on the lights while held); a profile name when it is committed with Enter or by leaving the field. Saves never overlap: one asked for meanwhile follows at once. A refused value is not retried until edited | None asked: a refused stale save waits for the newer settings, puts the user's changed controls on them and saves again |
| Video source, Lights (`capture`, `output`) | The editor's one button; Cancel discards the draft | The section must still match what the editor started from; otherwise the user chooses theirs or mine |

Every save copies the latest observed settings, replaces one section and sends
the current revision, so fields the UI does not show (new settings, unknown
adapter fields) are kept, and saving one section never
conflicts with another. Settings says in one line whether everything is saved
(`SaveStatus`). `invalid` failures mark the control named by `field`
(`capture.width`, `output.leds` for the LED group) with the adapter's own
sentence (`AdapterUi.problems`) in place of its help, and move focus to it; a
field the form does not show is reported in the form's status line. Limits come from the generated schemas
and are checked against them in [tests](tests/portal.test.jsx); where a schema
has no maximum the panel uses the Rust type bound (`sample_density` `u16`,
file `fps` `u32`, device `fps` `i32` for AVFoundation). Profile limits (32
profiles, 64-character names, -100 to 100 per control) are checked the same way.

## Contracts

[Contract ownership](../docs/developers/contracts.md) is the review guide. The exporter prints
the `application` document (state, session, failures, requests, settings), one
`host-*` document per set of hosts with their adapter sections typed
(`host-desktop`, `host-webos`), the TV supervisor's `luna` document, and Rust
serialization fixtures; [codegen](scripts/codegen.mjs)
turns each document into `packages/contracts/generated/<name>.generated.ts`.
Generated files are checked in; never edit them.

```sh
npm --prefix frontend run codegen        # refresh after Rust contract changes
npm --prefix frontend run codegen:check  # fail on drift without writing
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
