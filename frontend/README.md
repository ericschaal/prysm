# Prysm frontend

An isolated npm workspace with its own lockfile. Node.js 24+ is required, and
Rust for codegen: `cargo xtask codegen` runs the Rust contract exporter and
hands its output to `npm run codegen`. Each script does one thing; `cargo xtask
check frontend` runs them in order.

```sh
# From the repository root:
npm --prefix frontend ci
cargo xtask codegen --check
npm --prefix frontend run typecheck
npm --prefix frontend run bundle
npm --prefix frontend test
npm --prefix frontend run lint          # ESLint, the root rule set
npm --prefix frontend run format:check  # Prettier; `format` rewrites
npm --prefix frontend run e2e:tv        # TV app with remote keys in Chromium (in-memory demo)
```

`npm run bundle` builds the four entry points (and the browser bundles' Rust
asset lists) into `dist`; `npm test` runs the schema/drift and DOM tests, each
DOM test file bundled on its own and run in its own process, and its TV tests
read `dist`, so bundle first. Bundle before compiling a Rust host:
[`prysm_host::assets`](../crates/hosts/common/src/assets.rs) embeds the asset lists
(a debug build without them serves a "not built" page, a release build
fails); Cargo never launches npm.

```sh
npm --prefix frontend run dev:headless      # desktop portal, with hot reload
npm --prefix frontend run dev:webos-portal  # TV portal: PRYSM=http://<tv>:8787
npm --prefix frontend run dev:webos         # TV UI: open /demo.html (in-memory fakes)
npm --prefix frontend run dev:webos-slim
```

The browser portals connect at once to the same-origin
[portal API](../crates/surfaces/web-portal/HTTP-CONTRACT.md), which needs no
credentials. The portal serves the page with the current state in it
([the page](../crates/surfaces/web-portal/HTTP-CONTRACT.md#the-page)), which
`embeddedState` reads, so the first render is the whole page; the state stream
then keeps it current. A page without that state (a dev server) shows its
own frame with still blocks where the cards will be, and says "Connecting to
Prysm…" in it only once the wait lasts (`connectionGraceMs`).
The two portal dev servers forward `/api` to a running
Prysm ([dev.config.mjs](dev.config.mjs)): `http://127.0.0.1:8787` unless
`PRYSM` names another one. Start a host first
(`cargo run -p prysm-headless -- crates/hosts/headless/settings.example.json`);
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
    lasts), then the mode, Brightness, Style and Calibration. The last three
    share one pattern, 24 px apart: a header with the name and one quiet item
    on the right (the value, "Adjust", "Calibrate colours"), then the control.
    Style and Calibration are one control: a radio group of segments on one
    full-width track, one per style (Custom only while chosen or kept), each
    described by its style's line, and one per calibration and "No
    correction". A chosen segment is a light pill with dark ink. Equal while
    they fit; otherwise the track scrolls sideways on one line (no
    scrollbar), the chosen one centred, and fades at an edge only while more
    is beyond it; a calibration name is cut with an ellipsis after 16
    characters. The mode is a radio group ("Mode") of two option cards about
    72 px tall: an icon, the name ("Screen sync", the output's effects name)
    and one line written to fit a phone's half card, never cut ("Follows the
    screen", "Patterns from WLED", else "Its own patterns"). Selected speaks
    the segments' chosen look, a light fill with dark ink, lit in the light's own
    colours; off, the chosen card is only outlined. Arrow keys move the
    choice; no transitions with reduced motion) and [Your setup](packages/web-portal/setup-card.tsx)
    (the video source and lights from the adapters' `summary`, a kind named
    without its qualifier such as "(development only)", Prysm by its active
    calibration's name beside the frame rate, the LEDs per edge
    from an output's `edges` in a drawing that fills the card, the
    [live preview](packages/web-portal/preview.tsx) in the drawing's screen
    when the host has a preview client, and a "Change setup" row at its
    foot). Problems show in the light card in plain
    words with the action that fixes them ("Set up Prysm"); the card keeps
    their space in every state, so nothing moves. The header names
    "Settings" beside its icon.
  - [Settings](packages/web-portal/settings-page.tsx) (`#/settings`, the
    icon in the header): one save status under the title
    ([save status](packages/web-portal/save-status.tsx)), then one column
    of [rows](packages/web-portal/rows.tsx) in groups with a small heading:
    Setup (Video source, Lights: name and facts, or the live problem in the
    same line in red), Picture (Style, Calibrations, Detail, the Remove
    black bars switch) and This device (Appearance, About this Prysm, Start
    over with setup). A row is a label, its value beneath and a chevron; the
    whole row is the link; a switch only for plain on/off. The pages under
    Settings (editors, style, calibrations, detail, appearance, about)
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
    version, and holds Developer options. Brightness and choosing the calibration
    are on Home only; Open WLED stays on Home.
  - Developer options ([developer](packages/web-portal/developer.ts)): an
    adapter marked `developer` (the video file, no lights) is offered only
    with them on, or while it is the saved kind. On in About this Prysm, kept
    per browser (`localStorage` `prysm.developer` = `on`; without storage,
    for the page). Tests turn it on with `saveDeveloper(true)` or that key.
  - [Lighting](packages/web-portal/lighting.tsx): the power button turns the
    lights `off`, or back on in the last mode (kept per browser in
    `localStorage`; screen sync without one); the mode cards choose Screen
    sync (`screen_sync`) or the output's own effects
    (`effects`, named by the output's `lighting().effects`, e.g. "WLED
    effects"), and a choice made while off turns that mode on; after a
    failure it retries. A mode that cannot work is disabled with the reason.
    Brightness and the calibrations are Prysm's processing, which only a running
    session applies (the engine stops the session and idles the output for
    `effects`), so with the lights' own effects they stay in place,
    dimmed and inert, and the card's line says "WLED controls its own
    effects." with "Open WLED". The card's light (CSS only, transform and
    opacity) drifts while the session runs; it grows out of the power button
    when the session lights up and draws back in when it stops. All colour
    stays inside the card; outside it has the same neutral shadow as the
    other cards. While screen sync runs and a
    `colors` client is given, the LEDs' colours (`/api/v1/colors/stream`,
    opened only while Home is shown, visible and online) become the card's
    four light fields: written as CSS custom properties on the Home grid,
    eased, with the LEDs' own hue and saturation and as strong as the LEDs
    are bright: dimmed LEDs glow lower in their colour, grey ones give white
    light, and black ones (or no picture) a faint colourless moonlight. The
    moonlight shows too until the first colours arrive and while the stream
    is lost. The TV drawing's LED band shows them too: each edge a gradient
    of its colours in screen order (left to right, top to bottom), eased.
    WLED effects keep the designed effects palette and open no
    stream. A failed or faulted session shows "Stopped" in a calm red light,
    whatever the session still reports (after a failed start from effects
    the session is still `effects`, carrying the failure), with the failure
    said plainly from its typed `kind` and `field` and the message under
    Details. A target or session state newer than the page is "Unknown":
    "Prysm is newer than this app", with no mode chosen. The state word
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
    and sends) is said once, on the Prysm stop after the active calibration;
    `sent_fps` stays in the API and `/metrics` but is not shown.
    When `Input.behind` names the step that keeps Prysm from keeping up
    (`processing` or `output`), that stop or link turns orange (`--behind`),
    with plain words in its tooltip; a failure shows no flowing path, so red
    always wins. A failure marks its stop (by the
    failure's `section`, else `field`) in red. On a wide card the TV drawing with the
    LED band follows in the light's colours.
  - [Picture](packages/web-portal/processing.tsx): black bars as a switch
    row; Style (`#/settings/style`): one [row choice](packages/web-portal/rows.tsx)
    per [style](packages/web-portal/styles.ts) (Natural, Calm, Vivid, Game,
    Custom), the saved `processing.style` ticked and applied at once. Below
    the list one disclosure row, "Adjust this style" (closed, open while Custom
    is chosen), holds the ticked style's sliders (smoothing 0 to 2 s, scene
    cuts 0 to 100 %, brightness ramp 0 to 2 s, edge depth, colour focus and
    colour boost 0 to 100 %, dark-scene glow 0 to 20 %); moving one on a
    built-in style saves `style.kind = custom` with that style's values plus
    the change in `processing.custom_style`. A first Custom starts from the
    ticked style's values, generated from Rust (`@prysm/contracts/constants`);
    later it returns to the kept values. Detail
    (`#/settings/detail`: 1 to 512) stays outside styles. All sliders are in
    Home's one slider style, over the whole range Prysm accepts, and apply
    live (debounced 400 ms, sliders on release) because processing-only
    changes reach a running session without restarting it. [Calibrations](packages/web-portal/calibrations.tsx)
    (`#/settings/calibrations`): one row each ("In use" or "Not in use") opening
    the calibration's page (`#/settings/calibrations/<id>`: name, an "In use" switch,
    warmth, tint, midtones, then red, green and blue with the wizard's two
    sliders each, Reset, Delete). A deleted calibration keeps its row
    in the list with Undo for 5 s (kept by the portal across the two pages);
    the "New calibration" row opens Calibrate. Home's brightness changes
    the lights while it is dragged (pointer or keys): each value is tried
    unsaved (`PUT /api/v1/processing/trial`), throttled to one every 100 ms
    with one request in flight and the latest value winning, and the value
    where it is let go is saved once; a failed step shows nothing, only the
    save's failure. A calibration segment saves at once.
  - [Preview](packages/web-portal/preview.tsx): in the screen of Home's TV
    drawing, a play/pause button (paused on every visit); playing, the
    captured picture drawn on a canvas clipped to the screen, the cut-off
    black bars dimmed and the sampled area outlined, and one line under the
    drawing with the picture's size or the stream's state (connecting, idle
    and why, unavailable, busy or stopped with "Try again"). The LED bands
    go on showing the live colours. The stream is open only while
    playing, Home is shown, the browser tab is visible and the portal is
    connected (the portal counts viewers). On a phone-width card the
    drawing, otherwise hidden there, shows and takes the card's full width.
    Hosts pass `httpPreviewClient()`; without a preview client there is
    none.
  - [Section editor](packages/web-portal/section-editor.tsx) with its
    [draft](packages/web-portal/draft.ts): the video source
    (`#/settings/video`) or the lights (`#/settings/lights`) on a page of
    their own, opened from their row in Settings. The same step as in setup
    ([section step](packages/web-portal/section-step.tsx): the kinds offered
    as rows to tick, "Capture from" or "Send colours to", when there is more
    than one, else none; words in
    [step copy](packages/web-portal/step-copy.ts)), one button that saves
    ("Use this video source", "Use these lights"), Cancel. Saving restarts
    screen sync, so the editor then checks it: with screen sync on it waits
    up to 10 s for the new source or lights to work (flowing for 1.5 s) or
    fail (a new failure on that side), and says which; with it off it says
    nothing was tried and offers "Turn on screen sync to try". While it is
    not known to work, "Go back to the previous …" saves what was there
    before. A change on another device asks: Use theirs, or Keep mine.
    The rows in Settings show their side's live problem, and
    Home's "Change setup" opens the editor of a failing side (else
    Settings). After a failed check the buttons are "Go back to the previous
    …" and "Keep this video source" / "Keep these lights".
  - [Setup](packages/web-portal/setup.tsx): shown instead of the page while
    capture or output is missing, and on "Start over with setup": video source, lights
    (the same step as the editors), review. Nothing is saved
    until the end, which saves both sections with one request and can start
    screen sync; a refused field returns to its step.
  - [Calibrate](packages/web-portal/calibrate.tsx): the page at
    `#/calibrate` ("New calibration" in Calibrations, which it
    returns to; Home's "Calibrate colours"; the TV's QR code
    points there, and leaving returns Home). Start shows the first pattern, then creates a draft calibration (new, or a copy of
    one) and makes it active; if that save fails, the pattern ends and nothing
    is kept. White (`gray_192`: warmth, tint), red, green
    and blue (`red_pair`, `green_pair`, `blue_pair`: two sliders each, named
    by their ends), checks without sliders (`yellow`, `cyan`, `magenta`,
    `coast`, `garden`, `dusk`: "Looks right", or a link to a colour it is made
    of, which comes back to the check) and a review each show their pattern
    on the TV (`setCalibration`); each adjusting screen has its own Reset; sliders
    save live through `useLiveProcessing`, as the calibrations page does. Finish names
    it, Cancel deletes it and restores the previous active one;
    both end calibration. Without screen sync it says to choose Calibrate on
    the TV; if calibration ends elsewhere it offers Resume and keeps the draft,
    also after a Resume the TV refuses. Its state is one reducer
    ([wizard](packages/web-portal/calibrate-wizard.ts)) behind one hook
    ([useCalibration](packages/web-portal/use-calibration.ts)); its words are
    in [calibrate-copy](packages/web-portal/calibrate-copy.ts).
  - [Devices](packages/web-portal/discovery.tsx): the video device editor
    asks the portal (`GET /api/v1/candidates/capture/device`) what is present
    as it opens, and lists the devices as rows to tick once it answers;
    until then one row, which nothing can tick, stands where the first found
    device will be: the saved device, with "Looking for devices…" beneath
    (or that line alone), so the usual answer of one device moves nothing;
    "Look again", at the end of the "Device" heading (outside the
    group's name), asks again, keeping the rows and the focus while it
    looks; the line under the rows shows only while there is something to
    say. A row shows its device's path only when the name does not tell it
    apart (two of one name, or no name). The last row is always one to type
    in: a radio ("Another device") with its field ("Device path") in the
    label's place, in the tab order only while ticked; focusing or typing in
    it ticks it, and ticking a found row keeps what was typed there to tick
    again. A saved device the lookup does not list is in that row, ticked,
    with the name this browser last saw for it beneath ([device
    names](packages/web-portal/device-name.ts), the last eight per kind) and
    "Not plugged in" in the problem colour while it cannot be asked. The
    row says the device's problem ("Choose a device, or enter one by
    hand.") beneath it and takes the focus, unless a found row is ticked:
    then the line under the rows says it and the group takes the focus. The
    WLED editor lists its lights the same way (`.../output/wled`, the
    "Lights" heading, "Other lights" and "Address"; "Smooth dark colours"
    under Advanced); the saved address has no problem line, since lights
    can answer and not be found, and picking one notes its LED count under
    the drawing. Hosts pass `httpDiscoveryClient()`; without one there is
    only the typed row.
  - [Fields](packages/web-portal/fields.tsx): labelled inputs that show the
    server's `Failure.field` errors and their own range checks.
  - [Size and rate](packages/web-portal/picture-choice.tsx): every capture
    editor's width, height and frame rate as one choice in the rows pattern,
    from what the source delivers ([capture modes](packages/web-portal/capture-modes.ts):
    `POST /api/v1/capture/modes` with only the fields that select the
    hardware, a capture card's `device` or the TV's `encoding`; asked once
    for each, and again only when Try again is pressed under an answer that
    is not known, since nothing announces a device or a TV's offer;
    hosts pass
    `httpCaptureModesClient()`). The
    [presets](packages/web-portal/picture-presets.ts) aim for Balanced
    (640 × 360, 30 fps), Smooth (640 × 360, 60 fps) and Efficient (320 × 180,
    30 fps); each is its target where a mode takes it, else the smallest
    mode covering the target size (16:9 first) at the target rate, else not
    offered, and presets that come out the same show once. A preset set
    above its target says so when ticked ("The smallest size this device
    offers at 30 fps."). Each row shows
    the size and rate it sets. Nothing extra is stored: the ticked row
    follows from the saved numbers; a saved size and rate no preset sets is
    one more ticked row, "Custom", until a preset replaces it. One that a
    known answer excludes moves to the first preset (or to the size of a
    preset ticked earlier), and the line kept under the rows says so ("This
    device doesn't offer 640 × 360 at 30 fps. Balanced is chosen.", an
    unsaved change); following a ticked preset to a deliverable size is
    silent. The line takes no room while empty (Advanced then follows the
    rows as one more row), so the rows never move; the file source's own
    help sits in it otherwise. Until the answer is known the rows are
    locked on the current choice, with the reason while the hardware is out
    of reach ("The TV isn't connected.", "Plug in the device …"). A source
    that delivers nothing shows no rows; the field that selects it says so
    (a device says why: "This device offers no picture Prysm can use." when
    it lists nothing Prysm captures, else "This device doesn't offer 30 or
    60 fps at a size Prysm can use."; a TV encoding it
    does not offer is not listed, and a saved one stays listed, in sight,
    saying so). A refused size or rate is said in that line, and the rows
    take the focus.
- [Desktop host](hosts/headless/web-portal/app.tsx) (served by `prysm-headless`
  and `prysm-tui`) and [webOS browser host](hosts/webos/web-portal/app.tsx)
  register their kinds as `AdapterUis` maps; the types are exhaustive over the
  host's generated unions.
- Panels, each an
  `AdapterUi { label, create, optional, Editor, normalize?, lighting? }`:
  [video device](packages/capture-device-ui/index.ts),
  [video file](packages/capture-file-ui/index.ts),
  [remote capture](packages/capture-remote-ui/index.ts),
  [webOS capture](packages/capture-webos-ui/index.ts),
  [WLED](packages/output-wled-ui/index.ts) on Home's
  [LED drawing](packages/web-portal/led-drawing.tsx) (each edge's count
  field in the middle of its strip, LED 1 badged in its corner with one
  mark along the first lit strip, the wiring said once under the drawing,
  for example "First LED is bottom left. The strip runs clockwise.", and the
  total in the screen), and the
  [null output](packages/output-null-ui/index.tsx).
- [TV UI](packages/tv-ui/app.tsx) (both packages): status card from the
  supervisor's state ([rules](packages/tv-ui/model.ts)), with the capture
  rate and a countdown to a pending restart (counted from when the state
  arrived; its bar runs on through later states of the same wait); the full
  package's headline says what the lights do from the portal's state, so it
  agrees with the lights card; a primary button only for the service (set
  up, try again after a failure, slim capture on and off), never for what
  the lights do or while a setting saves, and busy rather than disabled while
  it waits, so focus stays on it;
  lighting modes on the Prysm that drives the lights ([portal](packages/tv-ui/portal.tsx)),
  which stay lit and focused while a choice applies; the last portal state
  stays shown for up to 4 s while its stream reconnects (also over a
  calibration pattern);
  settings rows that apply at once (a switch for Start with TV, a dialog for
  the slim Prysm address; [service](packages/tv-ui/service.ts));
  capture, lights and the slim stream encoding are set up only in the web
  portal, whose address the TV shows with a QR code (and names in the status card while no
  lights are set up); the address's place is kept until it is known, and
  focus is placed only once the service and the portal have answered. [Calibration](packages/tv-ui/calibration.tsx): a QR
  code of the portal's `#/calibrate` page, then the pattern the phone chose as
  full-screen video ([MSE player](packages/tv-ui/patterns.ts),
  [QR encoder](packages/tv-ui/qr.ts)). Service options
  (set up or repair, diagnostics, remove setup) and remote control
  ([navigation](packages/tv-ui/navigation.ts): arrows move focus between two
  columns and within rows, OK chooses, Back closes one dialog level and then
  asks before closing). Its [Luna client](packages/webos-platform/index.ts)
  checks replies against the generated `luna` contract, times out after 30 s
  and resubscribes after 3 s; one lost subscription keeps the last state on
  screen, and only the bus saying the service is missing, Remove setup, or a
  second failure in a row shows "Prysm needs setting up". The UI holds the
  subscriptions only while the page is visible.
- [Application client](packages/application-client/index.ts),
  [HTTP transport](packages/application-client/http.ts),
  [RTK state](packages/application-state/index.ts) and
  [React binding](packages/application-react/index.tsx).
- [Discovery fixture](testkit/discovery.ts): scripted candidates per
  `section/kind`, recorded calls and a scriptable refusal.
- [Capture modes fixture](testkit/capture-modes.ts): an answer per capture
  section (any size up to 1920 × 1080 at 60 fps unless scripted), recorded
  calls and a scriptable refusal; `renderPortal` passes it by default.
- [Application fixture](testkit/application.ts): revision checks, immediate
  state updates, engine-like target results and scripted failures, outside the
  shipped packages. DOM tests run with [setup-dom](tests/setup-dom.js) loaded
  first, so React sees an `input` event.

## Saving settings

| Section | When it is sent | Conflict check |
| --- | --- | --- |
| Picture (`processing`) | 400 ms after the last change; a slider where it is let go (tried on the lights while held); a calibration name when it is committed with Enter or by leaving the field. Saves never overlap: one asked for meanwhile follows the one on its way, against the revision it returned. A refused value is not retried until edited | None asked: a refused stale save waits for the newer settings, puts the user's changed controls (each calibration on its own) on them and saves again |
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
has no maximum the panel uses the Rust type bound (`sample_density` `u16`).
Capture sizes and rates have no limits written here: they come from what the
source delivers. Calibration limits (32
calibrations, 64-character names, -100 to 100 per control) are checked the same way.

## Contracts

[Contract ownership](../docs/developers/contracts.md) is the review guide. The exporter prints
the `application` document (state, session, failures, requests, settings), one
`host-*` document per set of hosts with their adapter sections typed
(`host-desktop`, `host-webos`), the TV supervisor's `luna` document, and Rust
serialization fixtures and the constants the frontends use as they are
(the built-in styles' values); [codegen](scripts/codegen.mjs)
turns each document into `packages/contracts/generated/<name>.generated.ts`
and the constants into `constants.generated.ts`.
Generated files are checked in; never edit them.

```sh
cargo xtask codegen          # refresh after Rust contract changes
cargo xtask codegen --check  # fail on drift without writing
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
