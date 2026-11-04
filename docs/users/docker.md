# Run Prysm with Docker

The image runs the Prysm application with its browser panel, LG TV receiver,
and Linux V4L2 capture. Use a 64-bit host (amd64 or arm64). Docker Desktop works
with the webOS capturer; USB capture requires access to a Linux host's video device.
For TrueNAS, use the [TrueNAS guide](truenas.md).

## 1. Build the image

There is no official release yet. From a checkout of this repository, build:

```sh
git clone https://github.com/ericschaal/prysm.git
cd prysm
docker build -t prysm:local .
```

Docker builds Prysm and the browser panel; you do not need Rust or Node.js on
the host. The image does not include FFmpeg, so video-file input needs a custom image.

## 2. Start Prysm

From the repository directory, use the included [Compose file](../../compose.yaml):

```sh
docker compose up -d
docker compose logs -f prysm
```

Open `http://localhost:8787`. The named volume keeps settings at
`/config/settings.json`. No initial settings file is needed. The container runs
as `10001:10001` with a read-only root filesystem and a writable config volume.

For access from a phone or the webOS capturer, configure the LAN address below
before continuing.

## Network and deployment options

Under `services.prysm` in Compose, replace the loopback port mapping and add
an allowed address. Use your Docker host's LAN IP instead of `192.168.1.20`:

```yaml
ports:
  - "192.168.1.20:8787:8787"
environment:
  PRYSM_ALLOWED_HOSTS: "192.168.1.20:8787"
```

Apply the change with `docker compose up -d`, then open
`http://192.168.1.20:8787`. Add any other browser hostname with its port to
`PRYSM_ALLOWED_HOSTS`, separated by commas. The panel and TV connection have no
login; publish them only on a trusted LAN or private VPN.

WLED must be reachable from the container over HTTP/WebSocket and UDP 4048.
No inbound UDP mapping is needed. Use WLED's IP address or ordinary DNS name;
`.local` names usually do not resolve on Docker's bridge network.

## TV and USB capture

For an LG TV, follow [webOS capturer setup](webos.md) and use the panel's LAN address
in the TV app. No extra capture port is needed.

For a USB capture card on Linux, add these under `services.prysm`:

```yaml
devices:
  - /dev/video0:/dev/video0
group_add:
  - "44" # Replace with the output of: stat -c '%g' /dev/video0
```

A stable `/dev/v4l/by-id/...` path can replace the left side of the device mapping.
Run `docker compose up -d` after editing. In Prysm, select **USB capture device**
and use `/dev/video0`, the path inside the container.
See [capture card setup](quickstart.md) for capture modes.

For either capture device, [complete browser setup](configuration.md) to connect
WLED and start Screen sync. Success means the LEDs follow the picture.

## Persistent host storage

The default named volume is sufficient for most installations. To store settings
in a host directory, create a dedicated directory such as `/srv/prysm` and give
the container user read, write, and directory traversal access. Replace the
service's volume mapping with:

```yaml
user: "1000:1000" # Use the numeric user/group with access to the directory.
volumes:
  - type: bind
    source: /srv/prysm
    target: /config
    bind:
      create_host_path: false
```

Mount the whole directory, not just `settings.json`: saving replaces the file.
The directory can start empty. Use one Prysm instance per settings directory.

## Stop or rebuild

`docker compose down` stops Prysm and preserves settings. Adding `-v` deletes
the named volume and its settings. Stopping Prysm releases live color control;
use the panel's power control first if you want the lights off.

To run a different checkout, back up the settings volume, rebuild the image,
and recreate the container:

```sh
docker build -t prysm:local .
docker compose up -d --force-recreate
```

Prysm starts idle after recreation. For image checks and build details, see
[developer packaging](../developers/packaging.md#docker).
