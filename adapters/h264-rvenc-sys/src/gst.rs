//! GStreamer pipeline ownership; only DMA-BUF allocation, mapping and
//! synchronisation need `unsafe`.
use crate::{Error, MAX_OUTPUT_BYTES, PITCH, copy_nv12};
use gstreamer::{self as gst, prelude::*};
use gstreamer_allocators::{DmaBufAllocator, prelude::*};
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_video::{VideoFormat, VideoFrameFlags, VideoMeta};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// `DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE` and `DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE`.
const SYNC_START: u64 = 2;
const SYNC_END: u64 = 6;
/// `DMA_HEAP_IOCTL_ALLOC` and `DMA_BUF_IOCTL_SYNC`.
const HEAP_ALLOC: libc::c_ulong = 0xc018_4800;
const BUF_SYNC: libc::c_ulong = 0x4008_6200;
const HEAP: &str = "/dev/dma_heap/dvr";

fn gst_error(error: impl std::fmt::Display) -> Error {
    Error::Gstreamer(error.to_string())
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { context, source }
}

/// `struct dma_heap_allocation_data` from the Linux UAPI.
#[repr(C)]
struct Allocation {
    len: u64,
    fd: u32,
    flags: u32,
    heap_flags: u64,
}

struct Dma {
    memory: gst::Memory,
    mapping: memmap2::MmapMut,
    fd: OwnedFd,
}

/// `appsrc ! rvenc ! appsink` for one geometry, rate and bitrate. Not `Sync`;
/// one caller encodes one picture at a time.
pub struct Pipeline {
    graph: gst::Pipeline,
    source: AppSrc,
    sink: AppSink,
    allocator: DmaBufAllocator,
    bus: gst::Bus,
    heap: File,
    width: u32,
    height: u32,
    stride: usize,
    // Stable DMA-BUF identities prevent the corruption seen with per-frame allocation/free.
    buffers: Vec<Dma>,
    slot: usize,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .finish_non_exhaustive()
    }
}

impl Pipeline {
    /// Initialise GStreamer, open the DMA heap and start the pipeline.
    ///
    /// # Errors
    /// Invalid configuration, no DMA heap, no `rvenc` element, or a refused
    /// state change.
    pub fn new(
        width: u32,
        height: u32,
        stride: usize,
        fps: u32,
        bitrate: u32,
    ) -> Result<Self, Error> {
        if !((2..=1920).contains(&width)
            && (2..=1080).contains(&height)
            && width.is_multiple_of(2)
            && height.is_multiple_of(2)
            && stride >= width as usize
            && stride <= 1920
            && stride.is_multiple_of(PITCH)
            && (1..=60).contains(&fps)
            && (64_000..=100_000_000).contains(&bitrate))
        {
            return Err(Error::Config("geometry, rate or bitrate out of range"));
        }
        gst::init().map_err(gst_error)?;
        let heap = File::options()
            .read(true)
            .write(true)
            .open(HEAP)
            .map_err(io("cannot open the DMA heap /dev/dma_heap/dvr"))?;
        // rvenc emits complete Annex B pictures; h264parse adds a next-input wait on the TV.
        // Static AVC caps prevent forcing byte-stream caps; the caller validates actual output.
        let graph = gst::parse::launch(&format!(
            "appsrc name=input is-live=true do-timestamp=true format=time block=false \
             caps=\"video/x-raw(memory:DMABuf),format=NV12,width={width},height={height},\
             framerate={fps}/1,colorimetry=bt709\" ! rvenc target-bitrate={bitrate} ! \
             appsink name=output async=false sync=false max-buffers=1 drop=false wait-on-eos=false"
        ))
        .map_err(gst_error)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| Error::Gstreamer("the hardware graph is not a pipeline".into()))?;
        let source = graph
            .by_name("input")
            .and_then(|element| element.downcast::<AppSrc>().ok())
            .ok_or_else(|| Error::Gstreamer("missing hardware appsrc".into()))?;
        let sink = graph
            .by_name("output")
            .and_then(|element| element.downcast::<AppSink>().ok())
            .ok_or_else(|| Error::Gstreamer("missing hardware appsink".into()))?;
        let bus = graph
            .bus()
            .ok_or_else(|| Error::Gstreamer("missing hardware bus".into()))?;
        let pipeline = Self {
            graph,
            source,
            sink,
            allocator: DmaBufAllocator::new(),
            bus,
            heap,
            width,
            height,
            stride,
            buffers: Vec::new(),
            slot: 0,
        };
        // `Drop` sets NULL even if PLAYING fails.
        pipeline
            .graph
            .set_state(gst::State::Playing)
            .map_err(gst_error)?;
        Ok(pipeline)
    }

    /// Encode one NV12 picture (`input_stride` is the width or the DMA
    /// pitch) and wait for its access unit, written to `output`.
    ///
    /// # Errors
    /// Invalid input layout, DMA failure, a rejected buffer, no output within
    /// 1 s, or more than one output for one input.
    pub fn encode(
        &mut self,
        pixels: &[u8],
        input_stride: usize,
        output: &mut Vec<u8>,
    ) -> Result<(), Error> {
        let size = self.stride * self.height as usize * 3 / 2;
        if !((input_stride == self.width as usize || input_stride == self.stride)
            && pixels.len() == input_stride * self.height as usize * 3 / 2)
        {
            return Err(Error::Config("invalid NV12 input layout"));
        }
        if self.buffers.len() < 2 {
            self.buffers
                .push(allocate_dma(&self.heap, &self.allocator, size)?);
        }
        let dma = &mut self.buffers[self.slot];
        if !dma.memory.is_writable() {
            return Err(Error::Busy);
        }
        dma_sync(&dma.fd, SYNC_START)?;
        let copied = if input_stride == self.stride {
            dma.mapping.copy_from_slice(pixels);
            Ok(())
        } else {
            copy_nv12(
                self.width as usize,
                self.height as usize,
                self.stride,
                pixels,
                &mut dma.mapping,
            )
        };
        // Always end the CPU access, even after a failed copy.
        let synchronized = dma_sync(&dma.fd, SYNC_END);
        copied?;
        synchronized?;
        let mut buffer = gst::Buffer::new();
        let buffer_ref = buffer
            .get_mut()
            .ok_or_else(|| Error::Gstreamer("hardware buffer is shared".into()))?;
        buffer_ref.append_memory(dma.memory.clone());
        let stride = i32::try_from(self.stride).map_err(|_| Error::Config("stride"))?;
        VideoMeta::add_full(
            buffer_ref,
            VideoFrameFlags::empty(),
            VideoFormat::Nv12,
            self.width,
            self.height,
            &[0, self.stride * self.height as usize],
            &[stride, stride],
        )
        .map_err(gst_error)?;
        self.slot = (self.slot + 1) % 2;
        self.source.push_buffer(buffer).map_err(gst_error)?;
        let sample = self
            .sink
            .try_pull_sample(gst::ClockTime::SECOND)
            .ok_or(Error::Timeout)?;
        let buffer = sample
            .buffer()
            .ok_or(Error::Output("sample without a buffer"))?;
        let length = buffer.size();
        if !(1..=MAX_OUTPUT_BYTES).contains(&length) {
            return Err(Error::Output("access unit length"));
        }
        output.resize(length, 0);
        buffer
            .copy_to_slice(0, output)
            .map_err(|_| Error::Output("incomplete access unit"))?;
        if self.sink.try_pull_sample(gst::ClockTime::ZERO).is_some() {
            return Err(Error::Output("more than one access unit for one picture"));
        }
        Ok(())
    }
}

fn allocate_dma(heap: &File, allocator: &DmaBufAllocator, size: usize) -> Result<Dma, Error> {
    let mut request = Allocation {
        len: size as u64,
        fd: 0,
        flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
        heap_flags: 0,
    };
    // SAFETY: DMA_HEAP_IOCTL_ALLOC reads and writes exactly the UAPI
    // `dma_heap_allocation_data` that `request` is laid out as, and returns a
    // new descriptor in `fd`.
    if unsafe { libc::ioctl(heap.as_raw_fd(), HEAP_ALLOC, &mut request) } != 0 {
        return Err(Error::Io {
            context: "DMA allocation",
            source: std::io::Error::last_os_error(),
        });
    }
    let raw = i32::try_from(request.fd).map_err(|_| Error::Output("DMA descriptor"))?;
    // SAFETY: the successful allocation transferred this new descriptor to us;
    // nothing else owns it.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: the new size-byte allocation has no other users; the mapping
    // stays private to this slot and lives no longer than `fd`.
    let mut mapping =
        unsafe { memmap2::MmapOptions::new().len(size).map_mut(&fd) }.map_err(io("DMA mapping"))?;
    dma_sync(&fd, SYNC_START)?;
    let (y, uv) = mapping.split_at_mut(size * 2 / 3);
    y.fill(16);
    uv.fill(128);
    dma_sync(&fd, SYNC_END)?;
    let duplicate = fd.try_clone().map_err(io("DMA descriptor duplicate"))?;
    // SAFETY: a live allocator and a duplicate of the size-byte DMA-BUF;
    // GStreamer takes ownership of the duplicate.
    let memory = unsafe { allocator.alloc_dmabuf(duplicate, size) }.map_err(gst_error)?;
    Ok(Dma {
        memory,
        mapping,
        fd,
    })
}

fn dma_sync(fd: &OwnedFd, flags: u64) -> Result<(), Error> {
    // SAFETY: DMA_BUF_IOCTL_SYNC reads one u64 from a live DMA-BUF descriptor.
    if unsafe { libc::ioctl(fd.as_raw_fd(), BUF_SYNC, &flags) } != 0 {
        return Err(Error::Io {
            context: "DMA synchronization",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.source.end_of_stream();
        let _ = self.bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(500),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        // The DMA slots drop after the pipeline is NULL (field order).
        if self.graph.set_state(gst::State::Null).is_err() {
            tracing::warn!("hardware encoder NULL transition failed");
        }
    }
}
