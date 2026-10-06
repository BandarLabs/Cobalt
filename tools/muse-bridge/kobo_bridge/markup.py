"""Turn the markdown subset Muse sends into blocks the reader draws.

The reader never parses markdown. It receives a flat list of blocks, each a
dict with a "t" key, and spans for inline emphasis.

    heading   {"t": "h", "level": 1|2, "spans": [...]}
    paragraph {"t": "p", "spans": [...]}
    list      {"t": "ul", "items": [[spans], ...]}
    quote     {"t": "q", "spans": [...]}
    rule      {"t": "hr"}
    image     {"t": "img", "url": str, "alt": str}

A span is {"s": text} or {"s": text, "b": true}.
"""

from __future__ import annotations

import re

MAX_BLOCKS = 200
MAX_SPAN_TEXT = 4000
MAX_BODY_CHARS = 20000

_BOLD = re.compile(r"\*\*(.+?)\*\*")
_IMAGE = re.compile(r"^!\[([^\]]*)\]\((https://[^\s)]+)\)\s*$")
_ITEM = re.compile(r"^\s*[-*]\s+(.*)$")


class MarkupError(ValueError):
    pass


def spans(text: str) -> list[dict]:
    out: list[dict] = []
    pos = 0
    for match in _BOLD.finditer(text):
        if match.start() > pos:
            out.append({"s": text[pos:match.start()]})
        out.append({"s": match.group(1), "b": True})
        pos = match.end()
    if pos < len(text):
        out.append({"s": text[pos:]})
    return out or [{"s": ""}]


def parse(body: str) -> list[dict]:
    if not isinstance(body, str):
        raise MarkupError("body must be text")
    if len(body) > MAX_BODY_CHARS:
        raise MarkupError("body is longer than %d characters" % MAX_BODY_CHARS)
    blocks: list[dict] = []
    paragraph: list[str] = []
    items: list[list[dict]] = []
    quote: list[str] = []

    def flush() -> None:
        if paragraph:
            blocks.append({"t": "p", "spans": spans(" ".join(paragraph))})
            paragraph.clear()
        if items:
            blocks.append({"t": "ul", "items": list(items)})
            items.clear()
        if quote:
            blocks.append({"t": "q", "spans": spans(" ".join(quote))})
            quote.clear()

    for raw in body.replace("\r\n", "\n").split("\n"):
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped:
            flush()
            continue
        if stripped in ("---", "***", "___"):
            flush()
            blocks.append({"t": "hr"})
            continue
        heading = re.match(r"^(#{1,3})\s+(.*)$", stripped)
        if heading:
            flush()
            blocks.append({"t": "h", "level": min(len(heading.group(1)), 2),
                           "spans": spans(heading.group(2))})
            continue
        image = _IMAGE.match(stripped)
        if image:
            flush()
            blocks.append({"t": "img", "alt": image.group(1), "url": image.group(2)})
            continue
        item = _ITEM.match(line)
        if item:
            if paragraph or quote:
                flush()
            items.append(spans(item.group(1)))
            continue
        if stripped.startswith(">"):
            if paragraph or items:
                flush()
            quote.append(stripped.lstrip(">").strip())
            continue
        if items or quote:
            flush()
        paragraph.append(stripped)
    flush()
    if len(blocks) > MAX_BLOCKS:
        raise MarkupError("more than %d blocks" % MAX_BLOCKS)
    for block in blocks:
        for span in block.get("spans", []):
            if len(span["s"]) > MAX_SPAN_TEXT:
                raise MarkupError("a paragraph is too long; split it")
    return blocks
