#!/usr/bin/env python3
"""Prove Frame's sleep ownership in the simulator: wake holds, scheduled
wakes, wake delivery on clock crossings and policy persistence."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile
import time
import urllib.request

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "scripts" / "fixtures" / "frame"
PHOTOS = ["birds-on-a-wire.jpg", "city-skyline.jpg", "hills-at-dawn.jpg"]
FRAME_HOLD_MILLIS = (15 * 60 + 60) * 1000
SLOW_WAKE_MILLIS = 6 * 3600 * 1000


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

    with tempfile.TemporaryDirectory(prefix="cobalt-frame-power-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private),
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000",
                   # Power ownership is an opt-in sim fixture; Frame's modes
                   # are exactly what it models.
                   KOBO_SIM_BACKENDS="network,battery-read,frontlight-control,wifi-control,cover-sensor,library,keep-awake,scheduled-wake")
        env.pop("KOBO_SIM_OFFLINE", None)

        album = private / "album"
        album.mkdir()
        for name in PHOTOS:
            shutil.copy(FIXTURES / name, album / name)
        # The shelf is prepared by the real companion CLI, exactly as an
        # owner would on their computer; only the sim root is redirected.
        subprocess.run([str(cli), "frame", "init", "--sim"], cwd=ROOT,
                       env=env, check=True, capture_output=True)
        subprocess.run([str(cli), "frame", "push", str(album), "--sim",
                        "--album", "Demo Album"],
                       cwd=ROOT, env=env, check=True, capture_output=True)

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
                                           cwd=ROOT / "apps/frame", env=env,
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

            def drive(*steps):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                               stderr=log, check=True, timeout=180)

            def get(endpoint):
                with urllib.request.urlopen(f"http://{address}/{endpoint}",
                                            timeout=5) as response:
                    return response.read()

            def power():
                return json.loads(get("power"))

            def capture(name):
                drive("wait-idle", "clean", "shot " + name)
                diagnostics = json.loads(get("diagnostics"))
                assert not [issue for issue in diagnostics["issues"]
                            if issue["severity"] == "error"], diagnostics

            def aid(name):
                value = 0x811c9dc5
                for byte in name.encode():
                    value = ((value ^ byte) * 0x01000193) & 0xffffffff
                return max(value, 1)

            def has(name):
                return any(node["action"] == aid(name)
                           for node in json.loads(get("layout"))["nodes"])

            try:
                start()
                drive("wait-for Photographs", "wait-for 1 of 3", "wait-idle")
                state = power()
                assert state["wakeHeld"] is True, state
                assert (int(state["wakeUntil"]) - int(state["monotonicMillis"])
                        == FRAME_HOLD_MILLIS), state
                assert state["scheduledWake"] is None, state
                capture("01-frame-mode-awake")
                result["checks"].append("frame mode holds the wake lock for the interval")

                assert has("menu"), "settings entry missing"
                drive("tap-id menu", "wait-for-id mode", "wait-idle")
                capture("02-settings-frame-mode")
                drive("tap-id mode", "wait-idle")
                state = power()
                assert state["wakeHeld"] is False, state
                assert (int(state["scheduledWake"]) - int(state["monotonicMillis"])
                        == SLOW_WAKE_MILLIS), state
                capture("03-settings-slow")
                result["checks"].append(
                    "slow slideshow releases the hold and schedules a real wake")
                drive("tap Back", "wait-idle")

                drive("clock advance " + str(SLOW_WAKE_MILLIS),
                      "wait-for 2 of 3", "wait-idle")
                state = power()
                assert (int(state["scheduledWake"]) - int(state["monotonicMillis"])
                        == SLOW_WAKE_MILLIS), state
                assert state["wakeHeld"] is False, state
                capture("04-wake-advanced")
                result["checks"].append(
                    "a clock crossing fires the scheduled wake, advances the photo and reschedules")

                drive("tap-id menu", "wait-for-id mode", "tap-id mode",
                      "tap Back", "wait-idle")
                state = power()
                assert state["wakeHeld"] is True, state
                assert state["scheduledWake"] is None, state
                result["checks"].append(
                    "switching back to frame mode cancels the scheduled wake")

                stop()
                start()
                drive("wait-for Photographs", "wait-for 2 of 3", "wait-idle")
                state = power()
                assert state["wakeHeld"] is True, state
                capture("05-restarted-frame-mode")
                result["checks"].append(
                    "a restart re-applies the saved power policy and position")

                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
