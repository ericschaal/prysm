//! `GStreamer` pipeline ownership; only kernel DMA allocation and mapping need unsafe code.
#[cfg(target_os = "linux")]
pub(super) use linux::Pipeline;

#[cfg(not(target_os = "linux"))]
pub(super) struct Pipeline;
#[cfg(not(target_os = "linux"))]
impl Pipeline {
    pub(super) fn new(_: u32, _: u32, _: usize, _: u8, _: u32) -> anyhow::Result<Self> {
        anyhow::bail!("hardware H.264 requires Linux DMA-BUF support")
    }
    #[allow(clippy::unused_self, reason = "same pipeline API on unsupported hosts")]
    pub(super) fn encode(&mut self, _: &[u8], _: &mut Vec<u8>) -> anyhow::Result<bool> {
        anyhow::bail!("hardware H.264 requires Linux DMA-BUF support")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, ensure};
    use gstreamer::{self as gst, prelude::*};
    use gstreamer_allocators::{DmaBufAllocator, prelude::*};
    use gstreamer_app::{AppSink, AppSrc};
    use gstreamer_video::{VideoFormat, VideoFrameFlags, VideoMeta};
    use std::{
        fs::File,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };

    #[repr(C)]
    struct Allocation {
        len: u64,
        fd: u32,
        flags: u32,
        heap_flags: u64,
    }

    struct Dma {
        memory: gst::Memory,
        fd: OwnedFd,
    }

    pub(in crate::encoder::hardware) struct Pipeline {
        graph: gst::Pipeline,
        source: AppSrc,
        sink: AppSink,
        allocator: DmaBufAllocator,
        bus: gst::Bus,
        heap: File,
        width: u32,
        height: u32,
        stride: usize,
        pending: bool,
        // Stable DMA-BUF identities prevent the corruption seen with per-frame allocation/free.
        buffers: Vec<Dma>,
        slot: usize,
    }

    impl Pipeline {
        pub(in crate::encoder::hardware) fn new(
            width: u32,
            height: u32,
            stride: usize,
            fps: u8,
            bitrate: u32,
        ) -> Result<Self> {
            ensure!(
                width > 0
                    && width <= 1920
                    && height > 0
                    && height <= 1080
                    && width.is_multiple_of(2)
                    && height.is_multiple_of(2)
                    && stride >= width as usize
                    && stride <= 1920
                    && stride.is_multiple_of(192)
                    && (1..=60).contains(&fps)
                    && (64_000..=100_000_000).contains(&bitrate),
                "invalid hardware encoder configuration"
            );
            gst::init()?;
            let heap = File::options()
                .read(true)
                .write(true)
                .open("/dev/dma_heap/dvr")?;
            let graph = gst::parse::launch(&format!(
                "appsrc name=input is-live=true do-timestamp=true format=time block=false caps=\"video/x-raw(memory:DMABuf),format=NV12,width={width},height={height},framerate={fps}/1,colorimetry=bt709\" ! rvenc target-bitrate={bitrate} ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=output async=false sync=false max-buffers=1 drop=false wait-on-eos=false"
            ))?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("hardware graph is not a pipeline"))?;
            let source = graph
                .by_name("input")
                .context("missing hardware appsrc")?
                .downcast::<AppSrc>()
                .map_err(|_| anyhow::anyhow!("invalid hardware appsrc"))?;
            let sink = graph
                .by_name("output")
                .context("missing hardware appsink")?
                .downcast::<AppSink>()
                .map_err(|_| anyhow::anyhow!("invalid hardware appsink"))?;
            let bus = graph.bus().context("missing hardware bus")?;
            let this = Self {
                graph,
                source,
                sink,
                allocator: DmaBufAllocator::new(),
                bus,
                heap,
                width,
                height,
                stride,
                pending: false,
                buffers: Vec::new(),
                slot: 0,
            };
            this.graph
                .set_state(gst::State::Playing)
                .context("hardware PLAYING failed")?;
            Ok(this)
        }

        pub(in crate::encoder::hardware) fn encode(
            &mut self,
            pixels: &[u8],
            output: &mut Vec<u8>,
        ) -> Result<bool> {
            let size = self.stride * self.height as usize * 3 / 2;
            ensure!(pixels.len() == size, "invalid padded NV12 length");
            if self.buffers.len() < 2 {
                self.buffers
                    .push(allocate_dma(&self.heap, &self.allocator, size)?);
            }
            let dma = &self.buffers[self.slot];
            ensure!(
                dma.memory.is_writable(),
                "hardware retained an input buffer"
            );
            // SAFETY: this slot has no in-flight users, is size bytes long, and is not exposed to callers.
            let mut map = unsafe { memmap2::MmapOptions::new().len(size).map_mut(&dma.fd)? };
            dma_sync(&dma.fd, 2)?;
            map.copy_from_slice(pixels);
            dma_sync(&dma.fd, 6)?;
            drop(map);
            let mut buffer = gst::Buffer::new();
            let buffer_ref = buffer.get_mut().context("hardware buffer is shared")?;
            buffer_ref.append_memory(dma.memory.clone());
            VideoMeta::add_full(
                buffer_ref,
                VideoFrameFlags::empty(),
                VideoFormat::Nv12,
                self.width,
                self.height,
                &[0, self.stride * self.height as usize],
                &[i32::try_from(self.stride)?, i32::try_from(self.stride)?],
            )?;
            self.slot = (self.slot + 1) % 2;
            self.source
                .push_buffer(buffer)
                .context("hardware input rejected")?;
            // shortcut: one frame of lookahead, replace when the vendor emits reliable AU markers.
            if !self.pending {
                self.pending = true;
                return Ok(false);
            }
            let sample = self
                .sink
                .try_pull_sample(gst::ClockTime::SECOND)
                .context("hardware output timeout")?;
            let buffer = sample.buffer().context("hardware sample has no buffer")?;
            let length = buffer.size();
            ensure!(
                (1..=8 * 1024 * 1024).contains(&length),
                "invalid hardware output length"
            );
            output.resize(length, 0);
            buffer
                .copy_to_slice(0, output)
                .map_err(|_| anyhow::anyhow!("incomplete hardware output"))?;
            ensure!(
                self.sink.try_pull_sample(gst::ClockTime::ZERO).is_none(),
                "unexpected extra hardware output"
            );
            Ok(true)
        }
    }

    fn allocate_dma(heap: &File, allocator: &DmaBufAllocator, size: usize) -> Result<Dma> {
        let mut request = Allocation {
            len: size as u64,
            fd: 0,
            flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
            heap_flags: 0,
        };
        // SAFETY: Linux DMA_HEAP_IOCTL_ALLOC writes the exact UAPI structure and returns a new fd.
        ensure!(
            unsafe { libc::ioctl(heap.as_raw_fd(), 0xc018_4800 as libc::c_ulong, &mut request) }
                == 0,
            "DMA allocation: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: successful allocation transferred a new descriptor to us.
        let fd = unsafe { OwnedFd::from_raw_fd(i32::try_from(request.fd)?) };
        // SAFETY: live allocator and a duplicate of the size-byte DMA-BUF; GStreamer owns the duplicate.
        let memory = unsafe { allocator.alloc_dmabuf(fd.try_clone()?, size)? };
        Ok(Dma { memory, fd })
    }

    fn dma_sync(fd: &OwnedFd, flags: u64) -> Result<()> {
        // SAFETY: DMA_BUF_IOCTL_SYNC reads one u64 from a live DMA-BUF descriptor.
        ensure!(
            unsafe { libc::ioctl(fd.as_raw_fd(), 0x4008_6200 as libc::c_ulong, &flags) } == 0,
            "DMA synchronization: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }

    impl Drop for Pipeline {
        fn drop(&mut self) {
            let _ = self.source.end_of_stream();
            let _ = self.bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(500),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            );
            if self.graph.set_state(gst::State::Null).is_err() {
                tracing::warn!("hardware NULL transition failed");
            }
        }
    }
}
