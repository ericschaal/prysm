# Video device capture transitions

One host-owned task sequences a native worker, control replies and one pending
frame. Native code owns opening, readiness, cancellation and verified release.
The adapter owns session seals, availability epochs and observations. File playback
is explicitly unavailable in this milestone; it needs a separate process owner.

| State | Input | Transition and result |
| --- | --- | --- |
| Idle | Valid Start with a fresh session | Retain native worker; publish Starting; retain Start reply |
| Starting | Native ready | Reply CaptureStarted; enter Waiting at epoch 0 |
| Starting | Readiness channel closes | Remain Starting, stop polling readiness, retain Start reply until worker completion or Stop |
| Waiting | Valid frame | Advance epoch; publish Available and matching stamped frame |
| Available | Valid frame | Replace pending frame; advance frame sequence and refresh silence deadline |
| Available | Delivery silence | Clear pending frame; advance epoch; publish Waiting |
| Waiting or Available | Frame source closes | Retain operational failure; cancel native work and enter Stopping until completion |
| Starting, Waiting or Available | Stop | Seal session, clear frame, cancel native work; enter Stopping |
| Stopping | Late ready or frame | Never publish it or resume Start |
| Stopping | Proven native release and join | Reply CaptureStopped; publish Idle, or Failed preserving an operational error with cleanup_complete=true |
| Starting, Waiting or Available | Backend exit | Retain cause; publish Failed with proven cleanup status |
| Any owned state | Unproven release or panic | Block reuse; publish Failed with RecoveryRequired |
| Stopping | Another Stop | Return CleanupPending; original cleanup remains owned |
| Idle | Stop for a released session | Idempotent success; never open anything |
| Owned/blocked session | Another Start | Reject; never replace a resource obligation |
| Any state | Host shutdown | Cancel owned work, await release, then close frame/observation ports |

The session selects one event, then applies its complete transition synchronously.
Selection priority is native completion, readiness, expired silence, then frame
updates. Completion prevents late readiness or delivery; expired silence precedes
a queued frame so a delayed owner poll cannot conceal an epoch change. Only
Available owns a silence deadline. Starting with closed readiness and Stopping
wait only for native completion; the owner can still dispatch Stop or shutdown.

Epoch zero is initial waiting. Every waiting/resumption discontinuity advances
the epoch; a held epoch is never reused. Frame sequence increases within the
session. Foreign-instance commands and Stop for a different active session are
rejected. A stop in Idle can seal a not-yet-started session, preventing a late Start.

Readiness means native capture started, not first-frame arrival. Frames received
before readiness stay bounded in the native latest slot. Stop can interrupt
opening at the adapter boundary; it cannot forcibly interrupt a native API call.
An interrupted Start gets an error, but cleanup remains owned until completion.

Delivery silence means no valid frame delivered for max(2 seconds, 3 frame periods).
It is not physical signal detection or proof of acquisition age. Black images are
valid frames. Native queues may delay acquisition; epochs describe continuity at
the adapter boundary. Waiting clears the adapter's pending frame, not processing
already in flight; output admission and the flow barrier fence that work.

The persistent FrameSource stays pending while idle, waiting or stopped. Only
owner closure returns None. Commands and observation buffers have capacity 16;
accepted commands survive caller cancellation. Observations remain available
while native open/release is pending. The host must retain the owner through
shutdown; dropping it requests cancellation but cannot certify release.

Normal release after an operational failure permits a fresh session. Worker panic
or failed teardown cannot be retried into success: host/device recovery is needed.
No automatic reopen, watchdog task, synthetic video frame or generic scheduler.
