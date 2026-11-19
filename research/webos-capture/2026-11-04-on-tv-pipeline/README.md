# Resume the on-TV Prysm experiment

Checkpoint from November 4, 2026. The rooted LG TV captured 1280×720 frames, processed them with the active lighting profile and sent DDP directly to WLED. The final build sustained 29.97 FPS on the brightness clip and 29.86 FPS on the moving clip. An 18-frame replay matched the original pipeline byte for byte at the converted RGB and DDP boundaries. The installed service and foreground app were restored after testing. `apps/prysm-webos-capture/local/README.md` (in the tag `pre-1.0-consolidation`)

## Starting points

| Commit | What you can reproduce |
| --- | --- |
| `3607ce8` | First on-TV native pipeline and three-resolution trial |
| `23dac94` | Parallel projection, roughly 30 FPS at 640×360, 16.95 FPS at 1280×720 |
| `bd38ac0` | 1280×720 baseline profile |
| `295874f` | Parallel grid, exact NEON conversion and paired projection at 1280×720 |

The exact final executable and the two video inputs are included. From this directory, run `shasum -a 256 -c SHA256SUMS` to verify them. The local webOS build script regenerates the executable with the ARMv7 target and relocated SDK. The saved ELF is specific to the tested webOS ARM/NEON environment. [Executable](bin/prysm-webos-local) · [Fixtures](fixtures/) · [Checksums](SHA256SUMS)

The measurement files are grouped by trial. The JSON rows hold cumulative frame counts and elapsed milliseconds. Device addresses in the WLED results are represented by `wled_source_was_tv`. The settings fixture preserves the tested calibration, LED counts and processing parameters with an anonymous profile and a loopback output address. [Initial trial](evidence/initial/) · [640×360 profile](evidence/640-profile/) · [720p baseline](evidence/720-profile/) · [720p optimization](evidence/720-grid/)

## Repeating a measurement

From this directory, rerun the archived final stage analysis:

```sh
python3 lab/summarize.py evidence/720-grid/timed-pair.log
```

For a live trial, set `PRYSM_TV_SSH` to the root SSH destination, `PRYSM_RECEIVER_URL` to the existing receiver’s HTTP base URL and `PRYSM_WLED_URL` to the controller’s HTTP base URL. The harness needs Python 3, Node with built-in WebSocket support, the installed Prysm TV app and its developer inspection endpoint. It snapshots the current settings, pauses the installed capture service, plays the supplied clip and stages a path-scoped Luna role and executable. It restores the foreground and service and prints a temporary results directory.

```sh
python3 lab/run.py 90 bin/prysm-webos-local fixtures/moving.mp4
```

The live runner uses the current receiver settings. Check the processing and layout against the saved fixture for a comparable result. The packet observer runs on the TV during playback and takes the WLED IPv4 address as its argument. Its filter expects the recorded 162-LED layout. [Packet observer](lab/wire.py)

Two optional patches reproduce the diagnostic builds. Apply each separately in a disposable checkout, starting from the repository root. The profile patch adds the monotonic stage timers used by the summarizer. The replay patch adds an offline execution path to the local binary. Build the replay binary once at `23dac94` and once at `295874f`. Run each on the TV with the included replay settings and `PRYSM_REPLAY_DIR` set to a separate existing directory. Compare the generated `rgb` and `ddp` files with `cmp`; these are the reference digests. [Profile patch](lab/profile.patch) · [Replay patch](lab/replay.patch) · [Replay results](evidence/720-grid/replay-pair.txt)

## Where to resume

The longest run was 90 seconds. A longer soak would establish whether thermal throttling changes the frame rate or packet-gap distribution. Physical screen-to-light latency and suspend/resume with the integrated worker are also open. The 15-second moving-clip packet capture is the baseline for a repeat: 443 packets, 442 consecutive payload changes, a 43.69 ms p95 gap and a 56.63 ms maximum. [Wire observation](evidence/720-grid/moving-wire.txt)
