#!/usr/bin/env python3
"""Drive offline computer games and their controls at two text sizes.

Covers the setup screen (side and clock), a clocked game as Black where the
computer opens, resign with New game, and an unclocked game as White: no
clock chips, no repaint while nobody moves, the last move outlined on both
of its squares, no dialog over the board after a move, and Undo.

This checks presentation only; app tests cover request recovery, and live
matchmaking and hardware acceptance remain separate.
"""

import argparse
import json
import os
import re
import signal
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from simulator_cli import build_cli, fingerprint, verify_cli

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--cli", type=Path,
                    help="use this kobo binary instead of building one; for a shared target "
                         "directory that other builds may rewrite during the run")
args = parser.parse_args()
target = Path(os.environ.get("CARGO_TARGET_DIR", str(root / "target"))).resolve()
if args.cli is None:
    cli, provenance = build_cli(root, target)
else:
    cli = args.cli.resolve()
    provenance = dict(
        source_revision=subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        prebuilt=str(cli),
        cli_sha256=fingerprint(cli),
    )


def action_id(name):
    value = 2166136261
    for byte in name.encode():
        value = ((value ^ byte) * 16777619) & 0xffffffff
    return value


SQUARES = {action_id(f"square-{file}{rank}"): f"{file}{rank}"
           for file in "abcdefgh" for rank in range(1, 9)}


def outlined(layout):
    """Squares drawn with the selection outline, which also marks the last move."""
    marked = set()
    for node in layout["nodes"]:
        found = re.match(r"Cell\(ActionId\((\d+)\), Board(?:Dark)?, true\)", node["kind"])
        if found:
            marked.add(SQUARES[int(found.group(1))])
    return marked


def first_square(layout):
    for node in layout["nodes"]:
        found = re.match(r"Cell\(ActionId\((\d+)\), Board", node["kind"])
        if found:
            return SQUARES[int(found.group(1))]
    raise AssertionError("no board on screen")


def has_overlay(layout):
    return any(node["kind"] == "Overlay" for node in layout["nodes"])


def clock_chips(layout):
    names = {action_id("your-clock"): "your-clock", action_id("opponent-clock"): "opponent-clock"}
    chips = {}
    for node in layout["nodes"]:
        found = re.match(r"Chip\(ActionId\((\d+)\), (true|false)\)", node["kind"])
        if found and int(found.group(1)) in names:
            chips[names[int(found.group(1))]] = found.group(2) == "true"
    return chips


for scale in ["default", "170"]:
    out = args.output.resolve() / scale
    out.mkdir(parents=True, exist_ok=True)
    with (
        tempfile.TemporaryDirectory(prefix="cb-pair-", dir="/tmp") as temporary,
        (out / "simulator.log").open("w") as log,
    ):
        env = dict(
            os.environ,
            TMPDIR=temporary,
            RUSTUP_TOOLCHAIN="1.85.1",
            CARGO_TARGET_DIR=str(target),
            CARGO_PROFILE_DEV_DEBUG="0",
            CARGO_INCREMENTAL="0",
            CARGO_BUILD_JOBS="2",
            KOBO_SIM_PROFILE="clara-bw-391",
            KOBO_TEXT_SCALE=scale,
            KOBO_SIM_OFFLINE="1",
        )
        env.pop("KOBO_LICHESS_DEMO", None)
        env.pop("KOBO_SIM_DATA", None)
        proc = subprocess.Popen(
            [str(cli), "dev", "127.0.0.1:0"],
            cwd=root / "apps/lichess",
            env=env,
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 300
            address = None
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    raise RuntimeError("simulator exited")
                match = re.search(
                    r"Kobo app simulator: http://(127\.0\.0\.1:\d+)",
                    (out / "simulator.log").read_text(),
                )
                if match:
                    address = match.group(1)
                    try:
                        with urllib.request.urlopen(
                            "http://" + address + "/layout", timeout=2
                        ) as response:
                            layout = json.load(response)
                        if "Computer" in json.dumps(layout):
                            break
                    except OSError:
                        pass
                time.sleep(0.3)
            else:
                raise RuntimeError("no home screen")

            def drive(*steps, script=None):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                if script is not None:
                    command += ["--script", str(script)]
                for step in steps:
                    command += ["--step", step]
                subprocess.run(command, cwd=root, env=env, stdout=log, stderr=log,
                               check=True, timeout=60)

            def layout():
                with urllib.request.urlopen("http://" + address + "/layout", timeout=5) as response:
                    return json.load(response)

            # The committed route: setup, a White game, resume, resign, and a
            # clocked game as Black that the computer opens.
            drive(script=root / "apps/lichess/drive-computer.kobo")
            clocked = layout()
            assert first_square(clocked) == "h1", "Black did not get the board turned to Black"
            assert clock_chips(clocked) == {"your-clock": True, "opponent-clock": False}, clocked
            assert outlined(clocked) == {"b1", "c3"}, outlined(clocked)
            before = clocked["paints"]
            time.sleep(2.5)
            assert layout()["paints"] > before, "a clocked game stopped repainting its clock"

            drive("tap-id game-menu", "wait-for Resign", "tap-id confirm-resign",
                  "wait-for Resign?", "tap-id resign", "wait-for New game",
                  "expect See the board", "clean", "shot resigned-new-game")
            assert has_overlay(layout())
            drive("tap-id computer-again", "wait-for Start game", "expect No clock",
                  "tap-id computer-side-white", "tap-id computer-clock-none",
                  "tap-id start-computer", "wait-for Kobo", "expect-missing 10:00",
                  "clean", "shot untimed-white-start")
            untimed = layout()
            assert clock_chips(untimed) == {}, "an unclocked game showed clock chips"
            assert first_square(untimed) == "a8"
            assert outlined(untimed) == set(), outlined(untimed)
            drive("tap-id square-e2", "expect e2 →", "tap-id square-e4",
                  "wait-for White · Your move · Casual · Nf6", "expect-missing e2 →",
                  "clean", "shot untimed-white-reply")
            replied = layout()
            assert not has_overlay(replied), "a move raised a dialog"
            assert outlined(replied) == {"g8", "f6"}, outlined(replied)
            before = replied["paints"]
            time.sleep(2.5)
            assert layout()["paints"] == before, "an unclocked board repainted while nobody moved"
            drive("tap-id take-back", "wait-for White · Your move · Casual · none",
                  "clean", "shot untimed-white-undo")
            assert outlined(layout()) == set(), "Undo left the old move outlined"

            for endpoint in ["layout", "diagnostics"]:
                with urllib.request.urlopen(
                    "http://" + address + "/" + endpoint, timeout=5
                ) as response:
                    data = json.load(response)
                (out / (endpoint + ".json")).write_text(
                    json.dumps(data, indent=2) + "\n"
                )
                if endpoint == "diagnostics":
                    assert not [
                        issue
                        for issue in data.get("issues", [])
                        if issue["severity"] == "error"
                    ], data
            verify_cli(cli, provenance)
            (out / "result.json").write_text(
                json.dumps(
                    dict(
                        build=provenance,
                        scale=scale,
                        profile="clara-bw-391",
                        status="pass",
                        checks=[
                            "setup offers side and clock",
                            "playing Black turns the board and the computer opens",
                            "clocked game shows both clocks and keeps repainting",
                            "resign offers New game back to setup",
                            "unclocked game shows no clocks and does not repaint idle",
                            "last move outlined on both squares",
                            "no dialog after a move",
                            "Undo takes back the move and the reply",
                        ],
                        scope="Offline computer game, with no credentials; not live matchmaking or physical hardware validation",
                    ),
                    indent=2,
                )
                + "\n"
            )
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=10)
    print(scale, "passed", flush=True)
