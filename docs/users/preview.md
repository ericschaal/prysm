# Preview and tune the picture

Prysm's browser panel shows the source image, detected black-bar crop and matching
perimeter colors. Keep your existing WLED output configured; preview is an optional
observer of the same processing pass that supplies the lights.

![Browser preview and all six live settings, verified using synthetic video and a local WLED fixture](../images/preview-tuning.jpg)

1. Start Prysm and configure the source and WLED in Settings. For file input,
   install FFmpeg 9+ and enter the absolute video path.
2. Open `http://localhost:8787`, select **Screen sync**, and turn the lights on.
3. Select **Preview**, then **Open preview**.
4. Open **Settings**, adjust picture controls and choose **Save & apply**, or **Discard** to return
   to saved values. Brightness, shadow lift, smoothing, sample density, spatial reach
   and black-bar removal apply live without restarting capture/output.

Preview remains closed until requested. Pause/close it to stop image requests;
hidden tabs also pause requests. The diagnostic image is scaled to at most 480px
and updates at up to 8 fps. Stopped/completed capture retains a clearly labeled last
image. Reopen/restart capture to inspect new frames. Preview never starts capture
or changes the configured lighting output.

Saved and processor-applied revisions are shown separately. If someone else edits
settings, review/reload the newer revision before saving; errors retain your draft.
Use Settings to edit input, output, source and processing. Configure network
bindings through CLI/environment when starting the process.

Configure WLED output with its HTTP URL and LED counts. There is no native
preview window or preview-only output. Closing the browser leaves Prysm running. File HDR is tone-mapped
to sRGB before processing; preview does not calibrate physical LEDs or the wall.
