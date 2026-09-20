#!/usr/bin/env python3
"""Drive Frame against a shelf prepared by the real `kobo frame` CLI."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile
import time

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "scripts" / "fixtures" / "frame"

# Deterministic taken dates (noon UTC keeps the civil date stable).
CROP = {
    "hills-at-dawn.jpg": "2025-03-03",
    "lighthouse.jpg": "2025-06-14",
    "city-skyline.jpg": "2025-01-20",
    "mountain-lake.jpg": "2025-08-09",
}
PAD = {
    "birds-on-a-wire.jpg": "2024-11-05",
    "monstera-leaf.jpg": "2025-04-22",
}
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
          "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def taken_line(name, iso):
    day = datetime.fromisoformat(iso).replace(tzinfo=timezone.utc)
    return f"{name} · Demo Album · {day.day} {MONTHS[day.month - 1]} {day.year}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--push-cli", type=Path, default=None,
                        help="CLI binary with `kobo frame`; defaults to the built one.")
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    push_cli = (args.push_cli or cli).resolve() if args.push_cli else cli
    if args.push_cli is None:
        verify_cli(push_cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-frame-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   # A frozen clock keeps the chrome's minute tick from
                   # overpainting mid-capture; 11:11 also keeps every digit
                   # off the interface font's slashed zero.
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)

        album = private / "album"
        (album / "crop").mkdir(parents=True)
        (album / "pad").mkdir()
        for group, names in (("crop", CROP), ("pad", PAD)):
            for name, iso in names.items():
                dest = album / group / name
                shutil.copy(FIXTURES / name, dest)
                stamp = datetime.fromisoformat(iso + "T12:00:00").replace(
                    tzinfo=timezone.utc).timestamp()
                os.utime(dest, (stamp, stamp))

        # The shelf is prepared by the real companion CLI, exactly as an
        # owner would on their computer; only the sim root is redirected.
        subprocess.run([str(push_cli), "frame", "init", "--sim"], cwd=ROOT,
                       env=env, check=True, capture_output=True)
        subprocess.run([str(push_cli), "frame", "push", str(album / "crop"), "--sim",
                        "--album", "Demo Album"],
                       cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        pushed = subprocess.run([str(push_cli), "frame", "push", str(album / "pad"), "--sim",
                                 "--fit", "pad", "--album", "Demo Album"],
                                cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        shelf = private / "cobalt-sim-data" / "frame"
        manifest = (shelf / "manifest.v1").read_text()
        fit_map = dict(line.split("\t")
                       for line in (shelf / "fit.v1").read_text().splitlines())
        digest_map = dict(line.split("\t")
                          for line in (shelf / "digests.v1").read_text().splitlines())
        photos = [line.split("\t") for line in manifest.splitlines()[1:]]
        assert len(photos) == 6, f"expected 6 photos on the shelf: {pushed.stdout}"
        assert len(fit_map) == 6, f"fit sidecar lost photos: {fit_map}"
        for fields in photos:
            photo_id, _digest, _taken, photo_album, name = fields
            assert photo_album == "Demo Album", f"album override lost: {fields}"
            expected = "pad" if name in PAD else "crop"
            assert fit_map.get(photo_id) == expected, \
                f"fit for {name}: expected {expected}, got {fit_map.get(photo_id)}"
            assert len(digest_map.get(photo_id, "")) == 64, \
                f"no transfer digest recorded for {name}"

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      shelf=dict(photos=len(photos), fits=len(fit_map)),
                      checks=[])
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

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                # Oldest first: the birds panorama, pushed with --fit pad.
                drive("wait-for Photographs", "wait-for 1 of 6",
                      "expect " + taken_line("birds-on-a-wire.jpg", PAD["birds-on-a-wire.jpg"]),
                      "wait 2500", "wait-idle")
                capture("frame-home")
                drive("tap Open")
                drive("wait 2500", "wait-idle")
                capture("frame-show-pad")
                drive("tap-at 536 724", "wait-for Verified against the manifest",
                      "expect Taken", "expect 5 Nov 2024", "wait 1200", "wait-idle")
                capture("frame-facts")
                drive("tap Next", "wait 2500", "tap-at 536 724", "wait-for city-skyline.jpg", "expect 20 Jan 2025")
                capture("frame-next")
                drive("tap Exit", "wait-for Photographs")
                drive("tap-at 536 724", "wait-for Settings", "expect Frame mode")
                capture("frame-settings")
                drive("tap-at 536 724", "wait-for Photographs")
                result["checks"].append(dict(
                    name="shelf-driven journey",
                    detail="CLI pushed a 6-photo album in two passes (crop then pad); the "
                           "app listed the album with count, name and taken date, showed "
                           "the pad-fit panorama letterboxed, reported the photo verified "
                           "against the transfer manifest, advanced to the next photo and "
                           "opened settings",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
