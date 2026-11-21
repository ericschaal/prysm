//! prysm-tui: the terminal previews what the LEDs show, next to the web portal.
use prysm_capture_device::DeviceSource;
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_host::{Program, assets};
use prysm_output_wled::Wled;
use prysm_ports::{Error, Result, Settings, SettingsStore};
use prysm_runtime::{Builder, NullOutput};
use prysm_settings_file::SettingsFile;
use prysm_surface_tui::Tui;
use std::{io, path::Path, process::ExitCode};

const ABOUT: &str = "Preview capture in the terminal, optionally driving WLED; use output kind
\"null\" to test without LEDs. Capture kinds: \"device\", \"remote\" and \"file\" (a
video played once through FFmpeg 9+ on PATH; see settings.file.example.json).
Quit with q, or :quit. Without an interactive
terminal the preview is unavailable and lighting and portal keep running.
While the preview uses the terminal, status is not printed and log lines go
only to --log-file.";

const PROGRAM: Program = Program {
    name: "prysm-tui",
    about: ABOUT,
    full_screen: true,
};

fn main() -> ExitCode {
    // A capture-only TV streams to the portal, which hands it to this source.
    let remote = RemoteReceiver::default();
    let tv_stream = remote.incoming();
    let adapters = |prysm: Builder| {
        prysm
            .source(DeviceSource)
            .source(FileSource)
            .source(remote)
            .output(Wled::default())
            .output(NullOutput)
    };
    prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
        let tui = Tui::new(read_settings);
        let quit = tui.quit();
        let signals = host.signals()?;
        prysm
            .surface(host.status())
            .surface(host.start())
            .surface(host.portal(assets::HEADLESS)?.capture_stream(tv_stream))
            .surface(tui)
            .run(async {
                tokio::select! {
                    () = signals => {}
                    () = quit.requested() => {}
                }
            })
            .await
    })
}

/// The settings file the terminal's `load <path>` names, read as this host
/// reads its own; a missing file is an error with its `NotFound` cause.
fn read_settings(path: &Path) -> Result<Settings> {
    SettingsFile::new(path).load()?.ok_or_else(|| {
        Error::adapter(
            format!("no settings file {}", path.display()),
            io::Error::from(io::ErrorKind::NotFound),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_reads_a_settings_file_as_the_host_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{ "format": 1, "output": { "kind": "null" } }"#).unwrap();
        assert_eq!(read_settings(&path).unwrap().output.unwrap().kind, "null");

        std::fs::write(&path, r#"{ "output": { "kind": "null" } }"#).unwrap();
        let unversioned = prysm_ports::chain(&read_settings(&path).unwrap_err());
        assert!(unversioned.contains("no `format` field"), "{unversioned}");

        let missing = read_settings(&dir.path().join("none")).unwrap_err();
        let Error::Adapter {
            source: Some(source),
            ..
        } = &missing
        else {
            panic!("{missing:?}");
        };
        let io = source.downcast_ref::<io::Error>().unwrap();
        assert_eq!(io.kind(), io::ErrorKind::NotFound);
    }
}
