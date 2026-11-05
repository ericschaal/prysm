//! Serialization regression samples, independent of the public schema registrations.
use prysm_assemblies::{
    DesktopCaptureSettings, RemoteCaptureSettings, ServerCaptureSettings, TvCaptureSettings,
    WledSettings,
};
use prysm_core::CaptureResolution;
use std::{error::Error, num::NonZeroU32};

pub fn serialized() -> Result<serde_json::Value, Box<dyn Error>> {
    let resolution = CaptureResolution::new(320, 180)?;
    let rate = NonZeroU32::new(30).ok_or("invalid fixture rate")?;
    Ok(serde_json::json!({
        "DesktopCaptureSettings": [
            DesktopCaptureSettings::Device { path: "/dev/video0".into(), resolution, rate },
            DesktopCaptureSettings::File { path: "fixture.mp4".into(), resolution, rate },
        ],
        "RemoteCaptureSettings": [RemoteCaptureSettings { peer: "127.0.0.1:9000".parse()? }],
        "ServerCaptureSettings": [
            ServerCaptureSettings::Desktop(DesktopCaptureSettings::Device { path: "/dev/video0".into(), resolution, rate }),
            ServerCaptureSettings::Desktop(DesktopCaptureSettings::File { path: "fixture.mp4".into(), resolution, rate }),
            ServerCaptureSettings::Remote(RemoteCaptureSettings { peer: "127.0.0.1:9000".parse()? }),
        ],
        "TvCaptureSettings": [TvCaptureSettings { resolution, rate }],
        "WledSettings": [WledSettings { address: "192.0.2.1".parse()?, leds: [10, 20, 30, 40] }],
    }))
}
