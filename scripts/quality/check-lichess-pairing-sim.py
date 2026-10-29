#!/usr/bin/env python3
"""Capture the offline Lichess pairing screens at two text sizes.

Scenarios: a seek checking its games, a failed check, and a casual seek,
whose pairing screen must say Casual and nowhere Rated.

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
parser.add_argument("--scenario", choices=["reconciling", "pairing-error", "pairing-casual"],
                    default="reconciling")
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
READY = {
    "reconciling": "Checking games",
    "pairing-error": "Could not check your game",
    "pairing-casual": "Rapid · Casual",
}
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
            KOBO_LICHESS_DEMO=args.scenario,
            KOBO_SIM_PROFILE="clara-bw-391",
            KOBO_TEXT_SCALE=scale,
            KOBO_SIM_OFFLINE="1",
        )
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
                        if READY[args.scenario] in json.dumps(layout, ensure_ascii=False):
                            break
                    except OSError:
                        pass
                time.sleep(0.3)
            else:
                raise RuntimeError("no pairing screen")
            subprocess.run(
                [
                    str(cli),
                    "drive",
                    "--address",
                    address,
                    "--ideal",
                    "--script",
                    str(root / "apps/lichess/drive" / (args.scenario + ".kobo")),
                    "--shots",
                    str(out),
                ],
                cwd=root,
                env=env,
                stdout=log,
                stderr=log,
                check=True,
                timeout=40,
            )
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
                        scenario=args.scenario,
                        scale=scale,
                        profile="clara-bw-391",
                        status="pass",
                        scope="Offline demo of pairing recovery guidance; not live matchmaking or physical hardware validation",
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
