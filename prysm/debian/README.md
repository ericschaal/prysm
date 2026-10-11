# Raspberry Pi OS arm64 package

Build on a Raspberry Pi running 64-bit Raspberry Pi OS, or an arm64 Linux
builder using the same Debian release. Build on the oldest release you intend
to support: a binary built on Trixie may not run on Bookworm.
`dpkg --print-architecture` must report `arm64`.

Install a current stable Rust toolchain with rustup (the project uses edition
2024), then install the build tools:

```sh
sudo apt update
sudo apt install build-essential pkg-config clang libclang-dev libudev-dev \
    libxkbcommon-dev libwayland-dev libegl1-mesa-dev dpkg-dev
cargo install cargo-deb --version 3.8.0 --locked
```

From the repository root:

```sh
cargo build --locked --release -p prysm --bin led
cargo deb -p prysm --no-build
```

The package is written to `target/debian/prysm-led_0.1.0-1_arm64.deb`.
Only the LED binary is installed, renamed to `/usr/bin/prysm-led`.
The current crate also compiles desktop dependencies, hence the graphics build
headers above. Cargo-deb detects shared-library runtime dependencies automatically.

Inspect the package before installing:

```sh
dpkg-deb --info target/debian/prysm-led_0.1.0-1_arm64.deb
dpkg-deb --contents target/debian/prysm-led_0.1.0-1_arm64.deb
```

Install and configure on the Pi:

```sh
sudo apt install ./target/debian/prysm-led_0.1.0-1_arm64.deb
sudo nano /etc/prysm/led.toml
sudo systemctl enable --now prysm-led
journalctl -u prysm-led -f
```

Installation creates a `prysm` system account using systemd-sysusers and leaves
the service disabled and stopped. Set the WLED address, LED counts, brightness,
and capture device before enabling it. `/etc/prysm/led.toml` is a Debian conffile,
so package upgrades preserve local edits.
Package upgrades stop the service; start it again with
`sudo systemctl start prysm-led` after the upgrade.

Use `ls -l /dev/v4l/by-id/` to find a stable capture device path, or select the
correct `/dev/videoN`. The service receives the `video` supplementary group;
check that the chosen device is readable and writable by that group. It does
not need a desktop login or root access.

```sh
sudo systemctl restart prysm-led # Apply configuration changes
sudo systemctl stop prysm-led
sudo systemctl disable prysm-led # Stop starting at boot
```

Logs go to the journal. Capture and transmission errors trigger a restart after
five seconds. Stop sends SIGINT, matching the binary's graceful shutdown handler.
WLED restores its normal effect after its realtime timeout.

For video-file input, set `source.type = "video"`, remove `source.device`, and set
`source.path` to an absolute path readable by `prysm`. That mode requires FFmpeg
9+ on the service's PATH; it is not installed by this camera-oriented package.
The service stops successfully at video EOF.

Packaging uses [cargo-deb's systemd integration](https://github.com/kornelski/cargo-deb/blob/main/systemd.md).
