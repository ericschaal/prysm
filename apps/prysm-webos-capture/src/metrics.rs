pub fn exporter(enabled: bool) -> Option<prysm_metrics::Exporter> {
    if enabled {
        match prysm_metrics::start(([0, 0, 0, 0], 9101).into(), prysm_metrics::Role::Capturer) {
            Ok(exporter) => {
                tracing::info!(address = %exporter.address(), "metrics listening");
                Some(exporter)
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "metrics disabled");
                None
            }
        }
    } else {
        None
    }
}
