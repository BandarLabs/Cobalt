"""Run the bridge: the stock musegadget link to Muse plus the reader's HTTPS API."""

from __future__ import annotations

import asyncio
import logging
import socket
from pathlib import Path

from musegadget import config, service as musegadget_service
from musegadget.identity import load_or_create

from kobo_bridge.auth import Auth
from kobo_bridge.board import Board
from kobo_bridge.commands import COMMAND_SPECS, KoboExecutor
from kobo_bridge.server import make_server, serve_in_thread
from kobo_bridge.tls import ensure_certificates

log = logging.getLogger(__name__)

SIDE_SESSION = "kobo-panel"


def lan_addresses() -> list[str]:
    found = {"127.0.0.1"}
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.connect(("192.0.2.1", 9))
            found.add(probe.getsockname()[0])
    except OSError:
        pass
    return sorted(found)


def make_notifier(service, loop: asyncio.AbstractEventLoop, session_id: str = SIDE_SESSION):
    """Return a function that posts a message to Muse from any thread."""

    def notify(message: str) -> None:
        session = service._current
        if session is None:
            log.warning("tap not sent: not connected to Muse")
            return
        future = asyncio.run_coroutine_threadsafe(session.send_chat(message, session_id), loop)
        try:
            result = future.result(timeout=70)
        except Exception as err:
            log.warning("tap not delivered: %s", err)
            return
        if not result.get("ok"):
            log.warning("Muse refused the message: HTTP %s", result.get("status"))

    return notify


async def run(state_dir: Path, host: str, port: int, hosts: list[str],
              allow_private_images: bool, sdk_token: str | None) -> None:
    board = Board(state_dir / "board")
    auth = Auth(state_dir)
    identity = load_or_create()
    executor = KoboExecutor(board, allow_private_images)

    # musegadget builds its device description from this module-level table.
    musegadget_service.COMMAND_SPECS = COMMAND_SPECS
    service = musegadget_service.Service(identity=identity, executor=executor, sdk_token=sdk_token)

    loop = asyncio.get_running_loop()
    certs = ensure_certificates(state_dir / "tls", hosts)
    server = make_server(host, port, board, auth, make_notifier(service, loop), certs)
    serve_in_thread(server)
    log.info("reader API on https://%s:%d, pairing code %s", host, server.server_port, auth.code)
    try:
        await service.run()
    finally:
        server.shutdown()
