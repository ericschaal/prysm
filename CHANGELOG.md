# Changelog

## 0.2.0 (2026-11-04)

First packaged Prysm release.

- Capture HDMI video on Linux or macOS, or stream from a rooted LG TV to a separate Prysm host.
- Process screen-edge colors and send them to WLED over DDP.
- Configure sources, LED layout, preview, and color calibration in the embedded browser panel.
- Run with Docker on amd64 or arm64, or install the Debian arm64 package with its systemd service.
- Install the matching webOS IPK, including experimental hardware H.264 support.

### Installation

- [Docker](https://github.com/ericschaal/prysm/blob/main/docs/users/docker.md)
- [Debian / Raspberry Pi](https://github.com/ericschaal/prysm/blob/main/docs/users/debian.md)
- [webOS capturer](https://github.com/ericschaal/prysm/blob/main/docs/users/webos.md)

Prysm has no login; use a trusted LAN or private VPN. Hardware H.264 requires the
TV's GStreamer/GLib libraries and a compatible vendor encoder. TV capture is
qualified on LG 43QNED87A6D.BEUFLJP running webOS 25 (10.3.2-34); other models and
firmware are unqualified. Video-file input requires FFmpeg 9+, which the Docker
image does not include. Screen sync starts off after a host restart.
