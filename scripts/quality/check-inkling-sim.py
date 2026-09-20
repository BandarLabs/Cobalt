#!/usr/bin/env python3
"""Drive Inkling end to end: deterministic pinned day, shape-marked
uppercase board, on-keyboard letter knowledge, real date display, statistics
with a win distribution, result export verified on disk, archive play that
leaves statistics untouched, and daily persistence across a restart.

The day is pinned with KOBO_INKLING_DAY=2026-09-01, whose answer is gravy;
the archive day 2026-08-31 answers dizzy.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

from simulator_cli import build_cli

ROOT = Path(__file__).resolve().parents[2]

PINNED_DAY = "2026-09-01"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)

    with tempfile.TemporaryDirectory(prefix="cobalt-inkling-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_INKLING_DAY=PINNED_DAY)
        env.pop("KOBO_SIM_OFFLINE", None)

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      pinned_day=PINNED_DAY, checks=[])
        with (out / "simulator.log").open("w") as log:
            def stop():
                nonlocal process
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=15)
                process = None

            def start():
                nonlocal process, address
                offset = (out / "simulator.log").stat().st_size
                process = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"],
                                           cwd=ROOT / "apps/inkling", env=env,
                                           stdout=log, stderr=log,
                                           start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError("Simulator exited; see simulator.log")
                    match = re.search(r"Kobo app simulator: http://(127\.0\.0\.1:\d+)",
                                      (out / "simulator.log").read_text()[offset:])
                    if match:
                        address = match.group(1)
                        return
                    time.sleep(.1)
                raise TimeoutError("Simulator startup timed out")

            def drive(*steps, timeout=120):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=timeout)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                # The title is a real date, not a puzzle number.
                drive("wait-for Inkling · Sep 1")
                capture("inkling-board")

                # Help stays reachable from the top bar.
                drive("tap How to play", "wait-for Find the five-letter word")
                capture("inkling-help")
                drive("tap Play", "wait-for Inkling · Sep 1")

                # First guess: the typing panel admits nothing is known yet.
                drive("tap Enter guess", "wait-for No letters known yet.")
                drive("type crane", "tap Guess", "wait-for 1 of 6")

                # Second guess: known letters are listed while typing.
                drive("tap Enter guess", "wait-for Placed: _RA__")
                capture("inkling-knowledge")
                drive("type gravy", "tap Guess", "wait-for Solved")
                capture("inkling-solved")

                # Statistics carry the win and its distribution slot.
                drive("tap Stats", "wait-for Played 1. Won 1.",
                      "wait-for Solved in 2 of 6: 1",
                      "wait-for Today is September 1, 2026.")
                capture("inkling-stats")

                # The export lands in the store and reads back exactly.
                drive("tap Export results", "wait 1500")
                exported = list(private.rglob("export-result.txt"))
                assert exported, "export-result.txt was not written to the sim store"
                text = exported[0].read_text()
                assert "Inkling, September 1, 2026" in text, text
                assert "Solved in 2 of 6." in text, text
                assert "C\u00d7 [R] [A] N\u00d7 E\u00d7" in text, text
                assert "[G] [R] [A] [V] [Y]" in text, text
                assert "Played 1. Won 1." in text, text
                assert "Solved in 2: 1" in text, text
                drive("tap Play", "wait-for Inkling · Sep 1")

                # Archive play solves a past day without touching statistics.
                drive("tap Archive", "wait-for August 31, 2026")
                capture("inkling-archive-list")
                drive("tap August 31, 2026",
                      "wait-for Archive puzzle from August 31, 2026.")
                capture("inkling-archive")
                drive("tap Enter guess", "type dizzy", "tap Guess",
                      "wait-for Archive games do not change statistics")
                capture("inkling-archive-solved")
                drive("tap Stats", "wait-for Played 1. Won 1.")
                drive("tap Play", "tap Today", "wait-for Inkling · Sep 1",
                      "wait-for Solved.")

                # The finished daily game and the statistics survive a restart.
                stop()
                start()
                drive("wait-for Solved.")
                capture("inkling-restored")
                drive("tap Stats", "wait-for Played 1. Won 1.",
                      "wait-for Solved in 2 of 6: 1")

                result["checks"].append(dict(
                    name="inkling journey",
                    detail="real date title, help, first-guess empty knowledge, "
                           "typing letter knowledge, uppercase shape-marked solve, "
                           "statistics with distribution, export verified on disk, "
                           "archive solve with statistics untouched, daily game and "
                           "statistics proven across a restart",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
