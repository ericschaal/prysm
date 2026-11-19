//! prysm-webos-slim: no engine, no processor; TV frames go to a headless host.
//! Fakes stand in for the real blocks named in the comments.
use prysm_runtime::Forwarder;
use prysm_runtime::fakes::{self, CallLog, FakeSink, FakeSource, FakeSourceSettings};
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> prysm_ports::Result<()> {
    let log = CallLog::new();
    let capture = FakeSource::new(&log, FakeSourceSettings::default()); // WebosCapture::new(luna)
    Forwarder::new(capture, FakeSink(log.clone())) // RemoteSender::new(peer)
        .surface(fakes::until_shutdown) // LunaBridge::new(luna); controls nothing here
        .run(tokio::time::sleep(Duration::from_millis(200)))
        .await?;
    println!("slim forwarded {} frames", log.count("sink.send"));
    Ok(())
}
