# Docker

Requires Docker on amd64 or arm64. The webOS capturer works with Docker Desktop;
USB capture needs a Linux host. For TrueNAS, use [this guide](truenas.md).

## 1. Download Compose

```sh
mkdir prysm && cd prysm
curl -fsSLO https://github.com/ericschaal/prysm/releases/latest/download/compose.yaml
```

## 2. Start Prysm

The [Compose file](../../compose.yaml) pins the released multi-platform image:

```sh
docker compose pull
docker compose up -d
```

Open `http://localhost:8787`. Settings stay in the `prysm-config` volume.

## Network and deployment options

For a phone or webOS capturer, edit `services.prysm` in `compose.yaml`.
Replace `192.168.1.20` with the Docker host's LAN IP:

```yaml
ports:
  - "192.168.1.20:8787:8787"
environment:
  PRYSM_ALLOWED_HOSTS: "192.168.1.20:8787"
```

Run `docker compose up -d`, then open `http://192.168.1.20:8787`.
Keep this on a trusted LAN; Prysm has no login.

Use WLED's IP address; `.local` names may not resolve in Docker.
Allow HTTP/WebSocket and UDP 4048 from the container to WLED. No inbound UDP
mapping is needed.

## TV and USB capture

- **LG TV:** [install the webOS capturer](webos.md) and enter Prysm's LAN URL.
- **USB card on Linux:** add this under `services.prysm`:

```yaml
devices:
  - /dev/video0:/dev/video0
group_add:
  - "44" # Replace with: stat -c '%g' /dev/video0
```

Run `docker compose up -d`. In Prysm, choose **USB capture device** and enter
`/dev/video0`.

Then [set up WLED and Screen sync](configuration.md).

## Storage and maintenance

The container uses user/group `10001:10001`. For a host directory instead of
the named volume, use the [TrueNAS bind-mount example](../../packaging/docker/compose.truenas.yaml)
and set its path and numeric user/group to ones with directory write access.
Mount the whole directory at `/config`, not just `settings.json`.

- Logs: `docker compose logs -f prysm`.
- Stop: `docker compose down`. Adding `-v` deletes settings.
- Upgrade: back up settings, update the image version in `compose.yaml`, then run:

```sh
docker compose pull
docker compose up -d
```

Screen sync starts off after recreation. Turn lights off in the panel before
stopping Prysm if you do not want WLED effects to resume.

Video-file input needs FFmpeg 9+, which the image does not include.
