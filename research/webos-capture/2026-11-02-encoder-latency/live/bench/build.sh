set -eu
export WEBOS_SDK=/Users/eschaal/webos-sdk/arm-webos-linux-gnueabi_sdk-buildroot
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
sdk=$(cd "$sdk" && pwd)
export WEBOS_SDK="$sdk"
root=/Users/eschaal/src/github.com/prysm
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/work/webos-encoder-latency/bench/linker.sh"
export CC_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-gcc"
export CXX_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-g++"
export AR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/arm-webos-linux-gnueabi-ar"
export CC_SHELL_ESCAPED_FLAGS=1
export CXXFLAGS_armv7_unknown_linux_gnueabi="--sysroot=\"$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot\" -mfloat-abi=softfp -mfpu=neon"
export CFLAGS_armv7_unknown_linux_gnueabi="$CXXFLAGS_armv7_unknown_linux_gnueabi"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUSTFLAGS="-C target-cpu=cortex-a9"
export PKG_CONFIG_armv7_unknown_linux_gnueabi="$WEBOS_SDK/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot"
export PKG_CONFIG_LIBDIR_armv7_unknown_linux_gnueabi="$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/lib/pkgconfig:$WEBOS_SDK/arm-webos-linux-gnueabi/sysroot/usr/share/pkgconfig"
export PKG_CONFIG_PATH_armv7_unknown_linux_gnueabi=
cd "$root"
out="$root/work/webos-encoder-latency/bench"
mode=${1:?baseline or current}
case "$mode" in
 baseline) features="--features baseline,hardware-h264" ;;
 current) features="--features hardware-h264" ;;
 *) exit 1 ;;
esac
cargo build --offline --manifest-path "$out/Cargo.toml" $features --target armv7-unknown-linux-gnueabi --profile webos --target-dir "$root/target" > "$out/build-$mode.log" 2>&1
cp "$root/target/armv7-unknown-linux-gnueabi/webos/prysm-encoder-latency-bench" "$out/bench-$mode"
shasum -a256 "$out/bench-$mode" > "$out/SHA256SUMS-$mode"
