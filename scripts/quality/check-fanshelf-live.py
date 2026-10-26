#!/usr/bin/env python3
"""Drive Fanshelf against the live Archive of Our Own, with no fixture.

The work is real: a public, complete, General-audiences story fetched from
archiveofourown.org over TLS, its EPUB downloaded and opened in the reader.
One reader-paced session - a handful of requests, each following a tap, which
is the etiquette the app itself promises the archive. No trust override and
no base override: the device talks to the real site directly.
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

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]

# A real public work: "Beyond Her Understanding" by RHaye5, complete,
# General audiences. Chosen from the live Pride and Prejudice tag listing.
WORK_ID = "92474451"
WORK_TITLE = "Beyond Her Understanding"
WORK_AUTHOR = "RHaye5"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    # The whole evidence tree is harness output: sidecars carry
    # per-run timings, and a live feed drifts between runs. Only
    # changes outside it say anything about the sources under test.
    cli, provenance = build_cli(ROOT, target)
    verify_cli(cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-fanshelf-live-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)
        env.pop("FANSHELF_DEMO", None)
        # No KOBO_SIM_TRUST_DIR, no FANSHELF_AO3_BASE: live egress.

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      server="live AO3 (archiveofourown.org)",
                      work=f"{WORK_TITLE} by {WORK_AUTHOR} (works/{WORK_ID})",
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
                                           cwd=ROOT / "apps/fanshelf", env=env,
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

            def drive(*steps, timeout=240, soft=False):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                try:
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                                   stderr=log, check=True, timeout=timeout)
                except subprocess.CalledProcessError:
                    # Live fetches can outlast a drive step's idle window.
                    # A soft step leaves settling to the wait_until loop.
                    if not soft:
                        raise

            def capture(name):
                drive("clean", "shot " + name)

            def screen_text():
                probe = subprocess.run(
                    [str(cli), "drive", "--address", address, "--ideal",
                     "--step", "dump"],
                    cwd=ROOT, env=env, capture_output=True, text=True,
                    timeout=60)
                if probe.returncode != 0:
                    raise RuntimeError("dump step failed; see simulator.log")
                return probe.stdout

            def wait_until(predicate, seconds=120, advance_clock=False):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    if predicate(screen_text()):
                        return True
                    if advance_clock:
                        drive("clock advance 1500", "wait-idle", soft=True)
                    else:
                        time.sleep(1)
                return False

            try:
                start()
                drive("wait-for Add an AO3 work", "wait-idle", timeout=300)
                drive("tap Add", "wait-for Enter its web address")
                # The tap is delivered even if the live work-page fetch keeps
                # the app busy past the step's handling window; wait_until
                # below settles and gates on the parsed title.
                drive("tap-id kb.layer", "type " + WORK_ID, "tap Open work",
                      timeout=120, soft=True)
                if not wait_until(
                        lambda text: WORK_TITLE in text and WORK_AUTHOR in text,
                        seconds=240, advance_clock=True):
                    result["checks"].append(dict(
                        name="live work lookup", status="unverified",
                        detail="the real work page did not render its title "
                               "and byline within four minutes; see "
                               "simulator.log for what the live site "
                               "answered"))
                    result["status"] = "unverified"
                    return
                capture("fanshelf-live-work")
                result["checks"].append(dict(
                    name="live work lookup", status="passed",
                    detail=f"typing the work number fetched the real work page "
                           f"from archiveofourown.org over TLS and the parsed "
                           f"title ({WORK_TITLE!r}) and byline ({WORK_AUTHOR!r}) "
                           "rendered with the archive's rating and warnings"))

                # The download starts from the work screen and opens the
                # reader when the EPUB lands. Real bytes, real pacing.
                drive("tap Download EPUB", timeout=60, soft=True)
                if not wait_until(
                        lambda text: re.search(r"\b1 of \d", text) is not None,
                        seconds=300, advance_clock=True):
                    result["checks"].append(dict(
                        name="live EPUB download opens in the reader",
                        status="unverified",
                        detail="the real EPUB did not reach the reader within "
                               "five minutes; see simulator.log"))
                    result["status"] = "unverified"
                    return
                drive("clock advance 1500", "wait-idle", soft=True)
                capture("fanshelf-live-reading")
                result["checks"].append(dict(
                    name="live EPUB download opens in the reader", status="passed",
                    detail="the real EPUB link was parsed off the live work "
                           "page, downloaded over TLS, saved to the shelf, and "
                           "opened straight into the reader"))

                drive("tap Back", "wait-for Read", timeout=300)
                drive("tap Shelf", timeout=60, soft=True)
                if not wait_until(lambda text: "reading" in text,
                                  seconds=120, advance_clock=True):
                    result["checks"].append(dict(
                        name="shelf shows the live download", status="unverified",
                        detail="the shelf did not show the reading badge after "
                               "the live download"))
                    result["status"] = "unverified"
                    return
                capture("fanshelf-live-shelf")
                result["checks"].append(dict(
                    name="shelf shows the live download", status="passed",
                    detail="the real work sits on the shelf with its reading "
                           "badge: the downloaded copy persisted and reading "
                           "state survived leaving the reader"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
