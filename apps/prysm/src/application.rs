//! Compose the pipeline, lighting control, and REST API; own application shutdown.
use crate::{
    actors::{
        pipeline::Renderers,
        runtime::{Pipeline, StopPipeline},
    },
    config::PipelineConfig,
    control::{Controller, LightingService, Shutdown},
};
use anyhow::{Context as _, Result};
use prysm_contract::Config;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "desktop")]
mod controls;

pub fn run(path: &Path, config: Config) -> Result<()> {
    crate::validate_config(&config)?;
    if config.output.is_egui() {
        #[cfg(feature = "desktop")]
        return run_with_window(path, config);
        #[cfg(not(feature = "desktop"))]
        anyhow::bail!("egui output requires building with --features desktop");
    }
    actix::System::new().block_on(serve(
        path,
        config,
        Renderers::default(),
        CancellationToken::new(),
        |_| {},
    ))
}

#[cfg(feature = "desktop")]
fn run_with_window(path: &Path, config: Config) -> Result<()> {
    use crate::actors::egui::Egui;
    use anyhow::{Context as _, anyhow};
    use prysm_capture::Frame;
    use prysm_core::EdgeColors;
    use prysm_renderer_desktop::{DesktopRendererBuilder, DisplayFrame};

    let [width, height] = config.input.capture_resolution().dimensions();
    let shutdown = CancellationToken::new();
    let (colors, colors_rx) = watch::channel(EdgeColors::default());
    let (frames, frames_rx) = watch::channel(DisplayFrame {
        frame: Frame::dummy(width, height)?,
        viewport: None,
    });
    let (mut controls, connect) = controls::Controls::new(&config.web, shutdown.clone())?;
    let app = DesktopRendererBuilder::new(colors_rx)
        .with_toolbar(move |ui| controls.show(ui))
        .with_frame_rx(frames_rx)
        .with_shutdown_token(&shutdown)
        .build();
    let renderers = Renderers {
        egui: Some(Egui { colors, frames }),
    };
    let path = path.to_path_buf();
    let worker_shutdown = shutdown.clone();
    let worker = std::thread::Builder::new()
        .name("prysm-api".into())
        .spawn(move || {
            // Every runtime exit wakes and closes the native window.
            let _close_window = worker_shutdown.clone().drop_guard();
            actix::System::new().block_on(serve(&path, config, renderers, worker_shutdown, connect))
        })
        .context("cannot start API runtime")?;
    let window = prysm_renderer_desktop::run(app, &shutdown);
    shutdown.cancel();
    let api = worker
        .join()
        .map_err(|_| anyhow!("API runtime thread panicked"))?;
    window.map_err(|error| anyhow!("cannot run egui window: {error}"))?;
    api
}

async fn serve(
    path: &Path,
    config: Config,
    renderers: Renderers,
    shutdown: CancellationToken,
    ready: impl FnOnce(std::net::SocketAddr),
) -> Result<()> {
    crate::validate_config(&config)?;
    let listener = tokio::net::TcpListener::bind(config.web.listen).await?;
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (pipeline, status) = Pipeline::with_renderers(PipelineConfig::from(&config), renderers)?;
    let lighting = Arc::new(LightingService::new(config_rx, pipeline.clone(), status)?);
    let (controller, app) = Controller::spawn(path.to_path_buf(), &config, lighting, config_tx);
    ready(listener.local_addr()?);
    let router = crate::control::router(app, config.web.clone());
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
    if tokio::time::timeout(Duration::from_secs(60), controller.send(Shutdown))
        .await
        .is_err()
    {
        tracing::warn!("controller shutdown timed out");
        controller.do_send(crate::actors::Stop);
    }
    pipeline
        .send(StopPipeline)
        .await
        .context("pipeline unavailable")??;
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

    #[actix::test]
    async fn shutdown_does_not_wait_forever_for_an_incomplete_request() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prysm.toml");
        let mut config: Config = toml::from_str(include_str!("../../../prysm.toml")).unwrap();
        config.web.listen = "127.0.0.1:0".parse().unwrap();
        config.web.allowed_hosts = vec!["*".into()];
        let shutdown = CancellationToken::new();
        let stopped = shutdown.clone();
        let (ready, address) = tokio::sync::oneshot::channel();
        let server = actix::spawn(async move {
            serve(&path, config, Renderers::default(), stopped, |address| {
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
