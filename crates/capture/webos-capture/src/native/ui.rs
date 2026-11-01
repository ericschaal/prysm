use super::ffi::{GmSurface, HalSurface};
use super::{Mapping, copy_mapping, driver_plane, mapping};
use super::{library, status};
use crate::{config::CaptureOptions, pixels};
use anyhow::{Context, Result, ensure};
use libloading::Library;
use std::fs::OpenOptions;
use std::sync::{Arc, OnceLock};

api!(GmApi {
    create: unsafe extern "C" fn(u32, u32, u32, *mut GmSurface) -> i32 = "GM_CreateSurface",
    capture: unsafe extern "C" fn(u32, *mut u32, *mut u32) -> i32 = "GM_CaptureGraphicScreen",
    destroy: unsafe extern "C" fn(u32) -> i32 = "GM_DestroySurface",
});
api!(HalApi {
    init: unsafe extern "C" fn() -> i32 = "HAL_GAL_Init",
    create: unsafe extern "C" fn(u32, u32, u32, *mut HalSurface) -> i32 = "HAL_GAL_CreateSurface",
    capture: unsafe extern "C" fn(*mut HalSurface) -> i32 = "HAL_GAL_CaptureFrameBuffer",
    destroy: unsafe extern "C" fn(*mut HalSurface) -> i32 = "HAL_GAL_DestroySurface",
});

pub enum Ui {
    Gm(GmCapture),
    Hal(HalCapture),
}
pub struct GmCapture {
    api: GmApi,
    surface: GmSurface,
    width: u32,
    height: u32,
}
pub struct HalCapture {
    api: Arc<HalApi>,
    surface: HalSurface,
    mapping: Option<Mapping>,
}

impl Ui {
    pub fn metrics_backend(&self) -> prysm_metrics::Backend {
        match self {
            Self::Gm(_) => prysm_metrics::Backend::Gm,
            Self::Hal(_) => prysm_metrics::Backend::Halgal,
        }
    }

    pub fn open(options: &CaptureOptions) -> Result<Self> {
        match GmCapture::open(options) {
            Ok(gm) => Ok(Self::Gm(gm)),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "gm unavailable; trying halgal");
                Ok(Self::Hal(HalCapture::open(options)?))
            }
        }
    }

    pub fn capture(
        &mut self,
        width: u32,
        height: u32,
        output: &mut pixels::Bgra8Pixels,
    ) -> Result<()> {
        let output = output.bytes_mut();
        output.clear();
        match self {
            Self::Gm(gm) => {
                let (mut w, mut h) = (gm.width, gm.height);
                status(
                    // SAFETY: this thread owns the live native resource and all borrowed out parameters.
                    unsafe { (gm.api.capture)(gm.surface.id, &raw mut w, &raw mut h) },
                    "GM capture",
                )?;
                ensure!(
                    w == width && h == height && w == gm.width && h == gm.height,
                    "GM and video dimensions differ"
                );
                pixels::copy_plane(
                    // SAFETY: the owned native surface stays live and stable throughout this copy/call.
                    unsafe {
                        driver_plane(gm, gm.surface.framebuffer, w as usize * h as usize * 4)?
                    },
                    w as usize * 4,
                    w as usize * 4,
                    h as usize,
                    output,
                )
            }
            Self::Hal(hal) => {
                status(
                    // SAFETY: this thread owns the live native resource and all borrowed out parameters.
                    unsafe { (hal.api.capture)(&raw mut hal.surface) },
                    "HAL_GAL capture",
                )?;
                ensure!(
                    u32::from(hal.surface.width) == width
                        && u32::from(hal.surface.height) == height,
                    "HAL_GAL and video dimensions differ"
                );
                let mapping = hal.mapping.as_ref().context("HAL_GAL not mapped")?;
                copy_mapping(
                    mapping,
                    hal.surface.pitch as usize,
                    width as usize * 4,
                    height as usize,
                    output,
                )
            }
        }
    }
}

impl GmCapture {
    fn open(options: &CaptureOptions) -> Result<Self> {
        let api = GmApi::load(&["libgm.so.1", "libgm.so"])?;
        let mut surface = GmSurface::default();
        // SAFETY: output surface is initialized and matches the native layout.
        status(
            unsafe { (api.create)(options.width, options.height, 0, &raw mut surface) },
            "GM_CreateSurface",
        )?;
        Ok(Self {
            api,
            surface,
            width: options.width,
            height: options.height,
        })
    }
}
impl Drop for GmCapture {
    fn drop(&mut self) {
        // SAFETY: exclusively owned, successfully created surface.
        unsafe {
            (self.api.destroy)(self.surface.id);
        }
    }
}
impl HalCapture {
    fn open(options: &CaptureOptions) -> Result<Self> {
        // HAL finalization is a no-op on webOS 25; unloading loses its live FD bookkeeping.
        static API: OnceLock<Arc<HalApi>> = OnceLock::new();
        let api = if let Some(api) = API.get() {
            Arc::clone(api)
        } else {
            let api = Arc::new(HalApi::load(&[
                "libhalgal.so",
                "libhalgal.so.0",
                "libhalgal.so.1",
                "libhalgal.so.2",
            ])?);
            Arc::clone(API.get_or_init(|| api))
        };
        Self::open_with(api, options, |surface| {
            let file = OpenOptions::new().read(true).write(true).open("/dev/gfx")?;
            let length = if surface.property != 0 {
                surface.property as usize
            } else {
                surface.pitch as usize * surface.height as usize
            };
            mapping(&file, surface.offset, length)
        })
    }

    fn open_with(
        api: Arc<HalApi>,
        options: &CaptureOptions,
        map: impl FnOnce(&HalSurface) -> Result<Mapping>,
    ) -> Result<Self> {
        // SAFETY: initialization, capture and teardown all run on this thread
        // because HAL_GAL binds an EGL context to its initialization thread.
        status(unsafe { (api.init)() }, "HAL_GAL_Init")?;
        let mut surface = HalSurface::default();
        status(
            // SAFETY: this thread owns the live native resource and all borrowed out parameters.
            unsafe { (api.create)(options.width, options.height, 0, &raw mut surface) },
            "HAL_GAL_CreateSurface",
        )?;
        let mut this = Self {
            api,
            surface,
            mapping: None,
        };
        this.mapping = Some(map(&this.surface)?);
        Ok(this)
    }
}
impl Drop for HalCapture {
    fn drop(&mut self) {
        // SAFETY: exclusively owned surface on the thread that created it.
        unsafe {
            (self.api.destroy)(&raw mut self.surface);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DESTROYED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn init() -> i32 {
        0
    }
    unsafe extern "C" fn create(width: u32, height: u32, _: u32, surface: *mut HalSurface) -> i32 {
        // SAFETY: open_with supplies an initialized writable surface.
        unsafe {
            (*surface).width = u16::try_from(width).unwrap();
            (*surface).height = u16::try_from(height).unwrap();
            (*surface).vendor_data = 42;
        }
        0
    }
    unsafe extern "C" fn destroy(surface: *mut HalSurface) -> i32 {
        // SAFETY: the owned surface remains live throughout teardown.
        assert_eq!(unsafe { (*surface).vendor_data }, 42);
        DESTROYED.fetch_add(1, Ordering::Relaxed);
        0
    }

    #[test]
    fn failed_mapping_destroys_the_created_surface_once() {
        let config = Config::new("127.0.0.1:19400".parse().unwrap());
        let before = DESTROYED.load(Ordering::Relaxed);
        let result = HalCapture::open_with(
            Arc::new(HalApi {
                init,
                create,
                capture: destroy,
                destroy,
                _library: libloading::os::unix::Library::this().into(),
            }),
            &config.capture,
            |surface| {
                assert_eq!(surface.vendor_data, 42);
                assert_eq!(u32::from(surface.width), config.capture.width);
                // Exercise the actual mapping validation after surface creation.
                mapping(&std::fs::File::open("/dev/null")?, 0, 0)
            },
        );
        assert_eq!(
            result.err().unwrap().to_string(),
            "invalid native mapping size"
        );
        assert_eq!(DESTROYED.load(Ordering::Relaxed), before + 1);
    }
}
