# Run Prysm on TrueNAS

Use the same Prysm image as other Docker deployments. This guide targets
Docker-based TrueNAS Apps and its **Install via YAML** flow. For version-specific
screens, see [TrueNAS custom-app documentation](https://apps.truenas.com/managing-apps/installing-custom-apps/).

## Prepare storage

Create a dedicated dataset, for example `/mnt/tank/apps/prysm`. Give the numeric
user that will run Prysm read, write, and traversal access through the dataset's
permissions or ACL. The example uses `568:568`, TrueNAS's `apps` identity; use a
different numeric user/group if that is how you manage your applications.

Atomic settings saves require directory write permission, including creation and
replacement of files. Making just an existing settings file writable is not
enough. Keep configuration separate from shared media datasets and use one
Prysm instance per dataset. No initial file is needed.

## Install the app

Open Apps, Discover Apps, then **Install via YAML**. Copy the
[TrueNAS Compose example](../../packaging/docker/compose.truenas.yaml) into the
editor and change:

- The image version to the release you intend to run.
- `/mnt/tank/apps/prysm` to your dataset path.
- `user` to the numeric identity with dataset access.
- `192.168.1.20` to the NAS's trusted LAN address in the HTTP port mapping.
- `PRYSM_ALLOWED_HOSTS` to the actual IP address or DNS names, including port,
  that browsers will use.

Install the app and open `http://YOUR_NAS:8787`. Complete Settings: choose a video
input, WLED HTTP address, LED counts, and capture mode. Saving creates
`settings.json` in the dataset. Then select **Screen sync** and turn the lights
on. An unavailable WLED controller does not prevent saving valid settings;
connection errors remain visible in the panel.

For webOS, enter the NAS panel URL in the TV app. For USB, add the device and supplementary
group mapping described in the [Docker guide](docker.md#tv-and-usb-capture).

## Manage settings and updates

Application settings belong in the browser and are saved in the dataset.
Process options belong in the TrueNAS app configuration: listening sockets,
allowed browser hosts, logging and settings-file location. These are read on every
startup and never saved to JSON. CLI arguments take priority over environment
variables, then explicit defaults apply. Change these options in TrueNAS and
recreate the app; keep port mappings consistent with the container's bindings.
The image and runtime are the same as any other Docker deployment.

Snapshot or back up the config dataset before updating. Change the image version
in the app YAML and redeploy; saved settings remain in the dataset, and capture
starts idle. Removing the app does not replace a dataset backup. Both the panel
and receiver are unauthenticated; keep the published ports on your trusted LAN.

If saving fails, check the app logs and dataset permissions for the app's numeric
UID/GID. The container does not run as root or rewrite dataset ownership. An
empty bind mount works; an invalid existing JSON file requires manual repair,
not an automatic reset.
