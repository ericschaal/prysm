# Monitoring

## Enable exporters

```sh
prysm --config settings.json --metrics-listen 0.0.0.0:9102
```

TV: save **Prometheus metrics** in the TV app; its worker serves port 9101.
Both default off. Exporter failure does not stop lighting. Metrics have no
authentication: restrict ports to the monitoring network.

## Start Grafana

Set addresses in [targets.json](../../observability/prometheus/targets.json), then:

```sh
docker compose -f observability/compose.yaml up -d
```

Open [Grafana](http://localhost:3000/d/prysm-overview), initially `admin`/`admin`.
`GRAFANA_ADMIN_PASSWORD` sets a new volume's initial password. Check scrape errors
in [Prometheus targets](http://localhost:9090/targets).

Targets reload within five seconds; Prometheus config changes need a container
restart. Named volumes retain state; `down -v` deletes it. Edit provisioned dashboard
JSON to keep changes in source control.

## Read measurements

[Metric catalog](../../observability/CATALOG.md): units, labels and boundaries.
Compare the same build/content/dimensions/FPS/codec, including actual throughput
and failures. Process CPU uses cores; machine CPU is normalized. Histogram
percentiles are estimates; do not average p95/p99. Socket sends/receiver gaps do
not measure physical screen-to-light latency.

Dashboard self-check, using Docker and synthetic data:

```sh
python3 observability/tests/check-health.py
```
