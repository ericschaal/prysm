# File settings store

`prysm-settings-file` implements `prysm_ports::SettingsStore` with one JSON file.
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

## Format

Pretty-printed JSON (two-space indent, trailing newline) of `prysm_ports::Settings`
after the file's `format`. Saving the same settings produces the same bytes, so
the file can be hand-edited and kept under version control. Omitted
`capture`/`output` mean unconfigured; omitted `processing` fields take their
defaults. Unknown fields fail `load` with the field name, line and file path.
Adapter fields under `kind` are decoded by the adapter's factory, not here.

`format` is the file format, the constant `FORMAT` (now `1`), written by every
save. `load` reads it first, before the fields, and refuses with a typed
`FormatError` as the cause: a file without it (written by hand or before
Prysm 1.0; add `"format": 1` once its fields match), a value that is not a
whole number (`null`, `"1"`, `2.0`), any whole number above `FORMAT` however
large (written by a newer Prysm; both numbers are named), and a lower one.
There is no migration: a Prysm reads its own format only.

`FORMAT` is raised only when Prysm can no longer read files with the previous
number: a field renamed or removed, or its meaning changed. Additions keep
the number: an older Prysm refuses a newer file by the unknown field's name.

```json
{
  "format": 1,
  "capture": {
    "kind": "device",
    "path": "/dev/video0"
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
| `smoothing_seconds` | 0 or more | Seconds to complete most of a change; 0 disables smoothing |
| `cut_response_percent` | 0-100 | How much of a scene cut the lights take at once |
| `brightness_ramp_seconds` | 0-2 | The least time from dark to full brightness; 0 sets no limit |
| `spatial_reach_percent` | 1-100 | Edge depth: how far into the picture each light looks |
| `color_focus_percent` | 0-100 | How much each light keeps the colour next to it |
| `saturation_boost_percent` | 0-100 | Scales the lights' chroma by 1 + p/100 |
| `dark_floor_percent` | 0-20 | The neutral white glow, in percent of full light, kept in dark scenes |

A refused value names its path, such as
`processing.custom_style.dark_floor_percent`. `brightness_percent`,
`sample_density`, `remove_black_bars` and the calibrations are outside
styles.

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
whole.

## Atomicity

`save` writes a temporary file in the same directory, fsyncs it, then renames it
over the target, and on Unix fsyncs the directory. The rename is the commit
point: a reader or a crash sees either the old file or the new one, never a
truncated mix. Any failure before the rename removes the temporary file and leaves
the existing file untouched. A failure of the final directory sync is reported,
but the new contents are already in place.

Permissions of an existing file are kept. A newly created file is owner-only
(`0600` on Unix).
