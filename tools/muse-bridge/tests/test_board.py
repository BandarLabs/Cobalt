import pytest

from kobo_bridge.board import Board, BoardError


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now


@pytest.fixture
def clock():
    return Clock()


@pytest.fixture
def board(tmp_path, clock):
    return Board(tmp_path, clock)


def test_rev_moves_and_unchanged_is_short(board):
    first = board.snapshot()
    assert first["kind"] == "status" and first["status"]["line"] == "Waiting for Muse"
    assert board.snapshot(first["rev"]) == {"rev": first["rev"], "unchanged": True}
    board.set_status("Writing the brief")
    assert board.snapshot(first["rev"])["status"]["line"] == "Writing the brief"


def test_page_replaces_screen_and_expires(board, clock):
    board.set_status("idle")
    out = board.show_page("T", [{"t": "p", "spans": [{"s": "hi"}]}], keep_s=30)
    assert board.snapshot()["kind"] == "page"
    clock.now += 31
    shot = board.snapshot()
    assert shot["kind"] == "status" and shot["page"] is None
    assert shot["rev"] > out["rev"]


def test_ask_answer_once(board):
    r = board.ask("Ship it?", ["Yes", {"label": "Not yet", "id": "later"}], ask_id="a1")
    snap = board.snapshot()
    assert snap["kind"] == "ask"
    assert [c["id"] for c in snap["ask"]["choices"]] == ["c1", "later"]
    answered = board.answer("a1", "later")
    assert answered["label"] == "Not yet" and r["ask_id"] == "a1"
    assert board.snapshot()["kind"] == "ask"  # until Muse has the answer
    with pytest.raises(BoardError):
        board.answer("a1", "later")
    board.confirm_answer("a1")
    after = board.snapshot()
    assert after["kind"] == "status" and "Not yet" in after["status"]["detail"]
    with pytest.raises(BoardError):
        board.answer("a1", "later")


def test_a_pending_answer_can_be_reopened_and_survives_no_restart(board, tmp_path, clock):
    board.ask("Ship it?", ["Yes", "No"], ask_id="a1")
    board.answer("a1", "c1")
    board.reopen_answer("a1")
    assert board.snapshot()["ask"]["answered"] is None
    board.answer("a1", "c2")
    # A restart cannot say whether Muse got a pending answer, so the question reopens.
    again = Board(tmp_path, clock)
    assert again.snapshot()["kind"] == "ask" and again.snapshot()["ask"]["answered"] is None


def test_question_and_context_stay_within_what_the_reader_can_show(board):
    board.ask("q" * 60, ["Yes", "No"], context="c" * 120)
    with pytest.raises(BoardError):
        board.ask("q" * 61, ["Yes", "No"])
    with pytest.raises(BoardError):
        board.ask("Fine?", ["Yes", "No"], context="c" * 121)


def test_a_rejected_deadline_changes_nothing(board):
    board.ask("Ship it?", ["Yes", "No"], ask_id="a1")
    rev = board.snapshot()["rev"]
    with pytest.raises(BoardError):
        board.ask("Another?", ["Yes", "No"], ask_id="a2", expires_s=1)
    snap = board.snapshot()
    assert snap["rev"] == rev and snap["ask"]["ask_id"] == "a1"
    with pytest.raises(BoardError):
        board.show_page("T", [{"t": "p", "spans": [{"s": "x"}]}], keep_s=1)
    snap = board.snapshot()
    assert snap["rev"] == rev and snap["kind"] == "ask"


def test_ask_expiry_and_wrong_ids(board, clock):
    board.ask("Q?", ["a", "b"], ask_id="a1", expires_s=60)
    with pytest.raises(BoardError):
        board.answer("other", "c1")
    with pytest.raises(BoardError):
        board.answer("a1", "zzz")
    clock.now += 61
    with pytest.raises(BoardError):
        board.answer("a1", "c1")
    assert board.snapshot()["kind"] == "status"


def test_ask_validation(board):
    for bad in (["only"], ["a"] * 7, [{"label": ""}, "b"], [{"label": "a", "id": "x"}, {"label": "b", "id": "x"}]):
        with pytest.raises(BoardError):
            board.ask("Q?", bad)
    with pytest.raises(BoardError):
        board.ask("", ["a", "b"])


def test_state_survives_restart(tmp_path, clock):
    Board(tmp_path, clock).set_status("kept")
    assert Board(tmp_path, clock).snapshot()["status"]["line"] == "kept"


def test_blob_names_are_not_paths(board):
    blob = board.put_blob(b"\xff\xd8\xff\xe0data", "image/jpeg")
    assert board.blob(blob["blob"]) == b"\xff\xd8\xff\xe0data"
    assert board.blob("../../etc/passwd") is None


def test_long_poll_wakes_on_change(board):
    import threading, time
    rev = board.snapshot()["rev"]
    threading.Timer(0.2, lambda: board.set_status("x")).start()
    start = time.monotonic()
    shot = board.wait_snapshot(rev, 5)
    assert shot["status"]["line"] == "x" and time.monotonic() - start < 3
    assert board.wait_snapshot(shot["rev"], 0.3).get("unchanged") is True
