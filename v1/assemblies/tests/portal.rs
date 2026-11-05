//! The portal accepts a different control implementation without the local coordinator.
//! This is an offline client fixture, not a network protocol implementation.
use prysm_contract::{
    ControlError,
    application::{
        Accepted, ApplicationCommand, ApplicationControl, ApplicationObservation, OperationStatus,
        Request,
    },
    calibration::{CalibrationCommand, CalibrationReply},
    identity::OperationId,
};
use prysm_web_portal::{FrontendAsset, HttpPolicy, WebPortal};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct OfflineClient(Arc<AtomicUsize>);

impl ApplicationControl for OfflineClient {
    type Settings = ();
    async fn submit(
        &self,
        _request: Request<ApplicationCommand<()>>,
    ) -> Result<Accepted, ControlError> {
        Err(ControlError::Unavailable("fixture remote endpoint offline"))
    }
    async fn observe(&self) -> Result<ApplicationObservation<()>, ControlError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ControlError::Unavailable("fixture remote endpoint offline"))
    }
    async fn operation(&self, _id: OperationId) -> Result<OperationStatus, ControlError> {
        Err(ControlError::UnknownOperation)
    }
    async fn calibrate(
        &self,
        _request: Request<CalibrationCommand>,
    ) -> Result<CalibrationReply, ControlError> {
        Err(ControlError::Unavailable("fixture remote endpoint offline"))
    }
}

#[tokio::test]
async fn no_preview_portal_accepts_an_independent_client_and_preserves_its_failure() {
    let requests = Arc::new(AtomicUsize::new(0));
    let portal = WebPortal::without_preview(
        OfflineClient(requests.clone()),
        &[FrontendAsset {
            path: "/index.html",
            content_type: "text/html; charset=utf-8",
            bytes: b"<!doctype html><title>Independent host frontend</title>",
        }],
        HttpPolicy {
            allowed_authorities: vec!["localhost:8787".into()],
        },
    );
    assert!(portal.preview.is_none());
    assert!(matches!(
        portal.application.observe().await,
        Err(ControlError::Unavailable("fixture remote endpoint offline"))
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(matches!(
        portal.run(std::future::pending()).await,
        Err(ControlError::Unavailable(_))
    ));
}
