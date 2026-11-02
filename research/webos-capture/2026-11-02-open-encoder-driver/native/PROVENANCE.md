# Native probe build provenance

`BUILD-SHA256SUMS` records the source and executable used for the successful TV runs. It is a build record, not a standalone checksum list for this directory: the ARM executable is intentionally retained at `work/webos-v4l2-native/v4l2-probe`, outside this archived research directory. Its SHA-256 is `19389ffb62e920c7a7748f094fa6dfa9954424531ed0803ddb61d832914a5548`.

`v4l2_probe.c` is archived here. The corresponding build command remains in `work/webos-v4l2-native/build.sh`. `manifest.json` lists hashes of the files actually archived here and records the unchanged production normalizer/decoder source hashes used by the checker. It does not claim the executable is archived.
