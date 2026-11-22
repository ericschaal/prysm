//! `prysm-headless` with stand-in video devices, for the portal's end-to-end
//! tests: kind `"device"` finds and describes the devices that
//! `PRYSM_FAKE_DEVICES` lists, and never captures. Everything else is the
//! production host.
//!
//! ```sh
//! PRYSM_FAKE_DEVICES='[{"name":"Capture card","path":"/dev/video7","modes":[…]}]' \
//!     cargo run -p prysm-headless --example fake_devices -- settings.json
//! ```
//!
//! Each device is `{ name, path, modes }`, `modes` as
//! `POST /api/v1/capture/modes` answers them; any other path is unknown, as
//! an unplugged device is. Opening one fails: the tests do not capture.
use prysm_capture_device::DeviceSettings;
use prysm_host::{Program, assets};
use prysm_ports::{
    Candidate, Error, FrameSource, Mode, Modes, Result, SourceFactory, ValidationError,
};
use serde::Deserialize;
use std::process::ExitCode;

const PROGRAM: Program = Program {
    name: "prysm-headless",
    about: "prysm-headless with the video devices PRYSM_FAKE_DEVICES lists.",
};

#[derive(Deserialize)]
struct FakeDevice {
    name: String,
    path: String,
    modes: Vec<Mode>,
}

/// The listed devices, in place of the ones present.
struct FakeDevices(Vec<FakeDevice>);

impl SourceFactory for FakeDevices {
    const KIND: &'static str = "device";
    type Settings = DeviceSettings;

    fn validate(&self, settings: &DeviceSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    fn open(&self, _: &DeviceSettings) -> Result<Box<dyn FrameSource>> {
        Err(Error::other("a fake device does not capture"))
    }

    fn candidates(&self) -> Vec<Candidate> {
        self.0
            .iter()
            .map(|device| {
                Candidate::new(&device.name, &serde_json::json!({ "path": device.path }))
                    .detail(&device.path)
            })
            .collect()
    }

    fn modes(&self, settings: &DeviceSettings) -> Modes {
        self.0
            .iter()
            .find(|device| device.path == settings.path)
            .map_or(Modes::Unknown, |device| Modes::Known(device.modes.clone()))
    }
}

fn main() -> ExitCode {
    let devices = match std::env::var("PRYSM_FAKE_DEVICES")
        .map_err(|error| error.to_string())
        .and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
    {
        Ok(devices) => FakeDevices(devices),
        Err(error) => {
            eprintln!("PRYSM_FAKE_DEVICES: {error}");
            return ExitCode::from(2);
        }
    };
    let (adapters, tv_stream) = prysm_host::desktop_adapters(devices);
    prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
        prysm
            .surface(host.status())
            .surface(host.start())
            .surface(host.portal(assets::HEADLESS)?.capture_stream(tv_stream))
            .run(host.signals())
            .await
    })
}
