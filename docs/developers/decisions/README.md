# Technical decisions

These decisions explain the current boundaries. Keep detailed rationale in a
dated record when a change needs alternatives and tradeoffs preserved.

| Decision | Reason and consequence | Maintained detail |
| --- | --- | --- |
| One host executable, embedded web UI and one saved JSON layer | UI/API share the same listener and policy. CLI/environment configure the process on every startup, separately from browser settings. Missing required settings open the same Settings form; no database or setup marker. | [Control API](../control-api.md) |
| Ordered lifecycle, bounded best-effort frames | Start/stop/configuration must finish in order; live frames can be dropped rather than building latency under load. Each run owns a current-thread Tokio processing worker and tracked output consumer. | [Architecture](../architecture.md) |
| Drain before changing WLED state | Accepted DDP output must finish before Off/effects, avoiding a late packet overriding the user's choice. Failed starts compensate where possible. | [Lighting transitions](../control-api.md#lighting-transitions) |
| Demand-driven browser preview | One bounded encoder observes paired source/crop/colors independently of lighting. Closing preview does not stop capture. No native window or preview output variant. | [Preview observation](../architecture.md#preview-observation) |
| Validated contracts separate from processing primitives | Disk, HTTP and generated TypeScript agree on configuration; processing/color primitives do not acquire application dependencies. | [Guidelines](../guidelines.md) |
| A supervised TV worker per receiver session | Vendor constructors/cleanup can fail or hang. Process exit reclaims native state and the supervisor bounds recovery. | [webOS lifecycle](../webos.md) |
| Hardware H.264 stays opt-in | Vendor availability and measured performance are platform-specific; build capability does not prove runtime support. | [Encoder contract](../hardware-h264.md) |

Experimental alternatives belong in [research](../../../research/README.md) until
adopted. A decision record should state context, decision, rejected alternatives,
consequences and the implementation/evidence links that justify it.

[Settings and application ownership](2026-11-03-settings-ownership.md) records the
encapsulation rules for browser configuration and application control.
