#!/bin/sh
# Run on Debian arm64 (native or in a Debian arm64 builder).
set -eu
cd "$(dirname "$0")/.."
npm ci --prefix apps/prysm-web/control-ui
npm run generate-api --prefix apps/prysm-web/control-ui
npm test --prefix apps/prysm-web/control-ui
cargo test --locked --workspace --no-default-features --exclude prysm-renderer-desktop
cargo build --locked --release -p prysm --no-default-features --bins
cargo build --locked --release -p prysm-web --no-default-features --bins
cargo deb -p prysm-web --no-build
