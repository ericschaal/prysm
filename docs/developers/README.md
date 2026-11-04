# Develop Prysm

[Architecture](architecture.md) · [Debugging](debugging.md) · [User setup](../users/README.md)

## Build

Requires stable Rust (edition 2024), Node.js 24+/npm, and Xcode Command Line Tools
on macOS. On Linux, install:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev
```

```sh
cargo xtask setup host
cargo build --locked --release
./target/release/prysm --config settings.json
```

Open `http://localhost:8787`, complete setup, select **Screen sync** and turn on
the lights. For development: `cargo run -p prysm -- --config settings.json`.
The executable embeds the panel; Node is only needed to build. File input needs
FFmpeg 9+. TV binaries need the [webOS SDK](packaging.md#webos).

## Test

```sh
cargo xtask setup
cargo xtask check
```

Use `cargo test --locked -p CRATE` for a targeted check. See
[tasks](../../xtask/README.md) for check groups and generation. Linux all-feature
builds need [GStreamer dependencies](../../.github/workflows/check.yml).

## Components

[Settings/API](control-api.md) · [Frontend](frontend.md) · [Calibration](calibration.md) ·
[webOS](webos.md) · [Hardware H.264](hardware-h264.md) ·
[WLED renderer](../../crates/rendering/prysm-renderer-wled/README.md) ·
[Monitoring](monitoring.md) · [Packaging](packaging.md)

[Contribution rules](guidelines.md) · [Errors](error-handling.md) · [Decisions](decisions/README.md)
