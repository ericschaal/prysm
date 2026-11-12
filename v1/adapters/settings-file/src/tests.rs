use super::FileSettings;
use prysm_ports::{AdapterSettings, Error, Processing, Settings, SettingsStore};
use serde_json::json;
use std::{error::Error as _, fs, path::Path};

fn sample() -> Settings {
    Settings {
        capture: Some(AdapterSettings::new(
            "device",
            &json!({ "path": "/dev/video0" }),
        )),
        output: Some(AdapterSettings::new(
            "wled",
            &json!({ "host": "10.0.0.5", "port": 21324 }),
        )),
        processing: Processing {
            brightness_percent: 80,
            smoothing_seconds: 0.3,
            ..Processing::default()
        },
    }
}

/// The context plus every source in the chain.
fn full_message(error: &Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// The adapter error's preserved cause, downcast.
fn cause<E: std::error::Error + 'static>(error: &Error) -> Option<&E> {
    match error {
        Error::Adapter {
            source: Some(source),
            ..
        } => source.downcast_ref(),
        _ => None,
    }
}

fn entries(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn missing_file_loads_as_none_without_creating_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    assert_eq!(FileSettings::new(&path).load().unwrap(), None);
    assert!(!path.exists());
}

#[test]
fn save_then_load_round_trips_as_pretty_json() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = FileSettings::new(&path);
    store.save(&sample()).unwrap();
    assert_eq!(store.load().unwrap(), Some(sample()));

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.ends_with("}\n"), "{text}");
    assert!(text.contains("\n  \"capture\": {\n"), "{text}");
    assert!(text.contains("\"smoothing_seconds\": 0.3,"), "{text}");
    // Saving the loaded value again is byte-for-byte stable.
    let loaded = store.load().unwrap().unwrap();
    store.save(&loaded).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(entries(directory.path()), ["settings.json"]);
}

#[test]
fn hand_written_minimal_file_loads_with_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(
        &path,
        r#"{ "output": { "kind": "wled", "host": "led.local" } }"#,
    )
    .unwrap();
    let settings = FileSettings::new(&path).load().unwrap().unwrap();
    assert_eq!(settings.capture, None);
    assert_eq!(settings.output.unwrap().fields["host"], "led.local");
    assert_eq!(settings.processing, Processing::default());
}

#[test]
fn unknown_field_is_rejected_with_its_name_and_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, r#"{ "processing": { "brightnes_percent": 80 } }"#).unwrap();
    let error = FileSettings::new(&path).load().unwrap_err();
    let message = full_message(&error);
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(
        message.contains("unknown field `brightnes_percent`"),
        "{message}"
    );
    assert!(message.contains("line 1"), "{message}");
    assert!(cause::<serde_json::Error>(&error).is_some());
}

#[test]
fn corrupt_json_error_names_the_path_and_keeps_the_serde_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ \"processing\": ").unwrap();
    let error = FileSettings::new(&path).load().unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot parse settings file {}", path.display())
    );
    assert!(cause::<serde_json::Error>(&error).is_some());
}

#[test]
fn unreadable_path_reports_io_source() {
    let directory = tempfile::tempdir().unwrap();
    // A directory exists at the path, so reading it fails with something other than NotFound.
    let error = FileSettings::new(directory.path()).load().unwrap_err();
    assert!(
        error.to_string().starts_with("cannot read settings file"),
        "{error}"
    );
    assert!(cause::<std::io::Error>(&error).is_some());
}

#[test]
fn save_into_missing_directory_errors_clearly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent").join("settings.json");
    let error = FileSettings::new(&path).save(&sample()).unwrap_err();
    let message = full_message(&error);
    assert!(
        message.starts_with(&format!(
            "cannot create a temporary file for settings file {}",
            path.display()
        )),
        "{message}"
    );
    let io = cause::<std::io::Error>(&error).unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    assert!(!path.exists());
}

#[test]
fn failed_replace_leaves_target_and_directory_untouched() {
    let directory = tempfile::tempdir().unwrap();
    // A non-empty directory at the target path: the temporary file is fully
    // written but the rename fails.
    let path = directory.path().join("settings.json");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), "x").unwrap();
    let error = FileSettings::new(&path).save(&sample()).unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("cannot replace settings file"),
        "{error}"
    );
    assert_eq!(entries(directory.path()), ["settings.json"]);
    assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "x");
}

#[cfg(unix)]
#[test]
fn failed_save_never_truncates_the_existing_file() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = FileSettings::new(&path);
    store.save(&sample()).unwrap();
    let before = fs::read(&path).unwrap();

    // A read-only directory refuses the temporary file.
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    if tempfile::tempfile_in(directory.path()).is_ok() {
        // Running as root: permissions do not apply, nothing to test.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let result = store.save(&Settings::default());
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();

    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(store.load().unwrap(), Some(sample()));
    assert_eq!(entries(directory.path()), ["settings.json"]);
}

#[cfg(unix)]
#[test]
fn save_preserves_existing_permissions_and_creates_new_files_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = FileSettings::new(&path);
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    store.save(&sample()).unwrap();
    assert_eq!(mode(&path), 0o600);

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    store.save(&Settings::default()).unwrap();
    assert_eq!(mode(&path), 0o644);
    assert_eq!(store.load().unwrap(), Some(Settings::default()));
}

#[test]
fn relative_path_saves_in_the_working_directory() {
    let store = FileSettings::new("settings.json");
    assert_eq!(store.directory(), Path::new("."));
    assert_eq!(store.path(), Path::new("settings.json"));
}
