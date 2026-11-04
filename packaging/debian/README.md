# Prysm on Debian arm64

The package includes the browser panel and a disabled `prysm.service`.

```sh
sudo apt install ./prysm-led_*_arm64.deb
sudo systemctl enable --now prysm
```

Open `http://localhost:8787`. Complete **Video source → Your lights → Review &
finish**, choose **Finish setup**, then **Screen sync** and turn on lights.
Enable WLED DDP input; Prysm sends to its host on UDP 4048. Every restart stays idle.

For access from a phone, run `sudo systemctl edit prysm` and add your host address:

```ini
[Service]
Environment="PRYSM_ALLOWED_HOSTS=192.168.1.20:8787"
```

Run `sudo systemctl restart prysm`; open `http://192.168.1.20:8787`.
No login: use a trusted LAN/private VPN. Errors: `journalctl -u prysm -f`.

Settings live in `/etc/prysm/settings.json`, created by the first browser save.
Keep its parent directory writable by `prysm` for atomic saves. One process owns
that directory. Package removal preserves settings; purge deletes them.

[Full guides and package builds](https://github.com/ericschaal/prysm)
