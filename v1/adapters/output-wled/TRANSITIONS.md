# WLED output transitions

One host-owned task serializes configuration, HTTP mode changes, DDP sends and
refresh. The control queue has capacity 16; the color handoff retains one newest
candidate. Observation uses an atomic snapshot/subscription and stays available
while a device call is pending. Neither dropping a caller nor a caller timeout
cancels an accepted write. No separate refresh task or deadline scheduler exists.

## Admission and ownership

| Input | Preconditions | Local transition | Work before successful reply |
| --- | --- | --- | --- |
| Configure | Valid unicast address and 1..65535 total LEDs; no unfinished session | Replace device configuration; keep session seal watermark | Create the renderer/socket; no device mode is changed |
| Hold(epoch, colors) | Correct instance; same active session or a fresh session; non-regressing epoch | Close admission, invalidate colors at/before epoch; keep one later candidate | Enter realtime if needed, then send the held colors |
| Admit(epoch) | Correct instance/session; positive epoch newer than any hold fence | Admit exactly this epoch; preserve a candidate from a later epoch | Enter realtime if needed; send matching candidate, otherwise held colors (or same-epoch last colors), otherwise black |
| Offer(frame) | Positive epoch, correct active session, beyond seals/holds, newer than received frame watermark | Replace the candidate only with a newer stamp | Nonblocking enqueue only; never display confirmation |
| Finish(session, mode) | Configured; matching active/finishing session, or non-regressing idle session | Seal session, discard its candidate, disable refresh; retain finishing obligation | Settle prior writes; release realtime, apply Off or Native, then release session/device for reuse |
| Repeated finish | Same last sealed session; no newer active session | Keep admission sealed | Reapply requested idle mode |
| New session | Previous streaming session finished | Claim fresh session; reset per-session frame watermark | Enter through hold/admit |
| Shutdown | Host request | Close admission and command intake | Settle current write, discard queued work, apply Off if needed, release resources |

A future-session offer cannot claim the output. A newer-epoch candidate may be
retained before its availability notification, but is not sent until exact epoch
admission. The received-frame watermark survives taking the slot, so duplicate
frames cannot replay. Configuring another device never erases session seals.
A rejected finish retains its session and device configuration until a same-session
finish retry succeeds. New sessions and reconfiguration cannot bypass cleanup.

## Wire behavior and failures

| Situation | Behavior |
| --- | --- |
| Enter realtime | HTTP `live:false`, then `on:true, lor:0, live:true, tt:0`; then DDP colors |
| Apply Off | HTTP `live:false`, then `on:false, lor:2, tt:0` |
| Apply Native | HTTP `live:false`, then `on:true, lor:2, tt:0` |
| No new frame while held/streaming | Refresh the last successfully sent colors every 500 ms |
| Begin finish or any failed send | Disable refresh before subsequent mode work |
| UDP send failure | Close local admission, retain failure; explicit cleanup remains possible |
| Completed WLED rejection | Retain failure and close admission; later cleanup may retry |
| Connection establishment failure | No HTTP request was sent; close admission and allow cleanup retry |
| Ambiguous HTTP failure (lost/incomplete/malformed response) | Poison the configured owner; reject further mutation/reconfiguration and report cleanup pending |
| Slow HTTP response | Keep the write owned; observe remains responsive; subsequent control work waits |
| Caller timeout/disconnect | Keep the accepted operation; a later Off cannot overtake its write |

The two HTTP requests preserve the existing main-segment release ordering: WLED
may ignore an override change while the previous realtime session is still active.
Mode responses are checked, but all public acknowledgments remain `Sent`.
Observations describe this adapter's admission and acknowledged local operations,
not external WLED UI changes or measured LED display. Native mode leaves the
existing effect/preset selected. The adapter does not overwrite brightness.

The HTTP client has a connection deadline, no request/read deadline, no redirects,
no proxy and no automatic request retries. Host shutdown must retain the owner
future through a stalled write. A poison cannot be cleared by configure, a later
GET or a speculative Off: those do not prove an earlier request cannot apply late.
Host/device recovery is required; owner shutdown returns an error, not OwnerStopped.

## Guarantee boundary

The adapter guarantees no *new local send* of fenced colors after hold/finish,
and no late local mode write after a successful later transition. Ordinary WLED
DDP has no application session/epoch identity or delivery acknowledgment. An
already-sent UDP packet can still be delayed/reordered on the network; held black
and newer epochs cannot claim physical ordering against it. Off/Native use lor=2
to suppress realtime after the final mode applies. No result fabricates display
confirmation. WLED realtime timeout should exceed the 500 ms refresh interval.

Protocol references: [JSON API](https://kno.wled.ge/interfaces/json-api/),
[DDP](https://kno.wled.ge/interfaces/ddp/). Existing renderer and runtime mode
ordering are reused; hardware verification remains necessary.
