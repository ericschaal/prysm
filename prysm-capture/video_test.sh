#!/bin/bash

sudo modprobe v4l2loopback

# print info about video device
v4l2-ctl -d /dev/video2 --all

# start video stream
ffmpeg -re -f lavfi -i testsrc=size=800x600:rate=30   -vf format=pix_fmts=rgb24 -f v4l2 /dev/video2

# Mandelbrot fractal - animated, colorful, great for testing edge colors
#ffmpeg -re -f lavfi -i mandelbrot=size=800x600:rate=30 -vf format=pix_fmts=rgb24 -f v4l2 /dev/video0

# TestSrc2 - improved test pattern with moving elements and varied colors
#ffmpeg -re -f lavfi -i testsrc2=size=800x600:rate=30 -vf format=pix_fmts=rgb24 -f v4l2 /dev/video0

# Cellular automaton - organic-looking animated patterns
#ffmpeg -re -f lavfi -i cellauto=size=800x600:rate=30 -vf format=pix_fmts=rgb24 -f v4l2 /dev/video0

# Gradient - smooth color transitions (great for testing edge depth uniformity)
#ffmpeg -re -f lavfi -i gradients=size=800x600:rate=30 -vf format=pix_fmts=rgb24 -f v4l2 /dev/video0