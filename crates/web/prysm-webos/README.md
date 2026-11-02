# webOS TV app

The React + TypeScript TV frontend lives here alongside the [web portal](../prysm-web/control-ui/README.md).
IPK assembly, service manifests and packaging tests remain in
[packaging/webos](../../../packaging/webos/README.md).

## Local development

Use Node.js 24/npm. From the repository root:

```sh
npm ci --prefix crates/web/prysm-webos
npm run dev --prefix crates/web/prysm-webos
```

Open http://127.0.0.1:5174. Vite provides React Fast Refresh for components and
reloads CSS changes. `src/App.tsx` composes the screen from `src/components/`;
`src/main.tsx` mounts it with React Strict Mode. `src/hooks/useCaptureService.ts`
owns the reducer, draft settings, commands and status subscription;
`src/hooks/useRemoteNavigation.ts` owns TV focus, arrow keys and dialog navigation.
`src/service.ts` defines the Luna contracts and scoped request client, including
timeouts and cancellation. `src/capturePresentation.ts` derives the status copy
and ready styling from the observed service state. Receiver edits stay local to
the mounted dialog until applied; configuration persists only on Save or enable.

The dev server
installs an in-memory Luna service simulation when the real `PalmServiceBridge`
is absent. Edit and save settings, enable/disable the sender, inspect diagnostics,
and use the arrow keys, Enter and Escape to exercise remote navigation. Enabling
the simulated sender also simulates a connected receiver and active capture.
Reloading resets the simulation. It never connects to a TV or captures video.
The simulation is removed from production builds.

## Build and test

```sh
npm test --prefix crates/web/prysm-webos
```

This typechecks the strict React/TypeScript, builds with Vite, and runs the UI tests
against the production bundle. The static manifest, icon and CSS live in `public/`.
Vite emits relative asset URLs and one classic IIFE script, so the built app can
load from a packaged `file:` URL without ES module support. The IPK build script
installs the locked dependencies, builds the frontend, and stages only `dist/`.

The JavaScript target is Chromium 53 ([webOS 4.x's engine](https://webostv.developer.lge.com/develop/specifications/web-api-and-web-engine)).
This is a syntax target, not a claim of full support for older TVs' CSS or native
capture drivers. Vite does not polyfill runtime or DOM APIs; application types
are restricted to ES2015 and DOM APIs. See the [package build guide](../../../docs/developers/packaging.md#webos)
for the SDK requirements and full IPK build.

The UI keeps its small existing stylesheet. Tailwind v4 requires Chrome 111+,
so it would raise the browser requirements beyond our conservative TV target
([Tailwind compatibility](https://tailwindcss.com/docs/compatibility)).

## TV layout

The screen keeps sender status and settings as its two main groups, following
[LG's design principles](https://webostv.developer.lge.com/develop/guides/design-principles).
Status appears once, with a short explanation; setting labels and values share
one row, while descriptions use a separate caption style. At 1280×720, captions
are 20 px and controls are at least 56 px high. The layout scales proportionally
to 1920×1080 and leaves all controls and text inside LG's
[20 px safe area](https://webostv.developer.lge.com/develop/guides/overscan).
Focus stays visible for both pointer and
[five-way remote navigation](https://webostv.developer.lge.com/develop/guides/magic-remote).
