//! HTTP, one configuration file, WLED switching, and the actor pipeline.
mod backend;
mod controller;
mod http;
mod persistence;
mod switching;

use crate::actors::{pipeline::Renderers, runtime::StopPipeline};
use anyhow::{Context as _, Result};
use backend::LightingService;
use controller::{Controller, Shutdown};
use prysm_contract::Config;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Serve the controller on the application Actix runtime.
pub async fn run(path: &Path, config: Config) -> Result<()> {
    serve(
        path,
        config,
        Renderers::default(),
        CancellationToken::new(),
        |_| {},
    )
    .await
}

pub(crate) async fn serve(
    path: &Path,
    config: Config,
    renderers: Renderers,
    shutdown: CancellationToken,
    ready: impl FnOnce(std::net::SocketAddr),
) -> Result<()> {
    crate::validate_config(&config)?;
    let listener = tokio::net::TcpListener::bind(config.web.listen).await?;
    let (config_tx, config_rx) = watch::channel(config.clone());
    let lighting = Arc::new(LightingService::new(config_rx, renderers)?);
    let (controller, app) =
        Controller::spawn(path.to_path_buf(), &config, lighting.clone(), config_tx);
    ready(listener.local_addr()?);
    let router = http::router(app, config.web.clone());
    tracing::info!(address=%config.web.listen,"Prysm listening");
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                () = shutdown_signal() => {},
                () = shutdown.cancelled() => {},
            }
        })
        .await;
    if tokio::time::timeout(Duration::from_secs(60), controller.send(Shutdown))
        .await
        .is_err()
    {
        tracing::warn!("controller shutdown timed out");
        controller.do_send(crate::actors::Stop);
    }
    lighting
        .pipeline
        .send(StopPipeline)
        .await
        .context("pipeline unavailable")??;
    result.context("HTTP server failed")
}

pub(crate) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminated =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("termination signal");
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminated.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests;
