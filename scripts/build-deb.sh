#!/bin/sh
# Run on Debian arm64 (native or in a Debian arm64 builder).
set -eu
cd "$(dirname "$0")/.."
npm ci --prefix crates/web/prysm-web/control-ui
npm run generate-api --prefix crates/web/prysm-web/control-ui
npm test --prefix crates/web/prysm-web/control-ui
cargo test --locked --workspace --no-default-features --exclude prysm-renderer-desktop
cargo build --locked --release -p prysm --no-default-features --bins
cargo deb -p prysm --no-build
