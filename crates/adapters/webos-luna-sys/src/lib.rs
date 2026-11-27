//! LG webOS Luna service bus (LS2) and a private GLib main loop behind a safe
//! API. The only Luna binding in the workspace: `prysm-capture-webos-sys` uses
//! [`check_registration`] from here.
//!
//! - [`run`]: register a service name with methods, keep subscription calls
//!   to other services, and poll a [`Handler`] at every event (a request, a
//!   reply, a [`Wake`]) and at the time it asks for, until it says stop.
//! - [`check_registration`]: register and immediately unregister names, to
//!   detect a missing role before vendor code allocates anything.
//!
//! Bindings are generated from the TV SDK headers (see [`generated`] and the
//! design doc); functions are loaded with `libloading` at run time, so this
//! crate builds anywhere. Off a webOS TV [`run`] refuses with
//! [`Error::Unsupported`] before touching any library.
//!
//! - [`VendorLibrary`]: load a vendor library and its entry points once per
//!   process, never to unload them; LS2 and GLib here, VTCapture and HAL GAL
//!   in `prysm-capture-webos-sys`.
//!
//! Safety contract kept by this crate, never by its callers: callback state
//! lives in one shared allocation registered with LS2 and freed only after
//! `LSUnregister`; callbacks run only on the thread inside [`run`]; a panic in
//! a callback, or a failed unregister of a service with callbacks, aborts
//! instead of unwinding into C or freeing memory LS2 may still use. No
//! library is ever unloaded.
mod check;
mod library;
mod service;

pub use check::check_registration;
pub use library::VendorLibrary;
pub use service::{
    Bus, Config, Handler, MAX_PAYLOAD, Next, Refusal, Request, Response, Wake, Waker, run,
};

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
                    g_source_destroy: m::source_destroy,
                    g_source_set_callback: m::source_callback,
                    g_timeout_source_new: m::timer_new,
                    g_io_channel_unix_new: m::channel_new,
                    g_io_channel_set_close_on_unref: m::channel_close_on_unref,
                    g_io_channel_unref: m::channel_unref,
                    g_io_create_watch: m::watch_new,
                }
            }
        }
    }
}

/// The TV's libraries.
const LUNA_LIBRARY: &str = "/usr/lib/libluna-service2.so.3";
const GLIB_LIBRARY: &str = "/usr/lib/libglib-2.0.so.0";

/// LS2's entry points, loaded on first use for the life of the process.
fn luna() -> Result<&'static generated::luna::Luna, Error> {
    // SAFETY: the SDK's LS2 library is safe to load in the service process.
    static LUNA: VendorLibrary<generated::luna::Luna> =
        unsafe { VendorLibrary::new(&[LUNA_LIBRARY]) };
    LUNA.load(|_| {
        // SAFETY: the pinned LS2 library matches the generated SDK signatures.
        unsafe { generated::luna::Luna::new(LUNA_LIBRARY) }
    })
}

/// GLib's entry points, loaded on first use for the life of the process.
fn glib() -> Result<&'static generated::glib::Glib, Error> {
    // SAFETY: the SDK's GLib library is safe to load in the service process.
    static GLIB: VendorLibrary<generated::glib::Glib> =
        unsafe { VendorLibrary::new(&[GLIB_LIBRARY]) };
    GLIB.load(|_| {
        // SAFETY: the pinned GLib library matches the generated SDK signatures.
        unsafe { generated::glib::Glib::new(GLIB_LIBRARY) }
    })
}

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
    /// LS2 refused to register `name`: the caller's role does not allow it.
    #[error("Luna denied the registration of {name}: {message}")]
    Denied { name: String, message: String },
    /// LS2 could not remove `name` after registering it, so it may stay
    /// registered for the life of this process.
    #[error("Luna could not unregister {name}: {message}")]
    Unremoved { name: String, message: String },
    /// An LS2 call failed; the text is LS2's own message.
    #[error("{operation}: {message}")]
    Luna {
        operation: &'static str,
        message: String,
    },
    #[error("{0}")]
    Glib(&'static str),
    #[error("cannot use the wake socket")]
    Wake(#[source] std::io::Error),
    #[error("invalid name or payload: contains NUL")]
    Nul,
}

#[cfg(test)]
mod mock;
