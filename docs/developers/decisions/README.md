# Technical decisions

This index records the rationale behind current boundaries. These are summaries
of the existing implementation, not newly adopted changes. Add a dated decision
record here when a future change needs alternatives and tradeoffs preserved.

| Decision | Reason and consequence | Maintained detail |
| --- | --- | --- |
| One host executable, embedded web UI and one TOML file | UI/API share the same listener and policy; configuration and capture lifecycle have one owner. No separate production frontend service or preferences store. | [Control API](../control-api.md) |
| Ordered lifecycle, bounded best-effort frames | Start/stop/configuration must finish in order; live frames can be dropped rather than building latency under load. Each session owns an Arbiter thread. | [Architecture](../architecture.md) |
| Drain before changing WLED state | Accepted DDP output must finish before Off/effects, avoiding a late packet overriding the user's choice. Failed starts compensate where possible. | [Lighting transitions](../control-api.md#lighting-transitions) |
| Native preview on the OS main thread | macOS windowing requires it. HTTP/control runs on a background runtime; closing the window shuts down the application. | [Native output](../architecture.md#native-output) |
| Validated contracts outside dependency-free core | Disk, HTTP and generated TypeScript agree on configuration; processing/color primitives do not acquire application dependencies. | [Guidelines](../guidelines.md) |
| A supervised TV worker per receiver session | Vendor constructors/cleanup can fail or hang. Process exit reclaims native state and the supervisor bounds recovery. | [webOS lifecycle](../webos.md), [cleanup evidence](../../../research/webos-capture/2026-11-01-lifecycle/cleanup-diagnosis.md) |
| Hardware H.264 stays opt-in | Vendor availability and measured performance are platform-specific; build capability does not prove runtime support. | [Encoder contract](../hardware-h264.md), [qualification history](../../../research/webos-capture/hardware-h264-qualification.md) |

Experimental alternatives belong in [research](../../../research/README.md) until
adopted. A decision record should state context, decision, rejected alternatives,
consequences and the implementation/evidence links that justify it.
