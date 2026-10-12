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
cargo build --locked --release -p prysm --bins
cargo deb -p prysm --no-build
```

The package is written to `target/debian/prysm-led_0.1.0-1_arm64.deb`.
The capture binary is installed as `/usr/bin/prysm`, and the independent control
panel as `/usr/bin/prysm-control`.
The package and systemd service retain the name `prysm-led`.
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
sudo nano /etc/prysm/prysm.toml
sudo systemctl enable --now prysm-led
journalctl -u prysm-led -f
```

Installation creates a `prysm` system account using systemd-sysusers and leaves
the service disabled and stopped. Set the WLED address, LED counts, brightness,
and input device before enabling it. Keep `output.type = "wled"` for the headless
service; `output.type = "egui"` requires a desktop session.
`/etc/prysm/prysm.toml` is a Debian conffile,
so package upgrades preserve local edits.
When upgrading from the separate `led` binary, migrate your customized
`/etc/prysm/led.toml` to `/etc/prysm/prysm.toml` before starting the service:
`[source]` becomes `[input]`, `camera` becomes `device`, `video` becomes `file`,
and file input requires a `path`. Move `wled_address` to `output.address`, set
`output.type = "wled"`, and move `[leds]` to `[output.leds]`.

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

For file input, set `input.type = "file"`, replace `input.device` with
`input.path` set to an absolute path readable by `prysm`. That mode requires FFmpeg
9+ on the service's PATH; it is not installed by this camera-oriented package.
The service stops successfully at video EOF.

Packaging uses [cargo-deb's systemd integration](https://github.com/kornelski/cargo-deb/blob/main/systemd.md).


## Mobile control panel

The controller requires Linux/systemd and WLED's JSON API (including `lor`). It
stays available when capture is stopped. The package installs its separate unit
but leaves it disabled. Its polkit rule permits only start/stop of
`prysm-led.service`; the controller runs without camera permissions.

For panel ownership, disable automatic capture startup, configure WLED's boot
preset to Off, and edit the controller configuration. Set `wled_url` to WLED's
HTTP address, not the DDP port. `allowed_hosts = ["*"]` accepts any browser host
and is the shipped default. To restrict it, list exact browser hosts plus ports
(for example `allowed_hosts = ["192.168.1.20:8787"]`). Same-origin checks remain
enforced with the wildcard. The panel requires no login.

```sh
sudo systemctl disable --now prysm-led
sudo nano /etc/prysm/control.toml
sudo systemctl enable --now prysm-control
journalctl -u prysm-control -f
```

Open `http://YOUR_DEBIAN_IP:8787` on your phone. Anyone who can reach the panel
can control the lights; use a trusted LAN or a private VPN and do not forward
its port from the internet. The controller deliberately has no CORS access.
An existing `token_file` configuration entry is accepted but ignored after
upgrading; no token file is needed.

If the page is blank after changing `allowed_hosts` to `["*"]`, rebuild and
reinstall the updated binary: older binaries treat `"*"` as a literal host.
Then restart with `sudo systemctl restart prysm-control`.

The page provides Lights On/Off, a Prysm/WLED effects source choice remembered
while Off, an observed status, and a link to WLED's own effects editor. Off stops
capture and asks WLED to suppress live input and go dark. Controller startup
only observes existing state; it does not switch the lights. A WLED reboot has
its own configured boot behavior. External WLED changes are displayed without
being overwritten automatically. Only one DDP sender is supported.

Controller settings live in `/etc/prysm/control.toml` (a conffile); the selected
source is atomically stored in `/var/lib/prysm-control/source.json`. A missing
file defaults the selector to Prysm without starting capture. Corrupt saved
state stops controller startup and is reported in the journal.

Upgrades stop the controller; start it again with `sudo systemctl start
prysm-control` after checking configuration. Its account is created at install,
and systemd is reloaded on install/removal. Removing the package stops the
controller but does not alter lights, WLED effects, or the saved selector.

Before relying on Off, verify on the installed WLED firmware while video/DDP is
active: switch Off, back to Prysm, and then to effects. Check that Off is dark,
effects remain unchanged, and DDP resumes after override clears. Status reflects
software state; it cannot confirm wiring or physical LEDs. The transition
logic is covered by fake-backend tests; hardware verification is still required.

The React UI is bundled into the binary; the Pi needs no frontend runtime or
internet connection. To edit it, run `npm ci --prefix prysm/control-ui`, edit
`prysm/control-ui/app.jsx` or `style.css`, then run
`npm run build --prefix prysm/control-ui` before rebuilding the Rust binary.
Commit the generated `prysm/src/control.html` with its source changes.

Development checks: `cargo test -p prysm --bin prysm-control` and
`npm test --prefix prysm/control-ui` (Node 20.19+ is only needed for UI development;
the test renders the bundled page and checks that it matches the source).
A read-only check on 2026-10-12 confirmed the configured device at
`http://192.168.18.39/` runs WLED 16.0.1 on ESP32 Ethernet and reports the required
power/realtime/override fields. It was receiving full-strip DDP while `on` was
false, which is why Off confirmation also requires no realtime and override 2.
No light state was changed during that check; switching remains unverified.
