set -eu
: "${WEBOS_SDK:?Set WEBOS_SDK to the relocated SDK directory}"
sdk=${WEBOS_SDK:?Set WEBOS_SDK to the relocated buildroot-nc4 SDK directory}
sdk=$(cd "$sdk" && pwd)
export WEBOS_SDK="$sdk"
root="$(pwd)/work/webos-encoder-latency/ui-combined/combined-source"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_LINKER="$root/packaging/webos/linker.sh"
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
export CARGO_TARGET_DIR="$(pwd)/target"
cd "$root"
cargo build --offline --locked -p prysm-webos-capture --features hardware-h264 --target armv7-unknown-linux-gnueabi --profile webos
cp "$CARGO_TARGET_DIR/armv7-unknown-linux-gnueabi/webos/prysm-webos-capture" "$CARGO_TARGET_DIR/../work/webos-encoder-latency/ui-combined/combined-worker"
