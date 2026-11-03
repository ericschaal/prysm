# Prysm on Debian arm64

The package installs one `prysm` executable with its browser panel and one
`prysm.service`. No Node runtime is needed. The service starts disabled.

## Install and run

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudo systemctl enable --now prysm
```

Open `http://localhost:8787` on the host. Complete Settings with the video source,
WLED HTTP URL and LED counts, save, select **Screen sync**, and turn the lights
on. WLED needs DDP enabled; packets go to the URL's host on UDP 4048. Capture
starts idle on every restart. Use `journalctl -u prysm -f` for errors.

For a headless host, run `sudo systemctl edit prysm` and add:

```ini
[Service]
Environment="PRYSM_LISTEN=0.0.0.0:8787"
Environment="PRYSM_ALLOWED_HOSTS=192.168.1.20:8787,prysm.example.lan:8787"
```

Replace the addresses with your actual browser addresses, restart the service,
and open `http://192.168.1.20:8787`. The panel has no login; use a trusted LAN or
private VPN. CLI/environment configure the process on every startup and are never browser settings.

The first successful browser save creates `/etc/prysm/settings.json`. The
parent directory is owned by `prysm` with mode `0700` and must remain writable
for atomic saves. No initial file is installed. Upgrades and removal preserve
settings; purge deletes them. One process owns each settings directory.

For the full installation and package-build guides, see the
[Prysm repository](https://github.com/ericschaal/prysm).
