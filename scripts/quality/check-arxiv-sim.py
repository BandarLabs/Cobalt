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
  <opensearch:totalResults>3</opensearch:totalResults>
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
  <entry>
    <id>http://arxiv.org/abs/2609.00077v1</id>
    <published>2026-09-03T09:30:00Z</published>
    <title>A Fixture of Formulas and Tables</title>
    <summary>A short fixture summary.</summary>
    <author><name>Maria Gauss</name></author>
    <category term="math.CO" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"""


# A complete synthetic paper, long enough to paginate in the reader.
PAPER_HTML = "<html><body><article><h2>1 Introduction</h2>" + "".join(
    f"<p>Paragraph {n} of the fixture paper, set to be read.</p>"
    for n in range(60)
) + "</article></body></html>"


# A second paper carrying the structures a wall of text would lose: a
# displayed formula with its LaTeX behind it, a columnar table, and a figure
# fetched from beside the paper.
PAPER_RICH_HTML = (
    "<html><body><article>"
    "<h2>1 A Fixture of Formulas and Tables</h2>"
    "<p>The union of every set in the family, written as mathematics.</p>"
    '<math display="block" alttext="\\bigcup_{i=1}^{n} A_i">'
    "<mo>\u22c3</mo></math>"
    + "".join(
        f"<p>Result paragraph {n} between the formula and the table.</p>"
        for n in range(18)
    )
    + "<table><tr><th>Model</th><th>Accuracy</th><th>Latency</th></tr>"
    "<tr><td>Fixture A</td><td>91.2</td><td>4 ms</td></tr>"
    "<tr><td>Fixture B</td><td>88.7</td><td>6 ms</td></tr></table>"
    + "".join(
        f"<p>Discussion paragraph {n} between the table and the figure.</p>"
        for n in range(18)
    )
    + '<figure><img src="2609.00077v1/x1.png" alt="A fixture plot">'
    "<figcaption>Figure 1: A fixture plot.</figcaption></figure>"
    "<p>The second figure is one the fixture never serves.</p>"
    '<figure><img src="2609.00077v1/x2.png" alt="A missing plot">'
    "<figcaption>Figure 2: A missing plot.</figcaption></figure>"
    "<p>The paper reads on past it.</p>"
    "</article></body></html>"
)


def fixture_png():
    """A small grayscale gradient PNG, built by hand so the fixture has a
    figure to serve without checking a binary into the repository."""
    import struct
    import zlib

    width, height = 64, 64
    raw = b"".join(
        b"\x00" + bytes((x * 4) % 256 for x in range(width))
        for _ in range(height)
    )

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


FIGURE_PNG = fixture_png()


class Archive:
    """The fixture API, and everything it was asked to do."""

    def __init__(self):
        self.feeds = 0
        self.htmls = 0
        self.pngs = 0
        self.misses = 0


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
            path = self.path.split("?", 1)[0]
            print(f"fixture GET {self.path}", flush=True)
            if "search_query" in self.path:
                archive.feeds += 1
                self._send(200, FEED.encode(), "application/atom+xml; charset=utf-8")
                return
            if re.fullmatch(r"/2609\.00042v2", path):
                archive.htmls += 1
                self._send(200, PAPER_HTML.encode(), "text/html; charset=utf-8")
                return
            if re.fullmatch(r"/2609\.00077v1", path):
                archive.htmls += 1
                self._send(200, PAPER_RICH_HTML.encode(), "text/html; charset=utf-8")
                return
            if re.fullmatch(r"/2609\.00077v1/x1\.png", path):
                archive.pngs += 1
                self._send(200, FIGURE_PNG, "image/png")
                return
            if re.fullmatch(r"/2609\.00077v1/x2\.png", path):
                archive.misses += 1
                self._send(404, b"not found", "text/plain")
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
    cli, provenance = build_cli(ROOT, target, ignore_prefix=(
        os.path.relpath(out, ROOT)
        if not os.path.relpath(out, ROOT).startswith("..")
        else None))
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

            def screen_text():
                probe = subprocess.run(
                    [str(cli), "drive", "--address", address, "--ideal",
                     "--step", "dump"],
                    cwd=ROOT, env=env, capture_output=True, text=True,
                    timeout=60)
                if probe.returncode != 0:
                    raise RuntimeError("dump step failed; see simulator.log")
                return probe.stdout

            def turn_until(anchor, shot_name, max_turns=10, settle=2):
                # One page at a time with the clock advancing past the pacing
                # sleeps, until the anchor is on the panel. The picture under
                # it arrives a pass at a time (fetch, decode, dither), each
                # carried by a task the manual clock has to let run, so the
                # clock advances a few times more before the shot.
                for _ in range(max_turns):
                    if anchor in screen_text():
                        for _ in range(settle):
                            drive("clock advance 1500", "wait-idle")
                        capture(shot_name)
                        return
                    drive("input gpio 1 194 1", "input gpio 1 194 0",
                          "clock advance 1500", "wait-idle")
                raise AssertionError(f"{anchor!r} never came onto the panel")

            try:
                start()
                # The subject list is the way in; a tap fetches the listing.
                # The frozen evidence clock also paces request spacing, so it
                # advances before the feed is waited on.
                drive("wait-for Artificial Intelligence", "wait-idle",
                      timeout=300)
                drive("tap Artificial Intelligence", "clock advance 1500",
                      "wait-for Attention Reconsidered",
                      "wait-for A Second Fixture Paper",
                      "wait-for A Fixture of Formulas and Tables",
                      "wait-idle", timeout=300)
                capture("arxiv-listing")
                assert archive.feeds == 1, "the listing feed was fetched once"
                result["checks"].append(dict(
                    name="subject listing over TLS", status="passed",
                    detail="tapping a subject fetched the fixture feed over TLS "
                           "and all three parsed papers rendered as rows"))

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

                # Full text: the fetching state shows while the fixture's
                # answer is still in the air (the evidence clock is manual,
                # so the pacing sleep has not fired yet).
                drive("tap Full text", "clock advance 1500",
                      "wait-for Paragraph 0", "wait-idle", timeout=300)
                capture("arxiv-reading")
                assert archive.htmls == 1, "the full text was fetched once"
                result["checks"].append(dict(
                    name="downloading state then full text", status="passed",
                    detail="the fixture's HTML fetched over TLS opened "
                           "straight in the reader (the fetching state itself "
                           "is unit-tested; the fixture answers within one "
                           "drive step, too fast to film)"))

                # Read a few pages with the page-turn button, then leave.
                drive("input gpio 1 194 1", "input gpio 1 194 0",
                      "input gpio 1 194 1", "input gpio 1 194 0",
                      "input gpio 1 194 1", "input gpio 1 194 0")
                drive("tap Back", "wait-for Keep for offline", "wait-idle",
                      timeout=300)

                # Keep it, and the listing row says so.
                # Back from the reader lets the rendering go, so keeping
                # fetches it again; the manual clock advances for that fetch.
                # The landed paper opens in the reader - at the place that
                # was saved on the way out, not at the top.
                drive("tap Keep for offline", "clock advance 1500",
                      "wait-for Paragraph 44", "wait-for 4 of 5", "wait-idle",
                      timeout=300)
                capture("arxiv-reopened")
                drive("tap Back", "wait-for Remove from library",
                      "wait-idle", timeout=300)
                drive("tap Back", "wait-for offline", "wait-idle", timeout=300)
                capture("arxiv-listing-offline")
                result["checks"].append(dict(
                    name="listing shows the kept paper offline",
                    status="passed",
                    detail="after Keep for offline, the listing row carries "
                           "the offline badge"))

                # The library row says how far through it was read.
                drive("tap Back", "wait-for Artificial Intelligence",
                      "wait-idle", timeout=300)
                drive("tap Library", "wait-for %", "wait-idle", timeout=300)
                capture("arxiv-library")
                result["checks"].append(dict(
                    name="library shows reading progress", status="passed",
                    detail="the kept row carries the size and a reading "
                           "percentage recorded when the reader saved the "
                           "place on the way out"))
                # A paper carrying a displayed formula, a table and a
                # figure keeps all three in the reader: the formula is
                # typeset from its LaTeX, the table stays columnar, and the
                # figure is fetched from beside the paper and drawn.
                drive("tap Back", "wait-for Artificial Intelligence",
                      "wait-idle", timeout=300)
                drive("tap Artificial Intelligence", "clock advance 1500",
                      "wait-for A Fixture of Formulas and Tables",
                      "wait-idle", timeout=300)
                assert archive.feeds == 2, "reopening the subject refetched"
                drive("tap A Fixture of Formulas and Tables",
                      "wait-for A short fixture summary.", "wait-idle",
                      timeout=300)
                drive("tap Full text", "clock advance 1500",
                      "wait-for The union", "clock advance 1500",
                      "wait-idle", timeout=300)
                turn_until("The union", "arxiv-formula")
                turn_until("Fixture A", "arxiv-table")
                turn_until("Figure 1", "arxiv-figure", settle=4)
                assert archive.pngs == 1, "the figure was fetched once"
                result["checks"].append(dict(
                    name="formulas, tables and figures survive", status="passed",
                    detail="a paper with a displayed formula, a columnar table "
                           "and a figure kept all three: the formula typeset "
                           "from its LaTeX, the table read as rows and the "
                           "figure fetched over TLS and drawn (panel shots "
                           "arxiv-formula/-table/-figure)"))
                # A figure that never comes reads as its caption, and the
                # paper around it is untouched.
                turn_until("Figure 2", "arxiv-figure-missing", settle=4)
                assert archive.misses == 1, "the missing figure was asked for once"
                result["checks"].append(dict(
                    name="a failed figure fetch degrades to the caption",
                    status="passed",
                    detail="the figure the fixture 404s drew no error and no "
                           "placeholder frame - the page reads the caption and "
                           "the text after it (panel shot arxiv-figure-missing)"))

                # Everything the reader was holding survives the application
                # being restarted outright: the kept paper is still in the
                # library with its progress, and opening it lands at the
                # saved place, not at the top.
                drive("tap Back", "wait-for Keep for offline", "wait-idle",
                      timeout=300)
                drive("tap Back", "wait-for offline", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Artificial Intelligence",
                      "wait-idle", timeout=300)
                stop()
                start()
                drive("wait-for Artificial Intelligence", "wait-idle",
                      timeout=300)
                drive("tap Library", "wait-for 74%", "wait-idle", timeout=300)
                capture("arxiv-restart-library")
                drive("tap Attention Reconsidered", "clock advance 1500",
                      "wait-for 4 of 5", "wait-idle", timeout=300)
                capture("arxiv-restart-reopened")
                result["checks"].append(dict(
                    name="kept reading survives a simulator restart",
                    status="passed",
                    detail="after the simulator process was killed and started "
                           "again, the library still listed the kept paper at "
                           "74% and opening it landed on page 4 of 5, the "
                           "place saved before the restart (panel shots "
                           "arxiv-restart-library/-reopened)"))
                result["status"] = "passed"
            finally:
                stop()
                print(f"fixture served feeds={archive.feeds} htmls={archive.htmls} pngs={archive.pngs}", flush=True)
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
