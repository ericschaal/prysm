# Changelog

## [1.0.0-rc.2](https://github.com/ericschaal/prysm/compare/v1.0.0-rc.1...v1.0.0-rc.2) (2026-11-21)


### Bug Fixes

* a release names its .deb by the product version, so SHA256SUMS matches ([#18](https://github.com/ericschaal/prysm/issues/18)) ([f8631c3](https://github.com/ericschaal/prysm/commit/f8631c3e62b605da1b91e4f3abe26674c1aa1a0a))
* **portal:** a capture device that offers nothing says why, and a limit names its axis ([#31](https://github.com/ericschaal/prysm/issues/31)) ([11f7272](https://github.com/ericschaal/prysm/commit/11f727275c9367f4e468938ae3afe845f09442cb))


### Performance Improvements

* **processor:** the sampler reuses its sample buffer and shared edge colours are mapped without a copy first ([#25](https://github.com/ericschaal/prysm/issues/25)) ([45e4658](https://github.com/ericschaal/prysm/commit/45e46581a6d36c2493ab26a2cf7116568f60f9a3))


### Documentation

* how to choose a release's version, and that every release candidate needs it ([446ca57](https://github.com/ericschaal/prysm/commit/446ca579d7b69773c8567277f8596bc8323eff1e))

## [1.0.0-rc.1](https://github.com/ericschaal/prysm/compare/v0.2.0...v1.0.0-rc.1) (2026-11-19)


### ⚠ BREAKING CHANGES

* Prysm 1.0, one runtime with plain adapters

### Features

* Prysm 1.0, one runtime with plain adapters ([1e6c18f](https://github.com/ericschaal/prysm/commit/1e6c18fabf7731a3ae5ddb9924909336e5fd283e))


### Bug Fixes

* **ci:** separate external link audits from pull request checks ([#10](https://github.com/ericschaal/prysm/issues/10)) ([69ac65e](https://github.com/ericschaal/prysm/commit/69ac65edb8032f8df2e2861522912c3358e4b82e))
* **release:** preserve repository context during publication ([#9](https://github.com/ericschaal/prysm/issues/9)) ([5398be7](https://github.com/ericschaal/prysm/commit/5398be7aa83f6bbeed3fa6a4cb70220c3e2ea9d4))

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
