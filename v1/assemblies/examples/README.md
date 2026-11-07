# Local host examples

Run these commands from `v1/`. The executable selects the output composition;
the JSON file configures it. Start without `--start` to stay idle.

| Binary | Settings example | Output |
| --- | --- | --- |
| `prysm-headless` | `headless.json` | WLED address and LED counts |
| `prysm-tui` | `terminal.json` | Terminal LED counts and interactive preview |
| `prysm-tui` | `prysm-webcam-terminal.json` | macOS webcam and terminal preview |

Configure the WLED address and capture device before starting the headless host:

```sh
cargo run --release -p prysm-assemblies --features headless --bin prysm-headless -- \
  assemblies/examples/headless.json --start
```

The webcam example contains a specific AVFoundation device UID. Use your webcam's
UID if it differs, then run:

```sh
cargo run --release -p prysm-assemblies --features tui --bin prysm-tui -- \
  assemblies/examples/prysm-webcam-terminal.json --start
```

In the TUI, press `:` to enter `on`, `off`, or `load /path/to/candidate.json`.
Enter submits, Escape cancels editing, and Ctrl-C shuts down and releases resources.
Prepare changes in a separate candidate file; `load` replaces the full saved settings
at the startup path. Both binaries use the same application and host shutdown lifecycle.
