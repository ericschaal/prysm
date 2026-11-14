# TrueNAS

Runs the `prysm-headless` image as a TrueNAS SCALE custom app (Docker-based
Apps). A NAS usually has no capture card, so this setup is mostly used with
the [slim TV package](webos.md#slim-package-the-tv-sends-the-picture): the TV
sends the picture, the NAS works out the colours and drives WLED.

*Not yet verified on hardware:* this app definition has been checked for
syntax only, not installed on TrueNAS.

## 1. Create storage

Create a dataset, for example `/mnt/tank/apps/prysm`. Give the `apps` user and
group (568:568) read, write and traverse permission on it. Prysm must be able
to create and replace files there; it writes `settings.json` on the first save.

## 2. Install

1. Open **Apps → Discover Apps → Install via YAML**.
2. Paste [compose.truenas.yaml](../../packaging/docker/compose.truenas.yaml).
3. Change:
   - `/mnt/tank/apps/prysm` to your dataset.
   - `user`, if your dataset belongs to another user and group.
4. Install. Open `http://NAS_IP:8787`.

The file's `image` names the latest
[release](https://github.com/ericschaal/prysm/releases); to install another
one, change the tag after `prysm-headless:`. To update, paste the current file
again or change the tag and redeploy.

The example publishes port 8787 (the portal, which also receives the TV's
picture) and starts Prysm with `--allow-origin null`, which the slim TV app
needs. It does not use host networking.

[TrueNAS custom app reference](https://apps.truenas.com/managing-apps/installing-custom-apps/).

## 3. Connect the TV and lights

1. In the portal's setup wizard, choose **Capture-only TV (remote)**.
2. Enter WLED by its **IP address**. Names such as `wled.local` and
   **Find lights** do not work on the app's network.
3. Follow the [slim TV steps](webos.md#slim-package-the-tv-sends-the-picture),
   using `http://NAS_IP:8787` as the Prysm address.

For a USB capture card on the NAS, uncomment `devices` and `group_add` in the
YAML and set the group ID it describes.

Keep the portal on a trusted network; it has no password.

## Maintenance

- Settings: `settings.json` in the dataset. Snapshot the dataset before an
  upgrade.
- Upgrade: change the image version in the app's YAML and redeploy. The lights
  start off.
- Settings will not save: check the dataset permissions for the `user` in the
  YAML.
- Logs: the app's log view in TrueNAS.
