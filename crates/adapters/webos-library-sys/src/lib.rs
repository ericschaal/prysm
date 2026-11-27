//! Process-lifetime storage for vendor libraries and their resolved entry points.
use libloading::Library;
use once_cell::sync::OnceCell;

#[derive(Debug, thiserror::Error)]
#[error("cannot load {library}")]
pub struct LoadError {
    library: String,
    #[source]
    source: libloading::Error,
}

/// Loads the first available library and retains it even if resolving symbols fails.
pub struct VendorLibrary<A> {
    names: &'static [&'static str],
    handle: OnceCell<Library>,
    api: OnceCell<A>,
}

impl<A> VendorLibrary<A> {
    /// Create a cache for these library names, in lookup order.
    ///
    /// # Safety
    /// Loading a library selected by these names must be safe for this process,
    /// including running its initialization routines.
    pub const unsafe fn new(names: &'static [&'static str]) -> Self {
        Self {
            names,
            handle: OnceCell::new(),
            api: OnceCell::new(),
        }
    }

    /// Resolve entry points once. Failed opens and failed symbol lookups can be retried.
    pub fn load(
        &'static self,
        symbols: impl FnOnce(&'static Library) -> Result<A, libloading::Error>,
    ) -> Result<&'static A, LoadError> {
        self.api
            .get_or_try_init(|| {
                let library = self.handle.get_or_try_init(|| self.open())?;
                symbols(library)
            })
            .map_err(|source| LoadError {
                library: self.names.join(" or "),
                source,
            })
    }

    fn open(&self) -> Result<Library, libloading::Error> {
        let mut last = None;
        for name in self.names {
            // SAFETY: the constructor requires every candidate to be safe to load.
            match unsafe { Library::new(*name) } {
                Ok(library) => return Ok(library),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or(libloading::Error::DlOpenUnknown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library every test host has.
    const SYSTEM: &str = if cfg!(target_os = "macos") {
        "/usr/lib/libSystem.B.dylib"
    } else {
        "libc.so.6"
    };

    /// Each state of the slot against each outcome of reading the symbols.
    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot dlopen")]
    fn a_library_loads_once_and_a_missing_symbol_keeps_it() {
        // SAFETY: SYSTEM names the host's system C library.
        static SLOT: VendorLibrary<&Library> = unsafe { VendorLibrary::new(&[SYSTEM]) };
        let missing = |_: &'static Library| Err(libloading::Error::DlSymUnknown);
        assert!(SLOT.load(missing).is_err());
        let first = SLOT.handle.get().unwrap();
        assert!(
            SLOT.load(|library| {
                assert!(std::ptr::eq(library, first));
                missing(library)
            })
            .is_err()
        );
        let api = SLOT.load(Ok).unwrap();
        assert!(std::ptr::eq(*api, first));
        let again = SLOT.load(|_| panic!("read again")).unwrap();
        assert!(std::ptr::eq(again, api));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot dlopen")]
    fn concurrent_callers_share_the_successful_retry() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        // SAFETY: SYSTEM names the host's system C library.
        static SLOT: VendorLibrary<u8> = unsafe { VendorLibrary::new(&[SYSTEM]) };
        let start = Barrier::new(8);
        let attempts = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let callers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        SLOT.load(|_| match attempts.fetch_add(1, Ordering::Relaxed) {
                            0 => Err(libloading::Error::DlSymUnknown),
                            _ => Ok(7),
                        })
                    })
                })
                .collect();
            let successes: Vec<_> = callers
                .into_iter()
                .filter_map(|caller| caller.join().unwrap().ok())
                .collect();
            assert_eq!(successes.len(), 7);
            assert!(successes.iter().all(|api| std::ptr::eq(*api, successes[0])));
        });
        assert_eq!(attempts.into_inner(), 2);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot dlopen")]
    fn a_missing_library_is_a_load_error_and_tried_again() {
        // SAFETY: the test names a nonexistent absolute path.
        static SLOT: VendorLibrary<u8> =
            unsafe { VendorLibrary::new(&["/nonexistent/libprysm-test.so"]) };
        for _ in 0..2 {
            let error = SLOT.load(|_| Ok(1)).unwrap_err();
            assert_eq!(error.library, "/nonexistent/libprysm-test.so");
            assert!(SLOT.handle.get().is_none());
        }
    }
}
