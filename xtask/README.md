# Repository tasks

Run `cargo xtask --help` from anywhere inside the repository. The alias builds
this small Rust task runner; no Cargo extension needs installing. Tasks run from
the workspace root and stop when a child command fails.

| Command | Purpose |
| --- | --- |
| `cargo xtask setup` | Install locked npm dependencies for both UIs. |
| `cargo xtask setup host` / `setup webos` | Install dependencies for one UI. |
| `cargo xtask check` | Run frontend, contract, packaging-helper, documentation, and default Rust checks. |
| `cargo xtask check rust` / `check frontend` | Run one check group. |
| `cargo xtask codegen` | Regenerate both committed JSON/TypeScript contracts. |
| `cargo xtask codegen api` / `codegen webos` | Regenerate one contract. |
| `cargo xtask codegen --check` | Reject stale or missing generated files without writing. |
| `cargo xtask package deb` | Verify the host contract, build and package on Debian arm64. |
| `cargo xtask package webos` | Verify the TV contract and build the IPK with `WEBOS_SDK`. |
| `cargo xtask package webos --hardware-h264` | Include the optional TV hardware encoder. |

Use Node.js 24 or newer. CI uses Node 24, recorded in `.node-version`. Run setup after cloning or changing the npm lockfiles.
Both contract generators use the host UI's locked `openapi-typescript` package;
`setup host` is required even when generating only the TV contract.

Ordinary `cargo build` builds the host application and bundles its embedded UI
into Cargo's build directory. It requires host UI dependencies installed by
setup, but does not install them itself. Standalone `npm run build` and UI tests
use the frontend's own `dist/` directory. Code generation is always explicit;
packaging checks committed output and never repairs it.

Rust owns the actual serialized types. The API exporter includes preview
WebSocket message schemas. The TV exporter uses the service configuration and
status types, with its `schema` feature keeping schema tooling out of production
TV builds. Generation computes every selected output before writing any files.

Debian packaging invokes Cargo directly. The webOS script under `packaging/`
handles SDK setup and package assembly. Both tasks verify contracts first.
Run checks separately before packaging; packaging neither installs nor deploys.
See [packaging prerequisites](../docs/developers/packaging.md).

The default checks do not require the TV SDK or GStreamer. CI separately runs
all-feature checks, ARM emulation, Miri, and platform packaging. Native binding
generation remains the existing [explicit SDK workflow](../apps/prysm-webos-service/README.md#build-and-ffi-generation).
