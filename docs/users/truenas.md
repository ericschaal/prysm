# TrueNAS

Requires Docker-based TrueNAS Apps.

## 1. Load the image

[Build `prysm:local`](docker.md#1-build-the-image) on the NAS, or on a computer
with the same CPU architecture. To transfer a build:

```sh
docker save -o prysm-image.tar prysm:local
```

Copy the file to the NAS, then run in the NAS shell:

```sh
docker load -i /path/to/prysm-image.tar
```

## 2. Create storage

Create a dataset such as `/mnt/tank/apps/prysm`. Give the app's numeric user/group
read, write, and traversal permission. The example uses `568:568` (`apps`).
Prysm must be able to create and replace files in the directory.

## 3. Install

In **Apps → Discover Apps → Install via YAML**, paste the
[Compose example](../../packaging/docker/compose.truenas.yaml). Change:

- The dataset path.
- `user` to the numeric user/group with dataset access.
- `192.168.1.20` in `ports` and `PRYSM_ALLOWED_HOSTS` to the NAS's LAN IP.
- `truenas.local:8787` to your browser hostname, or remove it.

Install and open `http://YOUR_NAS_IP:8787`. Keep it on a trusted LAN; Prysm has
no login. [TrueNAS screen reference](https://apps.truenas.com/managing-apps/installing-custom-apps/).

Connect the [webOS capturer](webos.md), or add the [USB mapping](docker.md#tv-and-usb-capture).
Then [set up WLED and Screen sync](configuration.md).

## Maintenance

Settings are in the dataset's `settings.json`. Snapshot the dataset before
replacing the image. Load the new build and recreate the app; Screen sync starts off.

If settings will not save, check the dataset permissions for the app's numeric
user/group. Change ports and device mappings in the app YAML, then redeploy.
