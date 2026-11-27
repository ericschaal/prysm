# Frontend state and explicit wiring

Each frontend host creates an `ApplicationRuntime` with one injected
`ApplicationClient`. The runtime publishes immutable connection snapshots for
React through `useSyncExternalStore`. Rust owns the lighting state.

| File | Responsibility |
| --- | --- |
| [Client port](packages/application-client/index.ts) | Generated wire types, `observe`/`setTarget`/`replaceSettings`/`tryProcessing`, `ApplicationError` |
| [HTTP client](packages/application-client/http.ts) | Fetch transport and SSE decoding for the [portal API](../crates/surfaces/web-portal/HTTP-CONTRACT.md); `httpPortalClient` for the preview stream |
| [Runtime](packages/application-state/index.ts) | Connection snapshots, observation leases, reconnect, and revision-checked commands |
| [Connection hook](packages/application-react/index.tsx) | React binding (`useSyncExternalStore`) of the portal and the TV app; `WebPortal` and the TV's `usePortal` each hold an observation lease |

## Commands

```ts
await application.commands.setTarget('screen_sync');
const revision = await application.commands.configureOutput(snapshot.revision, wled);
```

A command resolves when the engine has finished it and rejects with the
server's `Failure` (`invalid` with `field`, `stale_revision` with `current`,
`faulted`, `adapter`, ...); the store records neither, so each caller shows
the result of its own commands. Observed state changes only when the stream
delivers a new state.

`configureCapture`/`configureOutput`/`configureProcessing` copy the observed
settings, replace one section and send `expected` (`configureSetup` replaces
capture and output together, for the portal's guided setup). If the observed revision is
not `expected`, the command fails locally with `stale_revision` and nothing is
sent. The portal's forms pass the current revision after checking that their own
section has not changed since the draft started, so saving one section never
conflicts with another; see [saving settings](README.md#saving-settings).

## Observation

A host may pass a state the page came with
(`createApplicationRuntime(client, embeddedState())`): the store starts
`connected` with it, so the first render shows it, and it stays until the
stream's first state replaces it. So the portal starts connected before the
stream opens, and the state shown may be a moment old until then; commands
are guarded by revision as always. The first lease opens observation. Each state replaces the snapshot. On loss
the runtime retains the last state and reconnects after a delay (250 ms up to
5 s) for an unavailable portal. Other failures stop the stream. The final
release aborts observation and clears the snapshot. A stream acquired after
release is closed immediately. Releasing
observation never commands lighting.

## Local UI state

Form drafts, field errors, focus, clients, abort signals and preview frames
stay out of the store (Home's preview and the video source's status hold their own `PreviewClient` streams); each form shows the result of its own commands. The HTTP
client holds only the portal's base URL; the portal needs no credentials.
