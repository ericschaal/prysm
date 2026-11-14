# Embedded calibration video — 2026-11-03

The first playback feasibility gate passed on the project RTD2875Q TV. A separate
Prysm probe app used bundled MP4 video in a full-screen HTML video element, with
all controls hidden during playback. No kernel, rootfs, TVService, system
partition, vendor binary, or WLED gamma setting was changed.

The TV reported MAIN connected to VDEC, 640×360 at 29.97 fps, file playback,
SDR, limited range, with the probe app as owner. The installed combined native
capture ran at 1280×720 using hardware H.264 without a reported capture error.
The existing receiver processed 731 frames during the 25-second observation;
144 decoded preview images included the expected gray/red/green/blue sequence.
The user separately confirmed that physical wall lights followed the sequence.
Preview frame ids advanced from 388613 to 389342 (the first sample may precede
new capture). Representative decoded spatial means were approximately
(84,82,92), (184,184,216), (251,1,1), (0,240,0), and (2,0,251).

These captured values are **not** a reference transfer function or optical
measurement. The gray shift and settling are reasons to evaluate picture-mode
behavior physically, not to add another HDMI HDR conversion. This initial test
used the probe application identity; the integrated main app and pairing need
separate validation.

The receiver's saved configuration was byte-for-byte equivalent before/after
JSON serialization. Stop was confirmed with pipeline idle, WLED off, realtime
off and live override 2. The temporary probe was removed using the ordinary
webOS application installer. No return-to-input command was sent.

## Assets and reproduction

Run `python3 scripts/calibration-patterns.py` from a checkout of the tag
`pre-1.0-consolidation` (the generator was removed afterwards), or add
`--check` to verify existing files. It requires FFmpeg with libx264 and ffprobe.
The current assets were generated with FFmpeg 9.0.2 and libx264, one encoder
thread, CRF 12, constrained baseline level 3.0, YUV420p, 640×360, 30 fps, a
one-second GOP and fast-start MP4. RGB code values are converted explicitly to
limited-range BT.709; matrix, primaries and transfer metadata are set to BT.709.
Every decoded pixel is checked within 3/255 of the source RGB patch. These are
SDR encoded-code patches, not emitted-light targets. There is no audio.

Each stable clip loops a two-second shade. Response uses two seconds each of
gray 64, gray 192, red, green and blue; colors uses red, green, blue, cyan,
magenta and yellow. Gray advancement is manual in the wizard. Loop behavior
and changes between files must remain under hardware validation.

The small [probe](probe/index.html) uses those same bundled assets. To package
it, stage its files with the TV app icon and `public/calibration/` directory,
then run `ares-package` on the staged directory. Probe lifecycle tests live in
`crates/web/prysm-webos/tests/calibration-probe.test.cjs`.

Raw local captures and state snapshots were retained under
`work/calibration-playback/`; they are not distributed. No screen content is
needed to regenerate the supplied patterns. Long-duration behavior, other TV
models, and usefulness after switching to PS5/Freebox picture modes remain
unmeasured.

## Integrated wizard qualification

The same production TV UI was subsequently exercised under a disposable app
identity, against an isolated receiver with a copied configuration. The user
confirmed phone pairing and live white-balance changes on the physical lights.
A bounded debugger/API run verified the full relay path, gray video playback,
complete correction bypass/restoration, manual shade selection, color Pause,
Save retaining the approved video, remote Back/Exit, and Stop leaving the
pipeline idle and lights off. Saved normal processing stayed independent of the
profile. The user's saved trial was retained locally, separately from automated
qualification profiles.

Hardware found two integration issues that now have regressions: webOS uses a
named `file://…-webos` app origin, and an intentional Pause can reject a pending
`play()` promise with `AbortError`. The relay accepts file origins while keeping
its file-only frame policy and normal API protections; intentional aborts no
longer end calibration. The final hardware run passed both original repros.

The TV sender's original configuration was restored byte-for-byte, the test app
and temporary IPKs removed, the debugger tunnel and test receiver stopped, and
the installed Prysm app reopened. Native status confirmed its original receiver
connected with no error. Neither installed native executable nor system
partition was modified. Final local results are in
`work/calibration-playback/final-integration.json`; they are not required to
build the feature. The main installed app and usual receiver were not upgraded.


## Revised static-color smoke test

The revised TV frontend was installed as temporary web-only app
`io.prysm.calibrationcheck` version 0.0.2, using a separately built host receiver
and a copy of the user's settings. All six selected colors stayed on their
looping video clip while capture kept advancing (21–23 preview frames per
sample window). Representative decoded center RGB and mean LED output RGB:

| Pattern | Decoded RGB | LED output RGB |
| --- | --- | --- |
| Red | 251, 2, 0 | 214, 1, 0 |
| Green | 0, 241, 0 | 0, 206, 0 |
| Blue | 1, 0, 250 | 0, 0, 214 |
| Cyan | 0, 242, 254 | 0, 207, 217 |
| Magenta | 248, 2, 253 | 214, 0, 216 |
| Yellow | 240, 237, 0 | 206, 202, 0 |

The user confirmed that the physical lights followed the static patterns.
The new QR route, white-balance video, color cycle, optional gray selection,
normal-settings review, save retaining video, TV Back cleanup, and Stop also
passed. A synthetic profile was saved only to the isolated test settings file.
The user's original settings/profile file remained hash-identical; the TV's
sender configuration was restored byte-for-byte. The temporary app and package
were removed, the receiver/debug tunnel stopped, and WLED was verified off.
No main app deployment or system partition modification occurred.

Local detailed results and decoded frames are retained under
`work/calibration-playback/smoke-static/`. This is functional validation, not
an optical calibration or a long-duration/picture-mode qualification.


## Footer overflow follow-up

The user reported a cropped footer on the LG app. The revised frontend measured
1138.375px tall in the actual 1920×1080 viewport; its footer ended at 1093.375px.
The additional calibration button made the left panel taller. Reducing only
vertical outer padding, overview padding, and spacing above the actions brought
the footer bottom to 1018.375px, leaving 61.625px below it. Text remains 30px at
the root and button sizes are unchanged. Browser layout regressions passed at
1920×1080 and 1280×720; the corrected stylesheet was also verified on the LG.

Only those three spacing edits were applied to the installed `io.prysm.capture`
stylesheet, with the original backed up locally under the smoke-test directory.
At that point, the full calibration frontend and receiver were not yet deployed
to the main setup.
No native binaries, sender settings, profiles, or system partitions were changed
by this layout fix.

## Full-stack deployment

The main receiver, embedded web UI and TV app/service 0.1.26 were subsequently
deployed together. The user's saved profile and existing production processing
settings were preserved. Pairing, reload/rejoin, static-color capture, cancellation
and Stop passed on the deployed stack; the lights were left off. See the
[deployment record](../deployment-history.md) for validation and rollback details.
