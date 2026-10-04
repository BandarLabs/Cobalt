import http.server
import threading

import pytest

from kobo_bridge.board import Board
from kobo_bridge.commands import KoboExecutor

PNG = b"\x89PNG\r\n\x1a\n" + b"\x00" * 64


@pytest.fixture
def image_server():
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = PNG if self.path == "/ok.png" else b"<html>"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield "http://127.0.0.1:%d" % server.server_port
    server.shutdown()


def test_draw_url_refuses_private_hosts_by_default(tmp_path, image_server):
    run = KoboExecutor(Board(tmp_path)).run
    out = run("kobo.draw_url", {"url": image_server + "/ok.png"})
    assert out["ok"] is False and "public address" in out["error"]


def test_draw_url_and_image_blocks(tmp_path, image_server):
    board = Board(tmp_path)
    run = KoboExecutor(board, allow_private_images=True).run
    assert run("kobo.draw_url", {"url": image_server + "/ok.png"})["ok"]
    snap = board.snapshot()
    assert snap["kind"] == "image" and snap["image"]["mime"] == "image/png"
    assert board.blob(snap["image"]["blob"]) == PNG

    # A page holds text only: an image line becomes its description and nothing is fetched.
    result = run("kobo.show_page", {"body": "Intro\n\n![chart](https://example.com/c.png)\n\n![](https://example.com/x.png)"})
    assert result["ok"]
    blocks = board.snapshot()["page"]["blocks"]
    assert [b["t"] for b in blocks] == ["p", "p", "p"]
    assert blocks[1]["spans"][0]["s"] == "[chart]" and blocks[2]["spans"][0]["s"] == "[image]"


def test_device_info_before_and_after_hello(tmp_path):
    board = Board(tmp_path)
    run = KoboExecutor(board).run
    assert run("kobo.device_info", {})["payload"]["seen"] is False
    board.record_hello({"model": "Clara BW", "w": 1072, "h": 1448, "colour": False, "battery": 80})
    info = run("kobo.device_info", {})["payload"]
    assert info["seen"] and info["w"] == 1072 and info["colour"] is False


def test_show_text_and_clear(tmp_path):
    board = Board(tmp_path)
    run = KoboExecutor(board).run
    assert run("kobo.show_text", {"text": "One.\n\nTwo."})["ok"]
    assert len(board.snapshot()["page"]["blocks"]) == 2
    assert run("kobo.clear", {})["ok"] and board.snapshot()["kind"] == "status"
    assert run("kobo.show_text", {"text": "  "})["ok"] is False


def test_only_the_display_commands_are_exposed_and_others_are_refused(tmp_path):
    from kobo_bridge.commands import COMMAND_SPECS

    assert sorted(COMMAND_SPECS) == [
        "kobo.ask",
        "kobo.clear",
        "kobo.device_info",
        "kobo.draw_url",
        "kobo.set_status",
        "kobo.show_page",
        "kobo.show_text",
    ]
    executor = KoboExecutor(Board(tmp_path))
    for name in ("shell.run", "file.read", "kobo.exec", "linux.shell"):
        reply = executor.run(name, {})
        assert not reply.get("ok"), name
