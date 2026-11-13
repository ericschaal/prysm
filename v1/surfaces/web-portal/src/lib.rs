//! The web portal surface: serves the host's frontend bundle and a small JSON
//! API over plain HTTP on any address, without authentication. Browser pages
//! from other origins are refused unless [`Access`] allows them. See
//! `HTTP-CONTRACT.md` for routes, the request policy and limits.
//!
//! ```ignore
//! Prysm::builder(store)
//!     .source(DeviceSource)
//!     .output(Wled)
//!     .surface(WebPortal::from_listener(listener, ASSETS, Access::SameOrigin)?)
//!     .run(shutdown)
//!     .await
//! ```
mod access;
mod colors;
mod metrics;
mod preview;
mod response;
mod routes;
mod server;
mod timed_io;

pub use access::Access;

use futures::future::BoxFuture;
use prysm_ports::Result;
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};

/// A host-built asset; paths are exact URL paths, never filesystem paths.
#[derive(Debug, Clone)]
pub struct FrontendAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

/// The portal surface. Construction validates the assets and access;
/// [`Surface::run`] serves until shutdown.
#[derive(Debug)]
pub struct WebPortal {
    pub(crate) listener: TcpListener,
    pub(crate) assets: &'static [FrontendAsset],
    pub(crate) access: Access,
    pub(crate) metrics: bool,
}

impl WebPortal {
    /// Serve on an already bound listener, so bind failures surface before the
    /// host starts and the port is known in advance.
    ///
    /// # Errors
    /// [`prysm_ports::Error::Invalid`] for invalid or duplicate asset paths or
    /// a malformed origin in `access`.
    pub fn from_listener(
        listener: TcpListener,
        assets: &'static [FrontendAsset],
        access: Access,
    ) -> Result<Self> {
        access::validate_assets(assets)?;
        Ok(Self {
            listener,
            assets,
            access: access.validated()?,
            metrics: false,
        })
    }

    /// Also serve `GET /metrics` in the Prometheus text format (off by
    /// default: the path is then `404` like any unknown one).
    #[must_use]
    pub fn metrics(mut self, enabled: bool) -> Self {
        self.metrics = enabled;
        self
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

/// The URL a browser elsewhere on the LAN opens for a portal bound to
/// `address`. For a wildcard address this is the address of the interface the
/// default route uses; `None` when there is none, or it is only loopback.
#[must_use]
pub fn lan_url(address: SocketAddr) -> Option<String> {
    let ip = if address.ip().is_unspecified() {
        default_route_address(address.ip())?
    } else {
        address.ip()
    };
    Some(format!("http://{}/", SocketAddr::new(ip, address.port())))
}

/// Connecting a UDP socket only picks a route and a local address; nothing is sent.
fn default_route_address(unspecified: IpAddr) -> Option<IpAddr> {
    let (local, probe) = if unspecified.is_ipv4() {
        ("0.0.0.0:0", "192.0.2.1:9")
    } else {
        ("[::]:0", "[2001:db8::1]:9")
    };
    let socket = UdpSocket::bind(local).ok()?;
    socket.connect(probe).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}
