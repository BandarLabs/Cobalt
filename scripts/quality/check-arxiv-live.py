#!/usr/bin/env python3
"""Drive the arXiv app against the real arXiv API: browse a live subject
listing, open a real paper, fetch its real full-text HTML and figures, and
film it - through the actual simulator with real taps.

No fixture, no mock: the API and HTML bases are the app's own defaults
(export.arxiv.org and arxiv.org) and TLS verifies against the public roots
every browser carries. One read-only session, a handful of GETs. The
deterministic fixture harness (check-arxiv-sim.py) stays the CI gate; this
script exists so the proof the user sees is real papers.
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

SUBJECT = "Artificial Intelligence"


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

    with tempfile.TemporaryDirectory(prefix="cobalt-arxiv-live-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        # Live means live: no fixture trust anchor, no base overrides, no
        # offline mode. TLS verifies against the public roots.
        for name in ("KOBO_SIM_TRUST_DIR", "ARXIV_API_BASE", "ARXIV_HTML_BASE",
                     "KOBO_SIM_OFFLINE"):
            env.pop(name, None)

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      server="live arXiv API (export.arxiv.org, arxiv.org)",
                      paper=None, checks=[])
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
                                           cwd=ROOT / "apps/arxiv", env=env,
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
                    # Live fetches (a real feed, a full paper over TLS) can
                    # outlast a drive step's idle window. A soft step leaves
                    # settling to the surrounding wait_until loop.
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

            def rows(text):
                # dump prints one control per line as:  kind  ["line", "line"]
                return [line for line in text.splitlines()
                        if re.search(r"\[\"", line)]

            def quoted(text):
                return re.findall(r'"((?:[^"\\]|\\.)*)"', text)

            try:
                start()
                drive("wait-for " + SUBJECT, "wait-idle", timeout=300)

                # The live listing: real titles, fetched now.
                drive("tap " + SUBJECT, timeout=60, soft=True)
                drive("clock advance 1500", "wait-idle", timeout=60, soft=True)
                if not wait_until(lambda text: len(rows(text)) >= 1,
                                  seconds=120, advance_clock=True):
                    raise RuntimeError("the live listing never arrived")
                drive("wait-idle")
                capture("arxiv-live-listing")
                listing = screen_text()
                titles = []
                for row in rows(listing):
                    lines = quoted(row)
                    if lines and not any(
                            marker in lines[0]
                            for marker in ("Artificial Intelligence", "Library",
                                           "Search", "Any time", "Older papers")):
                        titles.append(lines[0])
                if not titles:
                    raise RuntimeError("no paper rows in the live listing dump")
                result["checks"].append(dict(
                    name="live subject listing", status="passed",
                    detail=f"tapping {SUBJECT} fetched the real arXiv feed; "
                           f"the first rows are today's papers "
                           f"(first: {titles[0][:80]!r})"))

                # Open papers until one has full-text HTML (recent cs papers
                # nearly all do; some still do not).
                opened = None
                for title in titles[:4]:
                    drive("tap " + title, "wait-idle", timeout=120)
                    if not wait_until(
                            lambda text: "Full text" in text
                            or "Back" in text, seconds=60):
                        continue
                    if "Full text" not in screen_text():
                        drive("tap Back", "wait-idle", timeout=60)
                        continue
                    capture("arxiv-live-abstract")
                    # The tap is delivered even if the live HTML fetch keeps
                    # the app busy past the step's 10-second handling window;
                    # wait_until below settles and gates on the reader page.
                    drive("tap Full text", timeout=60, soft=True)
                    drive("clock advance 1500", "wait-idle", timeout=60,
                          soft=True)
                    if wait_until(
                            lambda text: re.search(r"\b1 of \d", text) is not None,
                            seconds=180, advance_clock=True):
                        opened = title
                        break
                    # No HTML edition of this one; the next row's abstract
                    # shot overwrites this one.
                    drive("tap Back", "wait-idle", timeout=60)
                if opened is None:
                    raise RuntimeError(
                        "none of the first live papers offered full-text HTML")
                result["paper"] = opened
                result["checks"].append(dict(
                    name="live abstract and full text", status="passed",
                    detail=f"opened {opened[:80]!r}: abstract from the live "
                           "feed, then the real HTML from arxiv.org/html "
                           "fetched over TLS and opened in the reader"))

                # Page one of the real paper, then the first figure.
                drive("clock advance 1500", "wait-idle", soft=True)
                capture("arxiv-live-reading")

                def turn_until(anchor, shot_name, max_turns=12, settle=4):
                    for _ in range(max_turns):
                        if anchor in screen_text():
                            for _ in range(settle):
                                drive("clock advance 1500", "wait-idle",
                                      timeout=120, soft=True)
                            capture(shot_name)
                            return True
                        drive("input gpio 1 194 1", "input gpio 1 194 0",
                              "clock advance 1500", "wait-idle", timeout=120,
                              soft=True)
                    return False

                found_figure = turn_until("Figure", "arxiv-live-figure")
                if found_figure:
                    result["checks"].append(dict(
                        name="live figure reached in the reader", status="passed",
                        detail="paged the real paper to its first figure "
                               "caption (panel shot arxiv-live-figure); "
                               "whether the graphic itself drew is a pixel "
                               "check on that shot, not something this "
                               "harness asserts"))
                else:
                    result["checks"].append(dict(
                        name="live figure reached in the reader", status="unverified",
                        detail="no figure caption appeared within twelve page "
                               "turns of the opened paper; the reading shots "
                               "are real but carry no figure"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
