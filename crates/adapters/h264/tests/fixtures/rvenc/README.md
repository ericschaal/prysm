These project-generated synthetic NV12 fixtures come from the proven TV rvenc
probe in work/webos-hardware-encoding-poc. They contain fifteen grayscale frames
at 640x360, 15 FPS. original-00.au retains the incorrect 640x368/colorless SPS;
adjusted-00.au through adjusted-14.au have only SPS crop and BT.709 correction.
They verify the production Rust correction and unchanged receiver. No vendor
binaries or proprietary source are included.
