# Processing flow

The Tokio dispatcher reads capture frames; one dedicated worker serializes controls
and processing. The mailbox retains one pending frame, a newest-frame watermark
and a sealed-session fence. Commands take priority over pending frames.

Processing waits for configuration. The worker retains configuration and revision
together; applied revision advances only when a frame is processed, not delivered.
A new epoch resets processor history; live configuration preserves unrelated history.

After capture cleanup, a barrier waits for in-flight processing, seals the old
session and discards its pending frames. It does not await output device writes.
Shutdown discards pending work and joins the worker. Cancellation also joins,
synchronously as a fallback. Input and worker failures are both retained; if either
requires recovery, the combined result does too. Otherwise the input category wins.

## Preview

`flow.preview.subscribe().await` returns a `PreviewLease`. Subscribing does not
start capture or change processing/output state. It is valid before the owner
runs; the stream waits for a processed frame.

The worker publishes one coherent source image, detected viewport, processed
colors, frame stamp and settings revision. Source pixels are shared, not copied.
Consumers independently resize/encode the image; processing never waits for a
viewer. Slow consumers receive the latest snapshot, skipping intermediate frames.
A new subscriber receives the current snapshot if another lease retained it.

The tap retains at most one snapshot while there is demand. Dropping the final
lease clears it; a processing pass racing that drop cannot restore retention.
An in-flight pass may briefly retain its shared source frame. Consumers own any
snapshots they have already taken and must bound their own retention.

The publisher belongs to the flow owner and then its worker. Shutdown, failure,
panic, cancellation, or dropping an unstarted owner clears the retained snapshot
and ends existing streams. Later subscriptions return owner-closed unavailability.

A session barrier fences further processing for that session but does not erase
the last diagnostic image. Consumers must label it stale using capture/output
state. Preview is not evidence of output admission or device delivery: its colors
come from processing, and an output may hold/discard that frame independently.
