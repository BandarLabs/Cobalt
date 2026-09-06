#!/bin/sh
set -eu
cd "$(dirname "$0")"
ffmpeg -y -f concat -safe 0 -i frames.ffconcat -vf 'fps=4' -c:v libx264 -preset slow -crf 28 -pix_fmt yuv420p -movflags +faststart marketing.mp4
ffmpeg -y -f concat -safe 0 -i frames.ffconcat -vf 'fps=4' -c:v libvpx-vp9 -crf 36 -b:v 0 -pix_fmt yuv420p marketing.webm
ffmpeg -y -f concat -safe 0 -i frames.ffconcat -filter_complex '[0:v]fps=4,split[a][b];[a]palettegen=max_colors=48:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle' marketing.gif
