# webOS TV app

The TV frontend uses React, TypeScript and Vite. For shared tooling and generated
contracts, see [frontend development](../../../docs/developers/frontend.md).
For a complete IPK, see [packaging](../../../docs/developers/packaging.md#webos).

## Local development

```sh
npm ci --prefix crates/web/prysm-webos
npm run dev --prefix crates/web/prysm-webos
```

Open `http://127.0.0.1:5174`. Without `PalmServiceBridge`, a development-only Luna
simulation supplies settings/status; reload resets it. It never captures video or
contacts a TV. Use arrows, Enter and Escape to exercise remote navigation.

| Source | Responsibility |
| --- | --- |
| `src/App.tsx`, `src/components/` | Screens and dialogs |
| `src/hooks/useCaptureService.ts` | Service state, drafts, commands and subscriptions |
| `src/hooks/useRemoteNavigation.ts` | Focus, arrows and dialog navigation |
| `src/service.ts` | Scoped Luna requests, timeouts and cancellation |
| `src/capturePresentation.ts` | Status text from observed service state |

Edits stay local until Save/enable. Status updates and reconnects must preserve
unsaved edits and focus. Keep visible focus, pointer support and text-cursor
movement; arrows must not silently change settings.

## Build and test

```sh
npm test --prefix crates/web/prysm-webos
```

Tests type-check, build and exercise the production bundle. Vite emits relative
URLs and one classic script for packaged `file:` loading, targeting Chromium 53
syntax without runtime polyfills. That target does not qualify TV hardware
or every DOM/CSS feature. Keep new dependencies within the browser target.

## TV layout

Keep status and settings as the two main groups. At 1280×720, captions are 20 px,
controls at least 56 px high and content within a 20 px safe area. Scale to 1080p
without moving controls outside the safe area. The IPK stages only built assets;
service manifests and checks live in [packaging/webos](../../../packaging/webos/README.md).
