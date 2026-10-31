//! Bounded dimensions and emitter ownership. Callers choose a scoped operation,
//! never construct arbitrary combinations of component/operation/outcome labels.
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Capturer,
    Prysm,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Flow {
    CaptureFrames,
    InputFrames,
    LedFrames,
    PreviewFrames,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Component {
    VideoCapture,
    UiCapture,
    Composer,
    FrameEncoder,
    FrameSender,
    FrameReceiver,
    FrameDecoder,
    InputDispatcher,
    FrameProcessor,
    WledSender,
    PreviewPublisher,
    CaptureLoop,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Operation {
    Initialize,
    Acquire,
    Convert,
    Blend,
    ConvertToNv12,
    Compress,
    EncodeH264,
    PreparePayload,
    ValidateInput,
    EncodeHeader,
    ConfigureSocket,
    Connect,
    WriteFrame,
    WaitWritable,
    SendFrame,
    ReadHeader,
    ValidateHeader,
    ReadPayload,
    ReadFrame,
    Decompress,
    DecodeH264,
    FrameArrival,
    DispatchProcessor,
    DispatchRenderer,
    Process,
    WriteDdp,
    Iterate,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Outcome {
    Attempted,
    Succeeded,
    Reconnected,
    Failed,
    TimedOut,
    Rejected,
    PeerClosed,
    Truncated,
    SkippedDuplicate,
    SkippedBackoff,
    SkippedDeadline,
    SkippedRateControl,
    SkippedMailboxFull,
    RawFallback,
    IdrEncoded,
    IntraFrameEncoded,
    PredictedFrameEncoded,
}

#[derive(Clone, Copy, Debug)]
pub enum FrameSenderEvent {
    ConnectAttempt,
    Connected,
    Reconnected,
    ConnectFailed,
    ConnectTimedOut,
    ConfigureFailed,
    FrameWriteFailed,
    FrameWriteTimedOut,
    BackoffSkipped,
}
#[derive(Clone, Copy, Debug)]
pub enum EncoderEvent {
    H264RateControlSkipped,
    H264EncodeFailed,
    InitializationFailed,
    H264IdrEncoded,
    H264IntraFrameEncoded,
    H264PredictedFrameEncoded,
    InputRejected,
    CompressionFailed,
    HeaderRejected,
    RawFallback,
}
#[derive(Clone, Copy, Debug)]
pub enum FrameReceiverEvent {
    ConnectionAccepted,
    HeaderClosed,
    HeaderReadFailed,
    HeaderRejected,
    PayloadTruncated,
    PayloadReadFailed,
    FrameTimedOut,
}
#[derive(Clone, Copy, Debug)]
pub enum DecoderEvent {
    InitializationFailed,
    H264DecodeFailed,
    DecompressionFailed,
    ConversionFailed,
}
#[derive(Clone, Copy, Debug)]
pub enum VideoCaptureEvent {
    InitAttempt,
    InitFailed,
    AcquireFailed,
    Duplicate,
}
#[derive(Clone, Copy, Debug)]
pub enum UiCaptureEvent {
    AcquireFailed,
}
#[derive(Clone, Copy, Debug)]
pub enum PipelineEvent {
    MissedTick,
    InputFailed,
    ProcessorMailboxFull,
    RendererMailboxFull,
}
#[derive(Clone, Copy, Debug)]
pub enum WledEvent {
    WriteFailed,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct Event {
    flow: Flow,
    component: Component,
    operation: Operation,
    outcome: Outcome,
}

impl From<FrameSenderEvent> for Event {
    fn from(event: FrameSenderEvent) -> Self {
        let (operation, outcome) = match event {
            FrameSenderEvent::ConnectAttempt => (Operation::Connect, Outcome::Attempted),
            FrameSenderEvent::Connected => (Operation::Connect, Outcome::Succeeded),
            FrameSenderEvent::Reconnected => (Operation::Connect, Outcome::Reconnected),
            FrameSenderEvent::ConnectFailed => (Operation::Connect, Outcome::Failed),
            FrameSenderEvent::ConnectTimedOut => (Operation::Connect, Outcome::TimedOut),
            FrameSenderEvent::ConfigureFailed => (Operation::ConfigureSocket, Outcome::Failed),
            FrameSenderEvent::FrameWriteFailed => (Operation::WriteFrame, Outcome::Failed),
            FrameSenderEvent::FrameWriteTimedOut => (Operation::WriteFrame, Outcome::TimedOut),
            FrameSenderEvent::BackoffSkipped => (Operation::SendFrame, Outcome::SkippedBackoff),
        };
        Self {
            flow: Flow::CaptureFrames,
            component: Component::FrameSender,
            operation,
            outcome,
        }
    }
}
impl From<EncoderEvent> for Event {
    fn from(event: EncoderEvent) -> Self {
        let (operation, outcome) = match event {
            EncoderEvent::H264RateControlSkipped => {
                (Operation::EncodeH264, Outcome::SkippedRateControl)
            }
            EncoderEvent::H264EncodeFailed => (Operation::EncodeH264, Outcome::Failed),
            EncoderEvent::InitializationFailed => (Operation::Initialize, Outcome::Failed),
            EncoderEvent::H264IntraFrameEncoded => {
                (Operation::EncodeH264, Outcome::IntraFrameEncoded)
            }
            EncoderEvent::H264IdrEncoded => (Operation::EncodeH264, Outcome::IdrEncoded),
            EncoderEvent::H264PredictedFrameEncoded => {
                (Operation::EncodeH264, Outcome::PredictedFrameEncoded)
            }
            EncoderEvent::InputRejected => (Operation::ValidateInput, Outcome::Rejected),
            EncoderEvent::CompressionFailed => (Operation::Compress, Outcome::Failed),
            EncoderEvent::HeaderRejected => (Operation::EncodeHeader, Outcome::Rejected),
            EncoderEvent::RawFallback => (Operation::Compress, Outcome::RawFallback),
        };
        Self {
            flow: Flow::CaptureFrames,
            component: Component::FrameEncoder,
            operation,
            outcome,
        }
    }
}
impl From<FrameReceiverEvent> for Event {
    fn from(event: FrameReceiverEvent) -> Self {
        let (operation, outcome) = match event {
            FrameReceiverEvent::ConnectionAccepted => (Operation::Connect, Outcome::Succeeded),
            FrameReceiverEvent::HeaderClosed => (Operation::ReadHeader, Outcome::PeerClosed),
            FrameReceiverEvent::HeaderReadFailed => (Operation::ReadHeader, Outcome::Failed),
            FrameReceiverEvent::HeaderRejected => (Operation::ValidateHeader, Outcome::Rejected),
            FrameReceiverEvent::PayloadTruncated => (Operation::ReadPayload, Outcome::Truncated),
            FrameReceiverEvent::PayloadReadFailed => (Operation::ReadPayload, Outcome::Failed),
            FrameReceiverEvent::FrameTimedOut => (Operation::ReadFrame, Outcome::TimedOut),
        };
        Self {
            flow: Flow::CaptureFrames,
            component: Component::FrameReceiver,
            operation,
            outcome,
        }
    }
}
impl From<DecoderEvent> for Event {
    fn from(event: DecoderEvent) -> Self {
        Self {
            flow: Flow::CaptureFrames,
            component: Component::FrameDecoder,
            operation: match event {
                DecoderEvent::InitializationFailed => Operation::Initialize,
                DecoderEvent::H264DecodeFailed => Operation::DecodeH264,
                DecoderEvent::DecompressionFailed => Operation::Decompress,
                DecoderEvent::ConversionFailed => Operation::Convert,
            },
            outcome: Outcome::Failed,
        }
    }
}
impl From<VideoCaptureEvent> for Event {
    fn from(event: VideoCaptureEvent) -> Self {
        let (operation, outcome) = match event {
            VideoCaptureEvent::InitAttempt => (Operation::Initialize, Outcome::Attempted),
            VideoCaptureEvent::InitFailed => (Operation::Initialize, Outcome::Failed),
            VideoCaptureEvent::AcquireFailed => (Operation::Acquire, Outcome::Failed),
            VideoCaptureEvent::Duplicate => (Operation::Acquire, Outcome::SkippedDuplicate),
        };
        Self {
            flow: Flow::CaptureFrames,
            component: Component::VideoCapture,
            operation,
            outcome,
        }
    }
}
impl From<UiCaptureEvent> for Event {
    fn from(_: UiCaptureEvent) -> Self {
        Self {
            flow: Flow::CaptureFrames,
            component: Component::UiCapture,
            operation: Operation::Acquire,
            outcome: Outcome::Failed,
        }
    }
}
impl From<PipelineEvent> for Event {
    fn from(event: PipelineEvent) -> Self {
        let (flow, component, operation, outcome) = match event {
            PipelineEvent::MissedTick => (
                Flow::CaptureFrames,
                Component::CaptureLoop,
                Operation::Iterate,
                Outcome::SkippedDeadline,
            ),
            PipelineEvent::InputFailed => (
                Flow::InputFrames,
                Component::InputDispatcher,
                Operation::Acquire,
                Outcome::Failed,
            ),
            PipelineEvent::ProcessorMailboxFull => (
                Flow::InputFrames,
                Component::InputDispatcher,
                Operation::DispatchProcessor,
                Outcome::SkippedMailboxFull,
            ),
            PipelineEvent::RendererMailboxFull => (
                Flow::InputFrames,
                Component::FrameProcessor,
                Operation::DispatchRenderer,
                Outcome::SkippedMailboxFull,
            ),
        };
        Self {
            flow,
            component,
            operation,
            outcome,
        }
    }
}
impl From<WledEvent> for Event {
    fn from(_: WledEvent) -> Self {
        Self {
            flow: Flow::LedFrames,
            component: Component::WledSender,
            operation: Operation::WriteDdp,
            outcome: Outcome::Failed,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Stage {
    VideoAcquire,
    UiAcquire,
    CompositionConvert,
    CompositionBlend,
    CompositionToNv12,
    FrameEncode,
    FrameWrite,
    FrameWriteWait,
    FrameConnect,
    CaptureIteration,
    FrameReadPayload,
    FrameReadHeader,
    FrameDecompress,
    FrameDecodeH264,
    FrameConvert,
    ReceiverFrameGap,
    FrameProcess,
    WledWrite,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct StageLabels {
    flow: Flow,
    component: Component,
    operation: Operation,
}
impl From<Stage> for StageLabels {
    fn from(stage: Stage) -> Self {
        let (flow, component, operation) = match stage {
            Stage::VideoAcquire => (
                Flow::CaptureFrames,
                Component::VideoCapture,
                Operation::Acquire,
            ),
            Stage::UiAcquire => (
                Flow::CaptureFrames,
                Component::UiCapture,
                Operation::Acquire,
            ),
            Stage::CompositionConvert => {
                (Flow::CaptureFrames, Component::Composer, Operation::Convert)
            }
            Stage::CompositionBlend => (Flow::CaptureFrames, Component::Composer, Operation::Blend),
            Stage::CompositionToNv12 => (
                Flow::CaptureFrames,
                Component::Composer,
                Operation::ConvertToNv12,
            ),
            Stage::FrameEncode => (
                Flow::CaptureFrames,
                Component::FrameEncoder,
                Operation::PreparePayload,
            ),
            Stage::FrameWrite => (
                Flow::CaptureFrames,
                Component::FrameSender,
                Operation::WriteFrame,
            ),
            Stage::FrameWriteWait => (
                Flow::CaptureFrames,
                Component::FrameSender,
                Operation::WaitWritable,
            ),
            Stage::FrameConnect => (
                Flow::CaptureFrames,
                Component::FrameSender,
                Operation::Connect,
            ),
            Stage::CaptureIteration => (
                Flow::CaptureFrames,
                Component::CaptureLoop,
                Operation::Iterate,
            ),
            Stage::FrameReadPayload => (
                Flow::CaptureFrames,
                Component::FrameReceiver,
                Operation::ReadPayload,
            ),
            Stage::FrameReadHeader => (
                Flow::CaptureFrames,
                Component::FrameReceiver,
                Operation::ReadHeader,
            ),
            Stage::FrameDecodeH264 => (
                Flow::CaptureFrames,
                Component::FrameDecoder,
                Operation::DecodeH264,
            ),
            Stage::FrameDecompress => (
                Flow::CaptureFrames,
                Component::FrameDecoder,
                Operation::Decompress,
            ),
            Stage::FrameConvert => (
                Flow::CaptureFrames,
                Component::FrameDecoder,
                Operation::Convert,
            ),
            Stage::ReceiverFrameGap => (
                Flow::CaptureFrames,
                Component::FrameDecoder,
                Operation::FrameArrival,
            ),
            Stage::FrameProcess => (
                Flow::InputFrames,
                Component::FrameProcessor,
                Operation::Process,
            ),
            Stage::WledWrite => (Flow::LedFrames, Component::WledSender, Operation::WriteDdp),
        };
        Self {
            flow,
            component,
            operation,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum FrameStage {
    VideoCaptured,
    UiCaptured,
    Composited,
    Encoded,
    SocketAccepted,
    PayloadReceived,
    Decoded,
    InputDelivered,
    Processed,
    PreviewPublished,
    WledTransmitted,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Format {
    Rgb24,
    Bgr24,
    Yuyv,
    Mjpeg,
    Nv12,
    Nv16,
    Bgra,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct FrameLabels {
    flow: Flow,
    component: Component,
    boundary: FrameStage,
    format: Format,
    codec: Codec,
    content: Content,
    width: u32,
    height: u32,
}
impl FrameLabels {
    pub fn new(boundary: FrameStage, format: Format) -> Self {
        let (flow, component) = match boundary {
            FrameStage::VideoCaptured => (Flow::CaptureFrames, Component::VideoCapture),
            FrameStage::UiCaptured => (Flow::CaptureFrames, Component::UiCapture),
            FrameStage::Composited => (Flow::CaptureFrames, Component::Composer),
            FrameStage::Encoded => (Flow::CaptureFrames, Component::FrameEncoder),
            FrameStage::SocketAccepted => (Flow::CaptureFrames, Component::FrameSender),
            FrameStage::PayloadReceived => (Flow::CaptureFrames, Component::FrameReceiver),
            FrameStage::Decoded => (Flow::CaptureFrames, Component::FrameDecoder),
            FrameStage::InputDelivered => (Flow::InputFrames, Component::InputDispatcher),
            FrameStage::Processed => (Flow::InputFrames, Component::FrameProcessor),
            FrameStage::PreviewPublished => (Flow::PreviewFrames, Component::PreviewPublisher),
            FrameStage::WledTransmitted => (Flow::LedFrames, Component::WledSender),
        };
        Self {
            flow,
            component,
            boundary,
            format,
            codec: Codec::Unspecified,
            content: Content::Unspecified,
            width: 0,
            height: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WireBoundary {
    Encoded,
    SocketAccepted,
    PayloadReceived,
}
impl FrameLabels {
    pub fn wire(
        boundary: WireBoundary,
        format: Format,
        codec: Codec,
        content: Content,
        width: u32,
        height: u32,
    ) -> Self {
        let stage = match boundary {
            WireBoundary::Encoded => FrameStage::Encoded,
            WireBoundary::SocketAccepted => FrameStage::SocketAccepted,
            WireBoundary::PayloadReceived => FrameStage::PayloadReceived,
        };
        Self {
            codec,
            content,
            width,
            height,
            ..Self::new(stage, format)
        }
    }
}

impl From<prysm_webos_protocol::PixelFormat> for Format {
    fn from(format: prysm_webos_protocol::PixelFormat) -> Self {
        match format {
            prysm_webos_protocol::PixelFormat::Rgb => Self::Rgb24,
            prysm_webos_protocol::PixelFormat::Nv12 => Self::Nv12,
            prysm_webos_protocol::PixelFormat::Nv16 => Self::Nv16,
        }
    }
}
impl From<&prysm_webos_protocol::Header> for Codec {
    fn from(header: &prysm_webos_protocol::Header) -> Self {
        match header.codec {
            prysm_webos_protocol::Codec::Raw => Self::None,
            prysm_webos_protocol::Codec::Lz4 => Self::Lz4,
            prysm_webos_protocol::Codec::H264 => Self::H264,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum BytesKind {
    EncoderInput,
    EncoderOutput,
    SenderSocketAccepted,
    ReceiverPayload,
    WledPayload,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub(crate) enum ByteBoundary {
    EncoderInput,
    EncoderOutput,
    SocketAccepted,
    CompletePayload,
    DdpPayload,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct ByteLabels {
    flow: Flow,
    component: Component,
    boundary: ByteBoundary,
}
impl From<BytesKind> for ByteLabels {
    fn from(kind: BytesKind) -> Self {
        let (flow, component, boundary) = match kind {
            BytesKind::EncoderInput => (
                Flow::CaptureFrames,
                Component::FrameEncoder,
                ByteBoundary::EncoderInput,
            ),
            BytesKind::EncoderOutput => (
                Flow::CaptureFrames,
                Component::FrameEncoder,
                ByteBoundary::EncoderOutput,
            ),
            BytesKind::SenderSocketAccepted => (
                Flow::CaptureFrames,
                Component::FrameSender,
                ByteBoundary::SocketAccepted,
            ),
            BytesKind::ReceiverPayload => (
                Flow::CaptureFrames,
                Component::FrameReceiver,
                ByteBoundary::CompletePayload,
            ),
            BytesKind::WledPayload => (
                Flow::LedFrames,
                Component::WledSender,
                ByteBoundary::DdpPayload,
            ),
        };
        Self {
            flow,
            component,
            boundary,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum CaptureSource {
    Video,
    Ui,
    Both,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Content {
    Unspecified,
    Video,
    Ui,
    Blended,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Codec {
    Unspecified,
    None,
    Lz4,
    H264,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Layer {
    Video,
    Ui,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
pub enum Backend {
    Vtcapture,
    Dile,
    Gm,
    Halgal,
    Disabled,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct Device {
    pub device: String,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct BackendInfo {
    pub layer: Layer,
    pub backend: Backend,
}
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct CaptureInfo {
    pub source: CaptureSource,
    pub compression: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub composition_format: Format,
    pub target_bitrate_bps: u32,
}
