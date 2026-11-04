# Run Prysm on TrueNAS

This runs the Prysm application as a custom Docker-based TrueNAS app. You still
need a [capture device and WLED](README.md). The webOS capturer connects over the
LAN; a USB card needs a device mapping.

## 1. Put the image on the NAS

There is no official release yet. [Build the Docker image](docker.md#1-build-the-image)
for the NAS's CPU architecture. You can build it on the NAS if Docker, Git, and
outbound network access are available there.

If you build on another computer, export it:

```sh
docker save -o prysm-image.tar prysm:local
```

Copy the file to the NAS, then load it from the NAS shell:

```sh
docker load -i /path/to/prysm-image.tar
```

The image must be present in the NAS's Docker image store as `prysm:local`.

## 2. Prepare storage

Create a dedicated dataset, for example `/mnt/tank/apps/prysm`. Give the app's
numeric user/group read, write, and traversal access through the dataset's
permissions or ACL. The example uses `568:568`, the TrueNAS `apps` identity.

Prysm must be able to create and replace files in this directory. No initial
settings file is needed. Use one Prysm instance per dataset.

## 3. Install the app

In Apps, choose **Discover Apps → Install via YAML**. Copy the
[TrueNAS Compose example](../../packaging/docker/compose.truenas.yaml) and change:

- `/mnt/tank/apps/prysm` to your dataset path.
- `user` to the numeric user/group with dataset access.
- `192.168.1.20` to the NAS's LAN address in both `ports` and `PRYSM_ALLOWED_HOSTS`.
- Any additional allowed hostname to the one you will use, including its port.

The example uses the local image `prysm:local`. Install the app and open
`http://YOUR_NAS_IP:8787`. See the [TrueNAS custom-app guide](https://apps.truenas.com/managing-apps/installing-custom-apps/)
if your screens differ.

Follow [webOS capturer setup](webos.md), or add the [USB device and group mapping](docker.md#tv-and-usb-capture).
Then [complete browser setup](configuration.md). Success means Screen sync is
on and the LEDs follow the picture.

## Storage and maintenance

Browser settings are saved as `settings.json` in the dataset. Network addresses,
allowed hosts, and device mappings are set in the app YAML. Change those in
TrueNAS and redeploy the app.

Keep the app on your trusted LAN: the panel and TV connection have no login.
If saving fails, check app logs and dataset permissions for the configured
numeric user/group. The container does not change dataset ownership.

Snapshot or back up the dataset before replacing the image. Load the new build
on the NAS and recreate the app using that image; Prysm starts idle. Removing
the app is not a substitute for a dataset backup.
