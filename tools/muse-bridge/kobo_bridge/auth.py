"""Pairing code and reader tokens. Tokens are stored as hashes."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import secrets
import threading
import time
from pathlib import Path

from kobo_bridge.board import new_pairing_code

MAX_FAILURES = 5
LOCKOUT_S = 60
MAX_READERS = 4


class Auth:
    def __init__(self, directory: Path, clock=time.monotonic) -> None:
        self._path = Path(directory) / "auth.json"
        self._clock = clock
        self._lock = threading.Lock()
        self._failures = 0
        self._locked_until = 0.0
        self.data = self._load()

    def _load(self) -> dict:
        try:
            data = json.loads(self._path.read_text(encoding="utf-8"))
            if isinstance(data, dict) and data.get("code"):
                return data
        except (OSError, json.JSONDecodeError):
            pass
        data = {"code": new_pairing_code(), "tokens": []}
        self._save(data)
        return data

    def _save(self, data: dict) -> None:
        self._path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd = os.open(self._path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f)

    @property
    def code(self) -> str:
        return self.data["code"]

    def pair(self, code: str) -> str | None:
        """Exchange the pairing code for a token, or return None."""
        with self._lock:
            now = self._clock()
            if now < self._locked_until:
                return None
            supplied = (code or "").strip().upper()
            if not hmac.compare_digest(supplied.encode(), self.data["code"].encode()):
                self._failures += 1
                if self._failures >= MAX_FAILURES:
                    self._locked_until = now + LOCKOUT_S
                    self._failures = 0
                return None
            self._failures = 0
            token = secrets.token_urlsafe(32)
            tokens = self.data["tokens"][-(MAX_READERS - 1):]
            tokens.append(_digest(token))
            self.data["tokens"] = tokens
            # A pairing code is good for one reader. The next one needs a new
            # code, which `kobo-bridge init` prints.
            self.data["code"] = new_pairing_code()
            self._save(self.data)
            return token

    def valid(self, token: str) -> bool:
        digest = _digest(token or "")
        with self._lock:
            return any(hmac.compare_digest(digest, known) for known in self.data["tokens"])

    def locked(self) -> bool:
        return self._clock() < self._locked_until

    def reset(self) -> None:
        with self._lock:
            self.data = {"code": new_pairing_code(), "tokens": []}
            self._save(self.data)


def _digest(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()
