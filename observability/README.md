# Centralized Prysm monitoring

Prometheus and Grafana run on your computer. Prysm exposes an opt-in metrics
endpoint with its process and machine statistics. The supervised TV worker exposes
its encoder, capture, process and machine metrics on port 9101 while running,
when **Prometheus metrics** is checked and saved in the TV app (defaults off).
Prometheus pulls both endpoints every five seconds; egui is not involved in collection.

## Start the nodes

On your computer, run your normal Prysm configuration with a metrics listener:

```sh
cargo run --release -p prysm -- \
  --config prysm-webos.toml --metrics-listen 0.0.0.0:9102
```

Add `--features desktop` before `--` if your configuration uses egui. The metrics
listener is separate from the REST API and does not change your saved TOML.
Enable Screen sync as usual to open the video receiver.

Omit `--metrics-listen` to disable all instrumentation and the exporter thread.
Both nodes require opting in. A metrics bind or exporter failure is reported without
interrupting capture or lighting. Metrics are not written to a local log file.

The endpoint has no authentication. Bind to an appropriate trusted interface
and restrict ports 9101 (TV) and 9102 (host) to your monitoring computer. Do not expose it to
the internet. The Compose web ports below bind only to your computer's loopback.

## Start the central stack

Install Docker with its Compose plugin (Docker Desktop includes it).
[prometheus/targets.json](prometheus/targets.json) contains the live host and TV exporters.
`host.docker.internal:9102` points at Prysm on your computer; the Compose
host-gateway alias also supports Linux hosts. Set `192.168.18.10:9101` to your TV address.
The TV endpoint is available only while the supervised capture worker is running.

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
panels retain requested codec, capture mode, width and height filters for
live and historical TV series. Machine/pipeline overview panels remain visible so that
host-side bottlenecks stay in context.

[Prometheus targets](http://localhost:9090/targets) shows scrape errors. Check the
endpoint directly if Prysm is DOWN:

```sh
curl http://localhost:9102/metrics
curl http://192.168.18.10:9101/metrics
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
thread. Process CPU includes every thread of the exporting Prysm process and uses
cores (1.0 means one fully used core). Machine CPU is normalized across all cores. Interfaces are
reported separately; bridged/VPN traffic can appear on multiple interfaces and
must not be summed blindly. Missing/unsupported machine statistics are indicated
by the collector-availability gauge. Hardware statistics depend on the host platform.

Prometheus histograms estimate percentiles from buckets; never average p95/p99
values between nodes or intervals. Socket acceptance is not remote delivery.
Receiver frame gaps show output continuity, not capture-to-host or screen-to-light
latency. No clock synchronization, frame acknowledgements, video quality scoring,
or replay recorder is introduced by this monitoring setup.

The exporter uses maintained `prometheus-client`, `tiny_http`, and `sysinfo`
crates. It contains no unsafe Rust. Bounded label families and histograms (which use brief library locks)
are updated in process; sampling, encoding exposition, and HTTP I/O run on a
separate thread. Prometheus/Grafana outages therefore do not block capture.
