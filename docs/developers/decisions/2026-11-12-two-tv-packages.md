# Two TV packages, explicit stream encoding

Date: 2026-11-12

The TV ships as two packages of one app, `io.prysm.tv`, under one supervisor
name: **full** (the TV processes, drives WLED and serves the portal) and
**slim** (the TV only captures and streams to a desktop Prysm). Installing one
replaces the other; there is no processing-location switch in the app. The
flavour is compiled into each supervisor binary
([webos-service](../../../hosts/webos-service/src/lib.rs)).

The slim stream is requested by the desktop receiver (size, rate, menus,
encoding). Encoding is an explicit setting, `h264_hardware` (default),
`h264_software` or `raw`. An encoder that cannot start refuses the request
and lighting stops with its reason; nothing falls back on its own. LZ4 is
dropped ([capture-remote](../../../adapters/capture-remote/README.md)).

Kept: capture in a supervised worker process, because a hung LG capture call
is recovered only by killing that process.

Not carried over: settings migration and detection of the old
`io.prysm.capture` app (clean slate), Prometheus metrics on the TV.

Replaces the "opt-in hardware H.264" row. Design:
[webOS supervisor](../../plans/prysm-v1-webos-supervisor.md).
