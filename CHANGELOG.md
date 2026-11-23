# Changelog

## [1.0.0-rc.2](https://github.com/ericschaal/prysm/compare/v1.0.0-rc.1...v1.0.0-rc.2) (2026-11-23)


### ⚠ BREAKING CHANGES

* a capture device's setting is `device`, not `path` ([#82](https://github.com/ericschaal/prysm/issues/82))
* a style, a calibration and a request body are read from JSON objects only ([#77](https://github.com/ericschaal/prysm/issues/77))
* the Luna schema names the one StreamEncoding; a test guards the adapter lists ([#75](https://github.com/ericschaal/prysm/issues/75))
* value problems are 422 with a field path; final ranges; adapter defaults are generated ([#67](https://github.com/ericschaal/prysm/issues/67))
* tv.json is read tolerantly; instance is prysm_address; Luna errors carry an errorKind ([#66](https://github.com/ericschaal/prysm/issues/66))
* one lighting vocabulary (off, screen_sync, effects); a failure lives in the session; faulted carries a message; response enums are open ([#65](https://github.com/ericschaal/prysm/issues/65))
* the Debian service takes its options from /etc/default/prysm-headless; image tags :X and :latest follow the newest stable release ([#63](https://github.com/ericschaal/prysm/issues/63))
* an unreadable settings file starts Prysm unconfigured; the WLED check never blocks the lights; a stop idles the lights once within asserted limits ([#62](https://github.com/ericschaal/prysm/issues/62))
* a failed Luna check ends capture at once; a stop on the slim TV cuts its network calls; the TV portal takes only the TV app's origin ([#61](https://github.com/ericschaal/prysm/issues/61))
* the Luna API is version 3. A running slim worker in `getState` carries `link` (a tagged state) instead of `fps`, `encoding` and `paused`.
* at the same Detail value, a capture larger than 640x360 gets fewer samples (1080p at 128: 81 along the top, was 245). The MJPEG pixel format is removed.
* **capture-remote:** the capture stream is protocol v6 and tolerates additions ([#45](https://github.com/ericschaal/prysm/issues/45))
* the TV app id is io.github.ericschaal.prysm, and absent TV values are omitted ([#44](https://github.com/ericschaal/prysm/issues/44))
* the settings file carries a format number, the state groups the device's reports and shows the version, and pattern names are snake_case ([#42](https://github.com/ericschaal/prysm/issues/42))
* **webos:** the TV state and worker status are tagged by package, so neither carries the other's fields ([#33](https://github.com/ericschaal/prysm/issues/33))

### Features

* **capture-remote:** the capture stream is protocol v6 and tolerates additions ([#45](https://github.com/ericschaal/prysm/issues/45)) ([f5be320](https://github.com/ericschaal/prysm/commit/f5be3201f5386180c78854558e97abe8a10b1374))
* one lighting vocabulary (off, screen_sync, effects); a failure lives in the session; faulted carries a message; response enums are open ([#65](https://github.com/ericschaal/prysm/issues/65)) ([887d543](https://github.com/ericschaal/prysm/commit/887d543472149f17a71c00ba21ee375533f214b5))
* the settings file carries a format number, the state groups the device's reports and shows the version, and pattern names are snake_case ([#42](https://github.com/ericschaal/prysm/issues/42)) ([1f6aefe](https://github.com/ericschaal/prysm/commit/1f6aefe767e0f6fd8b49e607fa5abb8eaf9fa2f7))
* the TV app id is io.github.ericschaal.prysm, and absent TV values are omitted ([#44](https://github.com/ericschaal/prysm/issues/44)) ([080b822](https://github.com/ericschaal/prysm/commit/080b82254f8fdddb14aca30b6cb3129890bb7853))


### Bug Fixes

* a failed Luna check ends capture at once; a stop on the slim TV cuts its network calls; the TV portal takes only the TV app's origin ([#61](https://github.com/ericschaal/prysm/issues/61)) ([606f93f](https://github.com/ericschaal/prysm/commit/606f93fc9f466161bd8f90c5cb6c4f45e2ad19b3))
* a failure with the lights off is shown on the setup path, its section page, the settings rows and the full TV headline ([#72](https://github.com/ericschaal/prysm/issues/72)) ([0a70c62](https://github.com/ericschaal/prysm/commit/0a70c621457029b3804c4d074235bf2ee64c19ba))
* a panic in the macOS capture callbacks is caught, a V4L2 read error restarts the stream, and audit hardening ([#48](https://github.com/ericschaal/prysm/issues/48)) ([319f699](https://github.com/ericschaal/prysm/commit/319f69976ae326744fb9f5e68714495e9086e33d))
* a release names its .deb by the product version, so SHA256SUMS matches ([#18](https://github.com/ericschaal/prysm/issues/18)) ([f8631c3](https://github.com/ericschaal/prysm/commit/f8631c3e62b605da1b91e4f3abe26674c1aa1a0a))
* a remote capture connection ends through one table; each sender is checked against its own offer ([#53](https://github.com/ericschaal/prysm/issues/53)) ([d70a626](https://github.com/ericschaal/prysm/commit/d70a62673314723e5be1f4b82cbb29f229fdb294))
* a Start save that ran without an answer continues the calibration once the state shows it ([#69](https://github.com/ericschaal/prysm/issues/69)) ([61d6cc1](https://github.com/ericschaal/prysm/commit/61d6cc1911b9f51ad987a90332351c8a73e49999))
* a stop ends an output open or idle in flight, so every TV stop ends inside its deadline ([#68](https://github.com/ericschaal/prysm/issues/68)) ([d50777d](https://github.com/ericschaal/prysm/commit/d50777d2aea0536dab1dd7a0574f0ab822b4835a))
* a style, a calibration and a request body are read from JSON objects only ([#77](https://github.com/ericschaal/prysm/issues/77)) ([193455c](https://github.com/ericschaal/prysm/commit/193455c1d3ffcd20b5f3b3d20ad57972186cbca3))
* **acceptance:** the preflight judges an artifact by its manifest entry, not by file time ([#22](https://github.com/ericschaal/prysm/issues/22)) ([9345112](https://github.com/ericschaal/prysm/commit/9345112e068360aeac1a38063c88dd734f6c03a5))
* an unreadable settings file starts Prysm unconfigured; the WLED check never blocks the lights; a stop idles the lights once within asserted limits ([#62](https://github.com/ericschaal/prysm/issues/62)) ([4bb4cc1](https://github.com/ericschaal/prysm/commit/4bb4cc1d0405185084a4652b3d40194a89d60e2e))
* **capture-device:** a capture card that cannot deliver fails with a reason, on macOS and Linux ([#32](https://github.com/ericschaal/prysm/issues/32)) ([eadd0fe](https://github.com/ericschaal/prysm/commit/eadd0fe841157e877066a18289942210e7bff5ef))
* check-encoder catches wrong pictures, the dead rvenc check goes, and the TV smoke test matches the service ([#30](https://github.com/ericschaal/prysm/issues/30)) ([5385fd9](https://github.com/ericschaal/prysm/commit/5385fd9c6fd49112ec3598762b873a9911596c2d))
* closing a file source whose FFmpeg already exited succeeds on macOS ([#64](https://github.com/ericschaal/prysm/issues/64)) ([7d0dc1e](https://github.com/ericschaal/prysm/commit/7d0dc1efb54ba39b5cb8ad20efd578ee01c833a5))
* desktop hosts own the terminal explicitly; a second interrupt exits at once; FFmpeg is awaited without polling ([#60](https://github.com/ericschaal/prysm/issues/60)) ([c7e16e7](https://github.com/ericschaal/prysm/commit/c7e16e72ca779c06db1475f9035375244b94204f))
* failure messages read as sentences, WLED replies have one reader, and a release refuses unsafe file names ([#35](https://github.com/ericschaal/prysm/issues/35)) ([90b8227](https://github.com/ericschaal/prysm/commit/90b8227938c1a74ac09ed1836cc64e59fc35c565))
* **h264:** codec limits have one definition, one Annex B walk, and the buffer sync restarts when interrupted ([#49](https://github.com/ericschaal/prysm/issues/49)) ([cf70b56](https://github.com/ericschaal/prysm/commit/cf70b56e3e94eb036fa3b28aa193299c7c644923))
* **portal:** a capture device that offers nothing says why, and a limit names its axis ([#31](https://github.com/ericschaal/prysm/issues/31)) ([11f7272](https://github.com/ericschaal/prysm/commit/11f727275c9367f4e468938ae3afe845f09442cb))
* **runtime:** adapter calls and shutdown have limits, totals are read whole, and three latent defects are closed ([#27](https://github.com/ericschaal/prysm/issues/27)) ([7983fc5](https://github.com/ericschaal/prysm/commit/7983fc531eef7550083fd386afb7fe78ca6eb971))
* settings that do not validate say so whatever the target; loose ends from the second audit ([#80](https://github.com/ericschaal/prysm/issues/80)) ([5b83a5b](https://github.com/ericschaal/prysm/commit/5b83a5b5a77e3d001e74cd21f2215f0d518f7980))
* the device captures share one health rule; TV handles cannot be half-initialised; vendor libraries load once ([#58](https://github.com/ericschaal/prysm/issues/58)) ([c0bd91c](https://github.com/ericschaal/prysm/commit/c0bd91c09346f91e51111505959c4bd3867e6be9))
* the engine's lighting is one enum that owns the session; a stop meeting a session's own end has one outcome ([#56](https://github.com/ericschaal/prysm/issues/56)) ([b55aa39](https://github.com/ericschaal/prysm/commit/b55aa396581e876cea6f545801c43c6fa541a97d))
* the kernel ends a portal connection whose peer vanished, so its stream slot is freed ([#81](https://github.com/ericschaal/prysm/issues/81)) ([be7572b](https://github.com/ericschaal/prysm/commit/be7572b144a43aedf0bc97fffd3e7a6d82e2ddc2))
* the portal and the TV app read lighting from one derivation; saves queue; the calibration wizard is one reducer ([#57](https://github.com/ericschaal/prysm/issues/57)) ([e58ae14](https://github.com/ericschaal/prysm/commit/e58ae1406b5255ad8206a5f8adcbd6db6c2247da))
* the portal asks what a capture delivers once, with Try again; a saved section missing a field still opens its editor ([#83](https://github.com/ericschaal/prysm/issues/83)) ([b341943](https://github.com/ericschaal/prysm/commit/b341943fe117be3d27c4c6af2486a5bb6267c46d))
* the portal sends each event at once (TCP_NODELAY on accepted connections) ([#86](https://github.com/ericschaal/prysm/issues/86)) ([8885aaf](https://github.com/ericschaal/prysm/commit/8885aaf10d8f328cfe2e9959865b7c5fa463bcc8))
* the remaining polls and timers give way to events ([#71](https://github.com/ericschaal/prysm/issues/71)) ([9a6215b](https://github.com/ericschaal/prysm/commit/9a6215b2ae02f587e1bc4a3ac76275c8e2c2735d))
* the sample count follows the picture budget, not the capture width; the processor's stages are one pipeline ([#54](https://github.com/ericschaal/prysm/issues/54)) ([e9faaa4](https://github.com/ericschaal/prysm/commit/e9faaa481020e4419637885e7a145e63ca3820ba))
* the slim uplink and the TV supervisor each change state in one function; the slim link is one tagged state on the wire ([#55](https://github.com/ericschaal/prysm/issues/55)) ([8ce3c49](https://github.com/ericschaal/prysm/commit/8ce3c491da2bbe20c836a542bafb68abcc2ac634))
* the TV capture libraries are loaded once, luma index maths cannot overflow, and the install tests show their errors ([#26](https://github.com/ericschaal/prysm/issues/26)) ([4425ae4](https://github.com/ericschaal/prysm/commit/4425ae481aed7e2985e0572c97e83ffa783b9671))
* tv.json is read tolerantly; instance is prysm_address; Luna errors carry an errorKind ([#66](https://github.com/ericschaal/prysm/issues/66)) ([664e91d](https://github.com/ericschaal/prysm/commit/664e91d998c085a72785e76018455c330f3b9d41))
* value problems are 422 with a field path; final ranges; adapter defaults are generated ([#67](https://github.com/ericschaal/prysm/issues/67)) ([a680e66](https://github.com/ericschaal/prysm/commit/a680e6624fee75f506f9b7699bbd8a96e0392479))
* **webos:** an update outside the install helper no longer locks setup out, and the worker link keeps its last words ([#41](https://github.com/ericschaal/prysm/issues/41)) ([6d212d5](https://github.com/ericschaal/prysm/commit/6d212d57242ae5d5acaf181f4da77e688245f1b5))


### Performance Improvements

* **processor:** the sampler reuses its sample buffer and shared edge colours are mapped without a copy first ([#25](https://github.com/ericschaal/prysm/issues/25)) ([45e4658](https://github.com/ericschaal/prysm/commit/45e46581a6d36c2493ab26a2cf7116568f60f9a3))


### Documentation

* how to choose a release's version, and that every release candidate needs it ([446ca57](https://github.com/ericschaal/prysm/commit/446ca579d7b69773c8567277f8596bc8323eff1e))


### Code Refactoring

* a capture device's setting is `device`, not `path` ([#82](https://github.com/ericschaal/prysm/issues/82)) ([1a1ae45](https://github.com/ericschaal/prysm/commit/1a1ae45c25444d5bbe095d94170a96d337cbf577))
* the Luna schema names the one StreamEncoding; a test guards the adapter lists ([#75](https://github.com/ericschaal/prysm/issues/75)) ([ab2d4ac](https://github.com/ericschaal/prysm/commit/ab2d4ace6850f57deeb4c937dfdc848843482ca1))
* **webos:** the TV state and worker status are tagged by package, so neither carries the other's fields ([#33](https://github.com/ericschaal/prysm/issues/33)) ([6d4ce89](https://github.com/ericschaal/prysm/commit/6d4ce894797aef518b127eb8664ecaf2afd245aa))


### Build System

* the Debian service takes its options from /etc/default/prysm-headless; image tags :X and :latest follow the newest stable release ([#63](https://github.com/ericschaal/prysm/issues/63)) ([2ff3d55](https://github.com/ericschaal/prysm/commit/2ff3d5516f332b70b18b018b1f80d85f01fba1ec))

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
