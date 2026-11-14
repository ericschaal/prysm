#!/bin/sh
# Reproduces the adaptive-smoothing tables from the working tree. Nothing is
# downloaded and nothing is written to the repository: the harness builds in
# $WORK and prints Markdown.
#   WORK=/some/scratch/dir sh run.sh > results.md
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
WORK=${WORK:?set WORK to a scratch directory}
mkdir -p "$WORK/smoothrun"
cp "$HERE/smoothrun.rs" "$WORK/smoothrun/smoothrun.rs"
cp "$REPO/Cargo.lock" "$WORK/smoothrun/Cargo.lock"
cat > "$WORK/smoothrun/Cargo.toml" <<TOML
[package]
name = "smoothrun"
version = "0.0.0"
edition = "2024"
[[bin]]
name = "smoothrun"
path = "smoothrun.rs"
[dependencies]
prysm-core = { path = "$REPO/crates/core" }
prysm-processor = { path = "$REPO/crates/processor" }
[workspace]
TOML
cargo build --release --offline -j 4 --manifest-path "$WORK/smoothrun/Cargo.toml" \
  --target-dir "$WORK/target" >&2
"$WORK/target/release/smoothrun"
