#!/usr/bin/env python3
"""Capture Inkling callbacks through SDK IPC on all nine simulator profiles.

Build kobo-cli first, then pass an output directory. Uses only isolated,
synthetic game state. Includes no hardware or external service checks.
"""

import hashlib, json, os, re, signal, subprocess, sys, tempfile, time, urllib.request
from pathlib import Path

root = Path(__file__).resolve().parents[4]
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
profiles = [
    "clara-bw-391",
    "clara-bw-395",
    "clara-hd-376",
    "clara-colour-393",
    "elipsa-2e-389",
    "libra-2-388",
    "libra-colour-390",
    "libra-colour-390-4.46.23836",
    "libra-h2o-384",
]
cli = root / "target/debug/kobo"
for profile in profiles:
    d = out / profile
    d.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="inkling-review-") as tmp, (
        d / "simulator.log"
    ).open("w") as log:
        env = dict(
            os.environ,
            TMPDIR=tmp,
            KOBO_SIM_PROFILE=profile,
            KOBO_TEXT_SCALE="155",
            KOBO_INKLING_DAY="2026-09-01",
            KOBO_SIM_CLOCK_MILLIS="1788220800000",
            KOBO_SIM_UTC_OFFSET_MINUTES="0",
        )
        p = subprocess.Popen(
            [str(cli), "dev", "127.0.0.1:0"],
            cwd=root / "apps/inkling",
            env=env,
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                m = re.search(
                    r"Kobo app simulator: http://(127\.0\.0\.1:\d+)",
                    (d / "simulator.log").read_text(),
                )
                if m:
                    break
                if p.poll() is not None:
                    raise RuntimeError((d / "simulator.log").read_text())
                time.sleep(0.1)
            else:
                raise RuntimeError("start timeout")
            address = m[1]

            def drive(*steps):
                cmd = [
                    str(cli),
                    "drive",
                    "--address",
                    address,
                    "--ideal",
                    "--shots",
                    str(d),
                ]
                for step in steps:
                    cmd += ["--step", step]
                subprocess.run(
                    cmd,
                    cwd=root,
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                    timeout=90,
                )

            def capture(name):
                drive("wait-idle", "clean", "shot " + name)
                for ep in ["diagnostics", "layout"]:
                    with urllib.request.urlopen("http://" + address + "/" + ep) as r:
                        (d / (name + "." + ep + ".json")).write_bytes(r.read())

            drive("wait-for Inkling · Sep 1")
            capture("board")
            drive("tap How to play", "wait-for Find the five-letter word")
            capture("help")
            drive("tap Play", "tap Enter guess", "type cra")
            capture("typing")
            drive("tap Back")
            capture("typing-back")
            drive("tap Stats")
            capture("stats")
            drive("tap Export results", "wait-for Saved as export-result.txt.")
            capture("export-saved")
            exports = list(Path(tmp).rglob("export-result.txt"))
            assert exports
            (d / "export-result.txt").write_bytes(exports[0].read_bytes())
            print(profile + " captured", flush=True)
        finally:
            os.killpg(p.pid, signal.SIGTERM)
            p.wait(timeout=15)
(out / "provenance.json").write_text(
    json.dumps(
        {
            "type": "full simulator SDK IPC and native renderer; hardware untested",
            "source_revision": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=root, text=True
            ).strip(),
            "app_source_sha256": hashlib.sha256(
                (root / "apps/inkling/src/main.rs").read_bytes()
            ).hexdigest(),
            "cli_sha256": hashlib.sha256(cli.read_bytes()).hexdigest(),
            "scale": 155,
            "profiles": profiles,
        },
        indent=2,
    )
    + "\n"
)
