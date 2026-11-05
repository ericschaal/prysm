//! Optional portal server assembly with host-built frontend assets. HTTP/encoding are deferred.
use prysm_contract::{
    ControlError, OwnerStopped,
    application::ApplicationControl,
    preview::{PreviewLease, PreviewSource},
};
use std::future::Future;

/// One file from the frontend assembled and embedded by the host.
/// The portal has no knowledge of the adapter settings panels in that bundle.
#[derive(Debug, Clone)]
pub struct FrontendAsset {
    /// Absolute URL path, for example `/index.html`; never a filesystem path.
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

/// Host-supplied authorities; validation/enforcement belongs to the HTTP implementation.
#[derive(Debug, Clone)]
pub struct HttpPolicy {
    pub allowed_authorities: Vec<String>,
}

pub struct WebPortal<A, P = NoPreview> {
    pub application: A,
    /// None disables runtime preview. Future encoder dependencies need a Cargo feature.
    pub preview: Option<P>,
    /// Host-owned bundle, including `/index.html` and its referenced assets.
    /// HTTP routing and asset validation are deferred with server implementation.
    pub frontend_assets: &'static [FrontendAsset],
    pub http_policy: HttpPolicy,
}

#[derive(Clone)]
pub struct NoPreview;

impl PreviewSource for NoPreview {
    async fn subscribe(&self) -> Result<PreviewLease, ControlError> {
        Err(ControlError::Unavailable("preview is disabled"))
    }
}

impl<A: ApplicationControl> WebPortal<A, NoPreview> {
    pub fn without_preview(
        application: A,
        frontend_assets: &'static [FrontendAsset],
        http_policy: HttpPolicy,
    ) -> Self {
        Self {
            application,
            preview: None,
            frontend_assets,
            http_policy,
        }
    }
}

impl<A: ApplicationControl, P: PreviewSource> WebPortal<A, P> {
    pub fn new(
        application: A,
        preview: Option<P>,
        frontend_assets: &'static [FrontendAsset],
        http_policy: HttpPolicy,
    ) -> Self {
        Self {
            application,
            preview,
            frontend_assets,
            http_policy,
        }
    }

    /// Listener/server integration is deferred; no socket or assets are opened here.
    pub async fn run(
        self,
        _shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: portal HTTP and encoding are not implemented",
        ))
    }
}
