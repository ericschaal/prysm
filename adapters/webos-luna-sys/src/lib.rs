//! LG webOS Luna service bus (LS2) and a private GLib main loop behind a safe
//! API. The only Luna binding in v1: `prysm-capture-webos-sys` uses
//! [`check_registration`] from here.
//!
//! - [`run`]: register a service name with methods, keep subscription calls
//!   to other services, tick a [`Handler`] on a timer, until it says stop.
//! - [`check_registration`]: register and immediately unregister names, to
//!   detect a missing role before vendor code allocates anything.
//!
//! Bindings are generated from the TV SDK headers (see [`generated`] and the
//! design doc); functions are loaded with `libloading` at run time, so this
//! crate builds anywhere. Off a webOS TV [`run`] refuses with
//! [`Error::Unsupported`] before touching any library.
//!
//! Safety contract kept by this crate, never by its callers: callback state
//! lives in one shared allocation registered with LS2 and freed only after
//! `LSUnregister`; callbacks run only on the thread inside [`run`]; a panic in
//! a callback or a failed unregister aborts instead of unwinding into C or
//! freeing memory LS2 may still use.
mod check;
mod service;

pub use check::check_registration;
pub use service::{Bus, Config, Handler, Request, Response, run};

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    unsafe_op_in_unsafe_fn
)]
pub(crate) mod generated {
    //! `rust-bindgen` output from the TV SDK headers. Regenerate with the
    //! SDK; never edit by hand.
    pub mod luna {
        include!("generated/luna.rs");
        #[cfg(test)]
        impl Luna {
            /// Every function from `crate::mock`, on the test process itself.
            pub(crate) fn test_api() -> Self {
                use crate::mock as m;
                Self {
                    __library: m::library(),
                    LSErrorInit: m::error_init,
                    LSErrorFree: m::error_free,
                    LSRegister: m::register,
                    LSRegisterCategory: m::category,
                    LSCategorySetData: m::category_data,
                    LSUnregister: m::unregister,
                    LSMessageRef: m::message_ref,
                    LSMessageUnref: m::message_ref,
                    LSMessageGetApplicationID: m::application_id,
                    LSMessageGetSenderServiceName: m::sender_service,
                    LSMessageGetMethod: m::method,
                    LSMessageGetPayload: m::payload,
                    LSMessageReply: m::reply,
                    LSGmainAttach: m::attach,
                    LSCall: m::call,
                    LSCallCancel: m::cancel,
                    LSSubscriptionProcess: m::subscribe,
                    LSSubscriptionReply: m::publish,
                    LSSubscriptionGetHandleSubscribersCount: m::subscribers,
                }
            }
        }
    }
    pub mod glib {
        include!("generated/glib.rs");
        #[cfg(test)]
        impl Glib {
            pub(crate) fn test_api() -> Self {
                use crate::mock as m;
                Self {
                    __library: m::library(),
                    g_main_context_new: m::context_new,
                    g_main_context_unref: m::unref,
                    g_main_loop_new: m::loop_new,
                    g_main_loop_run: m::loop_run,
                    g_main_loop_quit: m::loop_quit,
                    g_main_loop_unref: m::unref,
                    g_source_unref: m::source_unref,
                    g_source_attach: m::source_attach,
                    g_source_destroy: m::source_unref,
                    g_source_set_callback: m::source_callback,
                    g_timeout_source_new: m::timer_new,
                }
            }
        }
    }
}

/// The TV's libraries.
pub(crate) const LUNA_LIBRARY: &str = "/usr/lib/libluna-service2.so.3";
pub(crate) const GLIB_LIBRARY: &str = "/usr/lib/libglib-2.0.so.0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("cannot load {library}")]
    Load {
        library: String,
        #[source]
        source: libloading::Error,
    },
    /// An LS2 call failed; the text is LS2's own message.
    #[error("{operation}: {message}")]
    Luna {
        operation: &'static str,
        message: String,
    },
    #[error("{0}")]
    Glib(&'static str),
    #[error("invalid name or payload: contains NUL")]
    Nul,
}

#[cfg(test)]
mod mock;
