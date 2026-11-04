# Direct WebSocket capture on webOS

On November 4, 2026, a staged native TV worker sent hardware H.264 to the same RP5, Prysm decoder, processing pipeline and WLED renderer over three paths. The RP5 accepted WebSockets directly on its HTTP router. No TCP relay ran between the upgrade and the decoder. The raw TCP listener was available for the baseline throughout the experiment.

## Results

**Direct WS met the 8 ms added-p95 criterion at the DDP output. WSS through Caddy missed it on both runs.** All three transports sustained 30 decoded FPS. The comparison covers the measured TV, RP5, controller, network and capture settings.

| Capture path | Native capture to DDP, p95 | Added p95 vs TCP | Decoded gaps over 50 ms across ten minutes |
| --- | ---: | ---: | ---: |
| Raw TCP | 23.08 ms | Baseline | 0 |
| Direct WS | 22.98 ms | −0.10 ms | 0 |
| Caddy WSS, two trials | 59.38 / 57.94 ms | +36.29 / +34.86 ms | 73 + 60 |

Each timed trial matched all 227 native transitions to a DDP transition. No measured run had a decoded-frame gap above 100 ms. The largest sampled clock-offset half-interval was 0.21 ms. Raw TCP and direct WS each had one native-marker latency trial. Caddy had two. All paths had two five-minute FPS trials.

WLED's own readback placed p95 at 68.6 ms for TCP and 69.5 ms for direct WS, with all 227 transitions observed in each. The second Caddy trial reached 79.2 ms across 220 of 227 transitions. The first preview connection stopped delivering samples partway through that trial sequence, so its first Caddy result covers only 131 transitions. Controller updates arrived at a median interval of 41.7 ms.

The output threshold matters with smoothing enabled. The first change of at least 12 green levels had a p95 of 23.04 ms on TCP, 22.97 ms on direct WS, and 38.34 / 31.78 ms through Caddy. On some Caddy dimming transitions the first packet changed the output but the following packet crossed the threshold. The test measures the entire WSS route, including TLS, the proxy and the additional network hop. It does not isolate the contribution of each.

One phone recording overlapped the TCP run. Its slow-motion section reproduced two known clip intervals at an effective rate of approximately 239–240 camera frames per real second despite its 30 FPS playback timeline. Three interior transitions measured about 60–63 ms from screen to wall at local half brightness. There is no comparable physical p95 for the other paths.

The installed TV sender and RP5 service were restored. The saved settings matched their initial values, the pipeline was idle, and WLED was off. [Run measurements](measurements.json) · [Individual transitions](transitions.csv)

## Measurement

The TV played a looping 13-second clip with ten alternating brightness transitions. Capture used the blended native tap, 640×280 resolution, 30 FPS and a 2 Mbit/s hardware encoder target. The RP5 used a temporary copy of the installed lighting settings, including 0.1 s of smoothing. The production configuration uses 1280×720.

Each transport had a one-minute smoke run. The five-minute runs followed TCP, direct WS, Caddy WSS, Caddy WSS, direct WS, TCP. Startup and teardown were outside the measured FPS intervals. The native worker's TLS implementation validated the Caddy certificate against the TV trust store. Every run used the same WLED controller and LED layout.

For automated latency, the TV worker timestamped a bright/dim transition in the native frame before encoding. On the RP5, a kernel packet filter selected only outbound IPv4 DDP packets to the WLED controller on UDP 4048. It retained the DDP sequence and first pixel along with the kernel timestamp. A four-timestamp UDP exchange between the TV and RP5 estimated their clock offset throughout the runs. The offset estimate uses the narrowest of twelve intervals in each burst and interpolates between bursts. Both binaries had identical marker instrumentation during the final Caddy, direct WS and TCP comparison.

The primary endpoint was the first DDP packet whose green channel crossed 120 in the expected direction. Both bright and dim transitions counted. A separate check used the first movement of at least 12 green levels from the previous plateau. The full-field clip made one native input pixel and one DDP output pixel sufficient. The analysis trimmed startup and teardown by the same rule for every path and counted unmatched transitions explicitly.

WLED 16.0.1's DDP handler supplies no per-packet acknowledgement. The optional LED preview reports sampled controller pixels, at a nominal 40 ms interval. We subscribed to that feed independently and timestamped when the first matching changed pixel was observable. Those values include sampling and return-path delay. The precise endpoint is packet egress at the RP5. [WLED DDP handler](https://github.com/wled/WLED/blob/v16.0.1/wled00/e131.cpp#L17-L79) · [WLED live preview](https://github.com/wled/WLED/blob/v16.0.1/wled00/ws.cpp#L173-L240)

## Direct TLS follow-up

A 90-second direct WS run and a 90-second direct WSS run each yielded 64 matched native-to-DDP transitions. Added TLS terminated inside the RP5 benchmark receiver, with the same worker, router, decoder, lighting settings and network destination as direct WS. The worker verified the RP5 IP against a temporary CA scoped to its process.

| Direct path | Capture to DDP p95 | Decoded FPS | Gaps over 50 ms |
| --- | ---: | ---: | ---: |
| WS | 23.10 ms | 30.03 | 0 |
| WSS | 22.95 ms | 30.03 | 0 |

Direct TLS did not reproduce the earlier 35–36 ms regression. These runs narrow the investigation to the proxied path and its network hop. The source switched to WLED during the reverse-order repeat, so the comparison uses only the uninterrupted first pair. The installed RP5 service and TV sender were restored. [Direct TLS measurements](direct-tls.json) · [Incremental benchmark patch](direct-tls.patch)

## Reproduction

The experimental patch applies to commit `56348837dc8799f64ade866ddbc0b245faa43493`. It includes the direct capture route, a staged WebSocket sender, the native timing marker and a binary-message framing test. Build the TCP worker with `hardware-h264` and the WS/WSS worker with `hardware-h264,bench-ws`. Both use the existing worker configuration and capture lock. The experimental WS worker reads its destination from `PRYSM_BENCH_WS`. The patch targets a fixed test configuration and one sender per run. Production discovery, reconnection and sender arbitration were outside this benchmark. [Benchmark patch](benchmark.patch) · [Build provenance](provenance.json) · [Prysm monitoring guide](../../../docs/developers/monitoring.md)

Machine-specific runners, raw metrics, timestamp traces and service restoration logs are retained locally under `work/ws-benchmark/`. The experiment used temporary executables and a scoped Luna capture role. No Caddy configuration change was required.
