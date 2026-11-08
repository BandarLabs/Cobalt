#!/usr/bin/env python3
"""Cuts a recorded tour into the loop on the front page.

`record-tour.sh` films the reader with `kobo record`, which keeps a frame only
when the panel changed and writes `timings.txt` beside the frames: each frame's
name and the millisecond it appeared. This keeps the runs of frames worth
showing and plays each for about as long as the reader showed it. Nothing is
redrawn, so every frame in the loop is one the panel really showed.

Which frames to keep is read off the recording, not written here. A take has
frames nobody should publish: one caught halfway through a refresh, with the
last screen still showing under the new one, or a tap that landed somewhere
other than planned. Look at the frames, then name the runs to keep.

How long a frame stays up is the time it was on the panel, held between a
floor and a ceiling. The floor keeps a quick screen readable; the ceiling keeps
a wait, such as the computer thinking about its move, from reading as a stall.
The last frame is held longer so the loop has a beat before it starts again.

usage: scripts/cut-tour.py DIRECTORY --keep 0-9,11-27,29-37
                           [--floor S] [--ceiling S] [--last S]

DIRECTORY holds the frames and `timings.txt` that `kobo record --out` wrote.
Writes `cut.txt` there and, with ffmpeg on the path, `cobalt-tour.mp4` and
`tour.gif` (600 px wide, the size README.md shows).
"""

import argparse
from pathlib import Path
import shutil
import subprocess
import sys


def ranges(text):
    """Frame indexes from "0-9,11,13-20", in the order written."""
    indexes = []
    for part in text.split(","):
        first, _, last = part.strip().partition("-")
        start = int(first)
        end = int(last) if last else start
        if end < start:
            raise ValueError(f"{part}: a run ends before it starts")
        indexes.extend(range(start, end + 1))
    return indexes


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--keep", required=True, help="frame runs to keep, such as 0-9,11-27")
    parser.add_argument("--floor", type=float, default=0.45, help="shortest a frame stays up, in seconds")
    parser.add_argument("--ceiling", type=float, default=1.4, help="longest a frame stays up, in seconds")
    parser.add_argument("--last", type=float, default=2.5, help="how long the last frame stays up")
    args = parser.parse_args()

    root = args.directory
    timings = [line.split() for line in (root / "timings.txt").read_text().splitlines() if line.strip()]
    at = [int(millis) / 1000 for _, millis in timings]
    try:
        keep = ranges(args.keep)
    except ValueError as error:
        parser.error(str(error))
    if not keep or max(keep) >= len(timings):
        parser.error(f"--keep must name frames from 0 to {len(timings) - 1}")

    lines = []
    for position, index in enumerate(keep):
        # How long the frame was on the panel, measured to the next frame of
        # the recording, which is not always the next frame of the cut.
        shown = at[index + 1] - at[index] if index + 1 < len(at) else args.last
        duration = args.last if position == len(keep) - 1 else min(max(shown, args.floor), args.ceiling)
        lines.append(f"file '{timings[index][0]}'\nduration {duration:.3f}")
    # The concat format holds the last duration only if the file is repeated.
    lines.append(f"file '{timings[keep[-1]][0]}'")
    (root / "cut.txt").write_text("\n".join(lines) + "\n")
    total = sum(float(line.split()[-1]) for line in "\n".join(lines).splitlines() if line.startswith("duration"))
    print(f"{len(keep)} of {len(timings)} frames, {total:.1f} s")

    if shutil.which("ffmpeg") is None:
        print("ffmpeg is not on the path, so only cut.txt was written", file=sys.stderr)
        return 0
    video = root / "cobalt-tour.mp4"
    subprocess.run(
        ["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
         "-i", str(root / "cut.txt"), "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=30",
         "-pix_fmt", "yuv420p", str(video)],
        check=True,
    )
    print(f"video {video}")

    # A GIF rather than the video because a repository path in an <img>
    # renders everywhere and a <video> element does not. Built from the video
    # with the same scale and palette `kobo record` uses for its own loop, so
    # a recording and the published tour look alike.
    gif = root / "tour.gif"
    subprocess.run(
        ["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", str(video),
         "-vf", "scale=600:-1:flags=lanczos,fps=12,split[a][b];"
                "[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=bayer:bayer_scale=3",
         str(gif)],
        check=True,
    )
    print(f"loop {gif}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
