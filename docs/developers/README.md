# Develop Prysm

The product is one Cargo workspace (`Cargo.toml`) and one npm workspace
(`frontend/`) at the repository root; see the [workspace map](workspace.md).

[Architecture](architecture.md) · [Debugging](debugging.md) ·
[User setup](../users/README.md)

## Requirements

- [rustup](https://rustup.rs) and Node.js 24+ with npm. The Rust version is
  pinned in [`rust-toolchain.toml`](../../rust-toolchain.toml), which rustup
  follows in the repository; `rustup toolchain install` there installs it
  with clippy and rustfmt.
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

`doctor` prints one line per tool (on Linux also libclang, looked up in
`LIBCLANG_PATH` when set, else in `ldconfig -p`) and fails only when a
tool `check` needs is missing or too old; CI's `frontend` job runs it. xtask
echoes the commands that build or check, not the questions it asks git.

For day-to-day work, run the hosts with Cargo. They embed the portal bundle
from `frontend/dist`, so build it first; Cargo never runs npm.

```sh
npm --prefix frontend ci
npm --prefix frontend run bundle
cargo build
```

Without the bundle, a debug build still compiles: Cargo warns, and the portal
shows one page saying the frontend is not built and how to build it. The API
works as usual, so Rust lints and tests need no npm. A release build (and the
`webos` profile) fails instead, so a shipped binary always carries the portal
([hosts/common/build.rs](../../hosts/common/build.rs)). The next build after
`npm run bundle` embeds it.

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

It prints how long each step took and stops at the first failing step, which
it names; `--keep-going` runs every step and lists all failures at the end
(the exit status is still non-zero). The steps fall into three groups, one
per CI job ([ci.yml](../../.github/workflows/ci.yml)); name one or more to run
only those, in the same order:

| Group | CI job | Steps |
| --- | --- | --- |
| `rust` | `rust (ubuntu-24.04)`, `rust (macos-26)` | `cargo fmt --check`, clippy with `-D warnings`, the Rust tests and doctests |
| `frontend` | `frontend` | lint and formatting, `cargo xtask codegen --check`, typecheck, bundle (`frontend/dist`), tests |
| `scripts` | `quality` | the Python tests in `scripts/tests` and `packaging/webos/tests`, the [acceptance runner's checks](acceptance.md#develop-the-runner), `scripts/check-doc-links.py` |

```sh
cargo xtask check rust            # no Node.js needed
cargo xtask check frontend scripts
```

Only the `frontend` group installs the npm packages and needs Node.js;
`scripts` needs `uv` (`cargo xtask doctor` checks both). Not included: shellcheck and
actionlint (CI's `quality` job runs them), the end-to-end tests, and the
armv7 and Miri jobs; CI's IPK and Docker jobs build with `cargo xtask dist`
([packaging](packaging.md#artifacts-in-dist)). In a full run, the contracts are checked before the
typecheck, so stale contracts fail as such, and the bundle is built before the
frontend tests and Rust, so the TV tests read it and the hosts embed the real
portal. Without the bundle, the Rust tests that check it are reported as
ignored.

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
