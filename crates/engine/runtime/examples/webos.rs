//! prysm-webos (full): TV capture into WLED, controlled from the TV UI and the portal.
//! Fakes stand in for the real blocks named in the comments.
use prysm_runtime::Prysm;
use prysm_runtime::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> prysm_ports::Result<()> {
    let log = CallLog::new();
    Prysm::builder(MemoryStore::with(fakes::settings())) // SettingsFile::new(path)?
        .source(FakeSources(log.clone())) // WebosCapture::new(luna)
        .output(FakeOutputs(log.clone())) // Wled
        .surface(fakes::light_until_shutdown) // LunaBridge::new(luna)
        .surface(fakes::until_shutdown) // WebPortal::new(assets, policy)
        .run(tokio::time::sleep(Duration::from_millis(200)))
        .await?;
    println!("webos wrote {} frames", log.count("output.write:"));
    Ok(())
}
