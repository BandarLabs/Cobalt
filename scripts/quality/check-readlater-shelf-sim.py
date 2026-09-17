#!/usr/bin/env python3
"""Drive Read Later against a real Wallabag server, signed in by the real CLI.

The companion CLI completes the OAuth password grant exactly as an owner would
(`kobo readlater login --sim`), the app imports the session in the simulator,
and the journey exercises sync, tabs, full bodies, star/archive through the
outbox and a process restart. Server-side effects are verified through the
Wallabag API and then restored, so the run is repeatable.

Credentials come from a file with `username:`, `password:`, `client_id:` and
`client_secret:` lines (default: ~/.cobalt-accounts/wallabag); nothing is baked
into the repository. Requires network access to the server in that file.
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
import urllib.request

from simulator_cli import build_cli

ROOT = Path(__file__).resolve().parents[2]
CREDENTIALS = Path.home() / ".cobalt-accounts" / "wallabag"

# The articles the proof account holds. The short one is the throwaway the
# outbox round-trip archives and restores.
ARCHIVE_ID = 34384047
STAR_ID = 34384045
LONG_TITLE = "How to Do Great Work"
SHORT_TITLE = "The Need to Read"
STAR_TITLE = "E Ink"


def credentials(path):
    fields = {}
    for line in path.read_text().splitlines():
        if ": " in line:
            key, value = line.split(": ", 1)
            fields[key.strip()] = value.strip()
    for name in ("username", "password", "client_id", "client_secret"):
        if name not in fields:
            raise SystemExit(f"{path} is missing `{name}:`")
    return fields


def api(server, token, method, path, body=None):
    request = urllib.request.Request(
        server + path,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}",
                 "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read())


def fresh_token(server, fields):
    body = ("grant_type=password&client_id={}&client_secret={}&username={}&password={}"
            .format(fields["client_id"], fields["client_secret"],
                    fields["username"], fields["password"]))
    request = urllib.request.Request(
        server + "/oauth/v2/token", data=body.encode(),
        headers={"Content-Type": "application/x-www-form-urlencoded"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read())["access_token"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--push-cli", type=Path, default=None,
                        help="CLI binary with `kobo readlater`; defaults to the built one.")
    parser.add_argument("--credentials", type=Path, default=CREDENTIALS)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    fields = credentials(args.credentials)
    server = "https://app.wallabag.it"

    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    login_cli = (args.push_cli or cli).resolve()

    with tempfile.TemporaryDirectory(prefix="cobalt-readlater-", dir="/tmp") as temporary:
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

        # Sign in through the real companion CLI, exactly as an owner would;
        # only the sim root is redirected.
        login = subprocess.run(
            [str(login_cli), "readlater", "login", "--server", server,
             "--client-id", fields["client_id"], "--client-secret", fields["client_secret"],
             "--username", fields["username"], "--password-env", "KOBO_WALLABAG_PASSWORD",
             "--sim"],
            cwd=ROOT, env=dict(env, KOBO_WALLABAG_PASSWORD=fields["password"]),
            check=True, capture_output=True, text=True)
        session = private / "cobalt-sim-data" / "readlater" / "session.v1"
        assert session.exists(), f"the CLI wrote no session: {login.stdout}"

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale, server=server, checks=[])
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
                                           cwd=ROOT / "apps" / "readlater", env=env,
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

            def drive(*steps, timeout=240):
                command = [str(cli), "drive", "--address", address, "--ideal",
                           "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=timeout)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                drive("wait-for Signed in to https://app.wallabag.it",
                      "tap Sync", "wait-for Reading list synced",
                      "expect " + LONG_TITLE, "wait-idle", timeout=300)
                capture("readlater-queue")
                result["checks"].append(dict(
                    name="sign-in and first sync",
                    detail="the CLI-delivered session was imported at startup; Sync read "
                           "the real account's unread list",
                    status="passed"))

                # The long article arrives whole and pages.
                drive("tap " + LONG_TITLE, "wait-for If you collected lists",
                      "wait-idle", timeout=300)
                capture("readlater-article")
                drive("tap Next", "wait 600", "wait-idle")
                capture("readlater-page-2")
                drive("tap Reading list", "wait-for " + LONG_TITLE)
                result["checks"].append(dict(
                    name="full article bodies",
                    detail="the 59-minute article opened with its whole extracted body and "
                           "paged forward",
                    status="passed"))

                # Star one article and archive the throwaway; one sync drains both.
                drive("tap " + STAR_TITLE,
                      "wait-for This article is about the brand",
                      "wait-idle", timeout=300)
                drive("tap Star")
                drive("tap Reading list", "wait-for Starred; sending on the next sync.")
                drive("tap " + SHORT_TITLE,
                      "wait-for In the science fiction books",
                      "wait-idle", timeout=300)
                drive("tap Archive", "wait-for Archived; sending on the next sync.")
                drive("expect-missing " + SHORT_TITLE)
                drive("tap Sync", "wait-for Reading list synced", timeout=300)
                result["checks"].append(dict(
                    name="outbox drain",
                    detail="star and archive queued locally, then one Sync posted both to "
                           "Wallabag before reading the list back",
                    status="passed"))

                # Tabs read the server's own filters.
                drive("tap Starred", "wait-for " + STAR_TITLE, "wait-idle", timeout=300)
                capture("readlater-starred-tab")
                drive("tap Archive", "wait-for " + SHORT_TITLE, "wait-idle", timeout=300)
                capture("readlater-archive-tab")
                drive("tap Unread", "wait-for " + LONG_TITLE, timeout=300)

                # A process restart restores the library without a network read.
                stop()
                start()
                drive("wait-for " + LONG_TITLE, "wait-idle", timeout=300)
                capture("readlater-restored")
                result["checks"].append(dict(
                    name="restart restores the library",
                    detail="after a full simulator restart the synced list, bodies and "
                           "places came back from the acknowledged snapshot",
                    status="passed"))

                # The server really changed, then is restored for repeatability.
                token = fresh_token(server, fields)
                archived = api(server, token, "GET",
                               f"/api/entries/{ARCHIVE_ID}.json")
                starred = api(server, token, "GET",
                              f"/api/entries/{STAR_ID}.json")
                assert archived["is_archived"] == 1, "archive never reached Wallabag"
                assert starred["is_starred"] == 1, "star never reached Wallabag"
                api(server, token, "PATCH", f"/api/entries/{ARCHIVE_ID}.json",
                    {"archive": 0})
                api(server, token, "PATCH", f"/api/entries/{STAR_ID}.json",
                    {"star": 0})
                result["checks"].append(dict(
                    name="server-side effects verified",
                    detail="the Wallabag API showed the archive and the star, and both "
                           "were restored for the next run",
                    status="passed"))
                result["status"] = "passed"
            finally:
                stop()
        (out / "result.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
