# Develop Prysm

The product is one Cargo workspace (`Cargo.toml`) and one npm workspace
(`frontend/`) at the repository root; see the [workspace map](workspace.md).

[Architecture](architecture.md) · [Debugging](debugging.md) ·
[User setup](../users/README.md)

## Requirements

- Stable Rust (edition 2024) and Node.js 24+ with npm.
- Linux: `clang` and `libclang-dev` (V4L2 bindings are generated with bindgen).
- macOS: Xcode Command Line Tools.
- Optional: FFmpeg 9+ on `PATH` for the video-file source and its tests.
- TV builds: see [packaging](packaging.md#webos).

## Build and run

`cargo xtask` is the entry point for generating, checking and building
([tools/xtask](../../tools/xtask/src/main.rs)). It runs the npm scripts,
Cargo, the Dockerfile, the TV package script and the Python checks in the
right order, and runs `npm ci` first when `frontend/node_modules` is missing
or older than `frontend/package-lock.json`.

```sh
cargo xtask doctor    # which tools are installed, and what needs them
cargo xtask codegen   # contracts from Rust; --check fails on drift, --luna adds the TV bindings
cargo xtask check     # the checks below, in order
cargo xtask dist mac  # an artifact in dist/; see packaging
```

For day-to-day work, run the hosts with Cargo. They embed the frontend
bundle, so build it first; Cargo never runs npm.

```sh
npm --prefix frontend ci
npm --prefix frontend run bundle
cargo build
```

This builds the two desktop hosts:

| Host | Use |
| --- | --- |
| `prysm-headless` | The service: web portal only. Shipped as the Debian package and Docker image. |
| `prysm-tui` | Local testing: the web portal plus a terminal preview of what the LEDs show. |

Both take a settings file and the same options:

```sh
cargo run -p prysm-headless -- --help
cargo run -p prysm-headless -- my-settings.json
cargo run -p prysm-tui -- hosts/tui/settings.example.json
```

A missing settings file means "not set up": the portal opens the setup wizard
at `http://localhost:8787` and creates the file on the first save. The TUI
example uses the `null` output, so it needs no LEDs; its keys are `p` (Screen
sync), `n` (device effects), `o` (off), `:` (command bar) and `q` (quit).
Options, logging and exit codes: [hosts](workspace.md#hosts).

### Play a video file

The `file` source plays a video through FFmpeg 9+ and is for development only:
neither the Docker image nor the Debian package includes FFmpeg. Edit the path
in [settings.file.example.json](../../hosts/tui/settings.file.example.json),
then:

```sh
cargo run -p prysm-tui -- hosts/tui/settings.file.example.json
```

It plays once (`"repeat": true` loops it); at the end lighting stops cleanly.

## Check

```sh
cargo xtask check
```

It runs the checks of CI's `quality`, `frontend` and `rust` jobs
([ci.yml](../../.github/workflows/ci.yml)) and stops at the first failing
step, which it names. Not included: shellcheck, actionlint, the end-to-end
tests, the armv7, Miri, IPK and Docker jobs. The order: Rust formatting, frontend
lint and formatting, `cargo xtask codegen --check`, the frontend typecheck,
bundle (`frontend/dist`) and tests, clippy with `-D warnings`, the Rust tests
and doctests, the Python tests in `scripts/tests` and `packaging/webos/tests`,
and `scripts/check-doc-links.py`. The contracts are checked before the
typecheck, so stale contracts fail as such, and the bundle is built before the
frontend tests and Rust because the TV tests read it and the hosts `include!`
it.

Use `cargo test -p CRATE` for one crate. The
TV crates are also linted and unit-tested for armv7 under qemu in CI; the IPK
build needs the webOS SDK. Tests on a TV are separate and manual
([debugging](debugging.md#hardware-checks), [acceptance runner](acceptance.md)).

## Topics

[Workspace map](workspace.md) · [Architecture](architecture.md) ·
[Contracts](contracts.md) · [UI assemblies](ui-assemblies.md) ·
[Portal API and settings](control-api.md) ·
[Frontend](frontend.md) · [Calibration](calibration.md) · [LG TV](webos.md) ·
[Packaging](packaging.md) · [Hardware acceptance](acceptance.md) ·
[Releasing](releasing.md) ·
[Monitoring](monitoring.md) · [Errors](error-handling.md) ·
[Contribution rules](guidelines.md) · [Decisions](decisions/README.md)
