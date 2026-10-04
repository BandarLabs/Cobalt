import http.client
import json
import ssl

import pytest

from kobo_bridge.auth import Auth
from kobo_bridge.board import Board
from kobo_bridge.server import answer_message, make_server, serve_in_thread
from kobo_bridge.tls import ensure_certificates


@pytest.fixture
def stack(tmp_path):
    board, auth = Board(tmp_path / "b"), Auth(tmp_path)
    sent = []
    certs = ensure_certificates(tmp_path / "tls", ["127.0.0.1"])
    server = make_server("127.0.0.1", 0, board, auth, sent.append, certs)
    serve_in_thread(server)
    ctx = ssl.create_default_context(cafile=str(certs[0]))

    def call(method, path, body=None, token=None, extra=None):
        conn = http.client.HTTPSConnection("127.0.0.1", server.server_port, context=ctx, timeout=10)
        headers = {"Authorization": "Bearer " + token} if token else {}
        headers.update(extra or {})
        data = json.dumps(body).encode() if body is not None else None
        conn.request(method, path, body=data, headers=headers)
        resp = conn.getresponse()
        raw = resp.read()
        conn.close()
        return resp.status, raw

    yield board, auth, sent, call
    server.shutdown()


def pair(auth, call):
    status, raw = call("POST", "/v1/pair", {"code": auth.code})
    assert status == 200
    return json.loads(raw)["token"]


def test_requires_pairing(stack):
    _, auth, _, call = stack
    assert call("GET", "/v1/screen")[0] == 401
    assert call("POST", "/v1/pair", {"code": "WRONG1"})[0] == 403
    token = pair(auth, call)
    assert call("GET", "/v1/screen", token=token)[0] == 200


def test_query_token_is_refused(stack):
    _, auth, _, call = stack
    token = pair(auth, call)
    assert call("GET", "/v1/screen?t=" + token)[0] == 401


def test_app_token_header_is_accepted_and_wrong_one_refused(stack):
    _, auth, _, call = stack
    token = pair(auth, call)
    assert call("GET", "/v1/screen", extra={"X-Muse-Panel-Token": token})[0] == 200
    assert call("GET", "/v1/screen", extra={"X-Muse-Panel-Token": "nope"})[0] == 401


def test_lockout_after_wrong_codes(stack):
    _, auth, _, call = stack
    for _ in range(5):
        call("POST", "/v1/pair", {"code": "NOPE00"})
    assert call("POST", "/v1/pair", {"code": auth.code})[0] == 429


def test_screen_unchanged_reply(stack):
    board, auth, _, call = stack
    token = pair(auth, call)
    rev = json.loads(call("GET", "/v1/screen", token=token)[1])["rev"]
    status, raw = call("GET", "/v1/screen?rev=%d" % rev, token=token)
    assert json.loads(raw) == {"rev": rev, "unchanged": True}


def test_tap_reaches_notifier_once(stack):
    board, auth, sent, call = stack
    token = pair(auth, call)
    board.ask("Lunch?", ["Ramen", "Salad"], ask_id="lunch")
    assert call("POST", "/v1/event", {"ask_id": "lunch", "choice": "c1"}, token)[0] == 200
    assert sent == ['Kobo answer: "Ramen" (ask lunch: Lunch?)']
    assert call("POST", "/v1/event", {"ask_id": "lunch", "choice": "c1"}, token)[0] == 409
    assert len(sent) == 1


def test_blob_and_hello(stack):
    board, auth, _, call = stack
    token = pair(auth, call)
    blob = board.put_blob(b"\x89PNG\r\n\x1a\nxx", "image/png")
    status, raw = call("GET", "/v1/blob/" + blob["blob"], token=token)
    assert (status, raw[:4]) == (200, b"\x89PNG")
    assert call("GET", "/v1/blob/..%2f..%2fauth.json", token=token)[0] == 404
    assert call("POST", "/v1/hello", {"model": "Libra Colour", "w": 1264, "h": 1680,
                                      "colour": True, "battery": 71}, token)[0] == 200
    assert board.hello()["model"] == "Libra Colour"


def test_oversize_and_bad_json(stack):
    _, auth, _, call = stack
    token = pair(auth, call)
    assert call("POST", "/v1/hello", {"x": "a" * 20000}, token)[0] == 413


def test_a_pairing_code_works_once(stack):
    _, auth, _, call = stack
    first = auth.code
    status, raw = call("POST", "/v1/pair", {"code": first})
    assert status == 200
    token = json.loads(raw)["token"]
    assert auth.code != first
    assert call("POST", "/v1/pair", {"code": first})[0] == 403
    assert call("GET", "/v1/screen", token=token)[0] == 200
    status, _ = call("POST", "/v1/pair", {"code": auth.code})
    assert status == 200
