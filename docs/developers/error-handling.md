# Error handling

- Capture uses non-exhaustive `CaptureError`; match variants, not strings.
  Keep native/dependency causes, including AVFoundation `NSError`.
- Processing streams preserve source errors. Processing itself stays infallible;
  unsupported/empty frames produce black colors.
- Rendering returns `DDPError`. Application orchestration adds context with `anyhow`.

Preserve sources with `#[source]`, `#[from]` and `Context`. Report terminal failures
once at the application boundary. Frame replacement must never discard a terminal
error. EOF/cancellation are successful completion. Preview failures stay local.
Recover only where the cause is understood; do not add blanket reconnects.

Validate frame geometry/payload and color-strip bounds. Panics are for internal
invariants, not device/I/O failures. On contract changes, test domain variants,
inspectable causes and successful EOF/cancellation through callers.
