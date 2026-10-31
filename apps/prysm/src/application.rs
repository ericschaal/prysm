//! Run the REST API, keeping an optional native window on the OS main thread.
use anyhow::Result;
use prysm_contract::Config;
use std::path::Path;

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
    actix::System::new().block_on(crate::control::run(path, config))
}

#[cfg(feature = "desktop")]
fn run_with_window(path: &Path, config: Config) -> Result<()> {
    use crate::actors::{egui::Egui, pipeline::Renderers};
    use anyhow::{Context as _, anyhow};
    use prysm_capture::Frame;
    use prysm_core::EdgeColors;
    use prysm_renderer_desktop::{DesktopRendererBuilder, DisplayFrame};
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    let [width, height] = config.capture_resolution.dimensions();
    let shutdown = CancellationToken::new();
    let (colors, colors_rx) = watch::channel(EdgeColors::default());
    let (frames, frames_rx) = watch::channel(DisplayFrame {
        frame: Frame::dummy(width, height),
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
            actix::System::new().block_on(crate::control::serve(
                &path,
                config,
                renderers,
                worker_shutdown,
                connect,
            ))
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
