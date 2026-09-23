#!/usr/bin/env python3
"""Drive Fieldbook end to end: shelf pack decode, search, outing tally,
persistence across restart, and the eBird Checklist Format export.

The shelf pack is written by a format fixture here, byte-shaped exactly as
the `kobo fieldbook` companion CLI writes it (the CLI itself is built on the
companion branch). The app side being proven is the production decode path.
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

FIXTURE = ROOT / "scripts" / "fixtures" / "fieldbook" / "packs.v1"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)

    with tempfile.TemporaryDirectory(prefix="cobalt-fieldbook-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391")
        env.pop("KOBO_SIM_OFFLINE", None)

        shelf = private / "cobalt-sim-data" / "fieldbook"
        shelf.mkdir(parents=True)
        (shelf / "packs.v1").write_bytes(FIXTURE.read_bytes())

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      shelf_fixture="format fixture; companion CLI lands on the "
                                    "companion branch (MISSINGCLI-02)",
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
                                           cwd=ROOT / "apps/fieldbook", env=env,
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
                start()
                drive("wait-for 2 packs, 10 species.")
                capture("fieldbook-home")

                # Pack list shows both packs and the honest import failure.
                drive("tap Packs", "wait-for San Francisco Bay",
                      "wait-for Bengaluru Urban", "wait-for legacy-pack.csv")
                capture("fieldbook-packs")

                # Picking a pack scopes the search; keyboard search finds it.
                drive("tap San Francisco Bay", "wait-for Search packs")
                drive("type flicker", "tap Find", "wait-for Northern Flicker")
                capture("fieldbook-search")

                # Species detail carries the scientific name and code.
                drive("tap Northern Flicker", "wait-for Colaptes auratus",
                      "wait-for NOFL")
                capture("fieldbook-detail")

                # Start an outing: name the place on the keyboard.
                drive("tap Back", "wait-for Fieldbook", "tap Start an outing",
                      "wait-for Name this place")
                drive("type lake merced", "tap Save", "wait-for lake merced")
                capture("fieldbook-outing")

                # Tally the same species twice; the row counts it.
                drive("tap American Robin", "wait-for 1 logged")
                drive("tap American Robin", "wait-for 2 logged")
                capture("fieldbook-tally")

                # Review, delete, undo.
                drive("tap Review sightings", "wait-for American Robin ×2")
                capture("fieldbook-sightings")
                drive("tap American Robin ×2", "wait-for Sighting deleted.",
                      "tap Undo delete", "wait-for American Robin ×2")

                # Log from search while the outing is open: a species beyond
                # the six-row tally cap (Bufflehead is row 7 of the pack) is
                # found by name and logged through its detail page.
                drive("tap back", "tap Resume tally", "wait-for lake merced")
                drive("tap Type a species", "wait-for Search packs")
                drive("type bufflehead", "tap Find", "wait-for Bufflehead")
                drive("tap Bufflehead", "wait-for Bucephala albeola")
                capture("fieldbook-search-detail")
                drive("tap Log in the open outing", "wait-for 2 species, 3 birds")
                capture("fieldbook-log-from-search")

                # The logged bird is on the outing and in the export.
                drive("tap Review sightings", "wait-for Bufflehead")
                capture("fieldbook-search-sighting")

                # Finish the outing; the life list keeps the species.
                drive("tap back", "tap Finish outing", "wait-for Fieldbook")
                drive("tap Life list", "wait-for AMRO", "wait-for 2 birds")
                capture("fieldbook-life")

                # Export the eBird Checklist Format CSV and read it back.
                drive("tap Export", "wait-for 1 outing", "tap Write checklist file",
                      "wait 1500")
                capture("fieldbook-export")
                exported = list(private.rglob("export-checklist.csv"))
                assert exported, "export-checklist.csv was not written to the sim store"
                csv = exported[0].read_text().splitlines()
                assert len(csv) >= 15, f"export too short: {csv}"
                assert csv[0] == ",,lake merced", f"location row: {csv[0]!r}"
                assert re.fullmatch(r",,\d{2}/\d{2}/\d{4}", csv[3]), f"date row: {csv[3]!r}"
                assert re.fullmatch(r",,\d{2}:\d{2}", csv[4]), f"start row: {csv[4]!r}"
                assert csv[8] == ",,Incidental", f"protocol row: {csv[8]!r}"
                assert csv[11] == ",,Y", f"reported row: {csv[11]!r}"
                assert "American Robin,Turdus migratorius,2" in csv, \
                    f"species row missing: {csv}"
                assert "Bufflehead,Bucephala albeola,1" in csv, \
                    f"logged-from-search row missing: {csv}"

                # State survives a restart: the finished outing is on Today.
                stop()
                start()
                drive("wait-for lake merced")
                capture("fieldbook-home-restored")

                result["checks"].append(dict(
                    name="fieldbook journey",
                    detail="pack shelf decoded, pack scoped search, keyboard location "
                           "naming, tally, delete+undo, log-from-search beyond the "
                           "six-row tally cap, life list, eBird CSV export verified "
                           "on disk, state proven across a restart",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
