#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/prysm-blackbar-repro.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
python3 - "$repo" "$work" <<'PY'
import json, pathlib, sys
repo, work = map(pathlib.Path, sys.argv[1:])
(work/'Cargo.toml').write_text('''[package]
name = "prysm-blackbar-repro"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
prysm-core = { path = %s }
[[bin]]
name = "repro"
path = %s
''' % (json.dumps(str(repo/'crates/common/prysm-core')), json.dumps(str(repo/'research/black-bar-detection-2026-11-02/repro.rs'))))
PY
cargo run --offline --release --manifest-path "$work/Cargo.toml" -- "$@"
