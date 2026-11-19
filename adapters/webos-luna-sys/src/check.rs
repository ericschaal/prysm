//! Detect a known `LSRegister` denial before vendor code allocates state.
use crate::generated::luna::Luna;
use crate::service::{LunaError, check};
use crate::{Error, LUNA_LIBRARY};
use std::ffi::CString;
use std::ptr;

/// Register each name and unregister it again. `Ok` means the caller's role
/// allows all of them right now.
///
/// # Errors
/// [`Error::Load`] off the TV, [`Error::Luna`] when a name is denied.
pub fn check_registration(names: &[String]) -> Result<(), Error> {
    // SAFETY: ABI generated from the TV SDK; the table owns the library for
    // as long as any of its function pointers is used.
    let api = unsafe { Luna::new(LUNA_LIBRARY) }.map_err(|source| Error::Load {
        library: LUNA_LIBRARY.into(),
        source,
    })?;
    check_with(&api, names)
}

pub(crate) fn check_with(api: &Luna, names: &[String]) -> Result<(), Error> {
    for name in names {
        let name = CString::new(name.as_str()).map_err(|_| Error::Nul)?;
        let mut handle = ptr::null_mut();
        check(api, "LSRegister", |error| {
            // SAFETY: live name, initialized error and a writable local handle; no callbacks.
            unsafe { api.LSRegister(name.as_ptr(), &raw mut handle, error) }
        })?;
        // Reuse a fresh error so no fallible step can strand a registered handle.
        let mut error = LunaError::new(api)?;
        // SAFETY: a successful registration owns `handle`; unregister before unloading.
        let unregistered = unsafe { api.LSUnregister(handle, error.raw()) };
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
    use crate::mock;

    #[test]
    fn denial_frees_errors_and_every_successful_registration_is_removed() {
        let names = ["a.1".to_owned(), "b.1".to_owned()];
        for (deny, ok, expected) in [
            (Some("a.1"), false, vec!["init", "register a.1", "free"]),
            (
                Some("b.1"),
                false,
                vec![
                    "init",
                    "register a.1",
                    "free",
                    "init",
                    "unregister",
                    "free",
                    "init",
                    "register b.1",
                    "free",
                ],
            ),
            (
                None,
                true,
                vec![
                    "init",
                    "register a.1",
                    "free",
                    "init",
                    "unregister",
                    "free",
                    "init",
                    "register b.1",
                    "free",
                    "init",
                    "unregister",
                    "free",
                ],
            ),
        ] {
            mock::reset();
            mock::deny_register(deny);
            let result = check_with(&mock::luna(), &names);
            assert_eq!(result.is_ok(), ok, "{result:?}");
            if let Err(error) = result {
                assert!(error.to_string().contains("denied"), "{error}");
            }
            assert_eq!(mock::calls(), expected);
        }
    }
}
