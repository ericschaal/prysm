//! Prevent the known `LSRegister` denial before allocating vendor video state.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
};

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    clippy::all,
    clippy::pedantic
)]
mod ffi {
    include!("generated/luna.rs");
    #[cfg(test)]
    impl Luna {
        pub(super) fn test_api(
            init: unsafe extern "C" fn(*mut LSError) -> bool,
            free: unsafe extern "C" fn(*mut LSError),
            register: unsafe extern "C" fn(
                *const std::ffi::c_char,
                *mut *mut LSHandle,
                *mut LSError,
            ) -> bool,
            unregister: unsafe extern "C" fn(*mut LSHandle, *mut LSError) -> bool,
        ) -> Self {
            Self {
                __library: libloading::os::unix::Library::this().into(),
                LSErrorInit: init,
                LSErrorFree: free,
                LSRegister: register,
                LSUnregister: unregister,
            }
        }
    }
}

struct Error<'a> {
    api: &'a ffi::Luna,
    raw: ffi::LSError,
}
impl<'a> Error<'a> {
    fn new(api: &'a ffi::Luna) -> Result<Self> {
        let mut raw = MaybeUninit::uninit();
        // SAFETY: SDK LSErrorInit initializes the entire aligned output on success.
        ensure!(
            unsafe { api.LSErrorInit(raw.as_mut_ptr()) },
            "initialize Luna error"
        );
        // SAFETY: successful initialization above made every field valid.
        Ok(Self {
            api,
            raw: unsafe { raw.assume_init() },
        })
    }
    fn check(&self, success: bool) -> Result<()> {
        let message = if !success && !self.raw.message.is_null() {
            // SAFETY: initialized LSError owns this C string until Error is dropped.
            unsafe { CStr::from_ptr(self.raw.message) }
                .to_string_lossy()
                .into_owned()
        } else {
            "Luna registration failed".into()
        };
        ensure!(success, "{message}");
        Ok(())
    }
}
impl Drop for Error<'_> {
    fn drop(&mut self) {
        // SAFETY: free our one initialized error, on every success/failure path.
        unsafe {
            self.api.LSErrorFree(&raw mut self.raw);
        }
    }
}
pub(super) fn check() -> Result<()> {
    // SAFETY: ABI generated from the same TV SDK as the worker; table owns the library.
    let api = unsafe { ffi::Luna::new("/usr/lib/libluna-service2.so.3") }
        .context("load Luna for capture permission check")?;
    check_with(&api)
}
fn check_with(api: &ffi::Luna) -> Result<()> {
    for prefix in [
        "com.webos.rm.client.prysm",
        "com.webos.service.capture.client.prysm",
    ] {
        let name = CString::new(format!("{prefix}.{}", std::process::id()))?;
        let mut handle = ptr::null_mut();
        let mut error = Error::new(api)?;
        // SAFETY: live name, initialized error and writable local handle; no callbacks.
        let registered =
            unsafe { api.LSRegister(name.as_ptr(), &raw mut handle, &raw mut error.raw) };
        error
            .check(registered)
            .context("capture Luna permission check")?;
        // Reuse the initialized error so no fallible allocation can strand a handle.
        // SAFETY: a successful registration owns a handle; unregister before unloading.
        let unregistered = unsafe { api.LSUnregister(handle, &raw mut error.raw) };
        if !unregistered {
            // The library may still own live resources; never unload or retry this state.
            std::process::abort();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, ffi::c_char};
    thread_local! { static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) }; }
    thread_local! { static DENY: RefCell<usize> = const { RefCell::new(0) }; }
    unsafe extern "C" fn init(error: *mut ffi::LSError) -> bool {
        CALLS.with_borrow_mut(|calls| calls.push("init"));
        // SAFETY: caller supplies writable LSError storage; all-zero fields are valid.
        unsafe {
            error.write(std::mem::zeroed());
        }
        true
    }
    unsafe extern "C" fn free(_: *mut ffi::LSError) {
        CALLS.with_borrow_mut(|calls| calls.push("free"));
    }
    unsafe extern "C" fn register(
        _: *const c_char,
        handle: *mut *mut ffi::LSHandle,
        _: *mut ffi::LSError,
    ) -> bool {
        CALLS.with_borrow_mut(|calls| calls.push("register"));
        let denied = DENY.with_borrow_mut(|remaining| {
            *remaining = remaining.saturating_sub(1);
            *remaining == 0
        });
        if denied {
            return false;
        }
        // SAFETY: writable output from caller; sentinel is never dereferenced.
        unsafe {
            handle.write(std::ptr::NonNull::dangling().as_ptr());
        }
        true
    }
    unsafe extern "C" fn unregister(_: *mut ffi::LSHandle, _: *mut ffi::LSError) -> bool {
        CALLS.with_borrow_mut(|calls| calls.push("unregister"));
        true
    }
    #[test]
    fn denial_frees_errors_and_every_successful_registration_is_removed() {
        for (deny, expected) in [
            (1, vec!["init", "register", "free"]),
            (
                2,
                vec![
                    "init",
                    "register",
                    "unregister",
                    "free",
                    "init",
                    "register",
                    "free",
                ],
            ),
            (
                3,
                vec![
                    "init",
                    "register",
                    "unregister",
                    "free",
                    "init",
                    "register",
                    "unregister",
                    "free",
                ],
            ),
        ] {
            CALLS.with_borrow_mut(Vec::clear);
            DENY.set(deny);
            let api = ffi::Luna::test_api(init, free, register, unregister);
            assert_eq!(check_with(&api).is_ok(), deny == 3);
            assert_eq!(CALLS.with_borrow(Clone::clone), expected);
        }
    }
}
