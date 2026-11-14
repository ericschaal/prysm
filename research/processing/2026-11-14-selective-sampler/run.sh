#!/bin/sh
# Reproduces the colour focus tables from the working tree. Nothing is
# downloaded and nothing is written to the repository: everything builds in
# $WORK and the tables are printed as Markdown.
#   WORK=/some/scratch/dir sh run.sh > results.md
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
WORK=${WORK:?set WORK to a scratch directory}

# The two-part evaluation's own tests print the accuracy tables.
table() {
  cargo test --release --offline -j 4 --manifest-path "$REPO/Cargo.toml" \
    --target-dir "$WORK/target" -p prysm-processor --lib "nodes::spatial_sampler::color_focus::tests::$1" \
    -- --exact --include-ignored --nocapture 2>/dev/null | grep '^|'
  echo
}
echo "## Accuracy against every cell at full resolution: edge depth 10 %, p95 and max per picture (steps)"
echo
table the_two_part_evaluation_agrees_with_every_cell_at_full_resolution
echo "## Accuracy and cost by edge depth and evaluation sizes, focus 100 (steps)"
echo
table evaluation_sizes_table

echo "## Cost on this machine"
echo
echo '```text'
cargo run --release --offline -j 4 --manifest-path "$REPO/Cargo.toml" \
  --target-dir "$WORK/target" -p prysm-processor --example bench 2>/dev/null
echo '```'
echo

mkdir -p "$WORK/selectiverun"
cp "$HERE/selectiverun.rs" "$WORK/selectiverun/selectiverun.rs"
cp "$REPO/Cargo.lock" "$WORK/selectiverun/Cargo.lock"
cat > "$WORK/selectiverun/Cargo.toml" <<TOML
[package]
name = "selectiverun"
version = "0.0.0"
edition = "2024"
[[bin]]
name = "selectiverun"
path = "selectiverun.rs"
[dependencies]
prysm-core = { path = "$REPO/crates/core" }
prysm-processor = { path = "$REPO/crates/processor" }
[workspace]
TOML
cargo build --release --offline -j 4 --manifest-path "$WORK/selectiverun/Cargo.toml" \
  --target-dir "$WORK/target" >&2
echo "## Effect and flicker"
echo
"$WORK/target/release/selectiverun"
