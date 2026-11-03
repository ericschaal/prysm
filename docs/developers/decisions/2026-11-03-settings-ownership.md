# Settings and application ownership

Date: 2026-11-03

## Decision

Process options and application settings are separate, with no override relationship.
Clap reads listening sockets, allowed hosts, receiver binding, metrics and the
settings-file location on every startup. Logging reads `RUST_LOG`. Explicit
startup defaults are identical for native and container processes.

One typed JSON document owns browser-editable input, output/LEDs, processing and
selected color source. Missing files use defaults and open the same settings form
as a short first-run wizard. Power intent is never persisted; startup is idle.

The settings store owns validation, revision checks and atomic writes. It updates
memory only after the write succeeds. One controller owns that store and the
configured runtime and serializes accepted mutations. HTTP handles transport;
application startup composes HTTP, preview, controller and their shutdown.
The runtime owns capture/lighting ordering and recoverable failures.

The browser uses a complete typed PUT with an expected revision. The draft model
converts form text to typed settings; the form hook preserves drafts and handles
save conflicts. The wizard uses the same fields and save operation.

## Consequences

No generic override resolver, arbitrary JSON-path merging, provenance, locked
fields, network-restart status, transaction-phase wrappers, separate setup API
or test-only production behavior is needed. Deployment values cannot enter the
settings document. The generic image works with normal ports, environment,
numeric users and storage mounts, including on TrueNAS.
