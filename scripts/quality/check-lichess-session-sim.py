#!/usr/bin/env python3
"""Play and resume a Lichess fixture game through real simulator TLS streams."""
import argparse
import json
import os
from pathlib import Path
import queue
import re
import signal
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from simulator_cli import build_cli, verify_cli

ROOT = Path(__file__).resolve().parents[2]
GAME = "abcdEF12"
TOKEN = "fixture-only-not-a-live-token"


class Match:
    def __init__(self, drop_start_event=False):
        self.drop_start_event = drop_start_event
        self.start_events_sent = 0
        self.matched_at = None
        self.lock = threading.RLock()
        self.active = False
        self.moves = ""
        self.status = "started"
        self.draw_offer = False
        self.events = []
        self.boards = []
        self.requests = []
        self.errors = []
        self.stopping = threading.Event()

    def summary(self):
        return dict(id=GAME, gameId=GAME, color="white", rated=True, speed="rapid",
                    source="lobby", variant=dict(key="standard"), secondsLeft=600,
                    isMyTurn=True, lastMove="e7e5" if self.moves else "",
                    opponent=dict(username="Other"))

    def state(self):
        return dict(type="gameState", moves=self.moves, wtime=599000, btime=598000,
                    winc=0, binc=0, status=self.status, bdraw=self.draw_offer)

    def full(self):
        return dict(type="gameFull", id=GAME, rated=True, speed="rapid",
                    variant=dict(key="standard"), initialFen="startpos",
                    white=dict(id="owner123", name="Owner", rating=1500),
                    black=dict(id="other123", name="Other", rating=1510), state=self.state())

    def publish(self, moves=None, draw=False, finished=False):
        with self.lock:
            if moves is not None:
                self.moves = moves
            self.draw_offer = draw
            if finished:
                self.status = "draw"
                self.active = False
            for events in self.boards:
                events.put(self.state())


def handler(match):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def reply(self, data, status=200):
            body = json.dumps(data).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def stream(self, events, collection):
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            try:
                while not match.stopping.is_set():
                    try:
                        record = events.get(timeout=.25)
                        body = json.dumps(record).encode() + b"\n"
                    except queue.Empty:
                        body = b"\n"
                    self.wfile.write(f"{len(body):x}\r\n".encode() + body + b"\r\n")
                    self.wfile.flush()
            except (OSError, ssl.SSLError):
                pass
            finally:
                self.close_connection = True
                with match.lock:
                    if events in collection:
                        collection.remove(events)

        def authorize(self):
            if self.headers.get("Authorization") != "Bearer " + TOKEN:
                match.errors.append("Missing synthetic authorization")
                self.reply({}, 401)
                return False
            if self.headers.get("Host") != "lichess.org":
                match.errors.append("Original Host was not retained")
                self.reply({}, 400)
                return False
            with match.lock:
                match.requests.append(dict(method=self.command, path=self.path))
            return True

        def do_GET(self):
            if not self.authorize():
                return
            if self.path == "/api/account":
                self.reply(dict(id="owner123", username="Owner"))
            elif self.path == "/api/account/playing":
                with match.lock:
                    data = dict(nowPlaying=[match.summary()] if match.active else [])
                self.reply(data)
            elif self.path == "/api/stream/event":
                events = queue.Queue()
                with match.lock:
                    match.events.append(events)
                self.stream(events, match.events)
            elif self.path == "/api/board/game/stream/" + GAME:
                events = queue.Queue()
                with match.lock:
                    match.boards.append(events)
                    events.put(match.full())
                self.stream(events, match.boards)
            else:
                match.errors.append("Unexpected GET " + self.path)
                self.reply({}, 404)

        def do_POST(self):
            if not self.authorize():
                return
            length = int(self.headers.get("Content-Length", "0"))
            if length > 4096:
                raise ValueError("Oversized fixture request")
            body = self.rfile.read(length)
            if self.path == "/api/board/seek":
                assert body == b"rated=true&time=10&increment=0&variant=standard&color=random", body
                with match.lock:
                    match.active = True
                    match.matched_at = time.monotonic()
                    if not match.drop_start_event:
                        for events in match.events:
                            events.put(dict(type="gameStart", game=match.summary()))
                            match.start_events_sent += 1
                self.stream(queue.Queue(), [])
            elif self.path == f"/api/board/game/{GAME}/move/e2e4":
                assert not body
                # The driver releases the board acknowledgement independently.
                self.reply(dict(ok=True))
            elif self.path == f"/api/board/game/{GAME}/draw/yes":
                assert not body
                self.reply(dict(ok=True))
                match.publish(finished=True)
            else:
                match.errors.append("Unexpected POST " + self.path)
                self.reply({}, 404)
    return Handler


def certificate(private):
    trust = private / "trust"
    trust.mkdir()
    commands = [
        ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj",
         "/CN=Cobalt local fixture CA", "-keyout", str(private / "ca.key"), "-out", str(trust / "ca.pem")],
        ["req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=lichess.org",
         "-keyout", str(private / "server.key"), "-out", str(private / "server.csr")],
        ["x509", "-req", "-in", str(private / "server.csr"), "-CA", str(trust / "ca.pem"),
         "-CAkey", str(private / "ca.key"), "-CAcreateserial", "-days", "2",
         "-extfile", str(private / "extensions"), "-out", str(private / "server.pem")],
    ]
    (private / "extensions").write_text("subjectAltName=DNS:lichess.org\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
    for command in commands:
        subprocess.run(["openssl"] + command, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, check=True)
    return trust


def action_id(name):
    value = 2166136261
    for byte in name.encode():
        value = ((value ^ byte) * 16777619) & 0xffffffff
    return value


def pieces(layout):
    # Modal overlays disable actions, but the cell's stable ID stays in its
    # layout kind. Pair glyphs with their containing cells instead of relying
    # on reachability or screenshot pixel positions.
    board = {}
    cell = None
    for node in layout["nodes"]:
        found = re.match(r"Cell\(ActionId\((\d+)\), Board", node["kind"])
        if found:
            cell = int(found.group(1))
        elif node["kind"].startswith("InlineGlyph(Chess"):
            assert cell is not None
            board[cell] = node["kind"].split(",", 1)[0]
    return board


def wait_until(predicate, message, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.1)
    raise AssertionError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scale", default="default")
    parser.add_argument("--drop-start-event", action="store_true",
                        help="Match the seek but omit its event-stream notification")
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target"))).resolve()
    cli, provenance = build_cli(ROOT, target)
    match = Match(drop_start_event=args.drop_start_event)
    simulator = None
    with tempfile.TemporaryDirectory(prefix="cobalt-lichess-session-", dir="/tmp") as temporary:
        private = Path(temporary)
        trust = certificate(private)
        secrets = private / "cobalt-sim-secrets"
        secrets.mkdir(mode=0o700)
        (secrets / "lichess").write_text(TOKEN)
        (secrets / "lichess").chmod(0o600)
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler(match))
        server.daemon_threads = True
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(private / "server.pem", private / "server.key")
        server.socket = context.wrap_socket(server.socket, server_side=True)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        env = dict(os.environ, TMPDIR=str(private), RUSTUP_TOOLCHAIN="1.85.1",
                   CARGO_TARGET_DIR=str(target), CARGO_PROFILE_DEV_DEBUG="0",
                   CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1",
                   KOBO_SIM_HTTP_FIXTURE=f"lichess.org=127.0.0.1:{server.server_port}",
                   KOBO_SIM_TRUST_DIR=str(trust), KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_PROFILE="clara-bw-391")
        for key in ["KOBO_SIM_OFFLINE", "KOBO_LICHESS_DEMO", "KOBO_LICHESS_DEMO_BUILD"]:
            env.pop(key, None)
        with (out / "simulator.log").open("w") as log:
            address = None

            def stop():
                nonlocal simulator
                if simulator and simulator.poll() is None:
                    os.killpg(simulator.pid, signal.SIGTERM)
                    simulator.wait(timeout=10)
                simulator = None

            def start():
                nonlocal simulator, address
                address = None
                offset = log.tell()
                simulator = subprocess.Popen([str(cli), "dev", "127.0.0.1:0"],
                    cwd=ROOT / "apps/lichess", env=env, stdout=log, stderr=log, start_new_session=True)

                def ready():
                    nonlocal address
                    if simulator.poll() is not None:
                        raise RuntimeError("Simulator exited; inspect simulator.log")
                    text = (out / "simulator.log").read_text()[offset:]
                    found = re.search(r"Kobo app simulator: http://(127\.0\.0\.1:\d+)", text)
                    if found:
                        address = found.group(1)
                        return True
                    return False
                wait_until(ready, "Simulator did not start", 300)

            def drive(*steps):
                command = [str(cli), "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    command += ["--step", step]
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=45)

            def layout():
                with urllib.request.urlopen(f"http://{address}/layout", timeout=5) as response:
                    return json.load(response)

            def capture(name):
                drive("clean", "shot " + name)
                current = layout()
                (out / (name + ".layout.json")).write_text(json.dumps(current, indent=2) + "\n")
                return current

            def count(path, method="POST"):
                with match.lock:
                    return sum(r == dict(method=method, path=path) for r in match.requests)

            try:
                start()
                drive("wait-for-id play", "tap-id play", "wait-for Owner", "tap Back", "wait-for-id seek-10-0",
                      "tap-id seek-10-0")
                if args.drop_start_event:
                    capture("00-waiting-for-match")
                wait_until(lambda: len(pieces(layout())) == 32, "Matched game did not open", 35)
                drive("wait-for Other", "wait-for-id square-e2")
                matched_board_seconds = time.monotonic() - match.matched_at
                if args.drop_start_event:
                    assert match.start_events_sent == 0
                    paths = [request["path"] for request in match.requests]
                    seek = paths.index("/api/board/seek")
                    board = paths.index(f"/api/board/game/stream/{GAME}")
                    assert "/api/account/playing" in paths[seek + 1:board], "No matched-game reconciliation request"
                    assert "/api/account" not in paths[seek + 1:board], "Recovery depended on account polling"
                    assert matched_board_seconds < 15, "The ten-second pairing check was delayed"

                initial = pieces(capture("01-matched"))
                assert len(initial) == 32
                drive("tap-id square-e2", "expect e2 →", "tap-id square-e4")
                wait_until(lambda: count(f"/api/board/game/{GAME}/move/e2e4") == 1, "Move was not posted")
                drive("wait-for Move sent")
                assert pieces(capture("02-awaiting-ack")) == initial, "POST reply changed the board before stream acknowledgement"
                match.publish(moves="e2e4 e7e5")
                expected = initial.copy()
                expected[action_id("square-e4")] = expected.pop(action_id("square-e2"))
                expected[action_id("square-e5")] = expected.pop(action_id("square-e7"))
                wait_until(lambda: pieces(layout()) == expected, "Acknowledged board position was not rendered")
                capture("03-acknowledged")
                stored = private / "cobalt-sim-state/lichess/lichess.session.v1"
                wait_until(stored.is_file, "Session was not saved")
                saved = stored.read_bytes()
                assert GAME.encode() in saved
                stop()
                board_opens = count(f"/api/board/game/stream/{GAME}", "GET")
                start()
                drive("wait-for Other", "wait-for-id square-e2")
                wait_until(lambda: count(f"/api/board/game/stream/{GAME}", "GET") > board_opens,
                           "Restart did not reopen the board stream")
                assert stored.read_bytes() == saved
                wait_until(lambda: pieces(layout()) == expected, "Restart did not restore the acknowledged position")
                capture("04-resumed")
                match.publish(draw=True)
                drive("wait 500", "tap-id game-menu", "wait-for-id accept-draw", "tap-id accept-draw",
                      "wait-for Draw agreed")
                wait_until(lambda: not stored.exists(), "Finished session was not removed")
                capture("05-finished")
                account_checks = count("/api/account", "GET")
                wait_until(lambda: count("/api/account", "GET") > account_checks,
                           "Account polling did not resume after completion", 25)
                assert count("/api/board/seek") == 1
                assert count(f"/api/board/game/{GAME}/move/e2e4") == 1
                assert count(f"/api/board/game/{GAME}/draw/yes") == 1
                assert not match.errors, match.errors
                verify_cli(cli, provenance)
                (out / "result.json").write_text(json.dumps(dict(status="pass", build=provenance,
                    scale=args.scale, profile="clara-bw-391", requests=match.requests,
                    dropped_start_event=args.drop_start_event, start_events_sent=match.start_events_sent,
                    matched_board_seconds=round(matched_board_seconds, 3),
                    checks=[("recover matched seek without a start event" if args.drop_start_event else "pair through event stream"), "single move submission", "POST success does not move the board", "board stream acknowledgement",
                            "saved session survives process restart", "fresh board stream opened", "restored rendered piece positions", "draw completion clears store", "account polling resumes after completion"],
                    scope="Actual SDK app and simulator with private TLS fixture; no public Lichess requests or hardware validation"), indent=2) + "\n")
            finally:
                (out / "requests.json").write_text(json.dumps(dict(requests=match.requests, errors=match.errors), indent=2) + "\n")
                stop()
                match.stopping.set()
                server.shutdown()
                server.server_close()
                worker.join(timeout=5)
    print("Lichess session fixture passed", flush=True)


if __name__ == "__main__":
    main()
