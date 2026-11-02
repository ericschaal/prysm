# Try Prysm without LEDs

You need a desktop session, FFmpeg 9+ on `PATH`, and Prysm built with the
`desktop` feature. See [build instructions](../developers/README.md#build).

1. Save the [preview configuration](examples/preview.toml) as `prysm-preview.toml`.
2. Set `[input].path` to the absolute path of your video.
3. Run `prysm --config prysm-preview.toml` and click **Start capture** in the window,
   or press **Space**.

**Success:** the window shows video, detected black-bar crop, and colors along each edge.
At the end of the file the final frame stays visible; **Start capture** replays it.
Closing the window exits Prysm and its web panel.

The same preview works with device or [webOS input](webos.md#1-start-the-receiver).
File playback tone-maps HDR to sRGB before processing. The preview does not
calibrate your physical LEDs or wall.
