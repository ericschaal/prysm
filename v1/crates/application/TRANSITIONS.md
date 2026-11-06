# Lighting lifecycle transitions

The application coordinator owns command acceptance, user intent, policy and
publication. The private `session::Lifecycle` owns the complete lighting protocol:
phase, session/epoch identity, acquired resources, device calls and cleanup.
It is driven by the existing application task; it has no task or mailbox of its own.

## Phases and ownership

| Phase | Retained data | Next work |
| --- | --- | --- |
| Idle | Last acknowledged idle mode; optional completed-session failure | None until a command or shutdown |
| Starting | Session, latest capture availability, target and preparation stage | Configure processing/output, hold black, open capture; idle targets configure output then finish |
| Active | Session, latest capture availability and last acknowledged admission | Hold/admit when the source epoch changes |
| Stopping | Session resources, requested final mode, cleanup stage and latest failure | Hold black → stop capture → settle racing open → flow barrier → output finish |
| Cleanup blocked | The same cleanup obligation and its failure | None until explicit Off (or host shutdown) retries |

Capture/output resource flags represent independent cleanup obligations, not
lifecycle phases. A new Start inherits an existing Native output obligation.
Preparing a device may acquire resources even if its reply fails. Only cleanup
acknowledgments discharge those obligations. Output mode acknowledgments do not
invent device-confirmed display state.

## Requests and external events

| Current phase | Input | Transition / result |
| --- | --- | --- |
| Idle | Processed | Allocate a monotonically increasing session; begin processing configuration |
| Idle | Off / Native | Configure output and apply the requested mode; Off without output settings completes immediately |
| Starting | Another Processed | Reject as conflicting |
| Starting, before output preparation | Native | Reject as conflicting; it cannot truthfully apply Native without configuring output |
| Starting (output prepared or pending) / Active | Native | Enter Stopping; retain any in-flight write; supersede the old accepted operation |
| Starting / Active | Off | Enter Stopping; retain any in-flight write; supersede the old accepted operation |
| Active | Processed | Complete immediately; preserve the active session |
| Stopping | Another command | Reject as conflicting; retained request IDs still deduplicate |
| Cleanup blocked | Processed / Native | Reject with CleanupPending |
| Cleanup blocked | Off | Clear the previous diagnostic and retry cleanup from hold-black |
| Starting / Active | Matching, strictly newer capture epoch | Record availability; Active reconciles hold/admission after any outstanding call |
| Any | Old session or non-increasing epoch | No lifecycle transition |
| Starting / Active | Component failure | Enter Stopping toward Off, retaining the failure |
| Stopping | Component failure | Keep cleanup owned, update the diagnostic, request Off |
| Cleanup blocked | Repeated component failure | Update diagnostic; do not retry automatically |
| Any | Host shutdown | Request Off; retain work until cleanup completes; blocked cleanup may be retried |
| Idle with Native output | Shutdown / component failure | Finish output Off before considering the lifecycle idle/clean |
| Any | Client handles disappear | No lifecycle transition |

## Device completions

| Completion | Transition / outcome |
| --- | --- |
| Processing configuration succeeds | Configure output |
| Output configuration succeeds for Processed | Hold black at epoch zero |
| Output configuration succeeds for Off / Native | Finish output in the requested idle mode |
| Initial hold succeeds | Open capture |
| Capture start succeeds while Starting | Active; report resource readiness, independently of first-frame readiness |
| Active hold/admit succeeds | Record the acknowledged epoch/action; reconcile against the latest availability |
| Preparation/admission fails | Cleanup toward Off, retaining the failure |
| An earlier preparation/write completes after Stop | Keep the Stop sequence; never resume preparation |
| Stop interrupts an unpolled capture start | Discard the unissued call; no capture cleanup obligation |
| Stop interrupts a polled capture start | Retain its reply while requesting capture stop; settle both before the barrier |
| Cleanup hold succeeds or fails | Continue capture cleanup toward the requested idle mode; retain any hold failure |
| Capture stop succeeds | Await any racing open reply, then cross the processing barrier |
| Racing open reply fails during Stop | Retain capture-stop acknowledgment as the authority for cleanup; cancellation may normally fail the open |
| Barrier succeeds | Finish output in the requested mode |
| Capture stop / barrier / finish fails | Cleanup blocked; report failure, retain resources, forbid restart |
| Finish succeeds for an obsolete final mode | Issue finish for the latest requested mode before completing |
| Finish succeeds for the requested mode | Idle; report cleanup completion, with any retained failure |

## Review invariants

- Session phases are private and mutually exclusive; the coordinator cannot edit them.
- One ordinary device call is retained at a time. Only a racing capture open may
  coexist with cleanup. Losing a select branch never drops either device future.
- Transition decisions are synchronous. The device-call implementation only
  translates domain actions into port calls and checks acknowledgments.
- A blocked phase has no automatic action. A closed input never causes a busy loop.
- Application snapshots are projections of owned state, not a second mutable
  lifecycle model. Operation retention and publication cannot mutate the lifecycle.
- Component subscriptions recover gaps; only connection failures trigger cleanup.
- Failure categories survive cleanup and operation publication. `RecoveryRequired`
  needs host/device recovery; an explicit Off alone cannot repair that owner.
- The host retains the owner future and emergency controls through stuck cleanup.
  There is no async Drop cleanup or detached retry task.
