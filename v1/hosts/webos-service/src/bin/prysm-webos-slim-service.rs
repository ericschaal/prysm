//! The supervisor of the slim package (`io.prysm.tv.slim`, worker `prysm-webos-slim`).
fn main() -> std::process::ExitCode {
    prysm_webos_service::main(prysm_webos_service::state::Flavor::Slim)
}
