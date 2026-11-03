# Run Prysm with Docker

The image includes Prysm, its web panel, the webOS receiver, and Linux V4L2
capture. Deploy on a 64-bit Linux host (amd64 or arm64). Docker Desktop can run
the TV receiver; USB capture needs the device inside the Docker host's Linux
kernel. Video-file playback needs FFmpeg 9+, which this image does not include.

For a TrueNAS Apps deployment, use the [TrueNAS guide](truenas.md).

The Compose examples use a published version tag. Until that release is available
from GHCR, [build the image locally](#update-build-and-verify), change `image` to
`prysm:local`, and omit `docker compose pull`.

## Start and configure

Save the repository's [Compose file](../../compose.yaml) in an empty directory,
then run:

```sh
docker compose pull
docker compose up -d
docker compose logs -f prysm
```

Open `http://localhost:8787`. The panel opens Settings when input or output is
missing. Choose your video source, enter the WLED HTTP address and LED counts,
and save. You can configure every application setting in the browser. Prysm
starts idle; select **Screen sync** and turn the lights on when ready.

The default named volume stores `/config/settings.json`. No host configuration
file or UID/GID shell exports are needed. The container runs as `10001:10001`
with a read-only root filesystem and a writable config volume. Do not use
`docker compose down -v` unless you intend to delete your settings.

WLED traffic leaves the container over HTTP/WebSocket and UDP 4048. No inbound
UDP mapping is needed. Use an IP address or ordinary DNS name for WLED;
`.local` multicast DNS usually does not resolve on Docker's bridge network.

## Network and deployment options

The image defaults to listening on `0.0.0.0:8787` inside the container. Compose
publishes the panel on the host's loopback address. To access it from another
computer or phone, publish the host's trusted LAN address and allow that exact
address. For example:

```yaml
ports:
  - "192.168.1.20:8787:8787"
  - "192.168.1.20:19400:19400"
environment:
  PRYSM_ALLOWED_HOSTS: "192.168.1.20:8787,prysm.example.lan:8787"
```

Recreate the container after changing its deployment settings, then open the
allowed LAN address. Both the panel and TV receiver have no login; publish them
only on a trusted LAN or private VPN.

Process configuration is read on every startup: **CLI arguments → environment →
explicit defaults**. It is never persisted or edited in the browser. The browser
edits only application settings in the mounted JSON file.

| Deployment setting | Purpose |
| --- | --- |
| `PRYSM_CONFIG` / `--config` | Settings file location; image default `/config/settings.json` |
| `PRYSM_LISTEN` / `--listen` | HTTP socket; default `0.0.0.0:8787` |
| `PRYSM_ALLOWED_HOSTS` / `--allowed-host` | Exact browser authorities; default loopback hosts on port 8787 |
| `PRYSM_RECEIVER_LISTEN` / `--receiver-listen` | TV receiver socket; default `0.0.0.0:19400` |
| `PRYSM_METRICS_LISTEN` / `--metrics-listen` | Optional metrics socket; disabled by default |
| `RUST_LOG` | Log filter; default `info` |

A published host port and the container's listen port must match your port
mapping. Changing Settings cannot create Docker port mappings or grant devices.

## Persistent host storage

To use a bind-mounted directory instead of the named volume, create a dedicated
config directory on the Docker host and give the container's numeric user read,
write and directory traversal permission. Mount the whole directory at `/config`:

```yaml
user: "1000:1000" # Use the numeric identity that has access to your directory.
volumes:
  - type: bind
    source: /srv/prysm
    target: /config
    bind:
      create_host_path: false
```

The directory may start empty. The first successful browser save creates the
JSON file as the selected user. Prysm replaces that file atomically, so mounting
only `settings.json` does not work. An existing malformed or unreadable settings
file is reported as an error and is never reset automatically. Back up the
whole directory before updates. Use one Prysm instance per settings directory.

## TV and USB capture

For TV capture, choose webOS in Settings and listen on `0.0.0.0:19400`.
Point the [TV sender](webos.md) at the Docker host's LAN IP and port 19400.

For Linux USB capture, choose Device in Settings with `/dev/video0` as the
container path. Add these keys under `services.prysm`:

```yaml
devices:
  - /dev/video0:/dev/video0
group_add:
  - "44" # Replace with: stat -c '%g' /dev/video0
```

The numeric group must match the host device's group. A stable
`/dev/v4l/by-id/...` path can be used on the left side of the device mapping.
Remove the TV port mapping if unused. Privileged mode and host networking are
unnecessary. See [device setup](quickstart.md) for supported modes and LED wiring.

## Update, build, and verify

Use an explicit image version in Compose. To update, back up the settings volume,
change the version, then run:

```sh
docker compose pull
docker compose up -d
```

`docker compose down` stops the service and preserves settings. Prysm starts idle
after recreation. The settings format is JSON; old TOML configurations are not
loaded or migrated.

For a local development image, clone the repository and run
`cargo xtask package docker` or `docker build -t prysm:local .`. To use it with
Compose, set its `image` to `prysm:local` and omit the pull step. A first build
compiles Rust dependencies; subsequent builds reuse Docker caches. Host Rust and
Node.js are unnecessary for a direct Docker build.

```sh
python3 packaging/docker/smoke.py prysm:local
```

The smoke test creates and cleans temporary containers, a named volume and a
bind directory. It checks first-run browser setup, embedded assets, environment
and CLI precedence, non-root saves, failed writes, persistence and shutdown.
CI runs it on native amd64 and arm64. See [image maintenance](../developers/packaging.md#docker).
