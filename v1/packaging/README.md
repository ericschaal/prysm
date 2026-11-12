# Packaging prysm-headless

Two ways to run the v1 [headless host](../hosts/headless/src/main.rs) on Linux:
a Docker image and a Debian package with a systemd service. Both are built by
[`docker/Dockerfile`](docker/Dockerfile) from the repository root, for
`linux/amd64` and `linux/arm64` (Raspberry Pi OS 64-bit). They are separate from
the shipping product's [image](../../Dockerfile) and
[package](../../packaging/debian/README.md), and assume a machine without them.

Both offer the `device` (V4L2) and `remote` sources and the `wled` and `null`
outputs. The `file` source needs FFmpeg 9+ on `PATH`, which neither ships.

## Portal and settings

The portal listens on port **8787** on every interface: open
`http://<machine>:8787`. It has no password, so anyone who can reach the port
can control the lights; keep it on a trusted LAN. Browser pages from other
sites are refused unless allowed with `--allow-origin`
([access](../surfaces/web-portal/HTTP-CONTRACT.md#access)).

Settings are one JSON file ([format](../adapters/settings-file/README.md),
[example](../hosts/headless/settings.example.json)). A missing file means
"unconfigured": the host starts idle and the portal writes the file on the
first save. Hosts start idle; add `--start` to request lighting at launch.

| | Docker | Debian |
| --- | --- | --- |
| Settings | `/data/settings.json` (volume) | `/var/lib/prysm/settings.json` |
| Runs as | UID/GID 10001, plus `video` (GID 44) | system user `prysm`, plus `video` |
| Logs | `docker logs` | `journalctl -u prysm-headless` |
| Extra options | arguments after the image name | `systemctl edit prysm-headless` |

`RUST_LOG` filters log lines (default `warn,prysm=info`).

## Docker

Build (BuildKit/buildx required; `--platform linux/arm64` for a Pi):

```sh
docker buildx build -f v1/packaging/docker/Dockerfile -t prysm-headless:local --load .
v1/packaging/docker/smoke.sh prysm-headless:local   # optional check
```

Run with a USB capture device:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  --device /dev/video0 --group-add "$(getent group video | cut -d: -f3)" \
  prysm-headless:local
```

or `docker compose -f v1/packaging/docker/compose.yaml up -d`
([compose.yaml](docker/compose.yaml)). Notes:

- Arguments after the image name are appended to
  `prysm-headless /data/settings.json`, e.g. `--start` or
  `--allow-origin http://192.168.1.30:3000`.
- A bind mount instead of the `prysm-data` volume must be writable by UID
  10001 (`chown 10001:10001 DIR`).
- The `remote` source listens on the port in its settings (suggested 7380);
  publish it too (`-p 7380:7380`).
- A bridge network cannot resolve mDNS names such as `wled.local`; use the
  WLED's IP address or `--network host`.
- The image's `HEALTHCHECK` reads `GET /api/v1/state` on port 8787; it reports
  unhealthy with `--no-portal` or another `--listen` port.
- `docker stop` sends SIGTERM; the host idles the LEDs and exits. Allow 30 s.

Release images are published as `ghcr.io/ericschaal/prysm-headless:<version>`
by [v1-release.yml](../../.github/workflows/v1-release.yml) on a `v1.*` tag.

## Debian package

Build `prysm-headless_<version>_<arch>.deb` for the build machine's
architecture (any OS with Docker; add `--platform linux/arm64` for a Pi):

```sh
docker buildx build -f v1/packaging/docker/Dockerfile --target deb --output type=local,dest=dist .
```

[`debian/build-deb.sh BINARY OUT_DIR`](debian/build-deb.sh) does the
assembly with `dpkg-deb` and can run on any Debian/Ubuntu with `dpkg-dev`.
The binary is built on Debian 12 and needs systemd and glibc 2.34 or newer
(Debian 12+, Ubuntu 22.04+, Raspberry Pi OS bookworm, 64-bit).

Install and use:

```sh
sudo apt install ./prysm-headless_*.deb   # creates user prysm, enables and starts the service
systemctl status prysm-headless
journalctl -u prysm-headless -f
sudo systemctl edit prysm-headless        # e.g. ExecStart= then ExecStart=/usr/bin/prysm-headless /var/lib/prysm/settings.json --start
```

The package installs `/usr/bin/prysm-headless`, the
[unit](debian/prysm-headless.service) and a
[sysusers.d](debian/prysm-headless.sysusers) entry. The unit runs as `prysm`
with `video` for `/dev/video*`, keeps its state in `StateDirectory=prysm`,
restarts on failure and is sandboxed (read-only system, no capabilities,
only V4L2 devices, IPv4/IPv6/Unix sockets). `apt remove` stops and disables
the service; `apt purge` also deletes `/var/lib/prysm`. The `prysm` user is
kept.
