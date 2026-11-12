//! The supervisor of the full package (`io.prysm.tv`, worker `prysm-webos`).
fn main() -> std::process::ExitCode {
    prysm_webos_service::main(prysm_webos_service::state::Flavor::Full)
}
