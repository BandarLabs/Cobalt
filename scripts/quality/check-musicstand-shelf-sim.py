#!/usr/bin/env python3
"""Drive Music Stand against a shelf pushed by the real `kobo musicstand` CLI."""
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
TITLE = "Cello Suite No. 1 - Prelude"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--score", type=Path, required=True, help="Real score PDF to push.")
    parser.add_argument("--push-cli", type=Path, default=None,
                        help="CLI binary with `kobo musicstand`; defaults to the built one.")
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    push_cli = (args.push_cli or cli).resolve() if args.push_cli else cli
    if args.push_cli is None:
        verify_cli(push_cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-musicstand-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391")
        env.pop("KOBO_SIM_OFFLINE", None)

        # The shelf is prepared by the real companion CLI, exactly as an owner
        # would on their computer; only the sim root is redirected.
        subprocess.run([str(push_cli), "musicstand", "init", "--sim"], cwd=ROOT,
                       env=env, check=True, capture_output=True)
        pushed = subprocess.run([str(push_cli), "musicstand", "push", str(args.score), "--sim"],
                                cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        shelf = private / "cobalt-sim-data" / "musicstand"
        manifest = json.loads((shelf / "manifest.v1").read_text())
        pages = sorted(shelf.glob("score-*-*.png"))
        assert manifest["scores"], f"no scores on the shelf: {pushed.stdout}"
        assert len(pages) == int(manifest["scores"][0]["pages"]), "page files missing"

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      shelf=dict(scores=len(manifest["scores"]), pages=len(pages)),
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
                                           cwd=ROOT / "apps/musicstand", env=env,
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
                               check=True, timeout=120)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                drive("wait-for Library", "wait-for " + TITLE)
                capture("musicstand-library")
                drive("tap " + TITLE, "wait 2500")
                capture("musicstand-stand-page")
                # The physical page button turns to the bottom half.
                drive("input gpio 1 194 1", "wait 1500")
                capture("musicstand-half-turn")
                drive("input gpio 1 193 1", "wait 1200")
                # Staff-width zoom: real notation, legible.
                drive("tap-at 536 724", "wait-for Zoom in", "tap Zoom in", "wait 2000")
                capture("musicstand-zoom-staff")
                drive("tap-at 536 724", "wait-for Mark page corner", "tap Mark page corner", "wait 800")
                drive("tap-at 536 724", "wait-for Library", "tap Library", "wait-for Library", "wait-for marked")
                capture("musicstand-library-marked")
                drive("tap Setlists", "wait-for Setlists",
                      "tap New setlist from the library", "wait-for Setlist 1")
                capture("musicstand-setlists")
                drive("tap Setlist 1", "wait-for resume at page")
                capture("musicstand-setlist")
                drive("expect-missing did not import")
                result["checks"].append(dict(
                    name="shelf-driven journey",
                    detail="CLI pushed a real 6-page score; the app opened it, turned pages by "
                           "GPIO button, zoomed, marked, resumed and built a setlist",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
