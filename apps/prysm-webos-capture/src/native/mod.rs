//! Capture lifecycle ported from hyperion-webos; see LICENSE.hyperion.
//! Native libraries stay loaded until all their resources have been released.
mod ffi;
use anyhow::{Context, Result, ensure};
use libloading::Library;

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

mod permissions;
mod vtcapture;
pub use vtcapture::VtCapture as Video;

#[derive(Debug)]
pub struct FatalVideo(pub anyhow::Error);
impl std::fmt::Display for FatalVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "video worker must exit: {:#}", self.0)
    }
}
impl std::error::Error for FatalVideo {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_keeps_the_native_error_as_a_source() {
        let error = library(&["/nonexistent-prysm-capture-driver.so"]).unwrap_err();
        assert!(error.downcast_ref::<libloading::Error>().is_some());
    }
}
