# Technical decisions

| Choice | Why |
| --- | --- |
| One host executable, embedded panel, JSON settings | One owner and access policy; [process options remain separate](2026-11-03-settings-ownership.md). |
| Ordered lifecycle, replaceable frames | Preserve commands without accumulating video latency. |
| Drain before WLED Off/effects | Prevent late packets undoing the requested state. |
| Demand-driven bounded preview | Observation cannot stall lighting. |
| Supervised webOS worker per session | Bound vendor hangs and reclaim native state. |
| Opt-in hardware H.264 | Build support cannot establish TV encoder availability. |

Implementation: [architecture](../architecture.md), [control API](../control-api.md),
[webOS](../webos.md), [hardware encoding](../hardware-h264.md).
