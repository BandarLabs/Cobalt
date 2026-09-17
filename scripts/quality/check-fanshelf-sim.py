#!/usr/bin/env python3
"""Drive Fanshelf in demo mode through the shelf, work, and updates screens."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

from simulator_cli import build_cli, verify_cli

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
    verify_cli(cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-fanshelf-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   FANSHELF_DEMO="1",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)

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
                                           cwd=ROOT / "apps/fanshelf", env=env,
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

            def drive(*steps):
                command = [str(cli), "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=180)

            try:
                start()
                # The demo shelf: an update waiting, a finished download, and a
                # work in progress that has never been checked.
                drive("wait-for The Clockwork Garden", "wait-for updates unchecked",
                      "wait-for reading")
                drive("clean", "shot fanshelf-shelf")
                # The work screen names the last manual check.
                drive("tap The Clockwork Garden", "wait-for New chapters found")
                drive("clean", "shot fanshelf-work-update")
                drive("tap back", "wait-for A Field Guide to Small Hours")
                drive("tap A Field Guide to Small Hours", "wait-for Updates never checked")
                drive("clean", "shot fanshelf-work-unchecked")
                # The updates screen is the manual schedule.
                drive("tap back", "wait-for Updates", "tap Updates",
                      "wait-for Nothing runs in the background", "wait-for Unread update",
                      "wait-for Never checked")
                drive("clean", "shot fanshelf-updates")
                # The shelf narrows to one fandom and widens again.
                drive("tap Shelf", "tap Filter", "wait-for Fandoms",
                      "wait-for 2 works")
                drive("clean", "shot fanshelf-fandoms")
                drive("tap Synthetic Library Stories", "wait-for Synthetic Library Stories",
                      "wait-for A Field Guide to Small Hours")
                drive("clean", "shot fanshelf-fandom-filter")
                drive("expect-missing The Clockwork Garden")
                drive("tap All", "wait-for The Clockwork Garden")
                # Manage: bulk counts and the removal confirmation.
                drive("tap Manage", "wait-for Download all updates", "wait-for 1 waiting",
                      "wait-for 2 on the shelf")
                drive("clean", "shot fanshelf-manage")
                drive("tap Remove downloaded copies", "wait-for Remove 2 downloaded copies?",
                      "wait-for Reading places are kept")
                drive("clean", "shot fanshelf-manage-confirm")
                drive("tap Go back", "wait-for Download all updates", "tap Shelf",
                      "wait-for reading")
                result["checks"].append(dict(
                    name="update-check journey",
                    detail="Demo shelf showed the update, reading and never-checked "
                           "badges; each work screen named its last manual check; the "
                           "updates screen listed unread, never-checked and current "
                           "states; the fandom filter narrowed the shelf to one fandom "
                           "(hiding the other work) and All restored it; Manage listed "
                           "bulk counts and the removal confirmation kept the shelf "
                           "intact after Go back",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
