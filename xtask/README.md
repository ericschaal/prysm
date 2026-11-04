# Repository tasks

Run `cargo xtask --help` inside the repository. The Cargo alias builds the runner;
no extension is needed. Tasks run from the root and stop on failure.

| Command | Purpose |
| --- | --- |
| `cargo xtask setup` | Install locked shared tooling and both UIs. |
| `cargo xtask setup host` / `setup webos` | Install shared tooling and one UI. |
| `cargo xtask check` | Check contracts, frontends, packaging helpers, docs and default Rust. |
| `cargo xtask check rust` / `check frontend` | Run one group. |
| `cargo xtask codegen` | Regenerate both JSON/TypeScript contracts. |
| `cargo xtask codegen api` / `codegen webos` | Regenerate one contract. |
| `cargo xtask codegen --check` | Check freshness without writing. |
| `cargo xtask package deb` | Check the host contract and build a Debian arm64 package. |
| `cargo xtask package docker` | Build `prysm:local` for the Docker host. |
| `cargo xtask package webos` | Check the TV contract and build an IPK using `WEBOS_SDK`. |
| `cargo xtask package webos --hardware-h264` | Include the experimental TV encoder. |

Use Node.js 24+/npm. Run setup after cloning or changing lockfiles. Both generators
use the host UI's tooling, including TV runtime-validator generation, so TV codegen
also needs `setup host`. Rust owns serialized types; generation is explicit.

`cargo build` targets the host app and embeds its UI in Cargo's build directory.
It needs installed UI dependencies but does not install them. Standalone UI builds
and tests write their own `dist/`. Package tasks check generated contracts without
repairing them. Run checks before packaging; builds never install or deploy.

Docker builds supply their own Rust/Node dependencies; `docker build -t prysm:local .`
needs no host Rust or npm setup. See [package prerequisites](../docs/developers/packaging.md).
Default checks need neither the TV SDK nor GStreamer. CI checks additional features,
ARM and Miri. Luna bindings use a separate
[SDK generation command](../apps/prysm-webos-service/README.md#build-and-ffi-generation).
