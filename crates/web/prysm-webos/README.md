# webOS TV app

[Frontend setup/generation](../../../docs/developers/frontend.md) ·
[IPK build](../../../docs/developers/packaging.md#webos)

## Local development

```sh
npm ci --prefix crates/web/prysm-webos
npm run dev --prefix crates/web/prysm-webos
```

Open `http://127.0.0.1:5174`. Without `PalmServiceBridge`, an in-memory Luna
simulation resets on reload; no TV connection/capture.

State/drafts: [useCaptureService](src/hooks/useCaptureService.ts).
Focus/remote: [useRemoteNavigation](src/hooks/useRemoteNavigation.ts).
Requests/timeouts: [service.ts](src/service.ts).

## Build and test

`npm test --prefix crates/web/prysm-webos`

Production uses Chromium 53 syntax, classic scripts and relative URLs for `file:`
loading. Keep dependencies within that target; there are no runtime polyfills.

## TV layout

Preserve drafts/focus across status updates. Arrows move focus, not values.
At 720p: captions ≥20 px, controls ≥56 px, safe area ≥20 px; retain it at 1080p.
