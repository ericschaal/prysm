# Set up your lights and use Prysm

## 1. Prepare WLED

Open WLED in a browser and check that it lights the whole strip. Then set:

| WLED page | Setting |
| --- | --- |
| **Config → LED Preferences** | The physical LED count; **Use Gamma for color** on. |
| **Config → Sync Interfaces** | DDP input allowed; **Disable realtime gamma correction** off. |

Prysm sends colours to WLED over UDP port 4048 (DDP) and switches WLED into
live mode over HTTP. Both must be allowed between Prysm and WLED.

## 2. Open the web portal

| Where Prysm runs | Portal address |
| --- | --- |
| This computer | `http://localhost:8787` |
| Another computer, Raspberry Pi, Docker or NAS | `http://COMPUTER_IP:8787`, for example `http://192.168.1.20:8787` |
| LG TV, full package | The address the Prysm app shows on the TV |

The portal works from a phone on the same network; no extra setting is
needed. When Prysm starts it prints the address in its log.

## 3. Run the setup wizard

The portal opens the wizard until a video source and lights are saved.
Nothing is saved until the last step.

1. **Video source.** Choose where the picture comes from:
   **Video capture device** ([capture card](quickstart.md)),
   **Capture-only TV (remote)** ([slim TV package](webos.md#slim-package-the-tv-sends-the-picture)),
   or **This TV** on the full TV package.
2. **Your lights.** Choose **WLED**. Select **Find lights** to list WLED
   devices on your network, or type the address: a name such as `wled.local`
   or an IP address such as `192.168.1.30`, with `:PORT` only if WLED's web
   page is not on port 80. Do not type `http://`.
   Enter the number of LEDs on each edge; use 0 for an edge without LEDs.
   Prysm takes 1 to 16384 LEDs in total (the most a WLED device drives;
   smaller chips such as the ESP8266 stop far earlier).
3. **Review and start.** Check the summary, then choose
   **Save and start screen sync**, or **Save only** to start later.

**Find lights** only finds WLED devices on the same network segment. In
Docker it needs host networking, so in a normal container type the IP address.

Wire the strip so that, seen from the front, it starts at the **bottom-left
corner** and runs **up the left edge, across the top, down the right edge and
back along the bottom** (clockwise). The wizard draws this layout with your
counts.

**No lights (test)** runs Prysm without LEDs, for trying a video source.

## 4. Daily use

Home has two cards.

- **The light card**: the power button, the two modes **Screen sync** (the
  lights follow the picture) and **WLED effects** (WLED runs its own effects),
  brightness, and your colour profiles. Choosing a mode while the lights are
  off turns them on in that mode. Problems show here in plain words, with the
  action that fixes them. **Open WLED** opens WLED's own page.
- **Your setup**: the path from the video source through Prysm to the lights,
  with what each step is and whether pictures are flowing. A step that failed
  is marked.

Good to know:

- Prysm starts with the lights off after every restart. Turn Screen sync on
  again, or start Prysm with `--start` (see your install guide).
- Stopping Prysm turns the lights off.
- When no picture arrives for about 3 seconds, WLED goes back to what it was
  showing before. Screen sync takes over again when the picture returns.
- Closing the browser leaves Prysm running.

## 5. Change settings

Open **Settings** (the icon in the header). The line under the title says
whether everything is saved: **All changes saved**, **Saving…** or **Not
saved**; a change that could not be saved also says so at its control, with
**Try again**.

<a id="processing"></a>

### Picture

Picture changes apply while you move the control; there is no Save button.

| Setting | Default | Effect |
| --- | --- | --- |
| Brightness | 100% | Overall LED brightness. Also on Home. |
| Remove black bars | On | Ignores letterbox and pillarbox bars. Takes a second or two to settle after the picture changes. |
| Colour profile | No correction | A saved colour correction; see [Calibrate colours](calibration.md). |
| Shadow lift (Fine-tuning) | 0% | Brightens dark colours; black stays off. |
| Edge depth (Fine-tuning) | 10% | How far into the picture each light looks. |
| Smoothing (Fine-tuning) | 0.1 s | Softens quick changes. 0 reacts at once. |
| Detail (Fine-tuning) | 128 | Higher follows small details more closely but works the computer harder. Leave it at 128 to start. |

Use [Live preview](preview.md) to check a change.

### Video source and lights

Choose **Video source** or **Lights** in Settings to change them on a page of
their own, with the same steps as the setup wizard. **Use this video source**
or **Use these lights** saves; **Cancel** leaves without saving. A problem with
either shows on its row in Settings.

Saving restarts Screen sync if it is on, and Prysm then checks the change: it
says when the picture arrives or the lights work, or that they don't. Until
they work, **Go back to the previous video source** (or lights) puts the old
one back. With Screen sync off nothing is tried; **Turn on screen sync to
try** does that.

If someone changed the same part in another browser, Prysm asks: **Use
theirs** shows their settings, **Keep mine** saves yours. **Run setup again**
reopens the wizard.

Settings also has the theme (System, Light, Dark; kept in this browser only)
and **About this Prysm**.

## Settings file

Prysm keeps all of the above in one JSON file:

| Installation | Settings file |
| --- | --- |
| Debian package | `/var/lib/prysm/settings.json` |
| Docker | `/data/settings.json` in the container's volume |
| TrueNAS | `settings.json` in the app's dataset |
| LG TV, full package | `/var/lib/io.prysm.tv/settings.json` on the TV |
| From source | The path you pass on the command line |

The portal creates the file on the first save. Back it up before upgrades.
Stop Prysm before editing it by hand. The format is described in the
[settings-file reference](../../v1/adapters/settings-file/README.md).

To check a file without starting the lights:

```sh
prysm-headless --check-settings /var/lib/prysm/settings.json
```

It prints `settings OK` or the field that is wrong. It does not contact the
capture device or WLED.

## Video files

Playing a video file instead of a live source is a developer feature. It needs
FFmpeg 9 or newer and does not work in the Debian package or the Docker image.
See [Develop Prysm](../developers/README.md#play-a-video-file).
