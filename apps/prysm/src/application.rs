//! Application lifetime: bind HTTP, start the controller, and join all owners on exit.
use crate::{
    control::{Controller, ControllerHandle},
    preview::Preview,
    settings::SettingsStore,
};
use anyhow::{Context as _, Result};
use axum::Router;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

struct Application {
    listener: TcpListener,
    router: Router,
    controller: ControllerHandle,
    preview: Preview,
    shutdown: CancellationToken,
}

enum Stopped {
    Http(std::io::Result<()>),
    Controller(Result<()>),
    Requested,
}

impl Application {
    async fn bind(
        settings: SettingsStore,
        web: crate::config::WebConfig,
        receiver_listen: std::net::SocketAddr,
    ) -> Result<Self> {
        web.validate()?;
        let listener = TcpListener::bind(web.listen)
            .await
            .context("cannot bind the web listener")?;
        let preview = Preview::new();
        let (controller, app) = Controller::start(settings, preview.publisher(), receiver_listen);
        let router = crate::control::http::router(app, web, preview.router());
        tracing::info!(address=%listener.local_addr()?, "Prysm listening");
        Ok(Self {
            listener,
            router,
            controller,
            preview,
            shutdown: CancellationToken::new(),
        })
    }

    async fn run(self) -> Result<()> {
        let Self {
            listener,
            router,
            mut controller,
            preview,
            shutdown,
        } = self;
        let server = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.clone().cancelled_owned())
            .into_future();
        tokio::pin!(server);
        let stopped = tokio::select! {
            result = &mut server => Stopped::Http(result),
            result = controller.wait() => Stopped::Controller(result),
            () = shutdown_signal() => Stopped::Requested,
            () = shutdown.cancelled() => Stopped::Requested,
        };
        tracing::info!("application shutdown requested");
        shutdown.cancel();
        let (controller_result, finished_http) = match stopped {
            Stopped::Controller(result) => (
                result.and(Err(anyhow::anyhow!("controller stopped unexpectedly"))),
                None,
            ),
            Stopped::Http(result) => (controller.shutdown().await, Some(result)),
            Stopped::Requested => (controller.shutdown().await, None),
        };
        let preview_result = preview.shutdown().await;
        let http_result = match finished_http {
            Some(result) => result.context("HTTP server failed"),
            None => {
                if let Ok(result) = tokio::time::timeout(Duration::from_secs(5), &mut server).await
                {
                    result.context("HTTP server failed")
                } else {
                    tracing::warn!("HTTP shutdown timed out; closing remaining connections");
                    Ok(())
                }
            }
        };
        controller_result?;
        preview_result?;
        http_result?;
        tracing::info!("capture, lighting and preview stopped");
        Ok(())
    }
}

pub fn run(
    settings: SettingsStore,
    web: crate::config::WebConfig,
    receiver_listen: std::net::SocketAddr,
) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            Application::bind(settings, web, receiver_listen)
                .await?
                .run()
                .await
        })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminated =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("termination signal");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminated.recv()=>{} }
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
        let store = SettingsStore::load(directory.path().join("settings.json")).unwrap();
        let web = crate::config::WebConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            allowed_hosts: vec!["localhost:8787".into()],
        };
        let application = Application::bind(store, web, "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let address = application.listener.local_addr().unwrap();
        let shutdown = application.shutdown.clone();
        let server = tokio::spawn(application.run());
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client.write_all(b"PUT /api/settings HTTP/1.1\r\nHost: localhost:8787\r\nContent-Type: application/json\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n").await.unwrap();
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
            .expect("incomplete request must not prevent shutdown")
            .unwrap()
            .unwrap();
    }
}
