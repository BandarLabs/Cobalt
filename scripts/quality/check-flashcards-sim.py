#!/usr/bin/env python3
"""Drive Flashcards end to end in the simulator, offline: the first-use
screen on an empty shelf, starting the built-in sample deck, a full review
session (reveal and grade every card, including the Japanese and long cards),
persistence of the local review log across a simulator restart, and then a
deck prepared and staged by the real companion CLI, which keeps the earlier
review log beside the replaced collection.

Flashcards never connects; there is no fixture server and no network of any
kind. The companion half runs the real flashcards-import helper on an
original generated study package, exactly the commands the first-use screen
names.
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


def build_import_tools(env):
    # The host importer is its own workspace on a newer toolchain.
    build_env = dict(env)
    build_env.pop("RUSTUP_TOOLCHAIN", None)
    build = subprocess.run(
        ["cargo", "+1.88.0", "build", "--locked",
         "--manifest-path", "crates/kobo-flashcards-import/Cargo.toml",
         "--example", "quality_fixture", "--message-format=json"],
        cwd=ROOT, env=build_env, stdout=subprocess.PIPE, text=True, check=True)
    helper = fixture = None
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if artifact.get("reason") != "compiler-artifact":
            continue
        executable = artifact.get("executable")
        if not executable:
            continue
        name = artifact.get("target", {}).get("name")
        if name == "flashcards-import":
            helper = Path(executable).resolve()
        elif name == "quality_fixture":
            fixture = Path(executable).resolve()
    if helper is None or fixture is None:
        raise RuntimeError("Cargo did not identify the helper and fixture executables")
    return helper, fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)

    with tempfile.TemporaryDirectory(prefix="cobalt-flashcards-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_OFFLINE="1")
        shelf = private / "cobalt-sim-data" / "flashcards"
        log_path = shelf / "cobalt-review-log.ndjson"
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
                                           cwd=ROOT / "apps/flashcards", env=env,
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

            def drive(*steps, timeout=180):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=timeout)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                # First use on an empty shelf offers the sample deck.
                start()
                drive("wait-for No collection yet")
                capture("flashcards-first-use")
                drive("wait-for kobo flashcards stage collection.cobfc")
                drive("tap Start with the sample", "wait-for Getting started")
                capture("flashcards-sample-decks")
                result["checks"].append(dict(
                    name="first use and sample deck",
                    detail="an empty shelf offered the sample deck and named the companion "
                           "commands; starting the sample saved it and listed its deck",
                    status="passed"))

                # Review all six sample cards: reveal, grade Good, next card.
                drive("tap-id deck-0", "wait-for compass needle")
                capture("flashcards-question")
                drive("tap-id answer", "wait-for Magnetic north.")
                capture("flashcards-answer")
                drive("tap-id good", "wait 800", "wait-for vapour")
                drive("tap-id answer", "wait-for Evaporation.", "tap-id good",
                      "wait 800", "wait-for hexagon")
                drive("tap-id answer", "wait-for Six.", "tap-id good",
                      "wait 800")
                drive("wait-for こんにちは")
                capture("flashcards-japanese")
                drive("tap-id answer", "wait-for konnichiwa")
                capture("flashcards-japanese-answer")
                drive("tap-id good", "wait 800", "wait-for closest to the Sun")
                drive("tap-id answer", "wait-for Mercury.", "tap-id good",
                      "wait 800", "wait-for moons of Jupiter")
                drive("tap-id answer", "wait-for Ganymede")
                capture("flashcards-long-answer")
                drive("tap-id good", "wait 1000", "wait-for Review complete")
                capture("flashcards-complete")
                result["checks"].append(dict(
                    name="reveal and grade every card",
                    detail="all six sample cards revealed and graded Good, including the "
                           "Japanese greeting and the long multi-sentence answer, ending on "
                           "the Review complete screen",
                    status="passed"))

                # The local review log survives a restart beside the collection.
                assert log_path.is_file(), "no review log was saved on the shelf"
                records = [json.loads(line) for line in log_path.read_text().splitlines()]
                assert len(records) == 6, f"expected 6 saved reviews, found {len(records)}"
                assert all(r["grade"] == "good" for r in records)
                digests = {r["bundle_sha256"] for r in records}
                assert len(digests) == 1, "sample reviews name more than one collection"
                stop()
                start()
                drive("wait-for Getting started")
                capture("flashcards-restored")
                result["checks"].append(dict(
                    name="review log persistence",
                    detail="six Good reviews were appended to cobalt-review-log.ndjson "
                           "beside the collection and the deck still opens after a "
                           "simulator restart",
                    status="passed"))

                # Stage a deck prepared by the real companion CLI over the sample.
                stop()
                helper, fixture = build_import_tools(env)
                cli_env = dict(env, KOBO_FLASHCARDS_IMPORT=str(helper))
                package = private / "original study cards.apkg"
                bundle = private / "collection.cobfc"
                subprocess.run([str(fixture), str(package)], check=True,
                               capture_output=True, timeout=30)
                def run(*arguments):
                    completed = subprocess.run(
                        [str(cli), "flashcards", *map(str, arguments)],
                        env=cli_env, capture_output=True, text=True, timeout=60)
                    assert completed.returncode == 0, completed.stderr
                    return completed
                run("import", package, "--merge", bundle)
                run("verify", bundle)
                shelf.mkdir(parents=True, exist_ok=True)
                log_bytes = log_path.read_bytes()
                (shelf / "collection.cobfc").write_bytes(bundle.read_bytes())
                start()
                drive("wait-for Default")
                capture("flashcards-imported")
                drive("tap-id deck-0", "wait-for compass point toward")
                drive("tap-id answer", "wait-for Magnetic north.", "tap-id good",
                      "wait 800")
                grown = [json.loads(line)
                         for line in log_path.read_text().splitlines()]
                assert len(grown) == 7, f"expected 7 saved reviews, found {len(grown)}"
                assert {r["bundle_sha256"] for r in grown} == \
                    digests | {grown[-1]["bundle_sha256"]}
                assert log_path.read_bytes()[:len(log_bytes)] == log_bytes, \
                    "restaging rewrote the earlier review log"
                capture("flashcards-imported-answer")
                result["checks"].append(dict(
                    name="companion import over the sample",
                    detail="the real flashcards-import helper converted an original "
                           "three-card package, the CLI verified it, the staged bundle "
                           "replaced the sample collection and one more Good review "
                           "appended to the untouched earlier log",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
