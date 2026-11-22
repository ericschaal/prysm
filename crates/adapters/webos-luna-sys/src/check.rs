//! Detect a known `LSRegister` denial before vendor code allocates state.
use crate::Error;
use crate::generated::luna::Luna;
use crate::service::{LunaError, check};
use std::ffi::CString;
use std::ptr;

/// How LS2's message starts when the caller's role does not allow a name, as
/// a TV answers (`research/webos-capture/2026-11-01-lifecycle/evidence/`).
const DENIED: &str = "Invalid permissions";

/// Register each name and unregister it again. `Ok` means the caller's role
/// allows all of them right now.
///
/// # Errors
/// [`Error::Load`] off the TV, [`Error::Denied`] when a name is denied,
/// [`Error::Unremoved`] when one stays registered, and [`Error::Luna`] when
/// LS2 fails otherwise.
pub fn check_registration(names: &[String]) -> Result<(), Error> {
    check_with(crate::luna()?, names)
}

pub(crate) fn check_with(api: &Luna, names: &[String]) -> Result<(), Error> {
    for name in names {
        let text = CString::new(name.as_str()).map_err(|_| Error::Nul)?;
        // Initialized before registering, so no fallible step lies between a
        // registration and its removal.
        let mut unregister_error = LunaError::new(api)?;
        let mut handle = ptr::null_mut();
        check(api, "LSRegister", |error| {
            // SAFETY: live name, initialized error and a writable local handle; no callbacks.
            unsafe { api.LSRegister(text.as_ptr(), &raw mut handle, error) }
        })
        .map_err(|error| match error {
            Error::Luna { message, .. } if message.starts_with(DENIED) => Error::Denied {
                name: name.clone(),
                message,
            },
            other => other,
        })?;
        // SAFETY: a successful registration owns `handle`, which has no
        // callbacks or user data, so no Rust memory depends on its removal.
        if !unsafe { api.LSUnregister(handle, unregister_error.raw()) } {
            // LS2 may still own the handle; the library stays loaded anyway.
            return Err(Error::Unremoved {
                name: name.clone(),
                message: unregister_error.message(),
            });
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
        // Each name: the unregister error, the register error, register,
        // free the register error, then unregister and free its error.
        let registered = |name| {
            vec![
                "init".to_owned(),
                "init".to_owned(),
                format!("register {name}"),
                "free".to_owned(),
            ]
        };
        let unregistered = || vec!["unregister".to_owned(), "free".to_owned()];
        let denied = |name| [registered(name), vec!["free".to_owned()]].concat();
        for (deny, ok, expected) in [
            (Some("a.1"), false, denied("a.1")),
            (
                Some("b.1"),
                false,
                [registered("a.1"), unregistered(), denied("b.1")].concat(),
            ),
            (
                None,
                true,
                [
                    registered("a.1"),
                    unregistered(),
                    registered("b.1"),
                    unregistered(),
                ]
                .concat(),
            ),
        ] {
            mock::reset();
            mock::deny_register(deny);
            let result = check_with(mock::luna(), &names);
            assert_eq!(result.is_ok(), ok, "{result:?}");
            if let Err(error) = result {
                assert!(error.to_string().contains("Invalid permissions"), "{error}");
            }
            assert_eq!(mock::calls(), expected);
        }
    }

    #[test]
    fn a_failed_unregister_is_an_error_not_an_abort() {
        mock::reset();
        mock::fail(Some("unregister"));
        let names = ["a.1".to_owned(), "b.1".to_owned()];
        let error = check_with(mock::luna(), &names).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Luna could not unregister a.1: failed (code 0)"
        );
        assert!(matches!(error, Error::Unremoved { .. }));
        assert!(
            !mock::calls().contains(&"register b.1".to_owned()),
            "no name after a failed unregister"
        );
    }
}
