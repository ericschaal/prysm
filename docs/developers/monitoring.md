# Monitoring

The desktop hosts serve Prometheus metrics on the portal port when started
with `--metrics`:

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-headless -- settings.json --metrics
curl http://127.0.0.1:8787/metrics
```

Metrics are off by default, have no authentication and follow the portal's
origin policy. They are totals the session already keeps (frames received and
sent, processing and output-write time, write failures, opens) plus the
target, session state, revision and input rate. There are no system or
per-adapter metrics and no histograms. Names and meanings:
[portal contract](../../v1/surfaces/web-portal/HTTP-CONTRACT.md#metrics).

The TV workers have no metrics switch; use the rate in the TV app and the
portal's state instead.

## Dashboards

The Grafana and Prometheus stack in [`observability/`](../../observability/README.md)
was built for the metric names of the earlier implementation and has not
been updated for Prysm 1.x. Its dashboards will show no data for v1 hosts
until they are.

## Read measurements

Compare the same build, content, size, rate and encoding, including actual
throughput and failures. Use `rate()` and `_sum / _count` for averages.
Prysm's timings end at the output write; they do not measure the
screen-to-light latency.
