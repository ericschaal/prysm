# Monitoring

Prysm exposes optional Prometheus metrics. The included Docker Compose stack runs
Prometheus and Grafana on your computer; browser preview is independent.

## Enable exporters

Start the host with its normal settings and a metrics binding:

```sh
cargo run --release -p prysm -- --config settings.json --metrics-listen 0.0.0.0:9102
```

For the TV, save **Prometheus metrics** in the TV app. Its worker serves port 9101
while running. Both exporters default off. Omitting the host option disables its
instrumentation and exporter thread. Exporter failures are logged without stopping
capture or lighting.

Metrics have no authentication. Restrict ports 9101/9102 to your trusted monitoring
network. They are separate from the panel's listener and access policy.

## Start Prometheus and Grafana

Set host/TV addresses in
[`targets.json`](../../observability/prometheus/targets.json).
`host.docker.internal:9102` reaches Prysm on the Docker host, including Linux via
the Compose host-gateway alias. Replace the example TV address with yours.

```sh
docker compose -f observability/compose.yaml up -d
```

Open [Grafana](http://localhost:3000/d/prysm-overview). The initial login is
`admin` / `admin`; change it when prompted. `GRAFANA_ADMIN_PASSWORD` sets the
initial password for a new volume, not an existing account. Compose web ports bind
to loopback. The datasource and Prysm dashboard are provisioned automatically.

Use [Prometheus targets](http://localhost:9090/targets) for scrape errors, or check:

```sh
curl http://localhost:9102/metrics
curl http://TV_IP:9101/metrics
```

Target changes load within five seconds. Prometheus configuration changes need
`docker compose -f observability/compose.yaml restart prometheus`.
Named volumes retain 15 days of metrics and Grafana state; `down -v` deletes them.
Edit the provisioned dashboard JSON to keep dashboard changes in source control.

## Read the measurements

Use the [metric catalog](../../observability/CATALOG.md) for emitter, unit, labels
and exact measurement boundaries. The dashboard separates idle from stalled
capture, and requested settings from actual codec/format. Hardware H.264 panels
have no samples while hardware encoding is inactive.

Compare identical release builds, dimensions, FPS, source, codec and playback.
Let rates warm up and include static, moving and dark scenes. Compare output FPS
and failures alongside bytes/frame; dropped frames are not compression gains.

Process CPU uses cores (1.0 = one busy core); machine CPU is normalized across
cores. Interface counters may overlap on bridges/VPNs. Availability gauges mark
unsupported statistics. Histogram percentiles are bucket estimates; do not average
p95/p99 across nodes. Socket acceptance and receiver gaps do not measure delivery
or physical screen-to-light latency.

Sampling and HTTP exposition run on a separate exporter thread. Collection uses
bounded labels and brief library locks, so a Prometheus/Grafana outage does not
block capture. Validate the dashboard with Docker and synthetic series:

```sh
python3 observability/tests/check-health.py
```
