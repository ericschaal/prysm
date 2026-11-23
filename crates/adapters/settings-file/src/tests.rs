use super::{FormatError, SettingsFile};
use prysm_ports::{
    AdapterSettings, Processing, Settings, SettingsStore, Style, StyleValues, chain,
};
use serde_json::json;
use std::{fs, path::Path};

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
            // A built-in style is chosen; the custom values are kept.
            style: Style::Calm,
            custom_style: Some(StyleValues {
                smoothing_seconds: 0.3,
                cut_response_percent: 40,
                brightness_ramp_seconds: 0.5,
                spatial_reach_percent: 20,
                color_focus_percent: 60,
                saturation_boost_percent: 40,
                dark_floor_percent: 10,
            }),
            ..Processing::default()
        },
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
    assert_eq!(SettingsFile::new(&path).load().unwrap(), None);
    assert!(!path.exists());
}

#[test]
fn save_then_load_round_trips_as_pretty_json() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = SettingsFile::new(&path);
    store.save(&sample()).unwrap();
    assert_eq!(store.load().unwrap(), Some(sample()));

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("{\n  \"format\": 1,\n"), "{text}");
    assert!(text.ends_with("}\n"), "{text}");
    assert!(text.contains("\n  \"capture\": {\n"), "{text}");
    assert!(
        text.contains("\"style\": {\n      \"kind\": \"calm\"\n    },"),
        "{text}"
    );
    assert!(text.contains("\"custom_style\": {\n"), "{text}");
    assert!(text.contains("\"smoothing_seconds\": 0.3,"), "{text}");
    assert!(text.contains("\"brightness_ramp_seconds\": 0.5,"), "{text}");
    assert!(text.contains("\"color_focus_percent\": 60,"), "{text}");
    assert!(text.contains("\"dark_floor_percent\": 10\n"), "{text}");
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
        r#"{ "output": { "kind": "wled", "host": "led.local" }, "format": 1 }"#,
    )
    .unwrap();
    let settings = SettingsFile::new(&path).load().unwrap().unwrap();
    assert_eq!(settings.capture, None);
    assert_eq!(settings.output.unwrap().fields["host"], "led.local");
    assert_eq!(settings.processing, Processing::default());
}

#[test]
fn unknown_field_is_rejected_with_its_name_and_the_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    for (text, field, line) in [
        (
            r#"{ "format": 1, "processing": { "brightnes_percent": 80 } }"#,
            "brightnes_percent",
            "line 1",
        ),
        // Beside the format, at the top: the format is not a gap for other fields.
        (
            "{ \"format\": 1,\n  \"outptu\": { \"kind\": \"null\" } }",
            "outptu",
            "line 2",
        ),
    ] {
        fs::write(&path, text).unwrap();
        let error = SettingsFile::new(&path).load().unwrap_err();
        let message = chain(&error);
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(
            message.contains(&format!("unknown field `{field}`")),
            "{message}"
        );
        assert!(message.contains(line), "{message}");
        assert!(error.cause::<serde_json::Error>().is_some());
    }
}

/// A section is an object: an array is not read as its defaults, by position.
#[test]
fn a_section_written_as_an_array_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    for text in [
        r#"{ "format": 1, "processing": [] }"#,
        r#"{ "format": 1, "processing": { "calibrations": [{ "id": "a", "name": "A", "correction": [] }] } }"#,
        r#"{ "format": 1, "processing": [80] }"#,
        r#"{ "format": 1, "processing": { "calibrations": [["a", "A"]] } }"#,
        r#"{ "format": 1, "processing": { "custom_style": [0.5, 50, 0.3, 50, 50, 50, 5] } }"#,
        r#"{ "format": 1, "processing": { "style": ["natural"] } }"#,
    ] {
        fs::write(&path, text).unwrap();
        let error = SettingsFile::new(&path).load().unwrap_err();
        let message = chain(&error);
        assert!(
            message.contains("invalid type: sequence"),
            "{text}: {message}"
        );
        assert!(error.cause::<serde_json::Error>().is_some());
    }
}

/// What `load` refuses for the file's format.
fn format_error(text: &str) -> (FormatError, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, text).unwrap();
    let error = SettingsFile::new(&path).load().unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot load settings file {}", path.display())
    );
    (error.cause::<FormatError>().unwrap().clone(), chain(&error))
}

#[test]
fn a_newer_format_is_refused_for_its_number_before_its_fields() {
    // A newer Prysm's file may well have fields this one does not know.
    let (error, message) = format_error(r#"{ "added": true, "format": 2 }"#);
    assert_eq!(error, FormatError::Newer { found: 2.into() });
    assert!(
        message.ends_with("it is format 2, written by a newer Prysm; this Prysm reads format 1"),
        "{message}"
    );
}

#[test]
fn a_file_without_a_format_is_refused() {
    let (error, message) = format_error(r#"{ "output": { "kind": "null" } }"#);
    assert_eq!(error, FormatError::Missing);
    assert!(message.contains("add `\"format\": 1`"), "{message}");
    assert_eq!(format_error("{}").0, FormatError::Missing);
    assert_eq!(
        format_error(r#"{ "format": 0 }"#).0,
        FormatError::Older { found: 0.into() }
    );
}

#[test]
fn an_oversized_file_is_refused_before_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("big.json");
    fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    let error = SettingsFile::new(&path).load().unwrap_err();
    assert!(
        chain(&error).ends_with("it is larger than 1 MiB"),
        "{error}"
    );
    let io = error.cause::<std::io::Error>().unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::FileTooLarge);
}

#[test]
fn a_format_that_is_not_a_whole_number_says_so() {
    for (text, found) in [
        (r#"{ "format": "1" }"#, r#""1""#),
        (r#"{ "format": null }"#, "null"),
        (r#"{ "format": 2.0 }"#, "2.0"),
        (r#"{ "format": -1 }"#, "-1"),
    ] {
        let (error, message) = format_error(text);
        assert_eq!(
            error,
            FormatError::NotANumber {
                found: found.into()
            }
        );
        assert!(
            message.ends_with(&format!(
                "its `format` is {found}, not a format number; this Prysm reads format 1"
            )),
            "{message}"
        );
    }
}

#[test]
fn any_whole_number_above_the_format_is_newer() {
    for text in [r#"{ "format": 4294967296 }"#, r#"{ "format": 1e30 }"#] {
        assert!(
            matches!(format_error(text).0, FormatError::Newer { .. }),
            "{text}"
        );
    }
}

#[test]
fn only_an_object_has_a_format() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    for text in ["[2]", "[1]", "2"] {
        fs::write(&path, text).unwrap();
        let error = SettingsFile::new(&path).load().unwrap_err();
        assert!(
            error.to_string().starts_with("cannot parse settings file"),
            "{text}: {error}"
        );
        let message = chain(&error);
        assert!(message.contains("expected a settings object"), "{message}");
    }
}

#[test]
fn corrupt_json_error_names_the_path_and_keeps_the_serde_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ \"processing\": ").unwrap();
    let error = SettingsFile::new(&path).load().unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot parse settings file {}", path.display())
    );
    assert!(error.cause::<serde_json::Error>().is_some());
}

#[test]
fn unreadable_path_reports_io_source() {
    let directory = tempfile::tempdir().unwrap();
    // A directory exists at the path, so reading it fails with something other than NotFound.
    let error = SettingsFile::new(directory.path()).load().unwrap_err();
    assert!(
        error.to_string().starts_with("cannot read settings file"),
        "{error}"
    );
    assert!(error.cause::<std::io::Error>().is_some());
}

#[test]
fn save_into_missing_directory_errors_clearly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent").join("settings.json");
    let error = SettingsFile::new(&path).save(&sample()).unwrap_err();
    let message = chain(&error);
    assert!(
        message.starts_with(&format!(
            "cannot create a temporary file for settings file {}",
            path.display()
        )),
        "{message}"
    );
    let io = error.cause::<std::io::Error>().unwrap();
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
    let error = SettingsFile::new(&path).save(&sample()).unwrap_err();
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
    let mut store = SettingsFile::new(&path);
    store.save(&sample()).unwrap();
    let before = fs::read(&path).unwrap();

    // A read-only directory refuses the temporary file.
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    if tempfile::tempfile_in(directory.path()).is_ok() {
        // Running as root: permissions do not apply, nothing to test. CI's
        // test jobs are not root, so there it is a failure.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            std::env::var_os("CI").is_none(),
            "CI runs tests as a user that permissions apply to"
        );
        eprintln!("skipped: permissions do not apply to this user");
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
    let mut store = SettingsFile::new(&path);
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
    let store = SettingsFile::new("settings.json");
    assert_eq!(store.directory(), Path::new("."));
    assert_eq!(store.path(), Path::new("settings.json"));
}

/// What `load` refuses, `save` refuses too, before touching the file: a
/// saved file always loads.
#[test]
fn settings_too_large_to_load_are_not_saved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = SettingsFile::new(&path);
    store.save(&sample()).unwrap();
    let before = fs::read(&path).unwrap();
    let huge = Settings {
        capture: Some(AdapterSettings::new(
            "file",
            &json!({ "path": "a".repeat(1024 * 1024) }),
        )),
        ..Settings::default()
    };
    let error = store.save(&huge).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot save settings file {}", path.display())
    );
    assert!(
        chain(&error).ends_with("it is larger than 1 MiB"),
        "{error}"
    );
    let io = error.cause::<std::io::Error>().unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::FileTooLarge);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(entries(directory.path()), ["settings.json"]);
}

/// After the rename the new settings are what the next load returns, so a
/// directory sync that fails is not a failed save.
#[test]
fn a_failed_directory_sync_after_the_rename_still_saves() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let mut store = SettingsFile::new(&path)
        .with_directory_sync(|_| Err(std::io::Error::other("fsync not supported")));
    store.save(&sample()).unwrap();
    assert_eq!(store.load().unwrap(), Some(sample()));
}

/// The files beside `settings.json`, with their contents.
fn set_aside(directory: &Path) -> Vec<(String, Vec<u8>)> {
    entries(directory)
        .into_iter()
        .filter(|name| name != "settings.json")
        .map(|name| {
            let bytes = fs::read(directory.join(&name)).unwrap();
            (name, bytes)
        })
        .collect()
}

/// A file a load could not read is never replaced: the next save keeps it
/// beside the new one, under a name of its own.
#[test]
fn a_save_after_a_failed_load_keeps_the_unreadable_file_beside_it() {
    for text in [
        &br#"{ "output": { "kind": "null" } }"#[..],
        br#"{ "format": 2, "added": true }"#,
        b"{ \"format\": 1, \"processing\": ",
        br#"{ "format": 1, "processing": { "brightness_percent": "80" } }"#,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, text).unwrap();
        let mut store = SettingsFile::new(&path);
        assert!(store.load().is_err());
        assert_eq!(fs::read(&path).unwrap(), text, "a load writes nothing");

        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        let aside = set_aside(directory.path());
        assert_eq!(aside.len(), 1, "{aside:?}");
        let (name, bytes) = &aside[0];
        assert!(name.starts_with("settings.json.unreadable-"), "{name}");
        assert_eq!(bytes, text);

        // Once replaced, saves replace as usual.
        store.save(&Settings::default()).unwrap();
        assert_eq!(set_aside(directory.path()).len(), 1);
    }
}

/// A save whose rename fails after the unreadable file got its second
/// name leaves it at its path, with no second name: what a load finds, and
/// so the failure reported, is what it was.
#[test]
fn a_failed_replace_after_a_failed_load_leaves_the_unreadable_file_at_its_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();
    let mut store =
        SettingsFile::new(&path).with_replace(|_, _| Err(std::io::Error::other("disk full")));
    let before = store.load().unwrap_err().to_string();
    let error = store.save(&sample()).unwrap_err();
    assert!(chain(&error).ends_with("disk full"), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    assert_eq!(entries(directory.path()), ["settings.json"]);
    assert_eq!(store.load().unwrap_err().to_string(), before);
}

#[test]
fn a_save_after_a_failed_load_of_an_oversized_file_keeps_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let big = vec![b' '; 1024 * 1024 + 1];
    fs::write(&path, &big).unwrap();
    let mut store = SettingsFile::new(&path);
    assert!(store.load().is_err());
    store.save(&sample()).unwrap();
    let aside = set_aside(directory.path());
    assert_eq!(aside.len(), 1);
    assert_eq!(aside[0].1, big);
}

#[test]
fn a_save_after_a_failed_load_of_a_vanished_file_just_saves() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    let mut store = SettingsFile::new(&path);
    assert!(store.load().is_err());
    fs::remove_file(&path).unwrap();
    store.save(&sample()).unwrap();
    assert_eq!(entries(directory.path()), ["settings.json"]);
}

/// A file this user cannot read is set aside at the next save, and the new
/// one does not take its permissions.
#[cfg(unix)]
#[test]
fn a_save_after_an_unreadable_file_keeps_it_and_owns_the_new_one() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ \"format\": 1 }").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let mut store = SettingsFile::new(&path);
    let Err(error) = store.load() else {
        // Running as root: permissions do not apply, nothing to test.
        assert!(
            std::env::var_os("CI").is_none(),
            "CI runs tests as a user that permissions apply to"
        );
        eprintln!("skipped: permissions do not apply to this user");
        return;
    };
    assert!(
        error.to_string().starts_with("cannot read settings file"),
        "{error}"
    );
    assert_eq!(
        error.cause::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    store.save(&sample()).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(store.load().unwrap(), Some(sample()));
    let names: Vec<_> = entries(directory.path())
        .into_iter()
        .filter(|name| name.starts_with("settings.json.unreadable-"))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

/// Only a regular file is replaced: whatever else is at the path stays.
#[cfg(unix)]
#[test]
fn a_save_never_replaces_what_is_not_a_regular_file() {
    use std::os::unix::fs::FileTypeExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let _socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let mut store = SettingsFile::new(&path);
    assert!(store.load().is_err());
    let error = store.save(&sample()).unwrap_err();
    assert!(
        chain(&error).ends_with("it is not a regular file"),
        "{}",
        chain(&error)
    );
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
    assert_eq!(entries(directory.path()), ["settings.json"]);
}

fn no_link(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
}

/// Where a hard link fails, the unreadable file is kept as a copy with its
/// permissions, and stays at its path until the save's rename.
#[cfg(unix)]
#[test]
fn without_a_hard_link_the_unreadable_file_is_kept_as_a_copy() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut store = SettingsFile::new(&path).with_link(no_link);
    assert!(store.load().is_err());
    store.save(&sample()).unwrap();
    let aside = set_aside(directory.path());
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(aside[0].1, b"{ not json");
    let mode = fs::metadata(directory.path().join(&aside[0].0))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o640);
    assert_eq!(store.load().unwrap(), Some(sample()));
}

/// A file this user may neither link (Linux's `fs.protected_hardlinks`)
/// nor read is moved aside, and moved back if the save's rename fails.
#[cfg(unix)]
#[test]
fn without_a_hard_link_or_a_copy_the_unreadable_file_is_moved_aside() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&path).is_ok() {
        // Running as root: permissions do not apply, nothing to test.
        assert!(
            std::env::var_os("CI").is_none(),
            "CI runs tests as a user that permissions apply to"
        );
        eprintln!("skipped: permissions do not apply to this user");
        return;
    }
    let size = |path: &Path| fs::symlink_metadata(path).unwrap().len();

    // The save's rename fails: the file is back at its path.
    let mut failing = SettingsFile::new(&path)
        .with_link(no_link)
        .with_replace(|_, _| Err(std::io::Error::other("disk full")));
    assert!(failing.load().is_err());
    assert!(failing.save(&sample()).is_err());
    assert_eq!(entries(directory.path()), ["settings.json"]);
    assert_eq!(size(&path), 10);

    let mut store = SettingsFile::new(&path).with_link(no_link);
    assert!(store.load().is_err());
    store.save(&sample()).unwrap();
    let names: Vec<_> = entries(directory.path())
        .into_iter()
        .filter(|name| name.starts_with("settings.json.unreadable-"))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert_eq!(size(&directory.path().join(&names[0])), 10);
    assert_eq!(store.load().unwrap(), Some(sample()));
}

/// A file moved aside that cannot be moved back after a failed rename: the
/// save says where it now is, typed.
#[cfg(unix)]
#[test]
fn a_file_moved_aside_that_cannot_go_back_is_named_in_the_failure() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&path).is_ok() {
        // Running as root: permissions do not apply, nothing to test.
        assert!(
            std::env::var_os("CI").is_none(),
            "CI runs tests as a user that permissions apply to"
        );
        eprintln!("skipped: permissions do not apply to this user");
        return;
    }
    // The rename fails, and leaves something at the path the file cannot
    // be moved back over.
    let mut store = SettingsFile::new(&path)
        .with_link(no_link)
        .with_replace(|_, path| {
            fs::create_dir(path)?;
            fs::write(path.join("in the way"), "")?;
            Err(std::io::Error::other("disk full"))
        });
    assert!(store.load().is_err());
    let error = store.save(&sample()).unwrap_err();
    let left = error.cause::<super::LeftAside>().expect("typed");
    let message = left.to_string();
    assert!(
        message.starts_with("disk full; the file it could not read is now only at "),
        "{message}"
    );
    let aside: Vec<_> = entries(directory.path())
        .into_iter()
        .filter(|name| name.starts_with("settings.json.unreadable-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert!(message.contains(&aside[0]), "{message}");
}
