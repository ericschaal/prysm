# Repository tasks

Run `cargo xtask --help`. Requires Node.js 24+/npm except Docker packaging.

| Command | Action |
| --- | --- |
| `cargo xtask setup [host\|webos]` | Install locked tooling/UI dependencies; omit group for both. |
| `cargo xtask check [rust\|frontend]` | Run checks; omit group to include both, packaging helpers and docs. |
| `cargo xtask codegen [api\|webos]` | Generate contracts; omit group for both. Add `--check` for freshness only. |
| `cargo xtask package deb` | Build Debian arm64 package. |
| `cargo xtask package docker` | Build `prysm:local`. |
| `cargo xtask package webos` | Build IPK using `WEBOS_SDK`, including hardware H.264 support. |

Run setup after lockfile changes. TV codegen also needs host UI tooling.
`cargo build` embeds the panel but never installs npm dependencies. Generation is
explicit; packaging verifies without repairing contracts and never installs/deploys.
[Package prerequisites](../docs/developers/packaging.md) ·
[Luna binding generation](../apps/prysm-webos-service/README.md#build-and-ffi-generation)
