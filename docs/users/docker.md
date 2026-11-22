# Docker

The `prysm-headless` image runs on Linux amd64 and arm64. For TrueNAS, use
[this guide](truenas.md).

*Not yet verified on hardware:* the image is smoke-tested in CI without a
capture card or WLED.

## 1. Start Prysm

Pick an image tag and use it in place of `VERSION`:

- `1.2.3`, a version from a [release](https://github.com/ericschaal/prysm/releases):
  always that image. Pre-releases (`1.2.3-rc.1`) have only this tag.
- `1`: the newest stable 1.x.y release. Within one major version an update
  keeps reading your settings
  ([what the version promises](../developers/releasing.md#what-the-version-promises)).
- `latest`: the newest stable release, which may be a new major version.

The tags `VERSION-amd64` and `VERSION-arm64` are not part of this promise: the
other tags hold both architectures.

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
  ghcr.io/ericschaal/prysm-headless:VERSION
```

Open `http://localhost:8787`, or `http://DOCKER_HOST_IP:8787` from a phone.
Keep it on a trusted network; the portal has no password.

For Compose, start from the example
[compose.yaml](../../packaging/docker/compose.yaml): set `image` to
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

In Prysm, choose **Capture card or camera** and tick the card, or enter
`/dev/video0` under **Advanced**. See [capture card](quickstart.md).

## Receive the picture from a TV (slim package)

The TV sends its picture to the portal's port; allow the TV app to control
this Prysm:

```sh
docker run -d --name prysm --restart unless-stopped --stop-timeout 30 \
  -p 8787:8787 -v prysm-data:/data \
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
  the new version (with `1` or `latest`, run `docker pull` first). The lights
  start off.

The image has no FFmpeg, so the video-file source does not work in it.
