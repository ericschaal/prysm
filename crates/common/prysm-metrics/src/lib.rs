//! Opt-in, process-wide Prometheus telemetry. Collection and HTTP run off the pipeline.
#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use prometheus_client::{
    encoding::text::encode,
    metrics::{counter::Counter, family::Family, gauge::Gauge, histogram::Histogram},
    registry::Registry,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System,
};
use tiny_http::{Header, Method, Response, Server, StatusCode};

mod labels;
pub use labels::{
    Backend, BytesKind, CaptureSource, Codec, Content, DecoderEvent, EncoderEvent, Event, Format,
    FrameReceiverEvent, FrameSenderEvent, FrameStage, Layer, PipelineEvent, Role, Stage,
    UiCaptureEvent, VideoCaptureEvent, WireBoundary, WledEvent,
};
use labels::{BackendInfo, ByteLabels, CaptureInfo, Device, FrameLabels, StageLabels};

type FloatGauge = Gauge<f64, AtomicU64>;
type BytesGauge = Gauge<u64, AtomicU64>;
type Timings = Family<StageLabels, Histogram, fn() -> Histogram>;

struct Metrics {
    registry: Registry,
    frames: Family<FrameLabels, Counter>,
    events: Family<Event, Counter>,
    bytes: Family<ByteLabels, Counter>,
    durations: Timings,
    capture: Family<CaptureInfo, Gauge>,
    backend: Family<BackendInfo, Gauge>,
    process_cpu: FloatGauge,
    process_memory: BytesGauge,
    system_cpu: FloatGauge,
    memory_used: BytesGauge,
    memory_total: BytesGauge,
    network_rx: Family<Device, Counter>,
    network_tx: Family<Device, Counter>,
    collector_ok: Gauge,
}

fn histogram() -> Histogram {
    Histogram::new([
        0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.0167, 0.025, 0.0334, 0.05, 0.1,
        0.25, 0.5, 1.0, 2.0, 5.0,
    ])
}

impl Metrics {
    fn new(role: Role) -> Self {
        let mut registry =
            Registry::with_labels(std::iter::once(("role".into(), format!("{role:?}").into())));
        let frames = Family::default();
        let events = Family::default();
        let bytes = Family::default();
        let durations = Family::new_with_constructor(histogram as fn() -> Histogram);
        let capture = Family::default();
        let backend = Family::default();
        registry.register(
            "prysm_capture_backend_info",
            "Currently selected native backend",
            backend.clone(),
        );
        registry.register(
            "prysm_pipeline_frames",
            "Owning component frames at the named boundary and actual pixel format; socket acceptance is not delivery",
            frames.clone(),
        );
        registry.register(
            "prysm_operation_events",
            "Owning component operation outcomes; mutually classified outcomes, not duplicate generic failures",
            events.clone(),
        );
        registry.register(
            "prysm_stream_bytes",
            "Owning component application bytes at a boundary; SocketAccepted includes headers and partial writes, CompletePayload excludes headers; no TCP overhead",
            bytes.clone(),
        );
        registry.register(
            "prysm_operation_duration_seconds",
            "Owning component local operation wall time in seconds, including waits; not CPU time or cross-machine latency",
            durations.clone(),
        );
        registry.register(
            "prysm_capture_requested_info",
            "Configured capture settings",
            capture.clone(),
        );
        let process_cpu = FloatGauge::default();
        let process_memory = BytesGauge::default();
        let system_cpu = FloatGauge::default();
        let memory_used = BytesGauge::default();
        let memory_total = BytesGauge::default();
        let network_rx = Family::default();
        let network_tx = Family::default();
        let collector_ok = Gauge::default();
        registry.register(
            "prysm_process_cpu_cores",
            "Process CPU use across all threads; 1 means one full core",
            process_cpu.clone(),
        );
        registry.register(
            "prysm_process_resident_memory_bytes",
            "Resident process memory",
            process_memory.clone(),
        );
        registry.register(
            "prysm_system_cpu_ratio",
            "Whole-machine CPU use, normalized from zero to one",
            system_cpu.clone(),
        );
        registry.register(
            "prysm_system_memory_used_bytes",
            "Whole-machine used memory",
            memory_used.clone(),
        );
        registry.register(
            "prysm_system_memory_total_bytes",
            "Whole-machine total memory",
            memory_total.clone(),
        );
        registry.register(
            "prysm_system_network_receive_bytes",
            "Interface bytes received since exporter start",
            network_rx.clone(),
        );
        registry.register(
            "prysm_system_network_transmit_bytes",
            "Interface bytes transmitted since exporter start",
            network_tx.clone(),
        );
        registry.register(
            "prysm_system_collector_success",
            "One when the current process and system metrics are available",
            collector_ok.clone(),
        );
        Self {
            registry,
            frames,
            events,
            bytes,
            durations,
            capture,
            backend,
            process_cpu,
            process_memory,
            system_cpu,
            memory_used,
            memory_total,
            network_rx,
            network_tx,
            collector_ok,
        }
    }

    fn render(&self) -> Result<String> {
        let mut output = String::new();
        encode(&mut output, &self.registry)?;
        Ok(output)
    }
}

static METRICS: OnceLock<Arc<Metrics>> = OnceLock::new();

pub fn enabled() -> bool {
    METRICS.get().is_some()
}

/// Record a frame using bounded, source-code-defined labels.
pub fn frame(stage: FrameStage, format: Format) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .frames
            .get_or_create(&FrameLabels::new(stage, format))
            .inc();
    }
}

/// Actual wire settings, measured after encoding or complete payload receipt.
pub fn wire_frame(
    boundary: WireBoundary,
    format: Format,
    codec: Codec,
    content: Content,
    width: u32,
    height: u32,
) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .frames
            .get_or_create(&FrameLabels::wire(
                boundary, format, codec, content, width, height,
            ))
            .inc();
    }
}

pub fn event(event: impl Into<Event>) {
    if let Some(metrics) = METRICS.get() {
        metrics.events.get_or_create(&event.into()).inc();
    }
}

pub fn bytes(kind: BytesKind, count: usize) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .bytes
            .get_or_create(&kind.into())
            .inc_by(count as u64);
    }
}

pub fn observe(stage: Stage, duration: Duration) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .durations
            .get_or_create(&stage.into())
            .observe(duration.as_secs_f64());
    }
}

/// A disabled timer avoids clock reads entirely.
pub fn timer(stage: Stage) -> Timer {
    Timer {
        stage,
        started: METRICS.get().map(|_| Instant::now()),
    }
}

pub struct Timer {
    stage: Stage,
    started: Option<Instant>,
}
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            observe(self.stage, started.elapsed());
        }
    }
}

pub fn capture_info(
    source: CaptureSource,
    compression: Codec,
    width: u32,
    height: u32,
    fps: u8,
    composition_format: Format,
) {
    if let Some(metrics) = METRICS.get() {
        metrics
            .capture
            .get_or_create(&CaptureInfo {
                source,
                compression,
                width,
                height,
                fps,
                composition_format,
            })
            .set(1);
    }
}

pub fn backend_info(layer: Layer, selected: Backend) {
    if let Some(metrics) = METRICS.get() {
        for backend in [
            Backend::Vtcapture,
            Backend::Dile,
            Backend::Gm,
            Backend::Halgal,
            Backend::Disabled,
        ] {
            metrics
                .backend
                .get_or_create(&BackendInfo { layer, backend })
                .set(i64::from(backend == selected));
        }
    }
}

/// Stops the exporter independently from capture and application shutdown.
pub struct Exporter {
    stop: Arc<AtomicBool>,
    address: SocketAddr,
}
impl Exporter {
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
impl Drop for Exporter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start once per process. Bind/spawn failure leaves instrumentation disabled.
/// There is no authentication: bind to a trusted interface or loopback.
pub fn start(address: SocketAddr, role: Role) -> Result<Exporter> {
    anyhow::ensure!(METRICS.get().is_none(), "metrics already enabled");
    let metrics = Arc::new(Metrics::new(role));
    let exporter = serve(address, Arc::clone(&metrics))?;
    METRICS
        .set(metrics)
        .map_err(|_| anyhow::anyhow!("metrics already enabled"))?;
    Ok(exporter)
}

fn serve(address: SocketAddr, metrics: Arc<Metrics>) -> Result<Exporter> {
    let server = Server::http(address)
        .map_err(anyhow::Error::from_boxed)
        .with_context(|| format!("cannot bind metrics on {address}"))?;
    let address = server
        .server_addr()
        .to_ip()
        .context("metrics listener is not TCP")?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("prysm-metrics".into())
        .spawn(move || run_exporter(server, &metrics, &stopped))
        .context("cannot start metrics exporter")?;
    Ok(Exporter { stop, address })
}

fn run_exporter(server: Server, metrics: &Metrics, stopped: &AtomicBool) {
    let refresh = RefreshKind::nothing()
        .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
        .with_memory(MemoryRefreshKind::nothing().with_ram());
    let mut system = System::new_with_specifics(refresh);
    let mut networks = Networks::new_with_refreshed_list();
    let mut sampled = Instant::now();
    let content_type = Header::from_bytes(
        "Content-Type",
        "application/openmetrics-text; version=1.0.0; charset=utf-8",
    )
    .expect("static header");
    while !stopped.load(Ordering::Relaxed) {
        if sampled.elapsed() >= Duration::from_secs(1) {
            collect(metrics, &mut system, &mut networks);
            sampled = Instant::now();
        }
        match server.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(request)) => {
                let response = if request.method() == &Method::Get && request.url() == "/metrics" {
                    match metrics.render() {
                        Ok(body) => Response::from_string(body).with_header(content_type.clone()),
                        Err(_) => Response::from_string("metrics encoding failed\n")
                            .with_status_code(StatusCode(500)),
                    }
                } else {
                    Response::from_string("not found\n").with_status_code(StatusCode(404))
                };
                let _ = request.respond(response);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("metrics listener stopped: {error}");
                break;
            }
        }
    }
}

fn collect(metrics: &Metrics, system: &mut System, networks: &mut Networks) {
    system.refresh_cpu_usage();
    system.refresh_memory();
    let pid = Pid::from_u32(std::process::id());
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    metrics
        .system_cpu
        .set(f64::from(system.global_cpu_usage()) / 100.0);
    metrics.memory_used.set(system.used_memory());
    metrics.memory_total.set(system.total_memory());
    if let Some(process) = system.process(pid) {
        metrics
            .process_cpu
            .set(f64::from(process.cpu_usage()) / 100.0);
        metrics.process_memory.set(process.memory());
        metrics
            .collector_ok
            .set(i64::from(!system.cpus().is_empty()));
    } else {
        metrics.collector_ok.set(0);
    }
    networks.refresh(true);
    for (device, data) in networks.iter() {
        let labels = Device {
            device: device.clone(),
        };
        metrics
            .network_rx
            .get_or_create(&labels)
            .inc_by(data.received());
        metrics
            .network_tx
            .get_or_create(&labels)
            .inc_by(data.transmitted());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

    #[test]
    fn standard_exposition_preserves_typed_context_and_histograms() {
        let metrics = Metrics::new(Role::Prysm);
        metrics
            .frames
            .get_or_create(&FrameLabels::new(FrameStage::SocketAccepted, Format::Rgb24))
            .inc_by(3);
        metrics
            .durations
            .get_or_create(&Stage::FrameEncode.into())
            .observe(0.002);
        metrics
            .durations
            .get_or_create(&Stage::CompositionToNv12.into())
            .observe(0.003);
        metrics
            .events
            .get_or_create(&FrameSenderEvent::FrameWriteTimedOut.into())
            .inc();
        let text = metrics.render().unwrap();
        assert!(text.contains("component=\"FrameSender\",boundary=\"SocketAccepted\",format=\"Rgb24\",codec=\"Unspecified\",content=\"Unspecified\",width=\"0\",height=\"0\"} 3"));
        assert!(text.contains("component=\"FrameEncoder\",operation=\"PreparePayload\"} 1"));
        assert!(text.contains("component=\"Composer\",operation=\"ConvertToNv12\"} 1"));
        assert!(text.contains(
            "component=\"FrameSender\",operation=\"WriteFrame\",outcome=\"TimedOut\"} 1"
        ));
        assert!(text.ends_with("# EOF\n"));
    }

    #[test]
    fn exporter_serves_only_metrics_and_stops_without_pipeline_participation() {
        let exporter = serve(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(Metrics::new(Role::Prysm)),
        )
        .unwrap();
        for (path, expected) in [("/metrics", "200 OK"), ("/missing", "404 Not Found")] {
            let mut socket = TcpStream::connect(exporter.address()).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(socket, "GET {path} HTTP/1.0\r\n\r\n").unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            assert!(response.contains(expected));
        }
        let address = exporter.address();
        drop(exporter);
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if std::net::TcpListener::bind(address).is_ok() {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn collector_samples_the_current_process_off_the_hot_path() {
        let metrics = Metrics::new(Role::Prysm);
        let mut system = System::new();
        let mut networks = Networks::new();
        collect(&metrics, &mut system, &mut networks);
        assert!(metrics.memory_total.get() > 0);
        assert!(metrics.process_memory.get() > 0);
    }
}
