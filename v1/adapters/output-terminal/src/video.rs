//! Optional preview work stays off the terminal thread and never writes to stdout.
use image::{DynamicImage, Rgb, RgbImage};
use prysm_contract::{
    identity::{FrameStamp, SettingsRevision},
    preview::PreviewFrame,
};
use prysm_core::{Frame, PixelFormat, Viewport};
use ratatui::layout::Rect;
use ratatui_image::{
    picker::ProtocolType,
    protocol::{Protocol, halfblocks::Halfblocks, iterm2::Iterm2, kitty::Kitty, sixel::Sixel},
};
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Instant,
};

pub(super) const KITTY_IMAGE_ID: u32 = 0x0050_5259;

const MAX_WIDTH: u32 = 640;
const MAX_HEIGHT: u32 = 360;

pub(super) struct Encoded {
    pub stamp: FrameStamp,
    pub revision: SettingsRevision,
    pub received_at: Instant,
    pub area: Rect,
    pub result: Result<Protocol, String>,
}

struct Job {
    frame: PreviewFrame,
    received_at: Instant,
    area: Rect,
    generation: u64,
    font: (u16, u16),
    protocol: ProtocolType,
}

#[derive(Default)]
struct State {
    pending: Option<Job>,
    completed: Option<Encoded>,
    generation: u64,
    closed: bool,
}

pub(super) struct Encoder {
    shared: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    font: (u16, u16),
    protocol: ProtocolType,
}
impl Encoder {
    pub fn new(font: (u16, u16), protocol: ProtocolType) -> io::Result<Self> {
        validate_font(font)?;
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let input = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("prysm-terminal-video".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (state, ready) = &*input;
                        let mut state = state.lock().expect("preview encoder state");
                        while state.pending.is_none() && !state.closed {
                            state = ready.wait(state).expect("preview encoder state");
                        }
                        if state.closed {
                            break;
                        }
                        state.pending.take().expect("notified preview job")
                    };
                    let result = catch_unwind(AssertUnwindSafe(|| encode(&job)))
                        .unwrap_or_else(|_| Err("preview image encoder panicked".into()));
                    let mut state = input.0.lock().expect("preview encoder state");
                    if !state.closed && job.generation == state.generation {
                        state.completed = Some(Encoded {
                            stamp: job.frame.stamp,
                            revision: job.frame.revision,
                            received_at: job.received_at,
                            area: job.area,
                            result,
                        });
                    }
                }
            })?;
        Ok(Self {
            shared,
            worker: Some(worker),
            font,
            protocol,
        })
    }

    /// Fence old geometry while keeping the same retained encoder thread.
    pub fn reconfigure(&mut self, font: (u16, u16), protocol: ProtocolType) -> io::Result<()> {
        validate_font(font)?;
        let mut state = self.shared.0.lock().expect("preview encoder state");
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "preview encoder is closed",
            ));
        }
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        state.completed = None;
        self.font = font;
        self.protocol = protocol;
        Ok(())
    }

    /// A busy encoder retains only the newest pending source; no producer waits for encoding.
    pub fn submit(&self, frame: PreviewFrame, area: Rect, received_at: Instant) {
        let mut state = self.shared.0.lock().expect("preview encoder state");
        if state.closed {
            return;
        }
        state.pending = Some(Job {
            frame,
            area,
            received_at,
            generation: state.generation,
            font: self.font,
            protocol: self.protocol,
        });
        self.shared.1.notify_one();
    }

    pub fn take(&self) -> Option<Encoded> {
        self.shared
            .0
            .lock()
            .expect("preview encoder state")
            .completed
            .take()
    }

    /// Clear also fences work already being encoded when the preview layout changes.
    pub fn clear(&self) {
        let mut state = self.shared.0.lock().expect("preview encoder state");
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        state.completed = None;
    }

    /// Permanently stop accepting work without waiting for an in-flight encoding.
    pub fn request_shutdown(&self) {
        let mut state = self.shared.0.lock().expect("preview encoder state");
        state.closed = true;
        state.pending = None;
        state.completed = None;
        self.shared.1.notify_one();
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        self.request_shutdown();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "preview encoder thread panicked".to_owned())?;
        }
        Ok(())
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn validate_font(font: (u16, u16)) -> io::Result<()> {
    let (width, height) = font;
    // Reject broken terminal geometry before the image crate pads to cell boundaries.
    if width == 0 || height == 0 || u32::from(width) > MAX_WIDTH || u32::from(height) > MAX_HEIGHT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid terminal cell size",
        ));
    }
    Ok(())
}

fn encode(job: &Job) -> Result<Protocol, String> {
    let font = job.font;
    let protocol = job.protocol;
    let image = sample(
        &job.frame.snapshot.frame,
        job.frame.snapshot.viewport,
        job.area,
        font,
    )?;
    let cells = job.area;
    let sampled = image.dimensions();
    let display = (
        u32::from(cells.width) * u32::from(font.0),
        u32::from(cells.height) * u32::from(font.1),
    );
    let image = DynamicImage::ImageRgb8(image);
    match protocol {
        ProtocolType::Halfblocks => Halfblocks::new(image, cells).map(Protocol::Halfblocks),
        ProtocolType::Iterm2 => Iterm2::new(image, cells, false).map(|mut protocol| {
            // The library ties display size to PNG resolution. Keep transmission bounded
            // and let the terminal scale the cropped image to the requested cell area.
            protocol.data = protocol.data.replacen(
                &format!("width={}px;height={}px;", sampled.0, sampled.1),
                &format!(
                    "width={};height={};preserveAspectRatio=0;",
                    cells.width, cells.height
                ),
                1,
            );
            Protocol::ITerm2(protocol)
        }),
        // These library backends don't expose independent display scaling.
        ProtocolType::Kitty | ProtocolType::Sixel if sampled != display => {
            Halfblocks::new(image, cells).map(Protocol::Halfblocks)
        }
        // One owned terminal image slot; the screen deletes it on restoration.
        ProtocolType::Kitty => Kitty::new(image, cells, KITTY_IMAGE_ID, false).map(Protocol::Kitty),
        ProtocolType::Sixel => Sixel::new(image, cells, false).map(Protocol::Sixel),
    }
    .map_err(|error| error.to_string())
}

/// Convert only sampled crop pixels, never allocating an RGB copy of the source frame.
fn sample(
    frame: &Frame,
    viewport: Option<Viewport>,
    area: Rect,
    font: (u16, u16),
) -> Result<RgbImage, String> {
    let crop = viewport.unwrap_or_else(|| Viewport::full_frame(frame.width(), frame.height()));
    if crop.width == 0
        || crop.height == 0
        || crop
            .x
            .checked_add(crop.width)
            .is_none_or(|right| right > frame.width())
        || crop
            .y
            .checked_add(crop.height)
            .is_none_or(|bottom| bottom > frame.height())
    {
        return Err("preview has an empty or invalid viewport".into());
    }
    if matches!(frame.format(), PixelFormat::MJPEG) {
        return Err("MJPEG preview is not supported".into());
    }
    // Ratatui and the half-block encoder use u16 cell-count products.
    if u32::from(area.width) * u32::from(area.height) > u32::from(u16::MAX)
        || area.width == 0
        || area.height == 0
    {
        return Err("preview area is empty or exceeds terminal cell limits".into());
    }
    let target = (
        u32::from(area.width) * u32::from(font.0),
        u32::from(area.height) * u32::from(font.1),
    );
    let crop = cover(crop, target);
    let (width, height) = bounded_resolution(target);
    Ok(RgbImage::from_fn(width, height, |x, y| {
        let x = crop.x + (u64::from(x) * u64::from(crop.width) / u64::from(width)) as u32;
        let y = crop.y + (u64::from(y) * u64::from(crop.height) / u64::from(height)) as u32;
        Rgb(pixel(frame, x, y))
    }))
}

fn bounded_resolution((width, height): (u32, u32)) -> (u32, u32) {
    if width <= MAX_WIDTH && height <= MAX_HEIGHT {
        return (width, height);
    }
    if u64::from(width) * u64::from(MAX_HEIGHT) > u64::from(height) * u64::from(MAX_WIDTH) {
        (
            MAX_WIDTH,
            (u64::from(height) * u64::from(MAX_WIDTH) / u64::from(width)).max(1) as u32,
        )
    } else {
        (
            (u64::from(width) * u64::from(MAX_HEIGHT) / u64::from(height)).max(1) as u32,
            MAX_HEIGHT,
        )
    }
}

fn cover(mut crop: Viewport, (width, height): (u32, u32)) -> Viewport {
    if u64::from(crop.width) * u64::from(height) > u64::from(crop.height) * u64::from(width) {
        let cropped = (u64::from(crop.height) * u64::from(width) / u64::from(height)).max(1) as u32;
        crop.x += (crop.width - cropped) / 2;
        crop.width = cropped;
    } else {
        let cropped = (u64::from(crop.width) * u64::from(height) / u64::from(width)).max(1) as u32;
        crop.y += (crop.height - cropped) / 2;
        crop.height = cropped;
    }
    crop
}

fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
    match frame.format() {
        PixelFormat::RGB24 | PixelFormat::BGR24 => {
            let offset = (y as usize * frame.width() as usize + x as usize) * 3;
            let data = &frame.as_slice()[offset..offset + 3];
            if frame.format() == PixelFormat::RGB24 {
                [data[0], data[1], data[2]]
            } else {
                [data[2], data[1], data[0]]
            }
        }
        PixelFormat::YUYV { range, matrix } => {
            let (r, g, b) = prysm_core::yuyv::yuyv_pixel_to_rgb(
                frame.as_slice(),
                x,
                y,
                frame.width(),
                range,
                matrix,
            );
            [r, g, b]
        }
        PixelFormat::MJPEG => unreachable!("format validated before sampling"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{YuvRange, YuvStandardMatrix};

    #[test]
    fn preview_fills_the_target_by_cropping_the_center_without_stretching() {
        let pixels = [10, 20, 30, 40]
            .into_iter()
            .flat_map(|r| [r, 0, 0])
            .collect::<Vec<_>>()
            .repeat(2);
        let frame = Frame::new(pixels, 4, 2, PixelFormat::RGB24).unwrap();
        let image = sample(&frame, None, Rect::new(0, 0, 2, 2), (1, 1)).unwrap();
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image.get_pixel(0, 0)[0], 20);
        assert_eq!(image.get_pixel(1, 1)[0], 30);
        // Filling a large pane does not require encoding more pixels.
        let image = sample(&frame, None, Rect::new(0, 0, 100, 30), (10, 20)).unwrap();
        assert_eq!(image.dimensions(), (600, 360));
    }

    #[test]
    fn samples_rgb_and_bgr_viewports_without_using_surrounding_pixels() {
        for (format, pixels) in [
            (
                PixelFormat::RGB24,
                vec![1, 2, 3, 40, 50, 60, 70, 80, 90, 4, 5, 6],
            ),
            (
                PixelFormat::BGR24,
                vec![3, 2, 1, 60, 50, 40, 90, 80, 70, 6, 5, 4],
            ),
        ] {
            let frame = Frame::new(pixels, 4, 1, format).unwrap();
            let crop = Some(Viewport {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            });
            let result = sample(&frame, crop, Rect::new(0, 0, 2, 1), (1, 1)).unwrap();
            assert_eq!(result.dimensions(), (2, 1));
            assert_eq!(result.as_raw(), &[40, 50, 60, 70, 80, 90]);
        }
    }

    #[test]
    fn yuyv_sampling_honors_odd_crop_offsets_range_and_matrix() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [YuvStandardMatrix::Bt601, YuvStandardMatrix::Bt709] {
                let frame = Frame::new(
                    vec![32, 90, 180, 210, 210, 180, 70, 100],
                    4,
                    1,
                    PixelFormat::YUYV { range, matrix },
                )
                .unwrap();
                let crop = Some(Viewport {
                    x: 1,
                    y: 0,
                    width: 2,
                    height: 1,
                });
                let sampled = sample(&frame, crop, Rect::new(0, 0, 2, 1), (1, 1)).unwrap();
                let full = prysm_core::yuyv::yuyv_to_rgb(&frame).unwrap();
                for (actual, expected) in sampled.as_raw().iter().zip(&full[3..9]) {
                    assert!(actual.abs_diff(*expected) <= 1);
                }
            }
        }
    }

    #[test]
    fn sampling_bounds_work_and_preserves_source_aspect() {
        let frame = Frame::dummy(1920, 1080).unwrap();
        let image = sample(&frame, None, Rect::new(0, 0, 200, 100), (12, 24)).unwrap();
        assert_eq!(image.dimensions(), (360, 360));
        assert_eq!(
            cover(Viewport::full_frame(1920, 1080), (100, 100)),
            Viewport {
                x: 420,
                y: 0,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            cover(Viewport::full_frame(1080, 1920), (100, 100)),
            Viewport {
                x: 0,
                y: 420,
                width: 1080,
                height: 1080
            }
        );
        assert_eq!(
            cover(Viewport::full_frame(1, 1920), (100, 100)),
            Viewport {
                x: 0,
                y: 959,
                width: 1,
                height: 1
            }
        );
    }

    #[test]
    fn invalid_or_unsupported_sources_are_preview_errors() {
        let frame = Frame::dummy(4, 2).unwrap();
        for crop in [
            Viewport {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Viewport {
                x: 3,
                y: 0,
                width: 2,
                height: 1,
            },
            Viewport {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            assert!(sample(&frame, Some(crop), Rect::new(0, 0, 4, 2), (1, 1)).is_err());
        }
        assert!(sample(&frame, None, Rect::default(), (1, 1)).is_err());
        assert!(
            sample(
                &Frame::dummy(0, 2).unwrap(),
                None,
                Rect::new(0, 0, 4, 2),
                (1, 1)
            )
            .is_err()
        );
        let jpeg = Frame::new(vec![1, 2, 3], 4, 2, PixelFormat::MJPEG).unwrap();
        assert!(sample(&jpeg, None, Rect::new(0, 0, 4, 2), (1, 1)).is_err());
    }
    fn preview(sequence: u64) -> PreviewFrame {
        use prysm_contract::identity::{InstanceId, SessionId, StreamEpoch};
        use std::num::NonZeroU64;
        PreviewFrame {
            stamp: FrameStamp {
                epoch: StreamEpoch {
                    session: SessionId {
                        instance: InstanceId(NonZeroU64::MIN),
                        sequence: NonZeroU64::MIN,
                    },
                    generation: 1,
                },
                sequence,
            },
            revision: SettingsRevision(4),
            snapshot: prysm_core::ProcessedSnapshot {
                frame: Frame::dummy(16, 9).unwrap(),
                viewport: None,
                colors: Default::default(),
            },
        }
    }

    #[test]
    fn worker_reaches_newest_job_and_preserves_encoded_lineage_and_age() {
        let mut encoder = Encoder::new((1, 2), ProtocolType::Halfblocks).unwrap();
        let received_at = Instant::now();
        let area = Rect::new(0, 0, 20, 10);
        for sequence in 1..=50 {
            encoder.submit(preview(sequence), area, received_at);
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        let encoded = loop {
            if let Some(encoded) = encoder.take()
                && encoded.stamp.sequence == 50
            {
                break encoded;
            }
            assert!(
                Instant::now() < deadline,
                "encoder did not reach newest job"
            );
            thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(encoded.revision, SettingsRevision(4));
        assert_eq!(encoded.received_at, received_at);
        assert_eq!(encoded.area, area);
        let protocol = encoded.result.unwrap();
        assert!(protocol.area().width <= area.width && protocol.area().height <= area.height);
        encoder.clear();
        assert!(encoder.take().is_none());
        encoder.request_shutdown();
        encoder.submit(preview(51), area, received_at);
        assert!(encoder.shared.0.lock().unwrap().pending.is_none());
        assert!(
            encoder.worker.is_some(),
            "shutdown request retains the join handle"
        );
        encoder.shutdown().unwrap();
        assert!(encoder.worker.is_none());
    }

    #[test]
    fn kitty_reuses_the_owned_image_id() {
        use ratatui::{buffer::Buffer, widgets::Widget};
        let font = (8, 16);
        let protocol = ProtocolType::Kitty;
        for sequence in [1, 2] {
            let job = Job {
                frame: preview(sequence),
                area: Rect::new(0, 0, 20, 10),
                received_at: Instant::now(),
                generation: 0,
                font,
                protocol,
            };
            let protocol = encode(&job).unwrap();
            let mut buffer = Buffer::empty(job.area);
            ratatui_image::Image::new(&protocol).render(job.area, &mut buffer);
            let payload: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(payload.contains(&format!("i={KITTY_IMAGE_ID},a=T")));
        }
    }
    #[test]
    fn selected_protocols_fill_large_panes_with_bounded_sampling() {
        for protocol in [
            ProtocolType::Halfblocks,
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
        ] {
            let font = (12, 24);
            let job = Job {
                frame: preview(1),
                area: Rect::new(0, 0, 200, 60),
                received_at: Instant::now(),
                generation: 0,
                font,
                protocol,
            };
            let encoded = encode(&job).unwrap();
            assert_eq!(encoded.area(), job.area);
            if let Protocol::ITerm2(protocol) = encoded {
                assert!(
                    protocol
                        .data
                        .contains("width=200;height=60;preserveAspectRatio=0;")
                );
            }
        }
    }
    #[test]
    fn reconfiguration_fences_old_work_and_reuses_the_worker() {
        let mut encoder = Encoder::new((8, 16), ProtocolType::Iterm2).unwrap();
        let worker = encoder.worker.as_ref().unwrap().thread().id();
        let area = Rect::new(0, 0, 100, 30);
        encoder.submit(preview(1), area, Instant::now());
        encoder.reconfigure((4, 8), ProtocolType::Kitty).unwrap();
        assert_eq!(encoder.worker.as_ref().unwrap().thread().id(), worker);
        assert!(encoder.take().is_none());
        encoder.submit(preview(2), area, Instant::now());
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        let encoded = loop {
            if let Some(encoded) = encoder.take() {
                break encoded;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(encoded.stamp.sequence, 2);
        let protocol = encoded.result.unwrap();
        assert!(matches!(protocol, Protocol::Kitty(_)));
        assert_eq!(protocol.area(), Rect::new(0, 0, 100, 30));
        assert!(encoder.reconfigure((0, 8), ProtocolType::Kitty).is_err());
        encoder.shutdown().unwrap();
    }
}
