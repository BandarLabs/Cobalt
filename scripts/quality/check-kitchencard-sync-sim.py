#!/usr/bin/env python3
"""Sync Kitchen Card against a local TLS fixture serving real Mealie demo payloads."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import ssl
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
HOST = "demo.mealie.io"
FIXTURES = ROOT / "apps/kitchencard/fixtures"
TOKEN = "kitchencard-fixture-token"
SLUGS = ["lemon-herb-roasted-chicken", "tomato-basil-pasta", "overnight-oats-with-berries"]


def certificate(private):
    trust = private / "trust"
    trust.mkdir()
    (private / "extensions").write_text(f"subjectAltName=DNS:{HOST}\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
    commands = [
        ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=Cobalt local fixture CA", "-keyout", str(private / "ca.key"), "-out", str(trust / "ca.pem")],
        ["req", "-newkey", "rsa:2048", "-nodes", "-subj", f"/CN={HOST}", "-keyout", str(private / "server.key"), "-out", str(private / "server.csr")],
        ["x509", "-req", "-in", str(private / "server.csr"), "-CA", str(trust / "ca.pem"), "-CAkey", str(private / "ca.key"), "-CAcreateserial", "-days", "2", "-extfile", str(private / "extensions"), "-out", str(private / "server.pem")],
    ]
    for command in commands:
        subprocess.run(["openssl"] + command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
    return trust


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    requests = []
    authed = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append(self.path)
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.send_response(401)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            authed.append(self.path)
            if self.path.startswith("/api/recipes?"):
                body = (FIXTURES / "mealie-list.json").read_bytes()
            elif self.path == "/api/recipes" or self.path.strip("/") in ("api/recipes",):
                body = (FIXTURES / "mealie-list.json").read_bytes()
            else:
                slug = self.path.rsplit("/", 1)[-1]
                candidate = FIXTURES / f"mealie-detail-{slug}.json"
                if self.path.startswith("/api/recipes/") and candidate.exists():
                    body = candidate.read_bytes()
                else:
                    self.send_error(404)
                    return
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    with tempfile.TemporaryDirectory(prefix="cobalt-kitchencard-", dir="/tmp") as temporary:
        private = Path(temporary)
        trust = certificate(private)
        secrets = private / "cobalt-sim-secrets"
        secrets.mkdir(mode=0o700)
        (secrets / "mealie").write_text(TOKEN)
        (secrets / "mealie").chmod(0o600)
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.daemon_threads = True
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(private / "server.pem", private / "server.key")
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1", CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1", KOBO_SIM_HTTP_FIXTURE=f"{HOST}=127.0.0.1:{server.server_port}", KOBO_SIM_TRUST_DIR=str(trust), KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391")
        env.pop("KOBO_SIM_OFFLINE", None)
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
                process = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"], cwd=ROOT / "apps/kitchencard", env=env, stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError("Simulator exited; see simulator.log")
                    match = re.search(r"Kobo app simulator: http://(127\.0\.0\.1:\d+)", (out / "simulator.log").read_text()[offset:])
                    if match:
                        address = match.group(1)
                        return
                    time.sleep(.1)
                raise TimeoutError("Simulator startup timed out")

            def drive(*steps):
                command = [str(cli), "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    command.extend(["--step", step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=120)

            def capture(name):
                drive("clean", "shot " + name)

            try:
                start()
                drive("wait-for Connect Mealie", "tap Add address", "wait-for Kitchen Card settings", "tap https://mealie.example")
                steps = []
                layer = "letters"
                for character in "demo.mealie.io":
                    wanted = "letters" if character.isalpha() else "symbols"
                    if wanted != layer:
                        steps.append("tap ?123" if wanted == "symbols" else "tap abc")
                        layer = wanted
                    steps.append("type " + character)
                drive(*steps)
                drive("tap Save", "expect Kitchen Card settings", "expect https://demo.mealie.io")
                drive("tap Back", "expect No recipes yet")
                drive("tap Sync Mealie", "wait-for Pick tonight")
                capture("kitchen-synced")
                assert "/api/recipes?perPage=24&page=1" in authed, f"list request missing or unauthenticated: {requests}"
                assert sum(1 for path in authed if path.startswith("/api/recipes/")) == 3, f"detail requests: {authed}"
                result["checks"].append("list and 3 detail fetches carried the bearer credential")
                drive("tap Pick a recipe", "expect Tomato Basil Pasta")
                capture("kitchen-browse")
                drive("tap Lemon Herb Roasted Chicken", "expect Serves")
                capture("kitchen-tonight")
                drive("tap Start cooking", "expect Heat the oven")
                capture("kitchen-cook-steps")
                drive("tap Ingredients", "expect whole chicken")
                capture("kitchen-cook-ingredients")
                result["checks"].append("synced real Mealie demo recipes through cook and ingredients views")
                stop()
                verify_cli(cli, provenance)
                result["status"] = "passed"
            except Exception as error:
                result["status"] = "failed"
                result["error"] = str(error)
                if process is not None and process.poll() is None and address:
                    try:
                        capture("failure")
                    except Exception:
                        pass
                raise
            finally:
                result["requests"] = requests
                (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
                stop()


if __name__ == "__main__":
    main()
