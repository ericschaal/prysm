# Upgrade to Prysm 1.0.0

This page lists what to do when you upgrade to 1.0.0. Read the section for
the version you run now:

| You run | Names you will recognise | Read |
| --- | --- | --- |
| **0.2.0** | Debian package `prysm-led`, service `prysm`, image `ghcr.io/ericschaal/prysm`, TV app **Prysm Capture** (`io.prysm.capture`) | [From 0.2.0](#from-020) |
| **1.0.0-rc.1** | Debian package `prysm-headless` 1.0.0-rc.1, image tag `1.0.0-rc.1`, TV app `io.prysm.tv` | [From 1.0.0-rc.1](#from-100-rc1) |
| Your own scripts or apps that use the HTTP API, the TV service, `/metrics` or `prysm-tui` | | [API and integration changes](#api-and-integration-changes) |

Upgrade the TV and the computer together when you use the slim TV package:
a 1.0 computer and an older TV app cannot send pictures to each other.

> **Not yet verified on hardware.** These steps follow the packages as
> built and tested in CI. They have not yet been tried on a Raspberry Pi with
> a capture card, on TrueNAS or on a TV.

## From 0.2.0

1.0 is a new implementation. It does not read the 0.2.0 settings
(`/etc/prysm/settings.json`, or `/config/settings.json` in Docker): their
content is different as a whole. After the upgrade the portal opens the
setup wizard, and you set up the video source, the lights and any
calibrations again. Keep a copy of the old file to look up your LED counts
and WLED address.

Also gone in 1.0:

- `PRYSM_ALLOWED_HOSTS`. The portal answers on every address of the
  computer, port 8787, so a phone works without any setting. Keep it on a
  trusted network; the portal has no password.
- `PRYSM_CONFIG` and every other `PRYSM_*` variable. Options are
  command-line arguments.

### Debian and Raspberry Pi

The package `prysm-led` is replaced by `prysm-headless`, and the service
`prysm` by `prysm-headless`. The two packages cannot be installed together.

Before upgrading:

1. Keep a copy of the old settings:

   ```sh
   sudo cp /etc/prysm/settings.json ~/prysm-0.2-settings.json
   ```

2. If you added settings with `sudo systemctl edit prysm` (for example
   `PRYSM_ALLOWED_HOSTS`), delete them: `sudo systemctl revert prysm`.
3. Stop and remove the old package:

   ```sh
   sudo systemctl disable --now prysm
   sudo apt remove prysm-led
   ```

   `remove` keeps `/etc/prysm`; `sudo apt purge prysm-led` deletes it.

Upgrade:

4. Install 1.0 as in [Debian package](debian.md#1-install):
   `sudo apt install ./prysm-headless_*.deb`. This enables and starts the
   `prysm-headless` service.

After upgrading:

5. Open the portal and run the [setup wizard](configuration.md). Settings
   now live in `/var/lib/prysm/settings.json`.
6. Options such as `--start` go in `/etc/default/prysm-headless`
   ([manage the service](debian.md#manage-the-service)).
7. The service now runs under a temporary user. The `prysm` account that
   0.2.0 created is no longer used.

### Docker

The image `ghcr.io/ericschaal/prysm` is replaced by
`ghcr.io/ericschaal/prysm-headless`. Its settings volume is mounted at
`/data`, not `/config`.

1. Stop the 0.2.0 container. With the 0.2.0 Compose file, run
   `docker compose down` in its directory (without `-v`, which would delete
   the old volume).
2. Start 1.0 as in [Docker](docker.md#1-start-prysm), with a new volume at
   `/data`. Choose a tag: `1.0.0`, `1` or `latest`.
3. Options such as `--allow-origin null` or `--start` go after the image
   name, not in `environment`.
4. Run the [setup wizard](configuration.md).
5. When 1.0 works, you can delete the old volume. `docker volume ls` shows
   its name, which ends in `prysm-config`.

### TrueNAS

1. Install the 1.0 app from the current
   [compose.truenas.yaml](../../packaging/docker/compose.truenas.yaml), as in
   [TrueNAS](truenas.md). It mounts the dataset at `/data`, not `/config`, and
   sets its options in `command`; it has no `PRYSM_ALLOWED_HOSTS`.
2. Use an empty dataset, or move the 0.2.0 `settings.json` out of the old
   one first.
3. Run the [setup wizard](configuration.md).

### LG TV

0.2.0's **Prysm Capture** did what the 1.0 slim package does: the TV sends
the picture to Prysm on a computer. 1.0 also has a full package that needs
no computer ([choose a package](webos.md)).

1. Upgrade Prysm on the computer first, as above.
2. Remove **Prysm Capture** (`io.prysm.capture`) as
   [LG TV](webos.md#before-you-start) describes: remove its service setup in
   the app, then uninstall it. 1.0 does not detect or migrate it.
3. Install 1.0 with the install helper, as in [LG TV](webos.md#1-install).
   `ares-install` does not work on webOS 10.
4. For the slim package, start Prysm on the computer with
   `--allow-origin null`, then follow the
   [slim steps](webos.md#slim-package-the-tv-sends-the-picture). The old
   **Prysm instance URL** is now **Prysm address**.

## From 1.0.0-rc.1

### Debian, Docker and TrueNAS: the settings file

What changed:

- **The settings file starts with a format number**, `"format": 1`. A
  1.0.0-rc.1 file has none. 1.0 does not fail on such a file: it starts
  without settings, the light card says why, and the file is left as it is.
  Add the number and the file works again. If you set Prysm up again in the
  portal instead, the first save keeps the old file beside the new one as
  `settings.json.unreadable-` and six random characters.
- **Values have final limits.** 1.0 refuses `smoothing_seconds` above 2,
  `fps` of a capture card or video file above 120, and `sample_density`
  (Detail) above 512. The portal's controls never went past these, so only a
  hand-edited file is affected. Such a file still loads, but the light card
  names the field, and Screen sync does not start until it is fixed.
- **Detail samples captures larger than 640x360 differently.** At the same
  Detail value, such a capture gets fewer samples than in rc.1, the same as
  640x360 gets. Check the picture after upgrading, and raise Detail if the
  lights follow small details less well.

#### Debian

Before upgrading:

1. Back up the settings:
   `sudo cp /var/lib/prysm/settings.json ~/prysm-settings.json`.
2. Run `systemctl cat prysm-headless`. If it shows a drop-in that replaces
   `ExecStart` (added with `systemctl edit`), note the options on its
   `ExecStart` line.

Upgrade:

3. `sudo apt install ./prysm-headless_*.deb`. The service restarts with 1.0.

After upgrading (the 1.0 check comes with the package, so it runs now):

4. Stop the service: `sudo systemctl stop prysm-headless`.
5. Edit the file with `sudo nano /var/lib/prysm/settings.json`: add
   `"format": 1,` after the first `{`, and bring any value above the limits
   back inside them.
6. Check it:

   ```sh
   sudo prysm-headless --check-settings /var/lib/prysm/settings.json
   ```

   It prints `settings OK`, or what is wrong and exits 1.
7. Options now go in `/etc/default/prysm-headless`. If step 2 found a
   drop-in, put its options in `OPTIONS=`, then delete the drop-in with
   `sudo systemctl revert prysm-headless`. A drop-in that replaces
   `ExecStart` keeps overriding the package's command, the options file
   included ([manage the service](debian.md#manage-the-service)).
8. Start the service: `sudo systemctl start prysm-headless`.

#### Docker

You can check the file with the new image before you replace the container.
The commands use the volume `prysm-data` from [Docker](docker.md); use your
own volume name or host directory.

Before upgrading:

1. Stop the container: `docker stop prysm`.
2. Edit `settings.json` in the volume: add `"format": 1,` after the first
   `{`, and bring any value above the limits back inside them. On a Linux
   host, a named volume is in the directory
   `docker volume inspect prysm-data --format '{{ .Mountpoint }}'` prints,
   readable by root only. A host directory stays owned by user ID 10001.
3. Check the file with the 1.0 image:

   ```sh
   docker run --rm -v prysm-data:/data \
     ghcr.io/ericschaal/prysm-headless:1.0.0 --check-settings
   ```

   It prints `settings OK`, or what is wrong and exits 1.

Upgrade:

4. Remove the container (`docker rm prysm`) and start it again with the new
   tag, as in [Docker](docker.md#1-start-prysm), with the same volume and
   options.

Image tags: `1.0.0` never moves, `1` follows the newest stable 1.x.y
release, and `latest` the newest stable release. A pre-release, such as
`1.0.0-rc.1`, has only its own tag, so an rc.1 tag stays on rc.1.

#### TrueNAS

1. Stop the app.
2. Edit `settings.json` in the dataset from the TrueNAS shell: add
   `"format": 1,` after the first `{`, and bring any value above the limits
   back inside them.
3. Change the image tag after `prysm-headless:` to `1.0.0`, or paste the
   current [compose.truenas.yaml](../../packaging/docker/compose.truenas.yaml)
   again, and redeploy ([TrueNAS](truenas.md#maintenance)).
4. If the light card says the settings file cannot be used, read why there,
   fix the file and restart the app.

<a id="tv-from-rc1"></a>

### LG TV

1.0 is a new app, `io.github.ericschaal.prysm`; the release candidate was
`io.prysm.tv`. Installing 1.0 does not replace it.

1. Remove the release candidate and install 1.0 as
   [LG TV](webos.md#1-install) describes under **Updating from a 1.0.0
   release candidate**.
2. Set up again. 1.0 does not read the release candidate's files in
   `/var/lib/io.prysm.tv/`, so there is no settings file to check:
   - full package: run the [setup wizard](configuration.md) in the TV's
     portal;
   - slim package: enter the **Prysm address** in the TV app again.
3. Slim package: upgrade the computer to 1.0 as well. 1.0 sends the picture
   with capture stream version 6 (`prysm-capture.v6`); rc.1 used version 5.
   A 1.0 TV app with an older computer says **Update Prysm or TV app**.
   The computer still needs `--allow-origin null`.

### Terminal UI (`prysm-tui`)

The keys are `s` (Screen sync), `e` (effects) and `o` (off); `p` and `n` are
gone. The commands are `on` or `screen_sync`, `effects` and `off`;
`processed` and `native` are gone.

## API and integration changes

For your own scripts and apps; the portal and the TV app that ship with 1.0
already use the new shapes. These are the changes from 1.0.0-rc.1. The
0.2.0 API is not described here: write a client for 1.0 against the
[HTTP contract](../../crates/surfaces/web-portal/HTTP-CONTRACT.md).

### HTTP API (`/api/v1`)

| Change | rc.1 | 1.0.0 |
| --- | --- | --- |
| `target`, also in `PUT /api/v1/target` | `off`, `processed`, `device_native` | `off`, `screen_sync`, `effects` |
| `session.state` | `stopped`, `running`, `native`, `faulted` | `stopped`, `screen_sync`, `effects`, `faulted` |
| A faulted session | `{ "state": "faulted", "phase": … }` | `{ "state": "faulted", "message": … }`; no phase |
| The lighting failure | `State.failure`, `null` when none | `session.failure`, only on `stopped` and `effects`, tagged by `kind`: `invalid { message, field? }`, `adapter { message, section? }`, `internal { message }` |
| The lights' own reports | `output_issue`, `output_brightness`, `device_led_count` | `device.issue`, `device.brightness_percent`, `device.led_count` |
| The Prysm version | — | `version` |
| An absent value | sometimes `null` | omitted |
| Calibration patterns | `gray-192`, `blue-pair`, `red-pair`, `green-pair`, `review-grid` | `gray_192`, `blue_pair`, `red_pair`, `green_pair`, `review_grid` |
| A pattern while not in screen sync | `409 not_processed` | `409 needs_screen_sync` |
| A wrong value in a settings, `processing` or capture body | `400 bad_request`, or `422 invalid` with or without a `field`, depending on the mistake | always `422 invalid` with a `field` path, such as `processing.style.kind` or `format` |
| A body not received within 5 s | `408 bad_request` | `408 request_timeout` |
| A capture stream sender of another version | `426 bad_request` | `426 upgrade_required` |
| A command that took over 15 s | `504 timeout` | removed; the portal sets no deadline of its own |
| Failure bodies | one shape, `{ kind, message }` with optional `field`, `section` and `current` | tagged by `kind`: `invalid { message, field? }`, `stale_revision { message, current }`, `adapter { message, section? }`, others `{ kind, message }` |
| `/metrics` labels `prysm_target`, `prysm_session_state` | the old values | the new values above |

Since 1.0, `400 bad_request` is only for a body that is not JSON or a
problem outside the settings it carries, such as an unknown top-level field.

Values that Prysm sends are open: a later 1.x may add one. Accept a value
you do not know and read it as the contract says for that field, never as
off or stopped ([contract](../../crates/surfaces/web-portal/HTTP-CONTRACT.md)).
Values you send stay closed: Prysm refuses one it does not know.

On the full TV package, the portal now takes only pages with no origin
(`Origin: null`, as the TV app sends) besides its own; rc.1 took any origin.
A page from another site gets `403 forbidden`.

### TV service (Luna)

The service is `io.github.ericschaal.prysm.service` (rc.1:
`io.prysm.tv.service`). Its API version is 4 (rc.1: 1). See the
[Luna API](../developers/webos.md#luna-api).

- `getState` carries only the installed package's fields, tagged by
  `flavor`. Full: `target` and a `worker` with `capturing`, `fps` and
  `portal`. Slim: `enabled` and a `worker` with `capturing` and its `link`,
  one object tagged by `state` that replaces rc.1's `encoding` and `paused`.
  The top-level `lighting`, `capturing`, `fps`, `portal`, `encoding` and
  `paused` are gone.
- Full: `lighting` is `target`, with `off`, `screen_sync` or `effects`.
  `setTarget { target }` replaces `setEnabled`. Slim keeps
  `setEnabled { enabled }`. Each package refuses the other's method with
  `errorKind` `wrong_package`.
- The preference `instance` is `prysm_address`.
- An absent value is omitted, never `null`.
- A failed call replies `{ "returnValue": false, "errorKind": …, "errorText": … }`.
- New `failure.kind` values: `replaced` (slim: another TV took over the
  stream), `receiver_failed` (slim: Prysm's receiving source failed with
  this TV's stream) and `preferences` (the TV's saved preferences could not
  be read).

### Exit codes

`prysm-headless` and `prysm-tui` exit with 0 after a clean stop, 1 on
failure and 2 for bad arguments, as in rc.1. New in 1.0: 101 when a panic
ended the host and 130 when a second SIGINT or SIGTERM made it exit at once
([hosts](../developers/workspace.md#hosts)). A settings file Prysm cannot
use no longer exits 1: Prysm starts without settings. `--check-settings`
still exits 0 when the file is valid and 1 otherwise.
