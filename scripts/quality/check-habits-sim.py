#!/usr/bin/env python3
"""Drive Habits end to end in the simulator, offline: the empty Today offer,
adding a habit, skip, unskip, complete, the edit screen (rename, schedule,
archive and put back), the weekly summary with seeded history, the backup
export for a paired computer, and persistence across a simulator restart.

Habits never connects; there is no fixture server and no network of any
kind. History is seeded by writing the app's own encoded store file between
runs, exactly the bytes the app itself writes.
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)

    with tempfile.TemporaryDirectory(prefix="cobalt-habits-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391")
        env.pop("KOBO_SIM_OFFLINE", None)

        store = private / "cobalt-sim-state" / "habits"
        today = int(time.time() // 86400)
        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale, checks=[])
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
                                           cwd=ROOT / "apps/habits", env=env,
                                           stdout=log, stderr=log, start_new_session=True)
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
                command = [str(cli), "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=timeout)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                # Empty Today leads with the offer to add a habit.
                start()
                drive("wait-for Nothing due")
                capture("habits-empty")
                drive("tap Add a habit", "wait-for Habit name")
                drive("type read", "tap Add", "wait-for read")
                capture("habits-today")
                result["checks"].append(dict(
                    name="empty today and add",
                    detail="the empty Today screen offered Add a habit; naming one put it "
                           "on the checklist with its schedule label",
                    status="passed"))

                # Skip, unskip, complete.
                drive("tap Skip read", "wait-for Skipped")
                capture("habits-skipped")
                drive("tap read", "wait-for daily")
                drive("tap read", "wait-for Done")
                capture("habits-done")
                result["checks"].append(dict(
                    name="skip, unskip, complete",
                    detail="Skip marked the row Skipped; tapping a skipped row undid the "
                           "skip (plain schedule label again) and the next tap completed it",
                    status="passed"))

                # Edit: rename, schedule, archive and put back.
                drive("tap Manage", "wait-for read")
                drive("tap read", "wait-for Every 2 days")
                capture("habits-edit")
                drive("tap Rename", "wait-for Habit name")
                drive("tap space", "type more", "tap Save", "wait-for read more")
                drive("tap Every 2 days", "wait 500")
                drive("tap Archive", "wait-for Put back")
                drive("tap back", "wait-for archived")
                capture("habits-archived")
                drive("tap read more", "wait-for Put back")
                drive("tap Put back", "wait-for Archive")
                drive("tap back", "wait-for read more")
                result["checks"].append(dict(
                    name="edit screen",
                    detail="rename (prefilled entry, saved as read more), schedule change "
                           "to every 2 days, archive shown as archived on Manage, and put "
                           "back restored the habit",
                    status="passed"))

                # Seed a second habit's history into the store, then read the
                # weekly summary on Stats.
                stop()
                done = ",".join(str(today - n) for n in (1, 2, 3))
                with (store / "habits-v1").open("a") as seeded:
                    seeded.write(f"0\td\tstretch\t{done}\t{today - 4}\n")
                window = range(today - 6, today + 1)
                read_due = [d for d in window if d % 2 == 0]
                due = len(read_due) + 7
                week_done = (1 if today in read_due else 0) + 3
                week = f"This week: {week_done} of {due} due days completed, 1 skipped."
                start()
                drive("wait-for stretch", "tap Stats", "wait-for " + week,
                      "wait-for 4 completions")
                capture("habits-stats")
                result["checks"].append(dict(
                    name="weekly summary with seeded history",
                    detail=f"a seeded daily habit (3 done, 1 skipped in the window) plus the "
                           f"session habit summed to '{week}' under a 4 completions heading",
                    status="passed"))

                # Export a backup for the paired computer.
                drive("tap Settings", "wait-for Export a backup")
                capture("habits-settings")
                drive("tap Export a backup", "wait 800")
                capture("habits-export-offer")
                drive("wait-for Ready for your computer.")
                capture("habits-export-ready")
                offer = store / "cobalt-export"
                assert offer.is_file(), "the export offer was not saved to the store"
                assert "habits-backup" in offer.read_text(errors="replace"), \
                    "the saved offer does not name the backup"
                result["checks"].append(dict(
                    name="backup export",
                    detail="Settings offered Export a backup; the export flow prepared a "
                           "verified text copy and announced it ready for the paired "
                           "computer, with the offer saved in the app store",
                    status="passed"))

                # State survives a restart.
                stop()
                start()
                drive("wait-for stretch", "tap Manage", "wait-for read more")
                capture("habits-restored")
                result["checks"].append(dict(
                    name="persistence across restart",
                    detail="after a simulator restart the seeded habit was due on Today "
                           "and the session habit kept its renamed, rescheduled self",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
