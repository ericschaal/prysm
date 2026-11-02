# Develop Prysm

Run commands from the repository root unless a guide says otherwise.
For installation and everyday use, see [Run Prysm](../users/README.md).

## Build

Install a current stable Rust toolchain supporting edition 2024, Node.js 24, and npm.
Node builds the embedded web panel; it is not needed to run the built executable.
On Linux, install native capture/build dependencies:

```sh
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev
```

Desktop builds also need the platform's windowing/OpenGL development libraries;
on Debian/Ubuntu install `libxkbcommon-dev libwayland-dev libgl1-mesa-dev`.
On macOS, install Xcode Command Line Tools. FFmpeg 9+ is needed at runtime only
for file input. Hardware H.264 for webOS has [separate SDK requirements](packaging.md#webos).

```sh
cargo build --locked --release -p prysm
# Optional native preview window:
cargo build --locked --release -p prysm --features desktop
```

The executable is `target/release/prysm`. To use user-guide commands verbatim in
this shell, run `export PATH="$PWD/target/release:$PATH"` from the repository root.
Use a [user setup guide](../users/README.md)
to create your configuration, then run:

```sh
./target/release/prysm --config prysm-local.toml check
./target/release/prysm --config prysm-local.toml
```

For development, use `cargo run -p prysm -- --config prysm-local.toml` instead.
Add `--features desktop` before `--` for egui output. The repository-root TOML files
are local examples, not portable defaults. Startup is idle for every output.

## Test

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
python3 scripts/check-doc-links.py
```

Run checks for the crate you changed first. Full Linux workspace builds with all
features also need GStreamer development libraries, as configured in
[CI](../../.github/workflows/check.yml). See [frontend tests](frontend.md) for UI/API changes.

CI covers Linux/macOS, headless/desktop, generated API freshness, ARM webOS tests,
Miri, and Debian/IPK packaging. Physical capture devices, a native desktop session,
and TV firmware behavior need [manual checks](debugging.md#hardware-checks).

## Find the right guide

| Task | Documentation |
| --- | --- |
| Understand threads, ownership, and frame delivery | [Architecture](architecture.md) |
| Understand why the system is structured this way | [Technical decisions](decisions/README.md) |
| Change code or documentation | [Guidelines](guidelines.md), [error handling](error-handling.md) |
| Change HTTP/configuration behavior | [Control API](control-api.md) |
| Work on React or generated clients | [Frontend](frontend.md) |
| Diagnose failures or performance | [Debugging](debugging.md), [monitoring](monitoring.md), [metric catalog](../../observability/CATALOG.md) |
| Build Debian or TV packages | [Packaging](packaging.md) |
| Work on the TV worker/service | [webOS lifecycle and permissions](webos.md), [worker](../../apps/prysm-webos-capture/README.md), [supervisor](../../apps/prysm-webos-service/README.md) |
| Work on hardware H.264 | [Encoder contract](hardware-h264.md) |
| Work on WLED transport | [Renderer contract](../../crates/rendering/prysm-renderer-wled/README.md) |

[Research](../../research/README.md) retains experiments and historical results.
