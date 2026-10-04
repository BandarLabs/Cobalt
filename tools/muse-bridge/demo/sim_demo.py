"""Muse (the SDK's FakeVm) drives the real bridge; the Cobalt simulator is the reader.

    python demo/sim_demo.py --kobo PATH_TO_kobo --app APP_DIR --out SHOTS --profile clara-bw-391

Everything runs on loopback with a private CA, a private TMPDIR and a throwaway
pairing. Nothing leaves this machine.
"""

import argparse
import asyncio
import http.server
import json
import os
import re
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import zlib
import struct
from pathlib import Path

# The SDK's own test harness (FakeVm) stands in for Muse. Point MUSE_GADGET_SDK
# at a checkout of the Muse Gadget SDK; its linux/ package must be installed.
_sdk = os.environ.get("MUSE_GADGET_SDK")
if not _sdk or not (Path(_sdk) / "linux/tests/test_link_client.py").is_file():
    sys.exit("set MUSE_GADGET_SDK to a Muse Gadget SDK checkout (linux/tests/test_link_client.py)")
sys.path.insert(0, str(Path(_sdk) / "linux/tests"))

from musegadget.link_client import DeviceDescription, LinkSession  # noqa: E402
from musegadget.noise import ApplicationResponse, ServiceFrame  # noqa: E402
from test_link_client import FakeVm, Pipe  # noqa: E402

from kobo_bridge.auth import Auth  # noqa: E402
from kobo_bridge.board import Board  # noqa: E402
from kobo_bridge.commands import COMMAND_SPECS, KoboExecutor  # noqa: E402
from kobo_bridge.server import make_server, serve_in_thread  # noqa: E402
from kobo_bridge.service import make_notifier  # noqa: E402
from kobo_bridge.tls import ensure_certificates  # noqa: E402

PAGE = """Two things need you, and the rest can wait.

- **Gym** at 9, bring the blue bag
- **Lunch** with Priya at 1, Rosemary Cafe
- Pick up the parcel before 5

> Rain from about four, so leave the bike at home.

## Tomorrow

Nothing is booked. The morning is yours.
"""


def png(width: int, height: int) -> bytes:
    """A sunrise over water, drawn from nothing, so the demo needs no assets."""
    rows = []
    cx, cy = width // 2, int(height * 0.58)
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            if y < cy:
                t = y / cy
                r, g, b = int(250 - 120 * t), int(170 - 90 * t), int(90 + 70 * t)
                d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
                if d < height * 0.2:
                    r, g, b = 255, 238, 190
                elif d < height * 0.26:
                    r, g, b = min(255, r + 25), min(255, g + 40), b
            else:
                t = (y - cy) / (height - cy)
                wave = 12 * ((x * 0.05 + y * 0.3) % 2 < 1)
                r, g, b = int(40 + 30 * t) + wave, int(70 + 40 * t) + wave, int(110 + 40 * t) + wave
            row += bytes((r, g, b))
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def typing_steps(text, layer="letters"):
    """kobo drive steps that type text, switching keyboard layers when needed."""
    steps = []
    for char in text:
        want = "symbols" if not char.isalpha() else "letters"
        if want != layer:
            steps.append("tap ?123" if want == "symbols" else "tap abc")
            layer = want
        steps.append(f"type {char}")
    return steps


async def main(args) -> int:
    root = Path(tempfile.mkdtemp(prefix="mk-", dir="/tmp"))
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    state = root / "state"
    state.mkdir()
    trust = root / "trust"
    trust.mkdir()

    images = http.server.ThreadingHTTPServer(("127.0.0.1", 0), type("H", (http.server.BaseHTTPRequestHandler,), {
        "do_GET": lambda self: (self.send_response(200), self.send_header("Content-Type", "image/png"),
                                self.end_headers(), self.wfile.write(png(720, 480)))[-1],
        "log_message": lambda *a: None}))
    threading.Thread(target=images.serve_forever, daemon=True).start()

    board, auth = Board(root / "board"), Auth(root / "auth")
    executor = KoboExecutor(board, allow_private_images=True)

    class Stub:
        _current = None

    service, loop = Stub(), asyncio.get_running_loop()
    certs = ensure_certificates(root / "tls", ["127.0.0.1"])
    (trust / "muse-bridge.pem").write_bytes(certs[0].read_bytes())
    server = make_server("127.0.0.1", 0, board, auth, make_notifier(service, loop), certs)
    serve_in_thread(server)
    port = server.server_port

    to_device, to_vm = asyncio.Queue(), asyncio.Queue()
    device_ws, vm_ws = Pipe(to_device, to_vm), Pipe(to_vm, to_device)

    async def connect(url, headers):
        return device_ws

    device = DeviceDescription("kobo-demo", "kobo", "0.1.0", COMMAND_SPECS)
    session = LinkSession(noise_host="gw.example", vm_id="vm1", vm_auth_token="tok", device=device,
                          run_command=executor.run, connect=connect)
    service._current = session
    vm = FakeVm(vm_ws)
    task = asyncio.ensure_future(session.run(asyncio.Event()))
    await vm.handshake()
    await vm.accept_control_stream()
    register = await vm.next_message()
    await vm.send_message({"type": "res", "id": register["id"], "ok": True})

    count = 0

    async def invoke(command, params):
        nonlocal count
        count += 1
        await vm.send_message({"method": "link.invoke", "id": f"i{count}", "command": command, "params": params})
        reply = await vm.next_message()
        print("muse>", command, "->", "ok" if reply.get("ok") else reply.get("error"), flush=True)
        assert reply.get("ok"), reply
        return reply["payload"]

    # Pair the way the reader would, so the simulator starts already paired.
    ctx = ssl.create_default_context(cafile=str(certs[0]))
    req = urllib.request.Request(f"https://127.0.0.1:{port}/v1/pair", data=json.dumps({"code": auth.code}).encode(),
                                 headers={"Content-Type": "application/json"})
    if not args.pairing:
        token = json.load(urllib.request.urlopen(req, context=ctx))["token"]
        store = state / "cobalt-sim-state" / "muse-panel"
        store.mkdir(parents=True)
        (store / "paired").write_bytes(f"127.0.0.1:{port}\n{token}".encode())

    env = dict(os.environ, TMPDIR=str(state), KOBO_SIM_TRUST_DIR=str(trust), KOBO_SIM_PROFILE=args.profile,
               KOBO_TEXT_SCALE=args.scale)
    for key in ("KOBO_SIM_OFFLINE", "KOBO_SIM_DEMO", "KOBO_SIM_HTTP_FIXTURE"):
        env.pop(key, None)
    log_path = root / "sim.log"
    with log_path.open("w") as log:
        proc = subprocess.Popen([args.kobo, "dev", "127.0.0.1:0"], cwd=args.app, env=env, stdout=log,
                                stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 120
            address = None
            while time.monotonic() < deadline:
                assert proc.poll() is None, log_path.read_text()[-2000:]
                found = re.search(r"Kobo app simulator: http://(127\.0\.0\.1:\d+)", log_path.read_text())
                if found:
                    address = found.group(1)
                    break
                await asyncio.sleep(0.2)
            assert address, "simulator did not start"

            async def drive(*steps):
                cmd = [args.kobo, "drive", "--address", address, "--ideal", "--shots", str(out)]
                for step in steps:
                    cmd += ["--step", step]
                done = await asyncio.to_thread(subprocess.run, cmd, env=env, stdout=log, stderr=log, timeout=90)
                assert done.returncode == 0, (steps, log_path.read_text()[-1500:])

            if args.pairing:
                await drive("wait-for Pair with your computer", "wait-idle", "shot pair-address")
                await drive("tap ?123", f"type 127.0.0.1:{port}", "wait-idle", "shot pair-address-typed", "tap Next",
                            "wait-for Now the pairing code", "wait-idle", "shot pair-code")
                await drive(*typing_steps(auth.code.lower(), "symbols"), "wait-idle", "shot pair-code-typed", "tap Pair",
                            "wait-for Waiting for Muse", "wait-idle", "shot pair-done")
                return
            await drive("wait-for Waiting for Muse")
            await invoke("kobo.set_status", {"line": "Planning your Saturday", "detail": "Nothing needs you yet."})
            await drive("wait-for Planning your Saturday", "wait-idle", "shot resting")

            await invoke("kobo.show_page", {"title": "Saturday", "body": PAGE})
            await drive("wait-for Rain from about four", "wait-idle", "shot page")

            await invoke("kobo.ask", {"question": "Move lunch with Priya to 2pm?",
                                      "context": "The Rosemary Cafe table is free at both times.",
                                      "choices": ["Yes, move it", "Keep 1pm", "Ask me later"], "ask_id": "lunch"})
            await drive("wait-for Move lunch with Priya", "wait-idle", "shot ask")

            tap = asyncio.ensure_future(vm.next_frame())
            await drive("tap Yes, move it")
            request = await asyncio.wait_for(tap, 20)
            body = json.loads(request.value.body)
            print("muse< ", request.value.verb, request.value.path, body["message"], flush=True)
            await vm.send_frame(ServiceFrame.response(
                request.stream_id, ApplicationResponse(status=200, body=b'{"accepted":true}', end_body=True)))
            (out / "tap-received-by-muse.txt").write_text(
                f"{request.value.verb} {request.value.path}\nsession_id: {body['session_id']}\n"
                f"device_id: {body['device_id']}\nmessage: {body['message']}\n")
            await invoke("kobo.set_status", {"line": "Lunch moved to 2pm", "detail": "I told Priya."})
            await invoke("kobo.clear", {})
            await drive("wait-for Lunch moved to 2pm", "wait-idle", "shot answered")

            await invoke("kobo.draw_url", {"url": f"http://127.0.0.1:{images.server_port}/dawn.png"})
            await drive("wait 6000", "shot picture")
            await invoke("kobo.device_info", {})
            print("device_info:", board.hello(), flush=True)
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
    await vm.send_message({"type": "evt", "event": "link.unpaired"})
    await asyncio.wait_for(task, 5)
    server.shutdown()
    images.shutdown()
    return 0


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--kobo", required=True)
    p.add_argument("--app", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--profile", default="clara-bw-391")
    p.add_argument("--scale", default="default")
    p.add_argument("--pairing", action="store_true",
                   help="start unpaired, type the address and code, and shoot each pairing screen")
    sys.exit(asyncio.run(main(p.parse_args())))
