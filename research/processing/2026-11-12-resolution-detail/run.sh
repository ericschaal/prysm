#!/bin/sh
# Reproduces the resolution-detail measurement. Nothing is written to the repo.
#   WORK=/some/scratch/dir sh run.sh
# Needs cargo, ffmpeg (with lavfi; no libass/drawtext required) and network
# access to download.blender.org for the Sintel excerpts (skipped when present).
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
WORK=${WORK:?set WORK to a scratch directory}
REV=${HEAD_REV:-HEAD}
mkdir -p "$WORK/build" "$WORK/sintel" "$WORK/out"

# 1. HEAD copy of prysm-processor (renamed), next to the working tree's.
rm -rf "$WORK/build/head"
mkdir -p "$WORK/build/head"
git -C "$REPO" archive "$REV" crates/processing/prysm-processor | tar -x -C "$WORK/build/head"
HEADCRATE="$WORK/build/head/crates/processing/prysm-processor"
cat > "$HEADCRATE/Cargo.toml" <<EOF
[package]
name = "prysm-processor-head"
version = "0.1.0"
edition = "2024"
autoexamples = false
autotests = false
autobenches = false
[dependencies]
futures = "0.3"
tracing = "0.1"
prysm-core = { path = "$REPO/crates/common/prysm-core" }
[features]
parallel-projection = []
EOF
# Working-tree copy that reads every pixel into the capped 640x360 grid: isolates
# the point-sampled lattice from the grid cap (and stands in for averaging s x s
# blocks in linear light at the capped grid).
rm -rf "$WORK/build/blocks"
mkdir -p "$WORK/build/blocks"
cp -R "$REPO/crates/processing/prysm-processor/src" "$WORK/build/blocks/src"
sed -i.orig 's/self.step = lattice_step(width, height);/self.step = 1;/' "$WORK/build/blocks/src/nodes/spatial_sampler.rs"
grep -q 'self.step = 1;' "$WORK/build/blocks/src/nodes/spatial_sampler.rs"
sed 's/name = "prysm-processor-head"/name = "prysm-processor-blocks"/' "$HEADCRATE/Cargo.toml" > "$WORK/build/blocks/Cargo.toml"
mkdir -p "$WORK/build/ledrun"
cp "$HERE/ledrun.rs" "$WORK/build/ledrun/ledrun.rs"
cp "$REPO/Cargo.lock" "$WORK/build/ledrun/Cargo.lock"
cat > "$WORK/build/ledrun/Cargo.toml" <<EOF
[package]
name = "ledrun"
version = "0.0.0"
edition = "2024"
[[bin]]
name = "ledrun"
path = "ledrun.rs"
[dependencies]
prysm-core = { path = "$REPO/crates/common/prysm-core" }
prysm-processor = { path = "$REPO/crates/processing/prysm-processor" }
prysm-processor-head = { path = "$HEADCRATE" }
prysm-processor-blocks = { path = "$WORK/build/blocks" }
[workspace]
EOF
cargo build --release --manifest-path "$WORK/build/ledrun/Cargo.toml" --target-dir "$WORK/build/target"
LEDRUN="$WORK/build/target/release/ledrun"

# 2. Real content: Sintel (Blender Foundation, CC BY 3.0), 1920x818 BT.709,
#    30 s excerpts copied without re-encoding (they start at the keyframe at or
#    before the given second; English subtitle track kept but not rendered).
SINTEL=https://download.blender.org/durian/movies/Sintel.2010.1080p.mkv
for spec in s1:95 s2:440 s3:500 s4:715; do
  name=${spec%%:*}; start=${spec##*:}
  [ -s "$WORK/sintel/$name.mkv" ] || ffmpeg -v error -y -ss "$start" -i "$SINTEL" -t 30 \
    -map 0:0 -map 0:3 -c copy "$WORK/sintel/$name.mkv"
done

# 3. Each source becomes a 1920x1080 BT.709 limited-range picture ("master"),
#    then is box-filtered to each capture size like the TV scaler, as NV12.
#    The film is letterboxed into 1920x1080 as a TV shows it.
GG="/System/Library/Wallpapers/.default/Golden Gate.mov"
TOYUV="scale=out_color_matrix=bt709:out_range=tv,format=yuv420p"
run() { # name fps "input args" "master filter"
  name=$1; fps=$2; input=$3; master=$4
  for size in 1920x1080 1280x720 640x360 320x180; do
    w=${size%x*}; h=${size#*x}
    # shellcheck disable=SC2086
    eval ffmpeg -v error $input -vf "\"$master,scale=$w:$h:flags=area:out_range=tv,format=nv12\"" \
      -f rawvideo - | "$LEDRUN" "$w" "$h" "$fps" "$WORK/out/$name.$size"
  done
}
for s in s1 s2 s3 s4; do
  run "sintel-$s" 24 "-i '$WORK/sintel/$s.mkv' -t 30" "pad=1920:1080:0:131:black"
done
if [ -f "$GG" ]; then
  run goldengate 24 "-t 20 -i '$GG'" "fps=24,scale=1920:1080:flags=area,format=yuv420p"
fi
run testsrc2 24 "-f lavfi -i testsrc2=s=1920x1080:r=24:d=10" "$TOYUV"
run mandelbrot 24 "-f lavfi -i mandelbrot=s=1920x1080:r=24 -t 10" "$TOYUV"
run zoneplate 24 "-f lavfi -i zoneplate=s=1920x1080:r=24:kx2=160:ky2=160:kt=1 -t 10" "$TOYUV"
# Hard red|blue vertical edge sweeping left to right at 192 px/s (1080p pixels).
run sweep 24 "-f lavfi -i 'color=c=red:s=1920x1080:r=24:d=10[a];color=c=blue:s=1920x1080:r=24:d=10[b];[a][b]overlay=x=192*t:shortest=1[out0]'" "$TOYUV"

# 4. Tables.
python3 "$HERE/analyze.py" "$WORK/out"
