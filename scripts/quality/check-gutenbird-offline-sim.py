#!/usr/bin/env python3
"""Download an original book and verify a complete offline simulator restart."""
import argparse
import io
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
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
HOST = "www.gutenberg.org"


def book_bytes():
    target = io.BytesIO()
    with zipfile.ZipFile(target, "w") as archive:
        archive.writestr("mimetype", "application/epub+zip")
        archive.writestr("META-INF/container.xml", '''<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>''')
        archive.writestr("book.opf", '''<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">cobalt-original-walking-notes</dc:identifier><dc:title>Walking Notes</dc:title><dc:language>en</dc:language></metadata><manifest><item id="body" href="body.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="body"/></spine></package>''')
        paragraphs = ''.join(f'<p>Note {n}. The path turns beside the river. A small bridge leads toward the garden, where the morning light falls across the stone steps. We stop to sketch the trees before continuing home.</p>' for n in range(1, 101))
        archive.writestr("body.xhtml", '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Walking Notes</title></head><body><h1>Walking Notes</h1>' + paragraphs + '</body></html>')
    return target.getvalue()


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
    book = book_bytes()
    requests = []
    catalog = json.dumps({"metadata": {"title": "Walking Library"}, "publications": [{"metadata": {"title": "Walking Notes", "author": "A. Walker", "language": "en"}, "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": f"https://{HOST}/walking.epub", "type": "application/epub+zip", "length": len(book)}]}]}).encode()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append(self.path)
            if self.path not in ("/ebooks.opds/", "/walking.epub"):
                self.send_error(404)
                return
            body = book if self.path == "/walking.epub" else catalog
            start, end = 0, len(body) - 1
            if self.headers.get("Range"):
                match = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers["Range"])
                assert match, "Unexpected range"
                start = int(match.group(1))
                if match.group(2):
                    end = min(end, int(match.group(2)))
            self.send_response(206 if self.headers.get("Range") else 200)
            self.send_header("Content-Type", "application/epub+zip" if self.path == "/walking.epub" else "application/opds+json")
            if self.headers.get("Range"):
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(body)}")
            self.send_header("Content-Length", str(len(body[start:end + 1])))
            self.end_headers()
            self.wfile.write(body[start:end + 1])

    with tempfile.TemporaryDirectory(prefix="cobalt-gutenbird-", dir="/tmp") as temporary:
        private = Path(temporary)
        trust = certificate(private)
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
                process = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"], cwd=ROOT / "examples/gutenbird", env=env, stdout=log, stderr=log, start_new_session=True)
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
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=60)

            def capture(name):
                drive("clean", "shot " + name)
                with urllib.request.urlopen(f"http://{address}/layout", timeout=5) as response:
                    layout = json.load(response)
                (out / f"{name}.layout.json").write_text(json.dumps(layout, indent=2) + "\n")
                return layout

            try:
                start()
                drive("wait-for-id read", "tap-id read", "wait-for Note 1.")
                initial = capture("01-reading")
                next_button = next(node for node in initial["nodes"] if node["kind"].startswith("PageNext("))
                point = next_button["centre"]
                drive(f"tap-at {point['x']} {point['y']}", f"tap-at {point['x']} {point['y']}")
                progress = capture("02-progress")
                text = lambda screen: [node["lines"] for node in screen["nodes"] if node["kind"].startswith("RichText(")]
                assert text(progress) != text(initial), "Page turns did not change the reading text"
                result["checks"].append("downloaded and paged original EPUB")
                state = private / "cobalt-sim-state/gutenbird"
                positions = list(state.glob("place-*"))
                assert positions, "Reading position was not stored"
                result["checks"].append("reading position stored")
                stop()
                env["KOBO_SIM_OFFLINE"] = "1"
                before = len(requests)
                start()
                drive("wait-for-id read", "tap-id read", "wait-for Note")
                reopened = capture("03-offline-reopened")
                assert text(reopened) == text(progress), "Offline reopen lost reading position"
                assert len(requests) == before, "Offline restart contacted the fixture"
                result["checks"].append("reopened after full offline process restart")
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
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    main()
