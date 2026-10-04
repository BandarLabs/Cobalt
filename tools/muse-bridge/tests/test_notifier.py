import asyncio
import threading

from kobo_bridge.service import make_notifier


class Session:
    def __init__(self):
        self.cancelled = threading.Event()
        self.started = threading.Event()

    async def send_chat(self, message, session_id):
        self.started.set()
        try:
            await asyncio.sleep(30)
        except asyncio.CancelledError:
            self.cancelled.set()
            raise


class Service:
    def __init__(self, session):
        self._current = session


def test_a_send_that_times_out_is_cancelled_not_left_running():
    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    try:
        session = Session()
        notify = make_notifier(Service(session), loop, timeout=0.3)
        assert notify("Kobo answer") is False
        assert session.started.is_set()
        assert session.cancelled.wait(5)
    finally:
        loop.call_soon_threadsafe(loop.stop)
        thread.join(5)


def test_no_session_is_a_failed_send():
    loop = asyncio.new_event_loop()
    assert make_notifier(Service(None), loop)("x") is False
    loop.close()
