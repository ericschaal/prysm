//! Load the application configuration and select its output.

use std::path::Path;

use anyhow::Result;

use crate::{
    config::{Output, RuntimeConfig},
    desktop, wled,
};

/// Load a TOML configuration and run its capture, processing, and output pipeline.
///
/// Desktop output blocks the calling thread, which must be the main thread.
/// Logging is configured by the caller. Returns when input ends or shutdown is requested.
///
/// # Errors
/// Returns configuration, input, runtime, or renderer failures with their context.
pub fn run_from_config(path: &Path) -> Result<()> {
    let RuntimeConfig {
        settings,
        input,
        output,
    } = RuntimeConfig::load(path)?;
    match output {
        Output::Egui { refresh_rate_hz } => {
            let resolution = settings.capture_resolution;
            desktop::run(settings, refresh_rate_hz, move |shutdown| {
                input.into_stream(resolution, shutdown)
            })
        }
        Output::Wled {
            address,
            refresh_rate_hz,
            leds,
        } => wled::run(input, settings, address, refresh_rate_hz, leds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_wled_pipeline_preserves_input_startup_errors() {
        let path = std::env::temp_dir().join(format!("prysm-startup-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"
[input]
type = "file"
path = "/missing/prysm-input.mp4"
[output]
type = "wled"
address = "127.0.0.1:4048"
[output.leds]
top = 1
right = 1
bottom = 1
left = 1
"#,
        )
        .unwrap();
        let result = run_from_config(&path);
        std::fs::remove_file(path).unwrap();

        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cannot open video /missing/prysm-input.mp4"
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
