# Centralized Prysm monitoring

Prometheus and Grafana run on your computer. The TV capturer and Prysm expose
opt-in metrics endpoints, including their process and machine statistics.
Prometheus pulls them every five seconds. egui is not involved in collection.
No metrics service is required on the TV beyond the capturer itself.

## Start the nodes

On your computer, run your normal Prysm configuration with a metrics listener:

```sh
cargo run --release -p prysm -- \
  --config prysm-webos.toml --metrics-listen 0.0.0.0:9102
```

Add `--features desktop` before `--` if your configuration uses egui. The metrics
listener is separate from the REST API and does not change your saved TOML.
Enable Screen sync as usual to open the video receiver.

Rebuild and copy the TV distribution using the
[webOS build/deployment instructions](../crates/capture/webos-capture/README.md).
On the TV:

```sh
cd /media/developer/prysm-capture
./prysm-webos-capture COMPUTER_IP:19400 \
  --source both --compression lz4 --metrics-listen 0.0.0.0:9101
```

Omit `--metrics-listen` to disable all instrumentation and the exporter thread.
Collection is opt-in. A metrics bind or exporter failure is reported without
interrupting capture or lighting. Metrics are not written to a local log file.

The endpoints have no authentication. Bind to an appropriate trusted interface
and restrict ports 9101/9102 to your monitoring computer. Do not expose them to
the internet. The Compose web ports below bind only to your computer's loopback.

## Start the central stack

Install Docker with its Compose plugin (Docker Desktop includes it). In
[prometheus/targets.json](prometheus/targets.json), replace `192.0.2.10` with your
TV's actual IP. That reserved example address intentionally starts as DOWN.
`host.docker.internal:9102` points at the Prysm process on your computer; the
Compose host-gateway alias also supports Linux hosts.

From the repository root:

```sh
docker compose -f observability/compose.yaml up -d
```

Open [Grafana](http://localhost:3000/d/prysm-overview). Initial login is
`admin` / `admin`; Grafana prompts for a new password. To set the initial password
explicitly, set `GRAFANA_ADMIN_PASSWORD` before the first `up`. This only initializes
a new Grafana data volume; it does not reset an existing account.

The datasource and **Prysm — systems and stream** dashboard are provisioned from
this directory. Select nodes at the top. Capture configuration and codec-specific
panels also support requested codec, composition format, capture mode, width and height filters. Machine/pipeline
overview panels remain visible so that host-side bottlenecks stay in context.

[Prometheus targets](http://localhost:9090/targets) shows scrape errors. Check the
endpoints directly if a node is DOWN:

```sh
curl http://TV_IP:9101/metrics
curl http://localhost:9102/metrics
```

Changing `targets.json` is picked up automatically (within five seconds). Add
more target entries with distinct `node` labels to monitor additional machines.
If Prysm runs on another machine, replace the computer target with that IP.
Prometheus configuration changes require restarting its container:

```sh
docker compose -f observability/compose.yaml restart prometheus
```

Prometheus keeps 15 days of data. Named volumes retain metrics and Grafana state
across container replacement and `docker compose down`. `down -v` deletes those
volumes and their history. Source-controlled dashboard edits are authoritative;
Grafana UI changes do not overwrite the provisioned JSON file.

The [instrumentation catalog](CATALOG.md) defines every emitter, operation, outcome,
measurement boundary, unit and label. Dashboard legends identify the node and
owning component; actual wire codec/content are separate from requested settings.

## Compare runs

Use the same release build, capture dimensions, FPS, source, backend and playback
sequence. Let CPU sampling and rate calculations warm up, then compare at least
one minute of static menus, motion, grain, dark scenes and scene changes. Compare
encoding bytes/frame alongside actual frame rates and failure counts: dropping
frames is not a compression gain. UI failures can change the emitted format;
inspect the format and backend panels before comparing runs.

The system collector uses `sysinfo`, refreshed once a second on the exporter
thread. CPU process use includes native worker threads and uses cores (1.0 means
one fully used core). Machine CPU is normalized across all cores. Interfaces are
reported separately; bridged/VPN traffic can appear on multiple interfaces and
must not be summed blindly. Missing/unsupported machine statistics are indicated
by the collector-availability gauge. webOS hardware values need TV validation.

Prometheus histograms estimate percentiles from buckets; never average p95/p99
values between nodes or intervals. Socket acceptance is not remote delivery.
Receiver frame gaps show output continuity, not capture-to-host or screen-to-light
latency. No clock synchronization, frame acknowledgements, video quality scoring,
or replay recorder is introduced by this monitoring setup.

The exporter uses maintained `prometheus-client`, `tiny_http`, and `sysinfo`
crates. It contains no unsafe Rust. Bounded label families and histograms (which use brief library locks)
are updated in process; sampling, encoding exposition, and HTTP I/O run on a
separate thread. Prometheus/Grafana outages therefore do not block capture.

## Compare RGB and blended NV12

Add `--composition-format nv12` to the TV command for NV12 after composition,
or use `--composition-format rgb` for the baseline (default). Keep the clip,
resolution and frame rate fixed. Select the requested composition format in
Grafana, inspect actual `Encoded` / `SocketAccepted` format and content, and
compare bandwidth, frame gaps and `Composer / ConvertToNv12` plus total loop
time. RGB→NV12 subsamples chroma: LZ4 round trips exactly, but NV12 output is not
pixel-identical to the RGB baseline. Sender restart resets counters; avoid rate
windows spanning the switch, and compare separate steady-state time ranges.
