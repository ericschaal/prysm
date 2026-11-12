//! The supervisor of the slim package, installed as `prysm-webos-service` of
//! `io.prysm.tv` with `prysm-webos-slim` as `prysm-webos-worker`.
fn main() -> std::process::ExitCode {
    prysm_webos_service::main(prysm_webos_service::state::Flavor::Slim)
}
