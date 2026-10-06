"""What the reader shows, and the questions it has been asked.

One Board holds the current screen. Every change bumps `rev`; the reader polls
with the rev it last drew and gets a short "unchanged" reply until it moves.
State is written to disk after each change, so a restart keeps the screen.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import threading
import time
import uuid
from pathlib import Path
from typing import Callable

DEFAULT_STATUS_LINE = "Waiting for Muse"
MAX_CHOICES = 6
MAX_LABEL = 60
MAX_QUESTION = 60
MAX_CONTEXT = 120
DEFAULT_ASK_TTL_S = 3600
MAX_ASK_TTL_S = 24 * 3600
MAX_EVENTS = 200
_ID = re.compile(r"^[A-Za-z0-9_.-]{1,48}$")


class BoardError(ValueError):
    pass


class Board:
    def __init__(self, directory: Path, clock: Callable[[], float] = time.time) -> None:
        self.dir = Path(directory)
        self.dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        (self.dir / "blobs").mkdir(mode=0o700, exist_ok=True)
        self._clock = clock
        self._lock = threading.RLock()
        self._changed = threading.Condition(self._lock)
        self._path = self.dir / "screen.json"
        self.state = self._load()

    # -- Persistence ---------------------------------------------------------

    def _load(self) -> dict:
        try:
            state = json.loads(self._path.read_text(encoding="utf-8"))
            if isinstance(state, dict) and state.get("kind"):
                ask = state.get("ask")
                if isinstance(ask, dict) and ask.get("delivery") == "pending":
                    # A restart cannot tell whether Muse got it; ask again.
                    ask["answered"] = None
                    ask.pop("delivery", None)
                return state
        except (OSError, json.JSONDecodeError):
            pass
        return {
            "rev": 1,
            "kind": "status",
            "status": {"line": DEFAULT_STATUS_LINE, "detail": "", "at": self._clock()},
            "page": None,
            "ask": None,
            "image": None,
            "expires_at": None,
        }

    def _save(self) -> None:
        tmp = self._path.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(self.state, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self._path)

    def _bump(self) -> int:
        self.state["rev"] += 1
        self._save()
        self._changed.notify_all()
        return self.state["rev"]

    # -- Screen changes (called from Muse commands) --------------------------

    def set_status(self, line: str, detail: str = "") -> int:
        line = _text(line, "line", 120)
        detail = _text(detail, "detail", 300, required=False)
        with self._lock:
            self.state["status"] = {"line": line, "detail": detail, "at": self._clock()}
            return self._bump()

    def show_page(self, title: str, blocks: list[dict], page_id: str | None = None,
                  keep_s: int | None = None) -> dict:
        title = _text(title, "title", 120, required=False)
        page_id = _identifier(page_id) or "page-" + uuid.uuid4().hex[:8]
        with self._lock:
            deadline = self._deadline(keep_s, 86400)
            self._clear_screen()
            self.state["kind"] = "page"
            self.state["page"] = {"id": page_id, "title": title, "blocks": blocks}
            self.state["expires_at"] = deadline
            return {"page_id": page_id, "rev": self._bump()}

    def ask(self, question: str, choices: list, context: str = "",
            ask_id: str | None = None, expires_s: int | None = None) -> dict:
        question = _text(question, "question", MAX_QUESTION)
        context = _text(context, "context", MAX_CONTEXT, required=False)
        normalized = _choices(choices)
        ask_id = _identifier(ask_id) or "ask-" + uuid.uuid4().hex[:8]
        with self._lock:
            self._expire()
            if self.state["ask"] and self.state["ask"]["ask_id"] == ask_id \
                    and self.state["ask"].get("answered") is None:
                raise BoardError("an ask with this id is already waiting")
            deadline = self._deadline(expires_s, DEFAULT_ASK_TTL_S)
            self._clear_screen()
            self.state["kind"] = "ask"
            self.state["ask"] = {
                "ask_id": ask_id, "question": question, "context": context,
                "choices": normalized, "answered": None,
            }
            self.state["expires_at"] = deadline
            return {"ask_id": ask_id, "rev": self._bump()}

    def show_image(self, blob: dict, fit: str = "contain") -> dict:
        if fit not in ("contain", "fill"):
            raise BoardError("fit must be contain or fill")
        with self._lock:
            self._clear_screen()
            self.state["kind"] = "image"
            self.state["image"] = {**blob, "fit": fit}
            self.state["expires_at"] = None
            return {"rev": self._bump()}

    def clear(self) -> int:
        with self._lock:
            self._clear_screen()
            return self._bump()

    def _clear_screen(self) -> None:
        self.state["kind"] = "status"
        self.state["page"] = None
        self.state["image"] = None
        self.state["expires_at"] = None
        ask = self.state.get("ask")
        if ask and ask.get("answered") is None:
            self.state["ask"] = None

    def _deadline(self, seconds: int | None, default: int) -> float:
        if seconds is None:
            seconds = default
        if not isinstance(seconds, int) or isinstance(seconds, bool) or seconds < 5:
            raise BoardError("seconds must be a whole number of at least 5")
        return self._clock() + min(seconds, MAX_ASK_TTL_S)

    def _expire(self) -> None:
        deadline = self.state.get("expires_at")
        if deadline and self._clock() >= deadline and self.state["kind"] != "status":
            if self.state["kind"] == "ask":
                self.state["ask"] = None
            self._clear_screen()
            self._bump()

    # -- Reader side ---------------------------------------------------------

    def snapshot(self, known_rev: int | None = None) -> dict:
        with self._lock:
            self._expire()
            if known_rev is not None and known_rev == self.state["rev"]:
                return {"rev": self.state["rev"], "unchanged": True}
            s = self.state
            return {
                "rev": s["rev"], "kind": s["kind"], "status": s["status"],
                "page": s["page"], "ask": s["ask"] if s["kind"] == "ask" else None,
                "image": s["image"], "expires_at": s["expires_at"],
                "now": self._clock(),
            }

    def answer(self, ask_id: str, choice_id: str) -> dict:
        """Record a tap. Returns the answered ask; raises BoardError otherwise."""
        with self._lock:
            self._expire()
            ask = self.state.get("ask")
            if not ask or ask["ask_id"] != ask_id:
                raise BoardError("that question is no longer waiting")
            if ask.get("answered") is not None:
                if ask.get("delivery") == "pending":
                    raise BoardError("that answer is still being sent")
                raise BoardError("that question was already answered")
            choice = next((c for c in ask["choices"] if c["id"] == choice_id), None)
            if choice is None:
                raise BoardError("unknown choice")
            # The answer is held as pending until Muse has it. The screen stays
            # on the question, so a failed send can be tapped again.
            ask["answered"] = choice["id"]
            ask["delivery"] = "pending"
            self._save()
            return {**ask, "label": choice["label"]}

    def confirm_answer(self, ask_id: str) -> None:
        """Muse has the answer: leave the question and say what was sent."""
        with self._lock:
            ask = self.state.get("ask")
            if not ask or ask["ask_id"] != ask_id or ask.get("delivery") != "pending":
                return
            ask["delivery"] = "sent"
            label = next((c["label"] for c in ask["choices"] if c["id"] == ask["answered"]), "")
            self._clear_screen()
            self.state["status"] = {
                "line": self.state["status"]["line"],
                "detail": 'Sent "%s" to Muse' % label,
                "at": self._clock(),
            }
            self._bump()

    def reopen_answer(self, ask_id: str) -> None:
        """Muse did not get the answer: the question can be answered again."""
        with self._lock:
            ask = self.state.get("ask")
            if ask and ask["ask_id"] == ask_id and ask.get("delivery") == "pending":
                ask["answered"] = None
                ask.pop("delivery", None)
                self._save()

    def wait_snapshot(self, known_rev: int | None, wait_s: float) -> dict:
        """Like snapshot, but hold the request until the rev moves or wait_s passes."""
        end = time.monotonic() + max(0.0, wait_s)
        with self._changed:
            while known_rev is not None and known_rev == self.state["rev"]:
                self._expire()
                left = end - time.monotonic()
                if known_rev != self.state["rev"] or left <= 0:
                    break
                self._changed.wait(min(left, 1.0))
            return self.snapshot(known_rev)

    # -- Reader hello (device facts for kobo.device_info) --------------------

    def record_hello(self, facts: dict) -> None:
        keep = {k: facts.get(k) for k in ("model", "fw", "w", "h", "colour", "battery")}
        with self._lock:
            path = self.dir / "hello.json"
            path.write_text(json.dumps({**keep, "seen_at": self._clock()}), encoding="utf-8")

    def hello(self) -> dict | None:
        try:
            return json.loads((self.dir / "hello.json").read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None

    # -- Blobs ---------------------------------------------------------------

    def put_blob(self, data: bytes, mime: str) -> dict:
        digest = hashlib.sha256(data).hexdigest()
        name = digest[:32]
        path = self.dir / "blobs" / name
        if not path.exists():
            tmp = path.with_suffix(".tmp")
            tmp.write_bytes(data)
            os.replace(tmp, path)
        self._trim_blobs(keep=name)
        return {"blob": name, "mime": mime, "sha256": digest, "bytes": len(data)}

    def blob(self, name: str) -> bytes | None:
        if not re.fullmatch(r"[0-9a-f]{32}", name):
            return None
        try:
            return (self.dir / "blobs" / name).read_bytes()
        except OSError:
            return None

    def _trim_blobs(self, keep: str, limit: int = 24) -> None:
        files = sorted((self.dir / "blobs").glob("*"), key=lambda p: p.stat().st_mtime)
        for old in files[:-limit]:
            if old.name != keep:
                old.unlink(missing_ok=True)


def new_pairing_code() -> str:
    alphabet = "ABCDEFGHJKMNPQRSTUVWXYZ23456789"
    return "".join(secrets.choice(alphabet) for _ in range(6))


def _text(value, name: str, limit: int, required: bool = True) -> str:
    if value is None:
        value = ""
    if not isinstance(value, str):
        raise BoardError("%s must be text" % name)
    value = value.strip()
    if required and not value:
        raise BoardError("%s is required" % name)
    if len(value) > limit:
        raise BoardError("%s is longer than %d characters" % (name, limit))
    return value


def _identifier(value) -> str | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str) or not _ID.match(value):
        raise BoardError("ids use letters, digits, dot, dash and underscore, up to 48")
    return value


def _choices(choices) -> list[dict]:
    if not isinstance(choices, list) or not 2 <= len(choices) <= MAX_CHOICES:
        raise BoardError("give between 2 and %d choices" % MAX_CHOICES)
    out, seen = [], set()
    for index, choice in enumerate(choices):
        if isinstance(choice, str):
            choice = {"label": choice}
        if not isinstance(choice, dict):
            raise BoardError("each choice is text or {label, id}")
        label = _text(choice.get("label"), "choice label", MAX_LABEL)
        cid = _identifier(choice.get("id")) or "c%d" % (index + 1)
        if cid in seen:
            raise BoardError("choice ids must be unique")
        seen.add(cid)
        out.append({"id": cid, "label": label})
    return out
