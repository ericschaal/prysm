# File settings store

`prysm-settings-file` implements `prysm_ports::SettingsStore` with one JSON file.
It is plain blocking I/O; the runtime calls it off the async executor and owns
ordering (validate, persist, apply).

```rust
let store = prysm_settings_file::FileSettings::new("/var/lib/prysm/settings.json");
```

## Location

The path is used as given; nothing is touched until the first `load` or `save`.
A relative path resolves against the process working directory. The parent
directory must exist: `save` never creates directories and fails with an error
naming the path.

## Format

Pretty-printed JSON (two-space indent, trailing newline) of `prysm_ports::Settings`.
Saving the same settings produces the same bytes, so the file can be hand-edited
and kept under version control. Omitted `capture`/`output` mean unconfigured;
omitted `processing` fields take their defaults. Unknown fields fail `load` with
the field name, line and file path. Adapter fields under `kind` are decoded by
the adapter's factory, not here.

```json
{
  "capture": {
    "kind": "device",
    "path": "/dev/video0"
  },
  "output": {
    "kind": "wled",
    "address": "wled.local",
    "leds": { "top": 96, "right": 54, "bottom": 96, "left": 54 }
  },
  "processing": {
    "brightness_percent": 80,
    "shadow_lift_percent": 0,
    "smoothing_seconds": 0.3,
    "sample_density": 128,
    "spatial_reach_percent": 10,
    "remove_black_bars": true
  }
}
```

`load` returns `None` only when the file does not exist; an empty or malformed
file is an error, never silently treated as unconfigured.

## Atomicity

`save` writes a temporary file in the same directory, fsyncs it, then renames it
over the target, and on Unix fsyncs the directory. The rename is the commit
point: a reader or a crash sees either the old file or the new one, never a
truncated mix. Any failure before the rename removes the temporary file and leaves
the existing file untouched. A failure of the final directory sync is reported,
but the new contents are already in place.

Permissions of an existing file are kept. A newly created file is owner-only
(`0600` on Unix).
