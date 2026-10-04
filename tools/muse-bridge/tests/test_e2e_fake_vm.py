"""Muse (the SDK's FakeVm) drives the bridge; the reader's HTTPS API shows the result."""

import pytest
import asyncio
import http.client
import json
import ssl

from musegadget.link_client import DeviceDescription, LinkSession
from musegadget.noise import ApplicationResponse
from musegadget.noise import ServiceFrame
test_link_client = pytest.importorskip(
    "test_link_client", reason="set MUSE_GADGET_SDK to a Muse Gadget SDK checkout"
)
FakeVm, Pipe = test_link_client.FakeVm, test_link_client.Pipe

from kobo_bridge.auth import Auth
from kobo_bridge.board import Board
from kobo_bridge.commands import COMMAND_SPECS, KoboExecutor
from kobo_bridge.server import make_server, serve_in_thread
from kobo_bridge.service import make_notifier
from kobo_bridge.tls import ensure_certificates


class StubService:
    _current = None


def test_muse_invokes_commands_and_the_tap_comes_back(tmp_path):
    async def scenario():
        board, auth = Board(tmp_path / "b"), Auth(tmp_path)
        executor = KoboExecutor(board)
        service, loop = StubService(), asyncio.get_running_loop()
        certs = ensure_certificates(tmp_path / "tls", ["127.0.0.1"])
        server = make_server("127.0.0.1", 0, board, auth, make_notifier(service, loop), certs)
        serve_in_thread(server)
        ctx = ssl.create_default_context(cafile=str(certs[0]))

        def reader(method, path, body=None, token=None):
            conn = http.client.HTTPSConnection("127.0.0.1", server.server_port, context=ctx, timeout=10)
            headers = {"Authorization": "Bearer " + token} if token else {}
            conn.request(method, path, body=json.dumps(body).encode() if body is not None else None,
                         headers=headers)
            resp = conn.getresponse()
            data = resp.read()
            conn.close()
            return resp.status, json.loads(data)

        to_device, to_vm = asyncio.Queue(), asyncio.Queue()
        device_ws, vm_ws = Pipe(to_device, to_vm), Pipe(to_vm, to_device)

        async def connect(url, headers):
            return device_ws

        device = DeviceDescription("homelink-abcdef", "kobo", "0.1.0", COMMAND_SPECS)
        session = LinkSession(noise_host="gw.example", vm_id="vm1", vm_auth_token="tok",
                              device=device, run_command=executor.run, connect=connect)
        service._current = session
        vm = FakeVm(vm_ws)
        task = asyncio.ensure_future(session.run(asyncio.Event()))
        await vm.handshake()
        await vm.accept_control_stream()

        register = await vm.next_message()
        commands = register["params"]["commands_v2"]
        assert {"kobo.show_page", "kobo.ask", "kobo.set_status", "kobo.draw_url"} <= set(commands)
        assert "system.run" not in commands
        await vm.send_message({"type": "res", "id": register["id"], "ok": True})

        _, token = await asyncio.to_thread(reader, "POST", "/v1/pair", {"code": auth.code})
        token = token["token"]

        async def invoke(invoke_id, command, params):
            await vm.send_message({"method": "link.invoke", "id": invoke_id,
                                   "command": command, "params": params})
            return await vm.next_message()

        out = await invoke("i1", "kobo.set_status", {"line": "Planning your Saturday"})
        assert out["ok"] and out["payload"]["rev"] > 1

        out = await invoke("i2", "kobo.show_page", {"title": "Today", "body": "# Plan\n\n- Gym\n- **Lunch** at 1"})
        assert out["ok"] and out["payload"]["blocks"] == 2
        _, screen = await asyncio.to_thread(reader, "GET", "/v1/screen", None, token)
        assert screen["kind"] == "page" and screen["page"]["title"] == "Today"
        assert screen["status"]["line"] == "Planning your Saturday"

        out = await invoke("i3", "kobo.ask", {"question": "Move lunch to 2?", "choices": ["Yes", "No"],
                                              "ask_id": "lunch"})
        assert out["ok"] and out["payload"]["ask_id"] == "lunch"
        _, screen = await asyncio.to_thread(reader, "GET", "/v1/screen", None, token)
        assert screen["kind"] == "ask" and screen["ask"]["choices"][0]["label"] == "Yes"

        tap = asyncio.ensure_future(asyncio.to_thread(
            reader, "POST", "/v1/event", {"ask_id": "lunch", "choice": "c1"}, token))
        request = await asyncio.wait_for(vm.next_frame(), 5)
        assert (request.value.verb, request.value.path) == ("POST", "/chat/stream")
        body = json.loads(request.value.body)
        assert body["session_id"] == "kobo-panel" and body["device_id"] == "homelink-abcdef"
        assert body["message"] == 'Kobo answer: "Yes" (ask lunch: Move lunch to 2?)'
        await vm.send_frame(ServiceFrame.response(
            request.stream_id, ApplicationResponse(status=200, body=b'{"accepted":true}', end_body=True)))
        assert (await tap)[0] == 200

        out = await invoke("i4", "system.run", {"command": "id"})
        assert out["ok"] is False and "unsupported" in out["error"]
        out = await invoke("i5", "kobo.ask", {"question": "?", "choices": ["only"]})
        assert out["ok"] is False and "between 2 and 6" in out["error"]

        await vm.send_message({"type": "evt", "event": "link.unpaired"})
        await asyncio.wait_for(task, 2)
        server.shutdown()

    asyncio.run(scenario())
