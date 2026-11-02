# Public references checked 2026-11-02

- [LG webOS TV 6.0 media specifications](https://webostv.developer.lge.com/develop/specifications/video-audio-60): playback specifications list H.264 4K30, with 4K60 on some models, and HEVC 4K60 on UHD models. They do not establish an encoder capability or qualify this particular TV.
- [GStreamer 1.24.0 H.264 parser](https://raw.githubusercontent.com/GStreamer/gstreamer/1.24.0/subprojects/gst-plugins-bad/gst/videoparsers/gsth264parse.c): lines 1737–1739 and 1915–1925 handle AU-aligned input as complete. Release-pinned source, not verified against the vendor parser binary.
- [GStreamer H.264 parser documentation](https://gstreamer.freedesktop.org/documentation/videoparsersbad/h264parse.html): supported stream-format/alignment conversions and SPS/PPS configuration interval. No generic promise that removing it makes a pipeline one frame faster.

No external source substitutes for the firmware-specific evidence and hashes
in the report. Public source review did not access the TV.
