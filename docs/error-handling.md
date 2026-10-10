# Error handling

Subsystems own their errors. Use a typed error when callers need to distinguish
failures; keep concrete dependency errors when they already provide the needed
contract. Do not introduce a workspace-wide error enum.

- `prysm-capture` exposes `CaptureError` for missing devices, disconnects,
  unsupported formats, unexpected worker exits and backend failures. Backend
  failures retain their original causes through `anyhow`; match domain variants
  instead of error message text. The enum is non-exhaustive so callers must handle
  future variants.
- `prysm-processor::into_stream` preserves the source's error type. Processing
  remains infallible: zero-sized and MJPEG frames log an error and produce black
  edge colors. Change this only as an explicit behavior/API decision.
- Renderers retain dependency errors (`DDPError`, `eframe::Error`).
- Binaries and application orchestration use `anyhow`, adding context about the
  operation, device, file or destination. Convert typed errors here, not in
  processing adapters. `prysm-core` stays dependency-free.

Preserve sources with `#[source]`, `#[from]` and `anyhow::Context`; avoid converting
errors into strings. macOS device input and runtime notification failures retain
`NSError` when supplied by AVFoundation. Notifications without a native error
retain their notification details. Desktop
errors retain `eframe::Error` until the application boundary, where they become
text because eframe errors may not be `Send + Sync`, as required by `anyhow`.

Cancellation and successful file EOF are normal completion. A terminal source
failure must reach the consumer, even if frames were skipped. Recover locally
where the cause is understood (for example, Linux capture timeouts); do not label
every backend failure retryable or reconnect automatically.

Report terminal errors once at the application boundary with the full chain.
Libraries propagate terminal failures and may log recoverable frame drops.
Panics remain for violated internal invariants, not expected device or I/O errors.

When changing an error contract, update its callers, examples and tests. Verify
that domain variants survive propagation, causes remain inspectable, and EOF and
cancellation remain successful.
