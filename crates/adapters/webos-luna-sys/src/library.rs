//! The TV's vendor libraries, each loaded once per process and never
//! unloaded: vendor code can leave threads running, global pointers set or
//! descriptors open, so a `dlclose` could unmap code that still runs. Also
//! used by `prysm-capture-webos-sys` for `libvtcapture` and `libhalgal`.
use crate::Error;
use libloading::Library;
use std::sync::{Mutex, PoisonError};

/// How far loading a library and its entry points `A` has come.
pub enum Loaded<A: 'static> {
    Nothing,
    /// The library is loaded (and stays loaded); an entry point was missing.
    Library(&'static Library),
    Api(&'static A),
}

/// The entry points `symbols` reads from the first of `names` that loads.
/// The library and the entry points are kept in `slot` and never unloaded;
/// a retry after a missing symbol reuses the library instead of loading,
/// and leaking, another handle.
///
/// # Errors
/// [`Error::Load`] when no name loads, or what `symbols` returns.
pub fn load_once<A>(
    slot: &Mutex<Loaded<A>>,
    names: &[&str],
    symbols: impl FnOnce(&'static Library) -> Result<A, Error>,
) -> Result<&'static A, Error> {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let library = match *slot {
        Loaded::Api(api) => return Ok(api),
        Loaded::Library(library) => library,
        Loaded::Nothing => {
            let library: &'static Library = Box::leak(Box::new(open(names)?));
            *slot = Loaded::Library(library);
            library
        }
    };
    let api: &'static A = Box::leak(Box::new(symbols(library)?));
    *slot = Loaded::Api(api);
    Ok(api)
}

/// The first of `names` that loads.
fn open(names: &[&str]) -> Result<Library, Error> {
    let mut last = None;
    for name in names {
        // SAFETY: only the TV's own system libraries are named (LS2, GLib,
        // VTCapture, HAL GAL); their constructors run as for any process that
        // links them, and each is loaded once per process.
        match unsafe { Library::new(*name) } {
            Ok(library) => return Ok(library),
            Err(error) => last = Some(error),
        }
    }
    Err(Error::Load {
        library: names.join(" or "),
        source: last.unwrap_or(libloading::Error::DlOpenUnknown),
    })
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
        // The entry points: the library they were read from.
        static SLOT: Mutex<Loaded<&'static Library>> = Mutex::new(Loaded::Nothing);
        let missing = |_: &'static Library| {
            Err(Error::Luna {
                operation: "symbol",
                message: "missing".into(),
            })
        };
        // Nothing, symbols missing: the library stays loaded.
        assert!(load_once(&SLOT, &[SYSTEM], missing).is_err());
        let Loaded::Library(first) = *SLOT.lock().unwrap() else {
            panic!("the library is kept");
        };
        // Library, symbols missing again: the same library, not a new one.
        assert!(
            load_once(&SLOT, &[SYSTEM], |library| {
                assert!(std::ptr::eq(library, first));
                missing(library)
            })
            .is_err()
        );
        // Library, symbols found: the entry points are kept.
        let api = load_once(&SLOT, &[SYSTEM], Ok).unwrap();
        assert!(std::ptr::eq(*api, first));
        // Entry points: returned without reading anything.
        let again = load_once(&SLOT, &[SYSTEM], |_| panic!("read again")).unwrap();
        assert!(std::ptr::eq(again, api));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot dlopen")]
    fn a_missing_library_is_a_load_error_and_tried_again() {
        static SLOT: Mutex<Loaded<u8>> = Mutex::new(Loaded::Nothing);
        let missing = ["/nonexistent/libprysm-test.so"];
        for _ in 0..2 {
            let error = load_once(&SLOT, &missing, |_| Ok(1)).err().unwrap();
            assert!(matches!(error, Error::Load { .. }), "{error}");
            assert!(matches!(*SLOT.lock().unwrap(), Loaded::Nothing));
        }
    }
}
