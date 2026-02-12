#!/bin/bash

sudo modprobe v4l2loopback

# print info about video device
v4l2-ctl -d /dev/video0 --all

# start video stream
ffmpeg -re -f lavfi -i testsrc=size=800x600:rate=30   -vf format=pix_fmts=rgb24 -f v4l2 /dev/video0