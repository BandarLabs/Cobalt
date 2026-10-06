"""HTTPS API the reader polls. One screen, one question at a time."""

from __future__ import annotations

import json
import logging
import ssl
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable
from urllib.parse import parse_qs, urlparse

from kobo_bridge.auth import Auth
from kobo_bridge.board import Board, BoardError

log = logging.getLogger(__name__)

MAX_BODY = 8 * 1024
MAX_WAIT_S = 25
Notify = Callable[[str], None]


def answer_message(ask: dict) -> str:
    return 'Kobo answer: "%s" (ask %s: %s)' % (ask["label"], ask["ask_id"], ask["question"])


def make_server(host: str, port: int, board: Board, auth: Auth, notify: Notify,
                certs: tuple[Path, Path, Path] | None = None) -> ThreadingHTTPServer:
    class Handler(BaseHTTPRequestHandler):
        server_version = "kobo-bridge"
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            log.debug("%s %s", self.address_string(), fmt % args)

        # -- plumbing --------------------------------------------------------

        def _send(self, status: int, body: bytes, ctype: str = "application/json") -> None:
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def _json(self, status: int, obj: dict) -> None:
            self._send(status, json.dumps(obj, separators=(",", ":")).encode())

        def _body(self) -> dict | None:
            try:
                length = int(self.headers.get("Content-Length") or 0)
            except ValueError:
                length = -1
            if not 0 <= length <= MAX_BODY:
                self._json(413, {"error": "body too large"})
                return None
            try:
                data = json.loads(self.rfile.read(length) or b"{}")
            except json.JSONDecodeError:
                self._json(400, {"error": "body is not JSON"})
                return None
            if not isinstance(data, dict):
                self._json(400, {"error": "body must be an object"})
                return None
            return data

        def _authorized(self) -> bool:
            header = self.headers.get("Authorization", "")
            if header.startswith("Bearer ") and auth.valid(header[7:]):
                return True
            if auth.valid(self.headers.get("X-Muse-Panel-Token", "")):
                return True
            self._json(401, {"error": "not paired"})
            return False

        # -- routes ----------------------------------------------------------

        def do_GET(self):
            url = urlparse(self.path)
            if url.path == "/v1/ping":
                return self._json(200, {"ok": True})
            if not self._authorized():
                return
            if url.path == "/v1/screen":
                rev = parse_qs(url.query).get("rev", [""])[0]
                known = int(rev) if rev.isdigit() else None
                wait = parse_qs(url.query).get("wait", ["0"])[0]
                wait_s = min(int(wait), MAX_WAIT_S) if wait.isdigit() else 0
                return self._json(200, board.wait_snapshot(known, wait_s))
            if url.path.startswith("/v1/blob/"):
                data = board.blob(url.path[len("/v1/blob/"):])
                if data is None:
                    return self._json(404, {"error": "no such image"})
                ctype = "image/png" if data.startswith(b"\x89PNG") else "image/jpeg"
                return self._send(200, data, ctype)
            self._json(404, {"error": "not found"})

        def do_POST(self):
            path = urlparse(self.path).path
            body = self._body()
            if body is None:
                return
            if path == "/v1/pair":
                if auth.locked():
                    return self._json(429, {"error": "too many wrong codes; wait a minute"})
                token = auth.pair(str(body.get("code", "")))
                if token is None:
                    return self._json(403, {"error": "wrong code"})
                return self._json(200, {"token": token})
            if not self._authorized():
                return
            if path == "/v1/hello":
                board.record_hello(body)
                return self._json(200, {"ok": True})
            if path == "/v1/event":
                try:
                    ask = board.answer(str(body.get("ask_id", "")), str(body.get("choice", "")))
                except BoardError as err:
                    return self._json(409, {"error": str(err)})
                try:
                    delivered = bool(notify(answer_message(ask)))
                except Exception:
                    log.exception("sending the answer to Muse failed")
                    delivered = False
                if not delivered:
                    board.reopen_answer(ask["ask_id"])
                    return self._json(502, {"error": "Muse did not get the answer; tap it again"})
                board.confirm_answer(ask["ask_id"])
                return self._json(200, {"ok": True})
            self._json(404, {"error": "not found"})

    class Quiet(ThreadingHTTPServer):
        def handle_error(self, request, client_address):
            log.debug("connection from %s dropped", client_address)

    server = Quiet((host, port), Handler)
    server.daemon_threads = True
    if certs is not None:
        _, cert, key = certs
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    return server


def serve_in_thread(server: ThreadingHTTPServer) -> threading.Thread:
    thread = threading.Thread(target=server.serve_forever, name="kobo-bridge-http", daemon=True)
    thread.start()
    return thread
