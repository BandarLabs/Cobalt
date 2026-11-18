#!/usr/bin/env python3
"""Drive the arXiv app against a local TLS fixture posing as the arXiv API:
browse a heading into a subject, fetch the listing feed, open a paper, read
it, keep it with its figures, and reopen it with the network gone - through
the actual simulator with real taps.

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
import urllib.request

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
    # The whole evidence tree is harness output: sidecars carry
    # per-run timings, and a live feed drifts between runs. Only
    # changes outside it say anything about the sources under test.
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
        # Only the computer's identity is wanted here: its certificate is what
        # the fixture serves TLS with. Naming the loopback as the reader stops
        # init scanning the network for a real one, and the pairing it then
        # cannot finish is expected, so its exit status is not the test.
        subprocess.run([str(cli), "stream", "init", "--device", "127.0.0.1",
                        "--host", "127.0.0.1"],
                       cwd=ROOT, env=env, check=False, capture_output=True, timeout=60)
        if not (config / "stream/cert.pem").is_file():
            raise RuntimeError("kobo stream init made no certificate for the fixture")

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

            def start(offline=False):
                nonlocal process, address
                offset = (out / "simulator.log").stat().st_size
                # Offline is the whole reader without a network, which is how
                # a kept paper is meant to be read: every request fails the
                # way it does on a train.
                session = dict(env, KOBO_SIM_OFFLINE="1") if offline else env
                process = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"],
                                           cwd=ROOT / "apps/arxiv", env=session,
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

            def top_bar(text):
                match = re.search(r'TopBarTitle\s+\["((?:[^"\\]|\\.)*)"', text)
                return match.group(1) if match else None

            def simulator(endpoint):
                with urllib.request.urlopen(f"http://{address}{endpoint}",
                                            timeout=30) as answer:
                    return json.loads(answer.read())

            def turn_back_until(anchor, max_turns=12):
                for _ in range(max_turns):
                    if anchor in screen_text():
                        return
                    drive("input gpio 1 193 1", "input gpio 1 193 0",
                          "clock advance 1500", "wait-idle")
                raise AssertionError(f"{anchor!r} never came back onto the panel")

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
                # The first screen is arXiv's own headings. A category sits
                # under its heading, not in one list of a hundred and fifty.
                drive("wait-for Computer Science", "wait-for 40 subjects",
                      "wait-idle", timeout=300)
                home = screen_text()
                assert "Artificial Intelligence" not in home, \
                    "a category was listed on the first screen"
                capture("arxiv-subjects")
                drive("tap Computer Science", "wait-for Artificial Intelligence",
                      "wait-for cs.AI", "wait-idle", timeout=300)
                capture("arxiv-group")
                result["checks"].append(dict(
                    name="categories under their headings", status="passed",
                    detail="the first screen lists arXiv's headings with how "
                           "many categories each holds, and Computer Science "
                           "opens its categories (panel shots arxiv-subjects, "
                           "arxiv-group)"))

                # A category's listing. The frozen evidence clock also paces
                # request spacing, so it advances before the feed is waited on.
                drive("tap Artificial Intelligence", "clock advance 1500",
                      "wait-for Attention Reconsidered",
                      "wait-for A Second Fixture Paper",
                      "wait-for A Fixture of Formulas and Tables",
                      "wait-idle", timeout=300)
                capture("arxiv-listing")
                assert archive.feeds == 1, "the listing feed was fetched once"
                result["checks"].append(dict(
                    name="subject listing over TLS", status="passed",
                    detail="tapping a category fetched the fixture feed over "
                           "TLS and all three parsed papers rendered as rows"))

                # Opening a paper needs no network: the abstract was in the
                # feed. The bar names the paper in words, not by its number.
                drive("tap Attention Reconsidered",
                      "wait-for We revisit the transformer",
                      "wait-for Ada Lovelace", "wait-for cs.LG, cs.CL",
                      "wait-for Published in J. Irrepr. Res.",
                      "wait-for 12 pages, 3 figures", "wait-idle",
                      timeout=300)
                capture("arxiv-abstract")
                assert archive.feeds == 1, "opening a paper fetches nothing"
                bar = top_bar(screen_text())
                assert bar == "Attention Reconsidered", f"the bar said {bar!r}"
                result["checks"].append(dict(
                    name="abstract titled in the bar, facts and prose below",
                    status="passed",
                    detail="the bar carries the paper's title rather than "
                           "2609.00042v2; the first page shows the title, the "
                           "byline, the categories, the journal reference and "
                           "the comment above the abstract"))

                drive("tap Full text", "clock advance 1500",
                      "wait-for Paragraph 0", "wait-idle", timeout=300)
                capture("arxiv-reading")
                assert archive.htmls == 1, "the full text was fetched once"
                bar = top_bar(screen_text())
                assert bar in (None, "Attention Reconsidered"), \
                    f"the reader's bar said {bar!r}"
                result["checks"].append(dict(
                    name="full text in the reader", status="passed",
                    detail="the fixture's HTML fetched over TLS opened "
                           "straight in the reader, titled in words"))

                # Read a few pages, then leave and keep it. Back from the
                # reader lets the rendering go, so keeping fetches it again
                # and opens it at the place saved on the way out.
                drive("input gpio 1 194 1", "input gpio 1 194 0",
                      "input gpio 1 194 1", "input gpio 1 194 0",
                      "input gpio 1 194 1", "input gpio 1 194 0")
                drive("tap Back", "wait-for Keep for offline", "wait-idle",
                      timeout=300)
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
                    detail="once the kept file was acknowledged, the listing "
                           "row carries the offline badge"))

                # Back from a listing reached through a heading is the
                # heading, and the heading's Back is the first screen.
                drive("tap Back", "wait-for cs.AI", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                drive("tap Library", "wait-for %",
                      "wait-for Ready offline: 1 paper", "wait-idle",
                      timeout=300)
                capture("arxiv-library")
                result["checks"].append(dict(
                    name="library says what is ready offline",
                    status="passed",
                    detail="Library opens with 'Ready offline: 1 paper, ...' "
                           "and the kept row carries its size and a reading "
                           "percentage"))

                # A paper carrying a displayed formula, a table and figures.
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                drive("tap Computer Science", "wait-for Artificial Intelligence",
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
                    detail="the formula typeset from its LaTeX, the table read "
                           "as rows and the figure fetched over TLS and drawn "
                           "(panel shots arxiv-formula/-table/-figure)"))
                turn_until("Figure 2", "arxiv-figure-missing", settle=4)
                assert archive.misses == 1, "the missing figure was asked for once"
                result["checks"].append(dict(
                    name="a failed figure fetch degrades to the caption",
                    status="passed",
                    detail="the figure the fixture 404s drew no error; the page "
                           "reads the caption and the text after it"))

                # Keep it, which puts the figure on the reader beside it.
                drive("tap Back", "wait-for Keep for offline", "wait-idle",
                      timeout=300)
                pngs_before = archive.pngs
                drive("tap Keep for offline", "clock advance 1500",
                      "wait-for Figure 2", "wait-idle", timeout=300)
                for _ in range(8):
                    drive("clock advance 1500", "wait-idle")
                drive("tap Back", "wait-for Remove from library", "wait-idle",
                      timeout=300)
                drive("tap Back", "wait-idle", timeout=300)
                if not any("offline" in line and "Maria Gauss" in line
                           for line in screen_text().splitlines()):
                    for _ in range(10):
                        drive("clock advance 1500", "wait-idle")
                        if any("offline" in line and "Maria Gauss" in line
                               for line in screen_text().splitlines()):
                            break
                    else:
                        raise AssertionError("the kept paper never said offline")
                assert archive.pngs > pngs_before, "keeping fetched no figure"
                drive("tap Back", "wait-for cs.AI", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                drive("tap Library", "wait-for Ready offline: 2 papers",
                      "wait-idle", timeout=300)
                capture("arxiv-library-two")
                result["checks"].append(dict(
                    name="keeping a paper keeps its figures",
                    status="passed",
                    detail="Keep for offline fetched the paper's figure beside "
                           "it and the library counts both papers ready "
                           "offline (panel shot arxiv-library-two)"))

                # Restart with no network at all. The library, the place in
                # each paper and the figures are all on the reader.
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                stop()
                served = (archive.feeds, archive.htmls, archive.pngs,
                          archive.misses)
                start(offline=True)
                drive("wait-for Computer Science", "wait-idle", timeout=300)
                drive("tap Library", "wait-for 64%",
                      "wait-for Ready offline: 2 papers", "wait-idle",
                      timeout=300)
                capture("arxiv-offline-library")
                drive("tap Attention Reconsidered", "clock advance 1500",
                      "wait-for 4 of 5", "wait-idle", timeout=300)
                capture("arxiv-offline-reopened")
                bar = top_bar(screen_text())
                assert bar in (None, "Attention Reconsidered"), \
                    f"the reader's bar said {bar!r}"
                drive("tap Back", "wait-for 64%", "wait-idle", timeout=300)
                drive("tap A Fixture of Formulas and Tables",
                      "clock advance 1500", "wait-for Figure 2", "wait-idle",
                      timeout=300)
                turn_back_until("Figure 1")
                for _ in range(4):
                    drive("clock advance 1500", "wait-idle")
                capture("arxiv-offline-figure")
                # The plate drawn right above its caption, which is the kept
                # figure and not the formula typeset earlier in the paper.
                nodes = simulator("/layout")["nodes"]
                caption = next(node for node in nodes
                               if any("Figure 1" in line for line in node["lines"]))
                pictures = [node for node in nodes
                            if node["kind"].startswith(("Picture(", "FramedPicture("))
                            and node["y"] + node["height"] <= caption["y"]
                            and caption["y"] - node["y"] - node["height"] < 200]
                assert pictures, "the kept figure was not on the page offline"
                missing = [issue for issue in
                           simulator("/diagnostics").get("issues", [])
                           if "MissingPicture" in json.dumps(issue)]
                assert not missing, f"the kept figure never drew: {missing}"
                assert (archive.feeds, archive.htmls, archive.pngs,
                        archive.misses) == served, \
                    "the offline session reached the fixture"
                result["checks"].append(dict(
                    name="kept papers open offline with their figures",
                    status="passed",
                    detail="restarted with KOBO_SIM_OFFLINE=1, the library "
                           "still listed both papers at their progress, the "
                           "first reopened on page 4 of 5, and the second drew "
                           "its kept figure with no request reaching the "
                           "fixture (panel shots arxiv-offline-library, "
                           "-reopened, -figure)"))
                drive("tap Back", "wait-for 64%", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                stop()

                # Saved searches and followed subjects, online again.
                start()
                drive("wait-for Computer Science", "wait-idle", timeout=300)
                drive("tap Computer Science", "wait-for Artificial Intelligence",
                      "wait-idle", timeout=300)
                drive("tap Artificial Intelligence", "clock advance 1500",
                      "wait-for Attention Reconsidered", "wait-idle",
                      timeout=300)
                assert archive.feeds == 3, "the subject refetched"
                drive("tap Follow this subject", "wait-for Stop following",
                      "wait-idle", timeout=300)
                capture("arxiv-followed")
                drive("tap Back", "wait-for cs.AI", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Artificial Intelligence",
                      "wait-for Computer Science", "wait-idle", timeout=300)
                capture("arxiv-subjects-followed")
                result["checks"].append(dict(
                    name="following pins the subject to the first screen",
                    status="passed",
                    detail="the listing offered Follow this subject and then "
                           "said Stop following; the first screen then leads "
                           "with the followed subject above the headings "
                           "(panel shots arxiv-followed, arxiv-subjects-followed)"))

                drive("tap Search arXiv",
                      "wait-for A phrase, an author, a title", "wait-idle",
                      timeout=300)
                drive("type neural fields", "wait-idle", timeout=300)
                drive("tap Search", "clock advance 1500",
                      "wait-for Attention Reconsidered", "wait-idle",
                      timeout=300)
                assert archive.feeds == 4, "the typed search fetched the feed"
                drive("tap Save this search", "wait-idle", timeout=300)
                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                drive("tap Saved", "wait-for neural fields",
                      "wait-for Artificial Intelligence", "wait-idle",
                      timeout=300)
                capture("arxiv-saved")
                result["checks"].append(dict(
                    name="Saved lists searches and subjects",
                    status="passed",
                    detail="the typed search saved from its listing and the "
                           "followed subject are listed together in Saved "
                           "(panel shot arxiv-saved)"))

                drive("tap neural fields", "clock advance 1500",
                      "wait-for Attention Reconsidered", "wait-idle",
                      timeout=300)
                assert archive.feeds == 5, "the saved search ran again"
                result["checks"].append(dict(
                    name="a saved search runs again",
                    status="passed",
                    detail="tapping the saved row fetched the same listing a "
                           "second time, with no typing"))

                drive("tap Back", "wait-for Computer Science", "wait-idle",
                      timeout=300)
                drive("tap Saved", "wait-for neural fields", "wait-idle",
                      timeout=300)
                drive("tap Manage", "wait-for Done", "wait-idle", timeout=300)
                capture("arxiv-saved-manage")
                drive("tap neural fields",
                      "wait-for Artificial Intelligence", "wait-idle",
                      timeout=300)
                capture("arxiv-saved-removed")
                drive("tap Done", "wait-idle", timeout=300)
                result["checks"].append(dict(
                    name="Manage removes a saved search",
                    status="passed",
                    detail="Manage turned the rows into removals and tapping "
                           "the search removed it, leaving the followed "
                           "subject (panel shots arxiv-saved-manage, "
                           "arxiv-saved-removed)"))
                result["status"] = "passed"
            finally:
                stop()
                print(f"fixture served feeds={archive.feeds} htmls={archive.htmls} pngs={archive.pngs}", flush=True)
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
