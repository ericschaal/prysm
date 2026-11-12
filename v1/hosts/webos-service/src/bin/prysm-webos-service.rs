//! The supervisor of the full package, installed as `prysm-webos-service` of
//! `io.prysm.tv` with `prysm-webos` as `prysm-webos-worker`.
fn main() -> std::process::ExitCode {
    prysm_webos_service::main(prysm_webos_service::state::Flavor::Full)
}
