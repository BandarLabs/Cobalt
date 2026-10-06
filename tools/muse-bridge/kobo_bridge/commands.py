"""The commands Muse can call on the reader, and the executor that runs them."""

from __future__ import annotations

import logging
from typing import Callable

from kobo_bridge import imagefetch, markup
from kobo_bridge.board import Board, BoardError

log = logging.getLogger(__name__)

LIMITS = (
    "The screen is e-ink: it does not animate, redraws when you change it, and the "
    "reader may be asleep, so a change can take up to a minute to appear. Keep text "
    "short and give each page one idea."
)

COMMAND_SPECS = {
    "kobo.show_page": {
        "description": (
            "Show a page of text on the reader. " + LIMITS + " The body is a small "
            "markdown subset: # and ## headings, paragraphs, - lists, > quotes, ---, "
            "**bold**. Images are not drawn inside pages: a ![alt](url) line shows its alt "
            "text; use kobo.draw_url for a picture. Long pages "
            "are split across screens by the reader."
        ),
        "required": {
            "body": {"type": "string", "description": "Page content in the markdown subset."},
        },
        "optional": {
            "title": {"type": "string", "description": "Short title shown above the page."},
            "page_id": {"type": "string", "description": "Your own id for this page."},
            "keep_s": {"type": "integer", "description": "Return to the status screen after this many seconds. Default: stay until replaced."},
        },
    },
    "kobo.show_text": {
        "description": "Show a short plain-text message on the reader. " + LIMITS,
        "required": {
            "text": {"type": "string", "description": "The message."},
        },
        "optional": {
            "title": {"type": "string", "description": "Short title."},
        },
    },
    "kobo.ask": {
        "description": (
            "Ask the person holding the reader a question with 2 to 6 tappable choices. "
            "Returns at once. When they tap, you receive a chat message in the side chat "
            "'kobo-panel' saying which choice they picked. " + LIMITS
        ),
        "required": {
            "question": {"type": "string", "description": "The question, one short sentence of at most 60 characters so it fits at the largest text size."},
            "choices": {"type": "array", "description": "2 to 6 choices. Each is a label string or {label, id}."},
        },
        "optional": {
            "context": {"type": "string", "description": "A short line of detail under the question, at most 120 characters."},
            "ask_id": {"type": "string", "description": "Your own id for this question; it comes back with the answer."},
            "expires_s": {"type": "integer", "description": "Withdraw the question after this many seconds. Default 3600."},
        },
    },
    "kobo.set_status": {
        "description": (
            "Set the line on the reader's resting screen, the one it shows when there is "
            "no page. Use it for what you are doing or what is next. " + LIMITS
        ),
        "required": {
            "line": {"type": "string", "description": "One short line, up to 120 characters."},
        },
        "optional": {
            "detail": {"type": "string", "description": "A smaller second line."},
        },
    },
    "kobo.draw_url": {
        "description": (
            "Show a full-screen image from a URL. JPEG or PNG, up to 4 MB, public "
            "addresses only. The reader draws it in grayscale, or colour on colour "
            "models. Text and line art look better than photos. Stays until replaced. "
            + LIMITS
        ),
        "required": {
            "url": {"type": "string", "description": "http:// or https:// URL of the image."},
        },
        "optional": {
            "fit": {"type": "string", "description": "contain (default) keeps the whole image; fill crops to cover the screen."},
        },
        "timeout_ms": 60000,
    },
    "kobo.clear": {
        "description": "Clear the reader back to its resting screen.",
        "required": {},
        "optional": {},
    },
    "kobo.device_info": {
        "description": (
            "What the reader reports: model, panel size in pixels, colour or not, battery, "
            "and when it last checked in. Use it to size pages and to tell whether the "
            "reader is awake."
        ),
        "required": {},
        "optional": {},
    },
}


def ok(payload: dict) -> dict:
    return {"ok": True, "payload": payload}


def error(message: str) -> dict:
    return {"ok": False, "error": message}


class KoboExecutor:
    def __init__(self, board: Board, allow_private_images: bool = False,
                 fallback: Callable[[str, dict, int | None], dict] | None = None) -> None:
        self.board = board
        self.allow_private_images = allow_private_images
        self.fallback = fallback

    def run(self, command: str, params: dict, timeout_ms: int | None = None) -> dict:
        handler = {
            "kobo.show_page": self._show_page,
            "kobo.show_text": self._show_text,
            "kobo.ask": self._ask,
            "kobo.set_status": self._set_status,
            "kobo.draw_url": self._draw_url,
            "kobo.clear": self._clear,
            "kobo.device_info": self._device_info,
        }.get(command)
        if handler is None:
            if self.fallback is not None:
                return self.fallback(command, params, timeout_ms)
            return error("unsupported command: " + command)
        try:
            return ok(handler(params))
        except (BoardError, markup.MarkupError, imagefetch.FetchError) as err:
            return error(str(err))
        except Exception as err:
            log.exception("%s failed", command)
            return error("%s: %s" % (type(err).__name__, err))

    def _show_page(self, p: dict) -> dict:
        blocks = markup.parse(p.get("body"))
        blocks = _images_as_text(blocks)
        result = self.board.show_page(p.get("title", ""), blocks, p.get("page_id"), p.get("keep_s"))
        return {**result, "blocks": len(blocks)}

    def _show_text(self, p: dict) -> dict:
        text = p.get("text")
        if not isinstance(text, str) or not text.strip():
            raise BoardError("text is required")
        blocks = [{"t": "p", "spans": [{"s": line}]} for line in text.strip().split("\n\n")]
        return self.board.show_page(p.get("title", ""), blocks)

    def _ask(self, p: dict) -> dict:
        return self.board.ask(p.get("question"), p.get("choices"), p.get("context", ""),
                              p.get("ask_id"), p.get("expires_s"))

    def _set_status(self, p: dict) -> dict:
        return {"rev": self.board.set_status(p.get("line"), p.get("detail", ""))}

    def _draw_url(self, p: dict) -> dict:
        url = p.get("url")
        if not isinstance(url, str):
            raise BoardError("url is required")
        data, mime = imagefetch.fetch_image(url, self.allow_private_images)
        blob = self.board.put_blob(data, mime)
        return self.board.show_image(blob, p.get("fit") or "contain")

    def _clear(self, p: dict) -> dict:
        return {"rev": self.board.clear()}

    def _device_info(self, p: dict) -> dict:
        hello = self.board.hello()
        if hello is None:
            return {"seen": False, "note": "The reader has not checked in yet."}
        return {"seen": True, **hello}


def _images_as_text(blocks: list[dict]) -> list[dict]:
    """Pages hold text only, so an image line becomes its description."""
    out = []
    for block in blocks:
        if block["t"] == "img":
            label = block["alt"].strip() or "image"
            out.append({"t": "p", "spans": [{"s": "[%s]" % label}]})
        else:
            out.append(block)
    return out
