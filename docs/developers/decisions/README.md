# Technical decisions

| Choice | Why |
| --- | --- |
| [One runtime, plain blocking adapters](2026-11-12-one-runtime-plain-adapters.md) | One place for concurrency and shutdown order; a new source or output is one trait. |
| [Settings file for run-time settings, command line for process options](2026-11-03-settings-ownership.md) | One owner per value; neither overrides the other. |
| Ordered commands, replaceable frames | Keep commands without letting video latency build up. |
| Close the source, then idle the output, on every exit | Late frames cannot undo the requested state; idle is the failure compensation. |
| Demand-driven, bounded preview and colour streams | Observation cannot stall lighting. |
| Supervised TV worker process | A hung vendor capture call is recovered only by killing its process. |
| [Two TV packages, explicit stream encoding](2026-11-12-two-tv-packages.md) | No in-app mode switch; an encoder failure is reported, not hidden by a fallback. |
| Portal on the LAN without a password; foreign origins refused | Simple LAN use; protects against other web pages, not against the network ([access](../../../v1/surfaces/web-portal/HTTP-CONTRACT.md#access)). |

Earlier choices that no longer apply: the Host allowlist (`PRYSM_ALLOWED_HOSTS`),
operation IDs with a confirmation window, and opt-in hardware H.264 with LZ4
as the default.

Implementation: [architecture](../architecture.md),
[portal API](../control-api.md), [LG TV](../webos.md).
