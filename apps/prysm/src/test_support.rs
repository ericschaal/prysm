//! Shared settings fixtures for application unit tests.
use crate::settings::SettingsStore;
use prysm_contract::{Config, Settings, UpdateSettings};

pub(crate) fn config() -> Config {
    serde_json::from_str(include_str!("../tests/fixtures/config.json")).unwrap()
}

pub(crate) fn settings_value(config: &Config) -> serde_json::Value {
    let mut value = serde_json::to_value(config).unwrap();
    value.as_object_mut().unwrap().remove("revision");
    value.as_object_mut().unwrap().remove("correction");
    value["calibration"] =
        serde_json::to_value(prysm_contract::CalibrationProfiles::default()).unwrap();
    value
}

pub(crate) fn store(directory: &tempfile::TempDir, config: &Config) -> SettingsStore {
    let path = directory.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&settings_value(config)).unwrap()).unwrap();
    SettingsStore::load(path).unwrap()
}

pub(crate) fn update(app: &prysm_api::App, config: &Config) -> UpdateSettings {
    let settings: Settings = serde_json::from_value(settings_value(config)).unwrap();
    UpdateSettings {
        expected_revision: app.settings().revision,
        settings,
    }
}

pub(crate) type Tv =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
pub(crate) async fn tv_socket(receiver: &prysm_capture::WebosReceiver) -> Tv {
    use futures::SinkExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/api/capture", listener.local_addr().unwrap());
    let router = receiver.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    socket
        .send(tokio_tungstenite::tungstenite::Message::binary(
            prysm_webos_protocol::HELLO.encode().unwrap(),
        ))
        .await
        .unwrap();
    socket
}
