# Prysm on Debian arm64

The package installs one `prysm` executable (web panel, REST API and capture pipeline)
and one `prysm.service`. No Node runtime is needed. The service starts disabled.

## Install and run

Install the downloaded package and configure your input, WLED HTTP URL and LED counts:

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudoedit /etc/prysm/prysm.toml
sudo -u prysm prysm --config /etc/prysm/prysm.toml check
sudo systemctl enable --now prysm
```

Run from a folder containing only the package you intend to install. Open
`http://localhost:8787` on the host, select **Screen sync**, and turn the lights on.
Prysm always starts idle. Enable DDP in WLED; packets go to the URL's host on UDP 4048.
Use `journalctl -u prysm -f` for errors.

For phone access set `[web].listen = "0.0.0.0:8787"` and add your actual
`hostname:8787` or `IP:8787` to `[web].allowed_hosts`, then restart the service.
The panel has no login; use a trusted LAN or private VPN.

Configuration is `/etc/prysm/prysm.toml`; Prysm needs write access to its parent
directory for atomic saves. Upgrades preserve it. Removal preserves it; purge deletes it.
A service restart never restores capture automatically.

Full installation, upgrades, legacy migration and package-build guides are in
the [Prysm repository](https://github.com/ericschaal/prysm), under **Run Prysm**
and **Develop Prysm**. For an older multi-service installation, read the migration
instructions and back up settings before purging anything.
