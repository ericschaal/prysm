# Develop Prysm

The product lives in [`v1/`](../../v1/README.md): one Cargo workspace
(`v1/Cargo.toml`) and one npm workspace (`v1/frontend`).

[Architecture](architecture.md) · [Debugging](debugging.md) ·
[User setup](../users/README.md)

## Requirements

- Stable Rust (edition 2024) and Node.js 24+ with npm.
- Linux: `clang` and `libclang-dev` (V4L2 bindings are generated with bindgen).
- macOS: Xcode Command Line Tools.
- Optional: FFmpeg 9+ on `PATH` for the video-file source and its tests.
- TV builds: see [packaging](packaging.md#webos).

## Build and run

Run everything from the repository root. The Rust hosts embed the frontend
bundle, so build it first; Cargo never runs npm.

```sh
npm --prefix v1/frontend ci
npm --prefix v1/frontend run build
cargo build --manifest-path v1/Cargo.toml
```

This builds the two desktop hosts:

| Host | Use |
| --- | --- |
| `prysm-headless` | The service: web portal only. Shipped as the Debian package and Docker image. |
| `prysm-tui` | Local testing: the web portal plus a terminal preview of what the LEDs show. |

Both take a settings file and the same options:

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-headless -- --help
cargo run --manifest-path v1/Cargo.toml -p prysm-headless -- my-settings.json
cargo run --manifest-path v1/Cargo.toml -p prysm-tui -- v1/hosts/tui/settings.example.json
```

A missing settings file means "not set up": the portal opens the setup wizard
at `http://localhost:8787` and creates the file on the first save. The TUI
example uses the `null` output, so it needs no LEDs; its keys are `p` (Screen
sync), `n` (device effects), `o` (off), `:` (command bar) and `q` (quit).
Options, logging and exit codes: [hosts](../../v1/README.md#hosts).

### Play a video file

The `file` source plays a video through FFmpeg 9+ and is for development only:
neither the Docker image nor the Debian package includes FFmpeg. Edit the path
in [settings.file.example.json](../../v1/hosts/tui/settings.file.example.json),
then:

```sh
cargo run --manifest-path v1/Cargo.toml -p prysm-tui -- v1/hosts/tui/settings.file.example.json
```

It plays once (`"repeat": true` loops it); at the end lighting stops cleanly.

## Check

The same commands CI runs ([v1.yml](../../.github/workflows/v1.yml)):

```sh
cargo fmt --manifest-path v1/Cargo.toml --all --check
cargo clippy --manifest-path v1/Cargo.toml --locked --workspace --all-targets --all-features -- -D warnings
cargo test --manifest-path v1/Cargo.toml --locked --workspace --all-targets --all-features
npm --prefix v1/frontend run lint
npm --prefix v1/frontend run format:check
npm --prefix v1/frontend run codegen:check
npm --prefix v1/frontend test
python3 -m unittest discover -s v1/packaging/webos/tests
python3 scripts/check-doc-links.py
```

Use `cargo test --manifest-path v1/Cargo.toml -p CRATE` for one crate. The
TV crates are also linted and unit-tested for armv7 under qemu in CI; the IPK
build needs the webOS SDK. Tests on a TV are separate and manual
([debugging](debugging.md#hardware-checks)).

## Topics

[Architecture](architecture.md) · [Portal API and settings](control-api.md) ·
[Frontend](frontend.md) · [Calibration](calibration.md) · [LG TV](webos.md) ·
[Packaging](packaging.md) · [Releasing](releasing.md) ·
[Monitoring](monitoring.md) · [Errors](error-handling.md) ·
[Contribution rules](guidelines.md) · [Decisions](decisions/README.md)
