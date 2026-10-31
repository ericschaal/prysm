use super::ffi::{GmSurface, HalSurface};
use super::{Mapping, copy_mapping, driver_plane, mapping};
use super::{library, status};
use crate::cli::UiBackend;
use crate::{cli::CaptureOptions, pixels};
use anyhow::{Context, Result, ensure};
use libloading::Library;
use std::fs::OpenOptions;

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
    api: HalApi,
    surface: HalSurface,
    mapping: Option<Mapping>,
}

impl Ui {
    pub fn open(options: &CaptureOptions) -> Result<Self> {
        match options.ui_backend {
            UiBackend::Auto => match GmCapture::open(options) {
                Ok(gm) => Ok(Self::Gm(gm)),
                Err(error) => {
                    eprintln!("gm unavailable: {error:#}; trying halgal");
                    Ok(Self::Hal(HalCapture::open(options)?))
                }
            },
            UiBackend::Gm => Ok(Self::Gm(GmCapture::open(options)?)),
            UiBackend::Halgal => Ok(Self::Hal(HalCapture::open(options)?)),
        }
    }

    pub fn capture(&mut self, width: u32, height: u32, output: &mut Vec<u8>) -> Result<()> {
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
        let api = HalApi::load(&[
            "libhalgal.so",
            "libhalgal.so.0",
            "libhalgal.so.1",
            "libhalgal.so.2",
        ])?;
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
        let file = OpenOptions::new().read(true).write(true).open("/dev/gfx")?;
        let length = if this.surface.property != 0 {
            this.surface.property as usize
        } else {
            this.surface.pitch as usize * this.surface.height as usize
        };
        this.mapping = Some(mapping(&file, this.surface.offset, length)?);
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
