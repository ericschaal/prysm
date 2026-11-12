//! The web portal surface: serves the host's frontend bundle and a small JSON
//! API over plain HTTP on any address, without authentication. Browser pages
//! from other origins are refused unless [`Access`] allows them. See
//! `HTTP-CONTRACT.md` for routes, the request policy and limits.
//!
//! ```ignore
//! Prysm::builder(store)
//!     .source(DeviceSource)
//!     .output(Wled)
//!     .surface(WebPortal::new(listen, ASSETS, Access::SameOrigin)?)
//!     .run(shutdown)
//!     .await
//! ```
mod access;
mod preview;
mod response;
mod routes;
mod server;
mod timed_io;

pub use access::Access;

use futures::future::BoxFuture;
use prysm_ports::Result;
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use std::net::SocketAddr;

/// A host-built asset; paths are exact URL paths, never filesystem paths.
#[derive(Debug, Clone)]
pub struct FrontendAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

/// The portal surface. Construction validates the assets and access;
/// [`Surface::run`] binds (unless given a listener) and serves until shutdown.
#[derive(Debug)]
pub struct WebPortal {
    pub(crate) listen: Listen,
    pub(crate) assets: &'static [FrontendAsset],
    pub(crate) access: Access,
}

#[derive(Debug)]
pub(crate) enum Listen {
    Address(SocketAddr),
    Listener(std::net::TcpListener),
}

impl WebPortal {
    /// Serve on `listen`, bound when the surface starts.
    ///
    /// # Errors
    /// [`prysm_ports::Error::Invalid`] for invalid or duplicate asset paths or
    /// a malformed origin in `access`.
    pub fn new(
        listen: SocketAddr,
        assets: &'static [FrontendAsset],
        access: Access,
    ) -> Result<Self> {
        Self::with(Listen::Address(listen), assets, access)
    }

    /// Serve on an already bound listener, so bind failures surface before the
    /// host starts and the port is known in advance.
    ///
    /// # Errors
    /// As [`WebPortal::new`].
    pub fn from_listener(
        listener: std::net::TcpListener,
        assets: &'static [FrontendAsset],
        access: Access,
    ) -> Result<Self> {
        Self::with(Listen::Listener(listener), assets, access)
    }

    fn with(listen: Listen, assets: &'static [FrontendAsset], access: Access) -> Result<Self> {
        access::validate_assets(assets)?;
        Ok(Self {
            listen,
            assets,
            access: access.validated()?,
        })
    }
}

impl Surface for WebPortal {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(server::serve(*self, engine, shutdown))
    }
}
