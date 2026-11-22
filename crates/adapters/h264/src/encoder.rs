//! The configured encoder: exactly one backend, sessions restarted on demand.
use crate::software::Software;
use crate::{BITRATE, DEFAULT_BITRATE, FORMAT, FrameKind, H264Error};
use prysm_core::Frame;
use prysm_ports::{Mode, Span};

/// `min` to `max` in steps of `step`, for the limits below.
const fn span(min: u32, max: u32, step: u32) -> Span {
    Span::new(min, max, step).expect("a span")
}

/// Which encoder runs. There is no automatic switching between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The webOS TV's rvenc (feature `hardware`, TV builds only).
    Hardware,
    /// OpenH264 on the CPU.
    Software,
}

impl Backend {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hardware => "hardware H.264",
            Self::Software => "software H.264 (OpenH264)",
        }
    }

    /// The sizes (even, as NV12 needs) and rates this backend encodes; the
    /// encoder refuses others. Limits to intersect with a capture's modes,
    /// not modes themselves.
    ///
    /// - Hardware: LG's rvenc. From 176×144, the vendor KADP path's minimum
    ///   (research `webos-capture/2026-11-01-hardware-encoding`), to
    ///   1920×1080, the largest `prysm-h264-rvenc-sys` takes; at most 30 fps
    ///   because rvenc programs 30 fps for any rate above 25
    ///   (`webos-capture/2026-11-02-encoder-latency`, F-001).
    /// - Software: from one macroblock (16×16) to 1920×1080, the largest
    ///   frame the capture stream carries (its decoder takes the coded
    ///   1920×1088), 1 to 60 fps.
    #[must_use]
    pub const fn limits(self) -> Mode {
        match self {
            Self::Hardware => Mode::new(span(176, 1920, 2), span(144, 1080, 2), span(1, 30, 1)),
            Self::Software => Mode::new(span(16, 1920, 2), span(16, 1080, 2), span(1, 60, 1)),
        }
    }

    /// Whether this build has the backend: hardware needs feature
    /// `hardware`, and even then runs only on the TV (elsewhere it fails at
    /// its first frame).
    #[must_use]
    pub const fn available(self) -> bool {
        match self {
            Self::Hardware => cfg!(feature = "hardware"),
            Self::Software => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderSettings {
    pub backend: Backend,
    /// Capture rate, within the backend's [limits](Backend::limits).
    pub fps: u32,
    /// Target bits per second, within [`BITRATE`](crate::BITRATE).
    pub bitrate: u32,
}

impl EncoderSettings {
    #[must_use]
    pub const fn new(backend: Backend, fps: u32) -> Self {
        Self {
            backend,
            fps,
            bitrate: DEFAULT_BITRATE,
        }
    }
}

/// One access unit and whether it starts an encoder session.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    /// Annex B NAL units of exactly one picture.
    pub data: &'a [u8],
    /// The first picture of a session: SPS, PPS and an IDR of the current
    /// frame. A receiver decodes it with fresh state.
    pub keyframe: bool,
}

enum Codec {
    Software(Box<Software>),
    #[cfg(feature = "hardware")]
    Hardware(Box<crate::hardware::Hardware>),
}

struct Session {
    codec: Codec,
    width: u32,
    height: u32,
    /// A picture of this session was returned.
    started: bool,
}

impl Session {
    /// The configured backend's native encoder for `width`x`height`.
    fn open(settings: EncoderSettings, width: u32, height: u32) -> Result<Self, H264Error> {
        let EncoderSettings {
            backend,
            fps,
            bitrate,
        } = settings;
        let codec = match backend {
            Backend::Software => Codec::Software(Box::new(Software::new(bitrate, fps)?)),
            #[cfg(feature = "hardware")]
            Backend::Hardware => Codec::Hardware(Box::new(crate::hardware::Hardware::new(
                width, height, fps, bitrate,
            )?)),
            #[cfg(not(feature = "hardware"))]
            Backend::Hardware => return Err(H264Error::HardwareNotBuilt),
        };
        Ok(Self {
            codec,
            width,
            height,
            started: false,
        })
    }
}

/// What one frame became.
enum Encoded {
    /// Software rate control skipped it.
    Skipped,
    /// A picture is in the encoder's output; `keyframe` when it starts the
    /// session.
    Picture { keyframe: bool },
}

/// Encodes NV12 frames with the configured [`Backend`]. A session (one
/// native encoder) opens on the first frame and lasts until [`restart`],
/// a size change or an error; each session starts with a keyframe.
///
/// [`restart`]: Encoder::restart
pub struct Encoder {
    settings: EncoderSettings,
    session: Option<Session>,
    output: Vec<u8>,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("settings", &self.settings)
            .field("open", &self.session.is_some())
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// Check the settings; nothing native opens until the first frame.
    ///
    /// # Errors
    /// A rate outside the backend's [limits](Backend::limits), a bitrate
    /// out of range, or [`Backend::Hardware`] in a build without it.
    pub fn new(settings: EncoderSettings) -> Result<Self, H264Error> {
        let fps = settings.backend.limits().fps();
        if !fps.contains(settings.fps) {
            return Err(H264Error::Rate {
                backend: settings.backend,
                fps: settings.fps,
                offered: fps,
            });
        }
        if !BITRATE.contains(&settings.bitrate) {
            return Err(H264Error::Bitrate(settings.bitrate));
        }
        if settings.backend == Backend::Hardware && !cfg!(feature = "hardware") {
            return Err(H264Error::HardwareNotBuilt);
        }
        Ok(Self {
            settings,
            session: None,
            output: Vec::new(),
        })
    }

    #[must_use]
    pub fn backend(&self) -> Backend {
        self.settings.backend
    }

    /// Close the session (releasing a hardware encoder); the next frame
    /// starts a new one with a keyframe.
    pub fn restart(&mut self) {
        self.session = None;
    }

    /// Encode one frame. `Ok(None)`: software rate control skipped it.
    ///
    /// # Errors
    /// The frame is not limited-range BT.709 NV12 of a size within the
    /// backend's [limits](Backend::limits), or the backend cannot
    /// start or failed. The session is closed then; the caller reports the
    /// failure (and may try again, which opens a new session).
    pub fn encode(&mut self, frame: &Frame) -> Result<Option<Picture<'_>>, H264Error> {
        let (width, height) = (frame.width(), frame.height());
        let limits = self.settings.backend.limits();
        let planes = frame.nv12_planes().filter(|_| {
            frame.format() == FORMAT
                && limits.width().contains(width)
                && limits.height().contains(height)
        });
        let Some((y, uv)) = planes else {
            return Err(H264Error::Input {
                width,
                height,
                format: frame.format(),
            });
        };
        if self
            .session
            .as_ref()
            .is_some_and(|s| (s.width, s.height) != (width, height))
        {
            self.restart();
        }
        let result = self.encode_planes(width, height, y, uv, frame.as_slice());
        match result {
            Ok(Encoded::Picture { keyframe }) => Ok(Some(Picture {
                data: &self.output,
                keyframe,
            })),
            Ok(Encoded::Skipped) => Ok(None),
            Err(error) => {
                self.restart();
                Err(error)
            }
        }
    }

    /// Encode into `self.output`, opening a session first if none is open.
    fn encode_planes(
        &mut self,
        width: u32,
        height: u32,
        y: &[u8],
        uv: &[u8],
        #[cfg_attr(not(feature = "hardware"), allow(unused_variables))] pixels: &[u8],
    ) -> Result<Encoded, H264Error> {
        let session = match &mut self.session {
            Some(session) => session,
            None => self
                .session
                .insert(Session::open(self.settings, width, height)?),
        };
        let kind = match &mut session.codec {
            Codec::Software(codec) => {
                match codec.encode(width as usize, height as usize, y, uv, &mut self.output)? {
                    Some(kind) => kind,
                    None => return Ok(Encoded::Skipped),
                }
            }
            #[cfg(feature = "hardware")]
            Codec::Hardware(codec) => codec.encode(pixels, &mut self.output)?,
        };
        let keyframe = !session.started;
        if keyframe && kind != FrameKind::Idr {
            return Err(H264Error::StartedWithoutIdr);
        }
        session.started = true;
        Ok(Encoded::Picture { keyframe })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decoder;

    /// A limited-range NV12 frame: luma ramp shifted by `shift`, chroma `(u, v)`.
    pub(crate) fn frame(width: u32, height: u32, shift: u8, (u, v): (u8, u8)) -> Frame {
        let n = (width * height) as usize;
        let mut data: Vec<u8> = (0..n)
            .map(|i| 16 + ((i % width as usize) as u8 / 2).wrapping_add(shift) % 200)
            .collect();
        data.extend([u, v].repeat(n / 4));
        Frame::new(data, width, height, FORMAT).unwrap()
    }

    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
        let sum: u64 = a
            .iter()
            .zip(b)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        sum as f64 / a.len() as f64
    }

    #[test]
    fn settings_and_inputs_are_checked() {
        let ok = EncoderSettings::new(Backend::Software, 30);
        assert!(Encoder::new(EncoderSettings { fps: 0, ..ok }).is_err());
        assert!(Encoder::new(EncoderSettings { fps: 60, ..ok }).is_ok());
        assert!(matches!(
            Encoder::new(EncoderSettings { fps: 61, ..ok }),
            Err(H264Error::Rate { fps: 61, .. })
        ));
        let small = frame(14, 16, 0, (128, 128));
        assert!(matches!(
            Encoder::new(ok).unwrap().encode(&small),
            Err(H264Error::Input {
                width: 14,
                height: 16,
                ..
            })
        ));
        for bitrate in [BITRATE.start() - 1, BITRATE.end() + 1] {
            assert!(matches!(
                Encoder::new(EncoderSettings { bitrate, ..ok }),
                Err(H264Error::Bitrate(rejected)) if rejected == bitrate
            ));
        }
        let mut encoder = Encoder::new(ok).unwrap();
        let rgb = Frame::new(vec![0; 32 * 32 * 3], 32, 32, prysm_core::PixelFormat::RGB24).unwrap();
        assert!(matches!(encoder.encode(&rgb), Err(H264Error::Input { .. })));
        let full = Frame::new(
            vec![0; 32 * 32 * 3 / 2],
            32,
            32,
            prysm_core::PixelFormat::NV12 {
                range: prysm_core::YuvRange::Full,
                matrix: prysm_core::YuvStandardMatrix::Bt709,
            },
        )
        .unwrap();
        assert!(matches!(
            encoder.encode(&full),
            Err(H264Error::Input { .. })
        ));
    }

    #[cfg(not(feature = "hardware"))]
    #[test]
    fn hardware_is_refused_without_the_feature() {
        assert!(matches!(
            Encoder::new(EncoderSettings::new(Backend::Hardware, 30)),
            Err(H264Error::HardwareNotBuilt)
        ));
    }

    /// Off the TV the hardware backend fails at its first frame, as a typed
    /// error, and never encodes in software instead.
    #[cfg(all(
        feature = "hardware",
        not(all(target_os = "linux", target_arch = "arm"))
    ))]
    #[test]
    fn hardware_off_the_tv_fails_without_switching_backend() {
        assert!(matches!(
            Encoder::new(EncoderSettings::new(Backend::Hardware, 31)),
            Err(H264Error::Rate { fps: 31, .. })
        ));
        let mut encoder = Encoder::new(EncoderSettings::new(Backend::Hardware, 30)).unwrap();
        assert!(matches!(
            encoder.encode(&frame(64, 32, 0, (128, 128))),
            Err(H264Error::Input { .. })
        ));
        for _ in 0..2 {
            let error = encoder.encode(&frame(176, 144, 0, (128, 128))).unwrap_err();
            assert!(
                matches!(
                    error,
                    H264Error::Hardware(prysm_h264_rvenc_sys::Error::Unavailable)
                ),
                "{error:?}"
            );
            assert_eq!(encoder.backend(), Backend::Hardware);
        }
    }

    #[test]
    fn backends_state_their_limits() {
        let hardware = Backend::Hardware.limits();
        assert!(hardware.contains(176, 144, 30) && hardware.contains(1920, 1080, 1));
        assert!(!hardware.contains(1920, 1082, 1));
        assert!(!hardware.contains(174, 144, 30) && !hardware.contains(640, 360, 31));
        assert!(!hardware.contains(641, 360, 30));
        let software = Backend::Software.limits();
        assert!(software.contains(16, 16, 60) && !software.contains(14, 16, 30));
        assert!(software.contains(1920, 1080, 60) && !software.contains(1920, 1082, 60));
        assert!(Backend::Software.available());
        assert_eq!(Backend::Hardware.available(), cfg!(feature = "hardware"));
    }

    #[test]
    fn software_sessions_start_with_a_decodable_current_picture() {
        let (width, height) = (64, 34);
        let mut encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        let mut decoder = Decoder::new().unwrap();
        let mut out = vec![0; 64 * 34 * 3 / 2];
        for i in 0..20u8 {
            let input = frame(width, height, i * 3, (90, 200));
            let picture = encoder.encode(&input).unwrap().expect("not skipped");
            assert_eq!(picture.keyframe, i == 0);
            if picture.keyframe {
                decoder = Decoder::new().unwrap();
            }
            decoder
                .decode(picture.data, width, height, &mut out)
                .unwrap();
            let error = mean_abs_diff(&out, input.as_slice());
            assert!(error < 3.0, "frame {i}: mean error {error}");
        }
        // A restart begins a new, independently decodable session.
        encoder.restart();
        let input = frame(width, height, 77, (200, 60));
        let picture = encoder.encode(&input).unwrap().unwrap();
        assert!(picture.keyframe);
        Decoder::new()
            .unwrap()
            .decode(picture.data, width, height, &mut out)
            .unwrap();
        assert!(mean_abs_diff(&out, input.as_slice()) < 3.0);
        // So does a size change.
        let bigger = frame(96, 64, 5, (128, 128));
        let picture = encoder.encode(&bigger).unwrap().unwrap();
        assert!(picture.keyframe);
        let mut out = vec![0; 96 * 64 * 3 / 2];
        Decoder::new()
            .unwrap()
            .decode(picture.data, 96, 64, &mut out)
            .unwrap();
    }

    #[test]
    fn the_decoder_rejects_mismatched_headers_and_garbage() {
        let mut encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        let picture = encoder
            .encode(&frame(64, 32, 0, (128, 128)))
            .unwrap()
            .unwrap()
            .data
            .to_vec();
        let mut wrong = vec![0; 32 * 32 * 3 / 2];
        assert!(matches!(
            Decoder::new().unwrap().decode(&picture, 32, 32, &mut wrong),
            Err(H264Error::Bitstream(_))
        ));
        let mut out = vec![0; 64 * 32 * 3 / 2];
        assert!(matches!(
            Decoder::new()
                .unwrap()
                .decode(&picture, 64, 32, &mut out[1..]),
            Err(H264Error::Destination {
                width: 64,
                height: 32,
                len: 3071
            })
        ));
        for garbage in [
            &b""[..],
            b"\x00\x00\x01",
            b"hello world",
            b"\x00\x00\x01\x0e\x00",
        ] {
            assert!(
                Decoder::new()
                    .unwrap()
                    .decode(garbage, 64, 32, &mut out)
                    .is_err()
            );
        }
        Decoder::new()
            .unwrap()
            .decode(&picture, 64, 32, &mut out)
            .unwrap();
    }

    /// A P picture without its reference, as after a lost frame, is an error
    /// (the receiver then waits for a keyframe) rather than a wrong picture.
    #[test]
    fn a_missing_reference_is_an_error() {
        let mut encoder = Encoder::new(EncoderSettings::new(Backend::Software, 30)).unwrap();
        let mut pictures = Vec::new();
        for i in 0..3u8 {
            let p = encoder
                .encode(&frame(64, 32, i * 20, (128, 128)))
                .unwrap()
                .unwrap();
            pictures.push(p.data.to_vec());
        }
        let mut out = vec![0; 64 * 32 * 3 / 2];
        assert!(
            Decoder::new()
                .unwrap()
                .decode(&pictures[1], 64, 32, &mut out)
                .is_err()
        );
        let mut decoder = Decoder::new().unwrap();
        decoder.decode(&pictures[0], 64, 32, &mut out).unwrap();
        // Frame 1 dropped.
        assert!(decoder.decode(&pictures[2], 64, 32, &mut out).is_err());
    }
}
