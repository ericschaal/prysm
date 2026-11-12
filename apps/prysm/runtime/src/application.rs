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
    inputs: crate::input::Inputs,
    listener: TcpListener,
    router: Router,
    controller: ControllerHandle,
    preview: Preview,
    shutdown: CancellationToken,
}

enum Stopped {
    Http,
    Controller(Result<()>),
    Requested,
}

impl Application {
    #[cfg(feature = "host-input")]
    async fn bind(settings: SettingsStore, web: crate::config::WebConfig) -> Result<Self> {
        let inputs = crate::input::Inputs::Host(prysm_capture::WebosReceiver::new()?);
        Self::bind_inputs(settings, web, inputs, CancellationToken::new()).await
    }

    async fn bind_inputs(
        settings: SettingsStore,
        web: crate::WebConfig,
        inputs: crate::input::Inputs,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        web.validate()?;
        let listener = TcpListener::bind(web.listen)
            .await
            .context("cannot bind the web listener")?;
        let preview = Preview::new();
        let (controller, app) = Controller::start(settings, preview.publisher(), inputs.clone());
        let media = match &inputs {
            #[cfg(feature = "host-input")]
            crate::input::Inputs::Host(receiver) => preview.router().merge(receiver.router()),
            crate::input::Inputs::Native(_) => preview.router(),
        };
        let router = crate::control::http::router(app, web, media);
        tracing::info!(address=%listener.local_addr()?, "Prysm listening");
        Ok(Self {
            inputs,
            listener,
            router,
            controller,
            preview,
            shutdown,
        })
    }

    async fn run(self, signal: impl std::future::Future<Output = ()>) -> Result<()> {
        let Self {
            inputs,
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
            () = &mut server => Stopped::Http,
            result = controller.wait() => Stopped::Controller(result),
            () = signal => Stopped::Requested,
            () = shutdown.cancelled() => Stopped::Requested,
        };
        tracing::info!("application shutdown requested");
        shutdown.cancel();
        let (controller_result, finished_http) = match stopped {
            Stopped::Controller(result) => (
                result.and(Err(anyhow::anyhow!("controller stopped unexpectedly"))),
                false,
            ),
            Stopped::Http => (controller.shutdown().await, true),
            Stopped::Requested => (controller.shutdown().await, false),
        };
        let receiver_result: Result<()> = match inputs {
            #[cfg(feature = "host-input")]
            crate::input::Inputs::Host(receiver) => receiver.shutdown().await,
            crate::input::Inputs::Native(_) => Ok(()),
        };
        let preview_result = preview.shutdown().await;
        if !finished_http
            && tokio::time::timeout(Duration::from_secs(5), &mut server)
                .await
                .is_err()
        {
            tracing::warn!("HTTP shutdown timed out; closing remaining connections");
        }
        controller_result?;
        receiver_result?;
        preview_result?;
        tracing::info!("capture, lighting and preview stopped");
        Ok(())
    }
}

/// Serve the same controller and portal with an in-process video source.
/// Readiness is reported only after the listener, router and controller are initialized.
pub async fn run_native(
    settings_path: std::path::PathBuf,
    web: crate::WebConfig,
    source: std::sync::Arc<dyn crate::NativeCapture>,
    stop: CancellationToken,
    ready: impl FnOnce(std::net::SocketAddr) -> Result<()>,
) -> Result<()> {
    anyhow::ensure!(
        web.allowed_hosts.iter().all(|host| host != "*"),
        "native portal requires explicit allowed hosts"
    );
    let settings = SettingsStore::load_native(settings_path)?;
    let app = Application::bind_inputs(
        settings,
        web,
        crate::input::Inputs::Native(source),
        stop.clone(),
    )
    .await?;
    let startup = ready(app.listener.local_addr()?);
    if startup.is_err() {
        stop.cancel();
    }
    let exit = app.run(std::future::pending()).await;
    startup.and(exit)
}

#[cfg(feature = "host-input")]
pub fn run(settings: SettingsStore, web: crate::config::WebConfig) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            Application::bind(settings, web)
                .await?
                .run(shutdown_signal())
                .await
        })
}

#[cfg(feature = "host-input")]
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

#[cfg(all(test, feature = "host-input"))]
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
        let application = Application::bind(store, web).await.unwrap();
        let address = application.listener.local_addr().unwrap();
        let shutdown = application.shutdown.clone();
        let server = tokio::spawn(application.run(std::future::pending()));
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

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::{InputStream, NativeCapture};
    use axum::http::{StatusCode, header};
    use prysm_contract::{CaptureResolution, Input, SettingsView, Source, UpdateSettings};
    use std::{num::NonZeroU32, sync::Arc};
    use tokio::sync::oneshot;

    struct IdleInput;

    impl NativeCapture for IdleInput {
        fn open(
            &self,
            _: CaptureResolution,
            _: NonZeroU32,
            _: CancellationToken,
        ) -> Result<InputStream> {
            panic!("an idle portal must not open the native capture device")
        }
    }

    async fn expect_status(request: reqwest::RequestBuilder, status: StatusCode) {
        assert_eq!(request.send().await.unwrap().status(), status);
    }

    #[tokio::test]
    async fn idle_native_portal_serves_and_persists_settings_without_capture_transport() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let stop = CancellationToken::new();
        let (ready, address) = oneshot::channel();
        let web = crate::WebConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            allowed_hosts: vec!["localhost:8787".into()],
        };
        let server = tokio::spawn(run_native(
            path.clone(),
            web,
            Arc::new(IdleInput),
            stop.clone(),
            |addr| {
                ready.send(addr).unwrap();
                Ok(())
            },
        ));
        let url = format!("http://{}", address.await.unwrap());
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let get = |path: &str| {
            client
                .get(format!("{url}{path}"))
                .header(header::HOST, "localhost:8787")
        };
        let put = |update: &UpdateSettings| {
            client
                .put(format!("{url}/api/settings"))
                .header(header::HOST, "localhost:8787")
                .header(header::ORIGIN, "http://localhost:8787")
                .json(update)
        };
        expect_status(get("/"), StatusCode::OK).await;
        expect_status(get("/api/capture"), StatusCode::NOT_FOUND).await;
        expect_status(
            get("/api/state").header(header::ORIGIN, "http://evil.invalid"),
            StatusCode::FORBIDDEN,
        )
        .await;
        expect_status(
            client
                .get(format!("{url}/api/state"))
                .header(header::HOST, "evil.invalid"),
            StatusCode::FORBIDDEN,
        )
        .await;
        let initial: SettingsView = get("/api/settings")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(initial.native_webos);
        assert!(matches!(initial.settings.input, Some(Input::Webos { .. })));
        let mut update = UpdateSettings {
            expected_revision: initial.revision,
            settings: initial.settings.clone(),
        };
        let host_device = crate::test_support::config().input;
        let file = Input::File {
            path: String::from("/etc/passwd").try_into().unwrap(),
            capture_resolution: [640, 360].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        for input in [host_device, file] {
            update.settings.input = Some(input);
            assert_eq!(
                put(&update).send().await.unwrap().status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert!(!path.exists());
        }
        update.settings = initial.settings;
        update.settings.output = Some(crate::test_support::config().output);
        update.settings.source = Source::Wled;
        update.settings.processing.brightness_percent = 42.try_into().unwrap();
        let saved: SettingsView = put(&update)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_ne!(saved.revision, initial.revision);
        assert_eq!(saved.settings, update.settings);
        assert_eq!(
            put(&update).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        stop.cancel();
        server.await.unwrap().unwrap();
        let restored = SettingsStore::load_native(path).unwrap();
        assert_eq!(restored.view().settings, saved.settings);
    }

    #[tokio::test]
    async fn occupied_listener_does_not_announce_readiness() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("settings.json");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web = crate::WebConfig {
            listen: listener.local_addr().unwrap(),
            allowed_hosts: vec!["localhost:8787".into()],
        };
        assert!(
            run_native(
                settings_path,
                web,
                Arc::new(IdleInput),
                CancellationToken::new(),
                |_| panic!("occupied socket announced")
            )
            .await
            .is_err()
        );
    }
}
