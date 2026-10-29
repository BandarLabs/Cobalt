#!/usr/bin/env python3
"""Drive Music Stand against a shelf pushed by the real `kobo musicstand` CLI."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import struct
import subprocess
import tempfile
import time
import zlib

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
TITLE = "Cello Suite No. 1 - Prelude"
SECOND = "Sarabande"


def staff_page(path, width=1072, height=1448, seed=0):
    """A white page with five-line staves, enough to read as a score."""
    rows = []
    for y in range(height):
        on_staff = (y - 120 - seed) % 160 < 80 and (y - 120 - seed) % 20 == 0
        rows.append(b"\0" + (b"\x20" if on_staff else b"\xff") * width)
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))
    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(b"".join(rows)))
                     + chunk(b"IEND", b""))


def page_folder(root, title, pages, seed):
    folder = root / title
    folder.mkdir()
    for index in range(pages):
        staff_page(folder / f"page-{index + 1}.png", seed=seed + index)
    return folder


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--score", type=Path, default=None,
                        help="Real score PDF to push; without it a folder of drawn pages "
                             "is pushed, which needs no Poppler.")
    parser.add_argument("--push-cli", type=Path, default=None,
                        help="CLI binary with `kobo musicstand`; defaults to the built one.")
    parser.add_argument("--scale", default="default")
    parser.add_argument("--profile", default="clara-bw-391")
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
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE=args.profile)
        env.pop("KOBO_SIM_OFFLINE", None)

        # The shelf is prepared by the real companion CLI, exactly as an owner
        # would on their computer; only the sim root is redirected.
        if shutil.which("pdftoppm") is None:
            # Without Poppler a PDF push says what to install, before it
            # reads anything or touches the shelf.
            bogus = private / "score.pdf"
            bogus.write_bytes(b"%PDF-1.4\n")
            refused = subprocess.run([str(push_cli), "musicstand", "push", str(bogus), "--sim"],
                                     cwd=ROOT, env=env, capture_output=True, text=True)
            assert refused.returncode != 0, refused.stdout
            assert "brew install poppler" in refused.stderr, refused.stderr
            assert "poppler-utils" in refused.stderr, refused.stderr
            assert not (private / "cobalt-sim-data" / "musicstand").exists()
        score = args.score or page_folder(private, TITLE, 6, 0)
        pushed = subprocess.run([str(push_cli), "musicstand", "push", str(score), "--sim"],
                                cwd=ROOT, env=env, check=True, capture_output=True, text=True)
        assert "Now on the reader: 1 score." in pushed.stdout, pushed.stdout
        shelf = private / "cobalt-sim-data" / "musicstand"
        manifest = json.loads((shelf / "manifest.v1").read_text())
        pages = sorted(shelf.glob("score-*-*.png"))
        assert manifest["scores"], f"no scores on the shelf: {pushed.stdout}"
        assert len(pages) == int(manifest["scores"][0]["pages"]), "page files missing"

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale, profile=args.profile,
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
                drive("tap-at 536 724", "tap-id mark", "wait 800")
                drive("tap-at 536 724", "wait-for-id library", "tap-id library", "wait-for Library", "wait-for marked")
                capture("musicstand-library-marked")
                drive("expect-missing did not import")
                result["checks"].append(dict(
                    name="shelf-driven journey",
                    detail="CLI pushed a 6-page score; the app opened it, turned pages by "
                           "GPIO button, zoomed, marked and resumed",
                    status="passed"))
                # A second push while the stand is open adds a score; the
                # first stays, and the library shows both on return.
                second = page_folder(private, SECOND, 2, 7)
                added = subprocess.run([str(push_cli), "musicstand", "push", str(second), "--sim"],
                                       cwd=ROOT, env=env, check=True, capture_output=True,
                                       text=True)
                assert "Now on the reader: 2 scores." in added.stdout, added.stdout
                drive("lifecycle background", "wait 300", "lifecycle foreground",
                      "wait-for " + SECOND, "expect " + TITLE)
                capture("musicstand-library-after-push")
                result["checks"].append(dict(
                    name="second push adds and is picked up on return",
                    detail="a second score pushed while Music Stand was open appeared on "
                           "return without a restart, beside the first",
                    status="passed"))
                # A setlist is built from the library, reordered and trimmed.
                drive("tap Setlists", "wait-for Setlists", "tap New setlist",
                      "wait-for Tap a score to add it", "tap " + TITLE, "tap " + SECOND,
                      "wait-for On this setlist as number 2")
                capture("musicstand-picker")
                drive("tap Done", "wait-for Resumes at page", "expect 2. " + SECOND)
                capture("musicstand-setlist")
                drive("tap-id entry-1-menu", "wait-for Move up")
                capture("musicstand-entry-menu")
                drive("tap Move up", "wait-for 1. " + SECOND, "expect 2. " + TITLE)
                drive("tap-id entry-1-menu", "tap Remove from setlist",
                      "expect-missing 2. " + TITLE)
                capture("musicstand-setlist-edited")
                drive("tap Delete", "wait-for Delete this setlist?", "tap Delete",
                      "wait-for New setlist", "expect-missing Setlist 1")
                result["checks"].append(dict(
                    name="setlist editing",
                    detail="a new setlist was filled from the library, an entry moved up, "
                           "another removed, and the setlist deleted after a confirmation",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
