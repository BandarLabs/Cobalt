#!/usr/bin/env python3
"""Drive Fanshelf against a local TLS fixture posing as AO3: look a work up
by its numeric ID, parse the work page, download the EPUB the page points to,
and open it in the reader - through the actual simulator with real taps.

The fixture serves a synthetic work page and the repository's own synthetic
EPUB (apps/calibre-web/fixtures/river.epub). No archive traffic leaves the
machine; TLS is anchored in the trust roots `kobo stream init` creates, the
same owner trust the device loads.
"""
import argparse
import http.server
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

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
EPUB = ROOT / "apps/calibre-web/fixtures/river.epub"

# Mirrors the parser test fixture in apps/fanshelf/src/library.rs: a complete
# synthetic work page whose EPUB link is relative, the shape AO3 serves.
WORK_PAGE = """
  <html><head><title>The Lantern Library | Archive of Our Own</title></head><body>
  <h2 class="title heading">The Lantern Library</h2>
  <h3 class="byline heading"><a rel="author">River Quill</a></h3>
  <dt class="rating tags">Rating:</dt>
  <dd class="rating tags"><ul><li><a>Teen And Up Audiences</a></li></ul></dd>
  <dt class="warning tags">Archive Warning:</dt>
  <dd class="warning tags"><ul><li><a>No Archive Warnings Apply</a></li></ul></dd>
  <dt class="fandom tags">Fandoms:</dt>
  <dd class="fandom tags"><ul><li><a>Public Domain Fairy Tales</a></li></ul></dd>
  <blockquote class="userstuff summary module"><p>A synthetic fixture.</p></blockquote>
  <dt class="updated">Updated:</dt>
  <dd class="updated">2026-09-01</dd>
  <dt class="chapters">Chapters:</dt>
  <dd class="chapters">12/?</dd>
  <a href="/downloads/4242/The_Lantern_Library.epub?updated_at=1">EPUB</a>
  </body></html>
"""


class Archive:
    """The fixture archive, and everything it was asked to do."""

    def __init__(self):
        self.pages = 0
        self.epub_bytes = 0


def handler_for(archive, epub):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def _send(self, code, body, content_type, extra=()):
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            for name, value in extra:
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            print(f"fixture GET {self.path} range={self.headers.get('Range')}", flush=True)
            if re.fullmatch(r"/works/4242", path):
                archive.pages += 1
                self._send(200, WORK_PAGE.encode(), "text/html; charset=utf-8")
                return
            if re.fullmatch(r"/downloads/4242/The_Lantern_Library\.epub", path):
                offset = 0
                header = self.headers.get("Range")
                if header:
                    match = re.fullmatch(r"bytes=(\d+)-(\d*)", header)
                    if match:
                        offset = int(match.group(1))
                body = epub[offset:]
                archive.epub_bytes += len(body)
                if offset:
                    self._send(206, body, "application/epub+zip",
                               [("Content-Range", f"bytes {offset}-{len(epub) - 1}/{len(epub)}")])
                else:
                    self._send(200, body, "application/epub+zip")
                return
            self._send(404, b"not found", "text/plain")

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    cli, provenance = build_cli(ROOT, target)
    verify_cli(cli, provenance)

    with tempfile.TemporaryDirectory(prefix="cobalt-fanshelf-epub-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)
        env.pop("FANSHELF_DEMO", None)
        config = private / "config"
        env.update(KOBO_STREAM_CONFIG_DIR=str(config),
                   KOBO_SIM_TRUST_DIR=str(config / "trust"))
        subprocess.run([str(cli), "stream", "init", "--host", "127.0.0.1"],
                       cwd=ROOT, env=env, check=True, capture_output=True, timeout=60)

        archive = Archive()
        epub = EPUB.read_bytes()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0),
                                                 handler_for(archive, epub))
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config / "stream/cert.pem", config / "stream/key.pem")
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env["FANSHELF_AO3_BASE"] = f"https://127.0.0.1:{server.server_port}"

        process = None
        address = None
        result = dict(provenance=provenance, scale=args.scale,
                      server="local TLS fixture", checks=[])
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
                                           cwd=ROOT / "apps/fanshelf", env=env,
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
                # An empty shelf offers to add a work; the work number is the
                # address form a reader copies off the archive.
                drive("wait-for Add an AO3 work", "wait-idle", timeout=300)
                drive("tap Add", "wait-for Enter its web address")
                drive("tap-id kb.layer", "type 4242", "tap Open work",
                      "wait-for The Lantern Library", "wait-for River Quill",
                      "wait-idle", timeout=300)
                capture("fanshelf-epub-work")
                assert archive.pages == 1, "the work page was fetched once"
                result["checks"].append(dict(
                    name="work lookup over TLS", status="passed",
                    detail="the numeric ID fetched the fixture work page over TLS "
                           "and the parsed title and byline rendered"))

                # The frozen evidence clock also paces the one-second request
                # spacing; advance it so the download fetch fires. A download
                # started from the work screen opens the reader when it lands.
                drive("tap Download EPUB", "clock advance 1500",
                      "wait-for A Walk by the River", "wait-idle", timeout=300)
                capture("fanshelf-epub-reading")
                assert archive.epub_bytes == len(epub), \
                    "the fixture delivered the whole EPUB exactly once"
                result["checks"].append(dict(
                    name="EPUB download opens in the reader", status="passed",
                    detail="the relative EPUB link resolved against the archive "
                           f"address, all {len(epub)} bytes saved to the shelf, and "
                           "the book opened straight into BookView on its first "
                           "page (A Walk by the River, page 1 of 12)"))

                drive("tap Back", "wait-for Read", timeout=300)
                drive("tap Shelf", "wait-for reading", "wait-idle", timeout=300)
                capture("fanshelf-epub-shelf")
                result["checks"].append(dict(
                    name="shelf shows the download", status="passed",
                    detail="back on the shelf the row carries the reading badge: "
                           "the downloaded copy persisted and the reading state "
                           "survived leaving the reader"))
                result["status"] = "passed"
            finally:
                stop()
                print(f"fixture served pages={archive.pages} epub_bytes={archive.epub_bytes}", flush=True)
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
