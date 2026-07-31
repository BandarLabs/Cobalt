#!/usr/bin/env python3
"""Cuts a recorded tour into a publishable one.

`record-tour.sh` produces frames, their timings, and a table of how fast each
stretch is worth watching. This takes those three and rearranges them: it drops
runs of frames, and it plays runs in an order other than the one they were
recorded in. Nothing is redrawn, so every frame is still a frame the panel
really showed.

Two reasons it exists rather than a re-record.

A recording is expensive. The reader takes a new address from DHCP most days,
sleeps two minutes after waking, and a run takes six minutes and has failed
more often than it has worked. A cut that costs seconds is worth having when
the alternative is another evening of it.

And some things can only be decided after watching. The Wi-Fi screens name the
owner's home network and their neighbours' networks, which is fine on a desk
and not fine on the front page of a public repository. The audiobook was
recorded at both ends of the tour, opening with a finished one and closing with
one being written, and the two read better together than apart.

Cuts are made where the panel shows the same thing on both sides, so a join is
invisible: leaving Settings lands on the launcher, and so does the frame before
entering it.

usage: scripts/cut-tour.py DIRECTORY [--speed X]

DIRECTORY is a run made by record-tour.sh: it holds `segments.txt` and a `tour`
directory of frames and `timings.txt`. Writes `cut.txt` beside them and, if
ffmpeg is on the path, `cobalt-tour.mp4` and `tour.gif`.
"""

import subprocess
import sys
from pathlib import Path

# The tour as it was recorded, in the order it is worth watching.
#
# Each piece is a run of frames, inclusive, and a note on why it starts and
# ends where it does. The frame numbers are of one particular recording and
# mean nothing in another; a new run needs them read off its own frames.
PIECES = [
    (19, 76, "the launcher, Gutenbird, Sidekick answering a real question, Hacker News"),
    (98, 216, "Terminal, Components, Daily Brief, Todo, tic-tac-toe, and the newspaper"),
    (0, 17, "the audiobook shelf and player, moved here from the opening"),
    (222, 255, "and the same shelf writing another one, which is where it ends"),
]

# Frames 77 to 97 are the Settings visit. Every screen in it apart from the
# battery names a wireless network: the connections screen names the one this
# reader is on, and the scan names four, three of which belong to neighbours.
# There is no cut that keeps Settings and keeps those out, so Settings goes.

# The audiobook was recorded twice, at both ends. The second visit re-entered
# the application from the launcher, which is redundant once the first visit is
# next to it, so frames 217 to 221 are dropped and the join is made on the
# shelf, which both pieces show.


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    root = Path(sys.argv[1])
    speed = 1.0
    if "--speed" in sys.argv:
        speed = float(sys.argv[sys.argv.index("--speed") + 1])

    frames = root / "tour"
    timings = [line.split() for line in (frames / "timings.txt").read_text().split("\n") if line]
    at = [int(millis) / 1000 for _, millis in timings]
    segments = [
        (float(start), float(rate))
        for start, rate in (line.split() for line in (root / "segments.txt").read_text().split("\n") if line)
    ]

    # How long each frame was really on the panel, and how fast that moment is
    # worth watching. Both are read from the recording rather than the cut, so
    # moving a piece does not change how it plays.
    def shown(index: int) -> float:
        held = at[index + 1] - at[index] if index + 1 < len(at) else 1.0
        rate = segments[0][1]
        for start, value in segments:
            if start <= at[index]:
                rate = value
        return max(held / (rate * speed), 0.025)

    lines = []
    kept = 0
    for first, last, why in PIECES:
        print(f"frames {first}-{last}: {why}")
        for index in range(first, last + 1):
            # The frame a piece ends on was held until whatever came next in
            # the recording, which is no longer what comes next. Half a second
            # is long enough to register as a beat and short enough not to
            # read as a stall.
            duration = min(shown(index), 0.5) if index == last else shown(index)
            lines.append(f"file '{timings[index][0]}'\nduration {duration:.3f}")
            kept += 1
    lines.append(f"file '{timings[PIECES[-1][1]][0]}'")
    (frames / "cut.txt").write_text("\n".join(lines) + "\n")
    print(f"{kept} frames of {len(timings)}")

    video = root / "cobalt-tour.mp4"
    subprocess.run(
        ["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
         "-i", str(frames / "cut.txt"), "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=30",
         "-pix_fmt", "yuv420p", str(video)],
        check=True,
    )
    print(f"video {video}")

    # And a still-legible loop of the whole thing, for a README. A GIF rather
    # than the video because a repository path in an <img> renders everywhere
    # and a <video> element does not, and because a hero that waits to be
    # clicked is a hero nobody sees.
    gif = root / "tour.gif"
    subprocess.run(
        ["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", str(video),
         "-vf", "setpts=PTS/6,scale=330:-1:flags=lanczos,fps=12,split[a][b];"
                "[a]palettegen=max_colors=32[p];[b][p]paletteuse=dither=bayer:bayer_scale=3",
         str(gif)],
        check=True,
    )
    print(f"loop {gif}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
