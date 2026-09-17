#!/usr/bin/env python3
"""Drive the arXiv app against a local TLS fixture posing as the arXiv API:
browse into a subject, fetch the listing feed, open a paper, and read its
abstract - through the actual simulator with real taps.

The fixture serves a synthetic Atom feed shaped like the parser test fixture
in apps/arxiv/src/atom.rs. No arXiv traffic leaves the machine; TLS is
anchored in the trust roots `kobo stream init` creates, the same owner trust
the device loads.
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

# Mirrors the parser test fixture in apps/arxiv/src/atom.rs: a complete
# synthetic feed whose first entry carries every fact a paper can - authors,
# categories, a revision date, a journal reference and a comment.
FEED = """<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:arxiv="http://arxiv.org/schemas/atom"
      xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <title>ArXiv Query</title>
  <id>http://arxiv.org/api/query</id>
  <updated>2026-09-16T00:00:00-05:00</updated>
  <opensearch:totalResults>2</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2609.00042v2</id>
    <updated>2026-09-14T18:00:00Z</updated>
    <published>2026-09-01T09:30:00Z</published>
    <title>Attention Reconsidered</title>
    <summary>We revisit the transformer and find it still works. The fixture
      abstract runs long enough to paginate beneath the paper's facts.</summary>
    <author><name>Ada Lovelace</name></author>
    <author><name>Alan Turing</name></author>
    <arxiv:comment>12 pages, 3 figures</arxiv:comment>
    <arxiv:journal_ref>J. Irrepr. Res. 4 (2026) 1-12</arxiv:journal_ref>
    <link href="http://arxiv.org/abs/2609.00042v2" rel="alternate" type="text/html"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2609.00043v1</id>
    <published>2026-09-02T09:30:00Z</published>
    <title>A Second Fixture Paper</title>
    <summary>Shorter.</summary>
    <author><name>Grace Hopper</name></author>
    <category term="cs.SE" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"""


class Archive:
    """The fixture API, and everything it was asked to do."""

    def __init__(self):
        self.feeds = 0


def handler_for(archive):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def _send(self, code, body, content_type):
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            print(f"fixture GET {self.path}", flush=True)
            if "search_query" in self.path:
                archive.feeds += 1
                self._send(200, FEED.encode(), "application/atom+xml; charset=utf-8")
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

    with tempfile.TemporaryDirectory(prefix="cobalt-arxiv-", dir="/tmp") as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_PROFILE="clara-bw-391",
                   KOBO_SIM_CLOCK_MILLIS="1767265860000")
        env.pop("KOBO_SIM_OFFLINE", None)
        config = private / "config"
        env.update(KOBO_STREAM_CONFIG_DIR=str(config),
                   KOBO_SIM_TRUST_DIR=str(config / "trust"))
        subprocess.run([str(cli), "stream", "init", "--host", "127.0.0.1"],
                       cwd=ROOT, env=env, check=True, capture_output=True, timeout=60)

        archive = Archive()
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0),
                                                 handler_for(archive))
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config / "stream/cert.pem", config / "stream/key.pem")
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env["ARXIV_API_BASE"] = f"https://127.0.0.1:{server.server_port}"
        env["ARXIV_HTML_BASE"] = env["ARXIV_API_BASE"]

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
                                           cwd=ROOT / "apps/arxiv", env=env,
                                           stdout=log, stderr=log,
                                           start_new_session=True)
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
                # The subject list is the way in; a tap fetches the listing.
                # The frozen evidence clock also paces request spacing, so it
                # advances before the feed is waited on.
                drive("wait-for Artificial Intelligence", "wait-idle",
                      timeout=300)
                drive("tap Artificial Intelligence", "clock advance 1500",
                      "wait-for Attention Reconsidered",
                      "wait-for A Second Fixture Paper", "wait-idle",
                      timeout=300)
                capture("arxiv-listing")
                assert archive.feeds == 1, "the listing feed was fetched once"
                result["checks"].append(dict(
                    name="subject listing over TLS", status="passed",
                    detail="tapping a subject fetched the fixture feed over TLS "
                           "and both parsed papers rendered as rows"))

                # Opening a paper needs no network: the abstract was in the
                # feed. The first page carries the title as a heading, each
                # fact on a muted line of its own, and the prose beneath.
                drive("tap Attention Reconsidered",
                      "wait-for We revisit the transformer",
                      "wait-for Ada Lovelace", "wait-for cs.LG, cs.CL",
                      "wait-for Published in J. Irrepr. Res.",
                      "wait-for 12 pages, 3 figures", "wait-idle",
                      timeout=300)
                capture("arxiv-abstract")
                assert archive.feeds == 1, "opening a paper fetches nothing"
                result["checks"].append(dict(
                    name="abstract separates title, facts and prose",
                    status="passed",
                    detail="the first page shows the title, the byline, the "
                           "categories, the journal reference and the comment "
                           "as distinct elements above the paginated abstract"))
                result["status"] = "passed"
            finally:
                stop()
                print(f"fixture served feeds={archive.feeds}", flush=True)
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
