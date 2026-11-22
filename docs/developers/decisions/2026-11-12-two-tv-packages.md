# Two TV packages, explicit stream encoding

Date: 2026-11-12

The TV ships as two packages of one app, `io.github.ericschaal.prysm`, under one supervisor
name: **full** (the TV processes, drives WLED and serves the portal) and
**slim** (the TV only captures and streams to a desktop Prysm). Installing one
replaces the other; there is no processing-location switch in the app. The
flavour is compiled into each supervisor binary
([webos-service](../../../crates/hosts/webos-service/src/lib.rs)).

The slim stream is requested by the desktop receiver (size, rate, menus,
encoding). Encoding is an explicit setting, `h264_hardware` (default),
`h264_software` or `raw`. An encoder that cannot start refuses the request
and lighting stops with its reason; nothing falls back on its own. LZ4 is
dropped ([capture-remote](../../../crates/adapters/capture-remote/README.md)).

Kept: capture in a supervised worker process, because a hung LG capture call
is recovered only by killing that process. The supervisor stays a process of
its own, so Luna's FFI and the engine never share one and a worker cannot
claim the Luna name.

One supervisor library, two thin binaries: each package ships only its own
supervisor and worker under the shared installed names, so a supervisor
cannot start the other package's worker. One binary with a `--flavor` in its
`.service` file would save a link step and add a way to mismatch them.

Not carried over: settings migration and detection of the old
`io.prysm.capture` app (clean slate), Prometheus metrics on the TV.

Replaces the "opt-in hardware H.264" row. How it is built:
[LG TV](../webos.md).
