//! Compose the pipeline, lighting control, and REST API; own application shutdown.
use crate::{
    actors::runtime::Pipeline,
    config::PipelineConfig,
    control::{Controller, LightingService},
};
use anyhow::{Context as _, Result};
use prysm_contract::Config;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub fn run(path: &Path, config: Config) -> Result<()> {
    crate::validate_config(&config)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(serve(path, config, CancellationToken::new(), |_| {}))
}

async fn serve(
    path: &Path,
    config: Config,
    shutdown: CancellationToken,
    ready: impl FnOnce(std::net::SocketAddr),
) -> Result<()> {
    crate::validate_config(&config)?;
    let listener = tokio::net::TcpListener::bind(config.web.listen).await?;
    let preview = crate::preview::Preview::new();
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (pipeline, status) =
        Pipeline::with_preview(PipelineConfig::from(&config), preview.publisher())?;
    let lighting = match LightingService::new(config_rx, pipeline.clone(), status) {
        Ok(lighting) => Arc::new(lighting),
        Err(error) => {
            pipeline
                .shutdown()
                .await
                .context("cannot clean up after lighting setup failure")?;
            return Err(error);
        }
    };
    let (controller, app) = Controller::spawn(path.to_path_buf(), &config, lighting, config_tx);
    ready(listener.local_addr()?);
    let router =
        crate::control::http::router_with_preview(app, config.web.clone(), preview.router());
    tracing::info!(address=%config.web.listen,"Prysm listening");
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future();
    tokio::pin!(server);
    let result = tokio::select! {
        result = &mut server => Some(result),
        () = shutdown_signal() => None,
        () = shutdown.cancelled() => None,
    };
    shutdown.cancel();
    let controller_result = controller.shutdown().await;
    let pipeline_result = pipeline.shutdown().await;
    let preview_result = preview.shutdown().await;
    controller_result?;
    pipeline_result?;
    preview_result?;
    if let Some(result) = result {
        return result.context("HTTP server failed");
    }
    if let Ok(result) = tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        result.context("HTTP server failed")
    } else {
        tracing::warn!("HTTP shutdown timed out; closing remaining connections");
        Ok(())
    }
}

async fn shutdown_signal() {
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
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn shutdown_does_not_wait_forever_for_an_incomplete_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prysm.toml");
        let mut config: Config = toml::from_str(include_str!("../../../prysm.toml")).unwrap();
        config.web.listen = "127.0.0.1:0".parse().unwrap();
        config.web.allowed_hosts = vec!["*".into()];
        let shutdown = CancellationToken::new();
        let stopped = shutdown.clone();
        let (ready, address) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            serve(&path, config, stopped, |address| {
                ready.send(address).unwrap();
            })
            .await
        });
        let mut client = tokio::net::TcpStream::connect(address.await.unwrap())
            .await
            .unwrap();
        client
            .write_all(b"PUT /api/state HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n")
            .await
            .unwrap();
        // The interim response proves the server is waiting for this request's body.
        let mut interim = [0; 25];
        tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut interim))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
        client.write_all(b"{").await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(8), server)
            .await
            .expect("an incomplete request must not prevent shutdown")
            .unwrap()
            .unwrap();
    }
}
