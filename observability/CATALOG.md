# Instrumentation catalog

The registry owns names and HELP text in `prysm-metrics`. Labels are encoded by
`prometheus-client` from Rust enums; call sites select component-scoped event
variants instead of free-form names or arbitrary operation/outcome combinations.
Spelling/case below matches the endpoint. Counters have `_total`; histogram
families have `_bucket`, `_sum` and `_count` suffixes.

## Attribution and dimensions

Prometheus adds `job`, `instance`, and configured `node` (physical machine).
Every exported series has `role=Capturer|Prysm` identifying the emitting process.
`flow` identifies CaptureFrames (TV → Prysm), InputFrames (host dispatch and
processing), LedFrames (Prysm → WLED), or PreviewFrames (Prysm → preview).
`component` identifies the owning module, `operation` the action being measured,
and `outcome` its classified result. These are source-code-defined enum values.
No frame IDs, error messages, peer addresses, or timestamps become labels.

Pixel counts carry `boundary`, `format`, `codec`, `content`, `width`, and `height`.
`format` comes from actual Frame/Header formats, or domain-owned Bgra8Pixels /
Rgb8Payload contracts which also own conversion/serialization. On Encoded,
SocketAccepted and PayloadReceived, codec/dimensions describe the actual header,
including None for an LZ4 raw fallback. TV wire content is Video, Ui, or Blended
according to the layers actually available that iteration. Receiver content is
Unspecified: the protocol does not convey layer provenance. Other frame boundaries
have codec/content=Unspecified and width/height=0 (not measured there), never an
invented encoding or resolution. Unspecified is not a codec or pixel format.

Requested configuration is separate from actual wire data. Native backend info
comes from the opened backend variant; Disabled means no open backend. Incoming
frames can change formats/resolutions; keep those dimensions separate when
comparing counts. Device names identify OS network interfaces and are the only
OS-enumerated label values. Counters reset on process restart. All other labels
are bounded enums or protocol-validated dimensions, not unbounded per-frame data.

## Pipeline frame boundaries

`prysm_pipeline_frames_total` (frames; labels above plus flow/component):

| Boundary | Owning emitter | Counted exactly when |
|---|---|---|
| VideoCaptured | TV / VideoCapture | Native acquisition returns a new usable frame; native call wall time may include copying/conversion |
| UiCaptured | TV / UiCapture | Native UI acquisition successfully fills the typed BGRA8 buffer |
| Composited | TV / Composer | Video conversion and UI blend finish, with the composed Header constructed |
| Encoded | TV / FrameEncoder | Payload preparation and header encoding succeed, before attempting socket write |
| SocketAccepted | TV / FrameSender | Entire header and payload accepted by local TCP writes; no delivery acknowledgement |
| PayloadReceived | Prysm / FrameReceiver | All validated-header payload bytes read, before any decompression or conversion |
| Decoded | Prysm / FrameDecoder | Valid payload is fully decoded/converted into the actual host Frame |
| InputDelivered | Prysm / InputDispatcher | Capture stream yields a usable Frame to the dispatcher, before processor mailbox submission |
| Processed | Prysm / FrameProcessor | Core processing returns edge colors; this does not mean rendered |
| PreviewPublished | Prysm / PreviewPublisher | Actor publishes the frame to egui's watch channel; not screen presentation |
| WledTransmitted | Prysm / WledSender | DDP write returns success for a complete nonempty, non-throttled frame; UDP delivery is not acknowledged |

Composition, capture, encoding and decoding populations differ. A video duplicate
can still produce a new UI-only frame. Watch channels intentionally replace old
frames without queues; upstream/downstream rates can differ without an error.
WLED rate-limited and empty calls do not count as transmitted frames.

## Operation outcomes

`prysm_operation_events_total` (events; role/flow/component/operation/outcome):

| Component | Operation | Outcomes and boundary |
|---|---|---|
| FrameSender | Connect | Attempted for each actual TCP connection attempt; Succeeded for first successful connection, Reconnected for later success; Failed or TimedOut at connect failure |
| FrameSender | ConfigureSocket | Failed if TCP_NODELAY/nonblocking setup fails after connecting |
| FrameSender | WriteFrame | Failed or TimedOut from the actual frame write; closes partial stream and enters normal retry backoff |
| FrameSender | SendFrame | SkippedBackoff when send is called during reconnect delay |
| FrameEncoder | Initialize | Failed if the H.264 encoder cannot initialize |
| FrameEncoder | EncodeH264 | SkippedRateControl when OpenH264 skips an input to meet its bitrate budget (no packet or reconnect); Failed at H.264 conversion/encoding failure; IdrEncoded, IntraFrameEncoded or PredictedFrameEncoded after native encoding, before header validation/write; not delivery |
| FrameEncoder | ValidateInput | Rejected before networking if header geometry/raw buffer length is invalid |
| FrameEncoder | Compress | Failed at compressor failure; RawFallback when LZ4 would not shrink input |
| FrameEncoder | EncodeHeader | Rejected if final header cannot be encoded |
| FrameReceiver | Connect | Succeeded on accepted TV TCP connection |
| FrameReceiver | ReadHeader | PeerClosed on EOF (including a partial header); Failed for other I/O errors |
| FrameReceiver | ValidateHeader | Rejected at protocol header validation |
| FrameReceiver | ReadPayload | Truncated on payload EOF; Failed for other payload I/O errors |
| FrameReceiver | ReadFrame | TimedOut when whole header+payload+decode operation exceeds five seconds; cancelled suboperation does not add a Failed event |
| FrameDecoder | Initialize | Failed if the H.264 decoder cannot initialize |
| FrameDecoder | DecodeH264 | Failed at H.264 validation, decoding or output color conversion |
| FrameDecoder | Decompress / Convert | Failed at the corresponding LZ4 decompressor/color converter |
| VideoCapture | Initialize | Attempted and Failed for native video initialization/retry |
| VideoCapture | Acquire | Failed for native acquisition errors; SkippedDuplicate when native backend has no new video frame |
| UiCapture | Acquire | Failed at native UI acquisition |
| CaptureLoop | Iterate | SkippedDeadline once per iteration that misses pacing deadline; not the number of elapsed periods |
| InputDispatcher | Acquire | Failed when host capture stream yields an error |
| InputDispatcher | DispatchProcessor | SkippedMailboxFull when the incoming frame is dropped |
| FrameProcessor | DispatchRenderer | SkippedMailboxFull when processed output is dropped |
| WledSender | WriteDdp | Failed if DDP write fails, including a partially sent multi-packet frame |

Timeout is a mutually exclusive failure classification, not a second generic
failure. Attempt and result events describe different boundaries; do not add them
together as a count of failures. There is no aggregate SendFailure/ReceiveFailure
catch-all. Connection setup, encoding, writes, reads, validation and decoding
remain distinguishable. SkippedBackoff describes calls, not retry duration: the
UI-only capture path intentionally avoids calling send during backoff.

## Local timing and byte boundaries

`prysm_operation_duration_seconds` (histogram seconds; role/flow/component/operation)
uses monotonic local wall time, including waits, on success or error. Disabled
instrumentation avoids clock reads. Histogram buckets span 100 μs to 5 s plus
+Inf. These are not CPU usage, remote delivery or synchronized end-to-end latency.

| Component / operation | Start → finish |
|---|---|
| VideoCapture / Acquire | Entire native video capture call |
| UiCapture / Acquire | Entire native UI capture call |
| Composer / Convert | Video→RGB or UI→RGB conversion, including output allocation/resize |
| Composer / Blend | Premultiplied BGRA blend into video RGB |
| Composer / ConvertToNv12 | Composed/UI RGB→packed limited-range BT.601 NV12 conversion, including output resize; emitted only when selected |
| FrameEncoder / PreparePayload | Raw payload selection or LZ4 compression; excludes header encoding |
| FrameSender / Connect | TCP connect call only; excludes socket configuration |
| FrameSender / WriteFrame | Entire header+payload write loop, including blocked waits |
| FrameSender / WaitWritable | Each poll waiting for socket readiness, nested inside WriteFrame |
| CaptureLoop / Iterate | Whole capture/composition/encoding/send iteration, excluding pacing sleep |
| FrameReceiver / ReadHeader | Header read including wait for next frame |
| FrameReceiver / ReadPayload | Complete payload read, including socket wait |
| FrameDecoder / Decompress | LZ4 decompression; compressed RGB includes only decompressor, chroma also includes decoded-buffer resize |
| FrameDecoder / Convert | Chroma→RGB conversion, including output allocation |
| FrameDecoder / FrameArrival | Gap between successful decoded frames; spans reconnection outages; no first-frame sample |
| FrameProcessor / Process | Core frame processing call only |
| WledSender / WriteDdp | DDP write only; excludes color generation, empty and rate-limited calls |

Timings overlap: Iterate includes capture, conversion, encoding and writes;
WaitWritable is inside WriteFrame. Never sum overlapping histograms or average
percentiles across nodes. Quantiles are estimates from buckets; gaps above five
seconds land in +Inf. FrameArrival measures decoded continuity, not transport delay.

`prysm_stream_bytes_total` (application bytes; role/flow/component/boundary):

| Component / boundary | Meaning |
|---|---|
| FrameEncoder / EncoderInput | Raw bytes of successfully prepared frames (header encoding succeeded); excludes encoder-skipped inputs so compression ratios do not count dropped frames as savings |
| FrameEncoder / EncoderOutput | Matching encoded payload bytes, including raw fallback; excludes header |
| FrameSender / SocketAccepted | Every successful local write, including protocol headers and bytes of a frame whose later write fails |
| FrameReceiver / CompletePayload | Complete validated-header payload bytes, even if subsequent decoding fails; excludes header and incomplete reads |
| WledSender / DdpPayload | Complete-frame color payload bytes after successful DDP write; excludes DDP headers and partially failed frames |

No counter includes Ethernet/IP/TCP/UDP overhead. Machine interface counters do.
Compression reduction compares matching encoder input/output populations; it is
not a delivery ratio. Byte families are per emitter rather than format, so inspect
the actual wire cohort panel and keep dimensions/mode constant when comparing runs.

## Configuration and system metrics

| Metric | Unit / owner / meaning | Additional labels |
|---|---|---|
| prysm_capture_requested_info | 1; TV configured source/compression/composition format/dimensions/FPS | source, compression, composition_format, width, height, fps |
| prysm_capture_backend_info | 1 selected, 0 unselected; TV actual opened backend per layer | layer, backend |
| prysm_process_cpu_cores | cores; emitting process including its native threads, 1 = one full core | none |
| prysm_process_resident_memory_bytes | bytes; emitting process RSS | none |
| prysm_system_cpu_ratio | ratio 0–1 across all machine cores | none |
| prysm_system_memory_used_bytes | bytes; machine-wide used RAM as defined by sysinfo/OS | none |
| prysm_system_memory_total_bytes | bytes; machine-wide physical RAM | none |
| prysm_system_network_receive_bytes_total | bytes received since exporter starts per interface | device |
| prysm_system_network_transmit_bytes_total | bytes sent since exporter starts per interface | device |
| prysm_system_collector_success | 1 when current process and CPU information available, otherwise 0 | none |
| up | Prometheus-generated scrape success, independent from collector availability | job, instance, node |

The exporter thread samples sysinfo every second. Initial CPU samples need a
refresh interval; endpoints can be available before resource samples warm up.
Machine values are repeated if multiple instrumented processes run on one node;
do not sum them. This is the actual macOS host or TV OS, not Docker's Linux VM.
Availability is coarse: it does not prove every network interface or OS statistic
is supported. Native webOS statistics need hardware validation. No extra service,
privileged container or unsupported GPU/thermal sensor assumption is introduced.

H.264 payload preparation uses FrameEncoder/PreparePayload (including I420
conversion). Receiver H.264 validation, decoding and RGB conversion use
FrameDecoder/DecodeH264. Codec labels derive from the packet enum and include H264.
For H.264 the wire format denotes decoded Rgb24, while the encoder's I420 surface
is internal. Capture info includes target_bitrate_bps (zero for raw/LZ4); compare
actual bytes and frame rates, since bitrate targets are not instantaneous caps.
