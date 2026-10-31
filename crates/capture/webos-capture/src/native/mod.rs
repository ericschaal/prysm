//! Capture lifecycle ported from hyperion-webos; see LICENSE.hyperion.
//! Native libraries stay loaded until all their resources have been released.
mod ffi;
use crate::cli::{CaptureOptions, VideoBackend};
use crate::pixels;
use anyhow::{Context, Result, ensure};
use libloading::Library;
use prysm_webos_protocol::Header;
use std::fs::File;

fn library(names: &[&str]) -> Result<Library> {
    let mut last_error = None;
    for name in names {
        // SAFETY: only known LG system libraries are loaded. Their constructors
        // require root and must run on the same thread as subsequent capture.
        match unsafe { Library::new(*name) } {
            Ok(library) => return Ok(library),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.context("no capture library candidates")?)
        .with_context(|| format!("cannot load {}", names.join(" or ")))
}

macro_rules! api {
    ($name:ident { $($field:ident: $ty:ty = $symbol:literal),* $(,)? }) => {
        struct $name { $($field: $ty,)* _library: Library }
        impl $name {
            fn load(names: &[&str]) -> Result<Self> {
                let library = library(names)?;
                // SAFETY: signatures and layouts match tv-native-apis. Function
                // pointers are never used after the owning library is dropped.
                unsafe { Ok(Self {
                    $($field: *library.get::<$ty>(concat!($symbol, "\0").as_bytes())
                        .context($symbol)?,)*
                    _library: library,
                }) }
            }
        }
    };
}

fn status(code: i32, operation: &str) -> Result<()> {
    ensure!(code == 0, "{operation} failed: {code}");
    Ok(())
}

type Mapping = memmap2::MmapRaw;
fn mapping(file: &File, offset: u32, length: usize) -> Result<Mapping> {
    ensure!(
        length > 0 && length <= 64 * 1024 * 1024,
        "invalid native mapping size"
    );
    Ok(memmap2::MmapOptions::new()
        .offset(u64::from(offset))
        .len(length)
        .map_raw_read_only(file)?)
}

/// # Safety
/// The driver must provide a readable allocation of `length` bytes that remains
/// valid and stable until the returned borrow has been copied, before release.
unsafe fn driver_plane<T>(_: &T, pointer: *const u8, length: usize) -> Result<&[u8]> {
    ensure!(
        !pointer.is_null() && length > 0 && length <= 64 * 1024 * 1024,
        "invalid driver plane"
    );
    // SAFETY: caller owns the live native frame; size and nullness checked above.
    Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
}

fn copy_mapping(
    mapping: &Mapping,
    stride: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    // SAFETY: the fixed device mapping remains live during this copy. The driver
    // cannot release the surface or unfreeze the video until capture returns.
    let source = unsafe { driver_plane(mapping, mapping.as_ptr(), mapping.len())? };
    pixels::copy_plane(source, stride, row_bytes, rows, output)
}

mod dile;
mod ui;
mod vtcapture;
use dile::DileCapture;
pub use ui::Ui;
use vtcapture::VtCapture;

pub enum Video {
    Vt(VtCapture),
    Dile(DileCapture),
}
impl Video {
    pub fn open(options: &CaptureOptions) -> Result<Self> {
        match options.video_backend {
            VideoBackend::Auto => match VtCapture::open(options) {
                Ok(vt) => Ok(Self::Vt(vt)),
                Err(error) => {
                    eprintln!("vtcapture unavailable: {error:#}; trying dile_vt");
                    Ok(Self::Dile(DileCapture::open(options)?))
                }
            },
            VideoBackend::Vtcapture => Ok(Self::Vt(VtCapture::open(options)?)),
            VideoBackend::Dile => Ok(Self::Dile(DileCapture::open(options)?)),
        }
    }

    pub fn capture(&mut self, output: &mut Vec<u8>) -> Result<Option<Header>> {
        output.clear();
        match self {
            Self::Vt(vt) => vt.capture(output),
            Self::Dile(dile) => dile.capture(output).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_keeps_the_native_error_as_a_source() {
        let error = library(&["/nonexistent-prysm-capture-driver.so"]).unwrap_err();
        assert!(error.downcast_ref::<libloading::Error>().is_some());
    }
}
