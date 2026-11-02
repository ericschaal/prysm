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
reloads CSS changes. Screens, dialogs, and state live in `src/App.tsx`;
`src/main.tsx` mounts the application. The dev server
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
