# Develop Prysm

Start with the [architecture](architecture.md) to see what runs where, or
[debugging](debugging.md) to investigate a problem. For installation and everyday
use, follow the [user guide](../users/README.md).

## Build

Run commands from the repository root. Install stable Rust with edition 2024
support and Node.js 24+/npm. Node builds the embedded panel; the finished Prysm
executable does not need it.

On Linux:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev
```

On macOS, install Xcode Command Line Tools. Then:

```sh
cargo xtask setup host
cargo build --locked --release
./target/release/prysm --config settings.json
```

Open `http://localhost:8787` and complete setup. Prysm starts idle; select
**Screen sync** and turn the lights on to capture. For a development build, use
`cargo run -p prysm -- --config settings.json`.

A normal build targets the host application and embeds its panel. File input
needs FFmpeg 9+ at runtime. TV binaries need a separate
[webOS cross-build](packaging.md#webos).

## Test

```sh
cargo xtask setup
cargo xtask check
```

For a smaller check, use `cargo test --locked -p CRATE`,
`cargo xtask check rust`, or `cargo xtask check frontend`.
The [task reference](../../xtask/README.md) lists generation and package commands.
CI also checks Linux/macOS, all features, ARM, Miri and packages. All-feature
Linux builds need GStreamer development libraries; see
[CI dependencies](../../.github/workflows/check.yml).
Hardware behavior needs [separate checks](debugging.md#hardware-checks).

## Focused guides

| Working on | Read |
| --- | --- |
| Settings, HTTP or lighting transitions | [Control API](control-api.md) |
| Browser panel or TV UI | [Frontend](frontend.md) |
| Color profiles and the TV/phone wizard | [Calibration](calibration.md) |
| TV capture, power or permissions | [webOS](webos.md) |
| Experimental TV hardware encoding | [Hardware H.264](hardware-h264.md) |
| WLED packets and LED order | [Renderer](../../crates/rendering/prysm-renderer-wled/README.md) |
| Metrics and performance | [Monitoring](monitoring.md) |
| Docker, Debian or IPK builds | [Packaging](packaging.md) |

Before contributing, read the [guidelines](guidelines.md) and
[error-handling rules](error-handling.md). [Decisions](decisions/README.md)
explain the main tradeoffs.
