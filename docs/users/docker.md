# Docker

The `prysm-headless` image runs on Linux amd64 and arm64. For TrueNAS, use
[this guide](truenas.md).

*Not yet verified on hardware:* the image is smoke-tested in CI without a
capture card or WLED.

## 1. Start Prysm

Pick a version from a `v1.*` [release](https://github.com/ericschaal/prysm/releases)
and use it in place of `VERSION`:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  ghcr.io/ericschaal/prysm-headless:VERSION
```

Open `http://localhost:8787`, or `http://DOCKER_HOST_IP:8787` from a phone.
Keep it on a trusted network; the portal has no password.

For Compose, start from the example
[compose.yaml](../../v1/packaging/docker/compose.yaml): set `image` to
`ghcr.io/ericschaal/prysm-headless:VERSION`, then run `docker compose up -d`.
To build the image yourself, see [packaging](../developers/packaging.md#docker).

Then [set up your lights](configuration.md). Type WLED's IP address: a
normal container cannot resolve names such as `wled.local`, and
**Find lights** does not work without host networking.

## Use a capture card

On a Linux host, map the device and its group into the container:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  --device /dev/video0 --group-add "$(getent group video | cut -d: -f3)" \
  ghcr.io/ericschaal/prysm-headless:VERSION
```

In Prysm, choose **Video capture device** and enter `/dev/video0`. See
[capture card](quickstart.md).

## Receive the picture from a TV (slim package)

Publish the receiver port and allow the TV app to control this Prysm:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -p 7380:7380 -v prysm-data:/data \
  ghcr.io/ericschaal/prysm-headless:VERSION --allow-origin null
```

Arguments after the image name are passed to Prysm. Then follow the
[slim TV steps](webos.md#slim-package-the-tv-sends-the-picture).

## Settings and maintenance

- Settings: `/data/settings.json` in the `prysm-data` volume. A host
  directory used instead must be writable by user ID 10001
  (`sudo chown 10001:10001 DIR`). Mount the whole directory, not the file.
- Logs: `docker logs -f prysm`.
- Start Screen sync with the container: add `--start` after the image name.
- Stop: `docker stop prysm`. Prysm turns the lights off and exits.
- Upgrade: back up the settings, remove the container, and start it again with
  the new version. The lights start off.

The image has no FFmpeg, so the video-file source does not work in it.
