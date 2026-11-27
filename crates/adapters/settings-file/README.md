# File settings store

`prysm-settings-file` implements `prysm_ports::SettingsStore` with a JSON configuration file.
It is plain blocking I/O; the runtime calls it off the async executor and owns
ordering (validate, persist, apply).

```rust
let store = prysm_settings_file::SettingsFile::new("/var/lib/prysm/settings.json");
```

## Location

The path is used as given; nothing is touched until the first `load` or `save`.
A relative path resolves against the process working directory. The parent
directory must exist: `save` never creates directories and fails with an error
naming the path.

The store saves the last selected lighting mode beside the configuration.
For `settings.json` the companion file is `settings.json.mode` and contains
the JSON string `"effects"` or `"screen_sync"`. Each write uses an atomic
rename. Powering off preserves this choice for the next Power press.

## Format

Pretty-printed JSON (two-space indent, trailing newline) of `prysm_ports::Settings`
after the file's `format`. Saving the same settings produces the same bytes, so
the file can be hand-edited and kept under version control. Omitted
`capture`/`output` mean unconfigured; omitted `processing` fields take their
defaults. Unknown fields fail `load` with the field name, line and file path;
an array where a section or an object in it belongs fails with what was
expected (`style values as an object`), line and file path.
Adapter fields under `kind` are decoded by the adapter's factory, not here.

`format` is the file format, the constant `FORMAT` (now `1`), written by every
save. `load` reads it first, before the fields, and refuses with a typed
`FormatError` as the cause: a file without it (written by hand or before
Prysm 1.0; add `"format": 1` once its fields match), a value that is not a
whole number (`null`, `"1"`, `2.0`), any whole number above `FORMAT` however
large (written by a newer Prysm; both numbers are named), and a lower one.
A repeated `format` field is refused. There is no migration: a Prysm reads its own format only.

`FORMAT` is raised only when Prysm can no longer read files with the previous
number: a field renamed or removed, or its meaning changed. Additions keep
the number: an older Prysm refuses a newer file by the unknown field's name.

```json
{
  "format": 1,
  "capture": {
    "kind": "device",
    "device": "/dev/video0"
  },
  "output": {
    "kind": "wled",
    "address": "wled.local",
    "leds": { "top": 96, "right": 54, "bottom": 96, "left": 54 },
    "dither": true
  },
  "processing": {
    "brightness_percent": 80,
    "style": { "kind": "calm" },
    "custom_style": {
      "smoothing_seconds": 0.5,
      "cut_response_percent": 60,
      "brightness_ramp_seconds": 0.5,
      "spatial_reach_percent": 15,
      "color_focus_percent": 0,
      "saturation_boost_percent": 20,
      "dark_floor_percent": 5
    },
    "sample_density": 128,
    "remove_black_bars": true,
    "calibrations": [
      {
        "id": "ps5",
        "name": "PS5",
        "correction": {
          "warmth": 12,
          "tint": -4,
          "midtones": 0,
          "red": { "hue": 8, "strength": -20 },
          "green": { "hue": 0, "strength": -10 },
          "blue": { "hue": -5, "strength": 0 }
        }
      }
    ],
    "active_calibration": "ps5"
  }
}
```

`processing.style` is the chosen style: `{ "kind": "natural" }` (the
default), `"calm"`, `"vivid"`, `"game"` or `"custom"`. A built-in style stores
only its name; its values are the table in
[`Style::built_in`](../../engine/ports/src/style.rs), so an update can retune
it. `processing.custom_style` holds the user's own values, applied while the
kind is `custom` and kept while another style is chosen; it is omitted until
first tuned, and `custom` without it is refused. Every one of its seven
fields is required:

| Field | Range | Effect |
| --- | --- | --- |
| `smoothing_seconds` | 0-2 | Seconds to complete most of a change; 0 disables smoothing |
| `cut_response_percent` | 0-100 | How much of a scene cut the lights take at once |
| `brightness_ramp_seconds` | 0-2 | The least time from dark to full brightness; 0 sets no limit |
| `spatial_reach_percent` | 1-100 | Edge depth: how far into the picture each light looks |
| `color_focus_percent` | 0-100 | How much each light keeps the colour next to it |
| `saturation_boost_percent` | 0-100 | Scales the lights' chroma by 1 + p/100 |
| `dark_floor_percent` | 0-20 | The neutral white glow, in percent of full light, kept in dark scenes |

A refused value names its path, such as
`processing.custom_style.dark_floor_percent`. `brightness_percent` (0-100),
`sample_density` (1-512), `remove_black_bars` and the calibrations are
outside styles.

With a custom style:

```json
{
  "format": 1,
  "processing": {
    "style": { "kind": "custom" },
    "custom_style": {
      "smoothing_seconds": 0.1,
      "cut_response_percent": 0,
      "brightness_ramp_seconds": 0,
      "spatial_reach_percent": 10,
      "color_focus_percent": 0,
      "saturation_boost_percent": 0,
      "dark_floor_percent": 0
    }
  }
}
```

`processing.calibrations` holds up to 32 saved calibrations (colour
corrections) with unique ids (1-64 ASCII letters, digits, `-` or `_`) and names
(1-64 characters, no surrounding whitespace or control characters); every
control is -100 to 100 and omitted ones are 0. `warmth` and `tint` balance
white, `midtones` lightens or darkens them; `red`, `green` and `blue` each turn
that colour toward the next (`hue` positive: red toward green, green toward
blue, blue toward red) and make it softer or stronger (`strength`), without
moving white ([correction maths](../../engine/core/src/correction.rs)). A
refused control names its path, such as
`processing.calibrations[0].correction.red.strength`. `active_calibration` names the
applied calibration; without it no correction is applied. Both are omitted when empty.

`load` returns `None` only when the file does not exist; an empty or malformed
file is an error, never silently treated as unconfigured. A file larger than
1 MiB is refused before parsing, so a wrong path (a device, a log) is not read
whole. `save` checks the bytes it is about to write the way `load` reads them
(size, format, fields) and refuses what `load` would refuse, before touching
anything, so whatever is saved loads.

## Unreadable files

A file that `load` refuses (no `format`, a newer or older format, cut-off or
invalid JSON, a wrong type or unknown field, over 1 MiB, a permission or I/O
error) is never replaced unseen: the engine then starts unconfigured and
shows the cause as the failure, and the file stays as it was. The next
`save`, which only a user's save from the portal makes, first gives it a
second name, `settings.json.unreadable-XXXXXX` (six random characters, a
name of its own, so no earlier one is overwritten): a hard link, so the file
never leaves its path before the new one replaces it. Then the new file is
renamed over the path as usual, owner-only rather than with the unreadable
file's permissions, and the kept name is logged. If that rename fails, the
second name is removed and the unreadable file is still where it was, so the
failure shown still matches the disk. When the hard link fails for any
reason but the file being gone (no hard links on the filesystem; on Linux
`fs.protected_hardlinks`, for a file Prysm neither owns nor may write, such
as one a `sudo` run left owned by root), the second name is a copy with the
file's permissions, which needs the file readable. When the copy fails too,
the file is renamed aside instead: then the path holds no file until the new
one is renamed over it, and a crash in between leaves it so (the next start
is unconfigured, with no failure shown, the old file kept beside). If that
rename fails, the file is moved back; if moving it back fails too, the save
fails saying so, with the name the file is now only under, and the path
holds no file. If all three fail, the save fails
naming each cause and nothing changes. If the file is gone by then, nothing
is set aside. None of these is fatal, since a save keeps the file and a restart
would fix none of them; only a panicking store stops start-up.

The tests drive these fallbacks through a hook that refuses the hard link.
The real Linux case was checked once in Docker: as a non-root user, with a
root-owned `0600` settings file in a directory the user may write, a save
kept the file as `settings.json.unreadable-…` and wrote the new one; making
another user's file needs root, so no test does it.

`save` replaces only a regular file (or a link to one): a directory, socket
or device at the path is refused (`it is not a regular file`) and left alone.

## Atomicity

`save` writes a temporary file in the same directory, fsyncs it, then renames it
over the target, and on Unix fsyncs the directory. The rename is the commit
point: a reader or a crash sees either the old file or the new one, never a
truncated mix. Any failure before the rename removes the temporary file and leaves
the existing file untouched, and `save` fails. After the rename the save has
happened: a failure of the final directory sync is logged as a warning (the
save may not survive a crash) and `save` succeeds, so the engine installs
the settings the file now holds.

Permissions of an existing file are kept. A newly created file is owner-only
(`0600` on Unix).

These steps are the public module `atomic` (`Staged`, `sync_directory`), and
`atomic::write` runs them in one call with given permissions; the TV
supervisor writes its files with it. There a failed directory sync is an
error, although the file is already replaced.
