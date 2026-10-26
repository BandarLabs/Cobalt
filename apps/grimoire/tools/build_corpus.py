#!/usr/bin/env python3
"""Build Grimoire's deterministic, device-readable SRD index.

The checked-in JSON snapshots are the reviewable source.  This program never
contacts a service: refreshing a snapshot is an explicit maintainer action.
Fields are deliberately limited to data that the Kobo UI renders.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "data" / "source"
OUT = ROOT / "data" / "corpus.tsv"
KINDS = {
    "Spells": "spell", "Monsters": "monster", "Conditions": "condition",
    "Rules": "rule", "Rule-Sections": "rule", "Magic-Items": "item",
}


def clean(value: object) -> str:
    """A snapshot field as readable text. Lists and objects are flattened; no
    escaping happens here, because escaping is a property of the file being
    written and doing it twice is how the rule sections came to be shipped
    with a literal backslash-n between every paragraph."""
    if isinstance(value, list):
        return ", ".join(clean(item) for item in value)
    if isinstance(value, dict):
        return ", ".join(f"{key}: {clean(item)}" for key, item in value.items())
    return str(value or "")


def escape(text: str) -> str:
    """One field of the tab-separated file, escaped exactly once."""
    return text.replace("\\", "\\\\").replace("\t", " ").replace("\n", "\\n")


def markup(text: str) -> str:
    """One line of prose as the reader's markup.

    The snapshots are Markdown. Shipped as written, a rule section reads
    "## Traps" and "**Mechanical traps**" on the panel, and its tables arrive
    as rows of pipes. Converting here rather than on the device keeps the
    application free of a parser and lets the shared document reader draw the
    headings, lists and tables it was built to draw.
    """
    out = (text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))
    out = re.sub(r"\*\*\*(.+?)\*\*\*", r"<strong><em>\1</em></strong>", out)
    out = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", out)
    out = re.sub(r"(?<!\*)\*([^*]+?)\*(?!\*)", r"<em>\1</em>", out)
    # A handful of snapshot entries open an emphasis and never close it. The
    # marker is punctuation the reader should not have to look at either way.
    out = out.replace("**", "").replace("*", "")
    return out.strip()


def cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def tabulate(rows: list[list[str]]) -> str:
    """A Markdown table as one paragraph per row.

    Not as a table. The panel is six inches wide and offers nine text sizes
    with nothing smaller to fall back on, so a column that fits at the default
    size is clipped at the larger ones, which is what the layout checks report
    for every table in the rule sections. A row read as "Setback, save DC 10
    to 11, attack bonus +3 to +5" survives every size, and it is how somebody
    reads a table out at a table anyway.
    """
    head, *rest = rows
    drawn = []
    for row in rest:
        lead, *tail = row
        details = " · ".join(
            f"{head[index + 1]}: {cell}" if index + 1 < len(head) else cell
            for index, cell in enumerate(tail)
            if cell
        )
        drawn.append(
            f"<p><strong>{lead}</strong> {details}</p>" if details else f"<p><strong>{lead}</strong></p>"
        )
    return "".join(drawn)


def document(text: str) -> str:
    """A Markdown body as a small HTML document the shared reader understands."""
    blocks: list[str] = []
    paragraph: list[str] = []
    bullets: list[str] = []
    table: list[list[str]] = []

    def settle() -> None:
        if paragraph:
            blocks.append("<p>" + " ".join(paragraph) + "</p>")
            paragraph.clear()
        if bullets:
            blocks.append("<ul>" + "".join(f"<li>{item}</li>" for item in bullets) + "</ul>")
            bullets.clear()
        if table:
            blocks.append(tabulate(table))
            table.clear()

    for line in text.split("\n"):
        stripped = line.strip()
        if not stripped:
            settle()
            continue
        heading = re.match(r"^(#{1,6})\s+(.*)$", stripped)
        if heading:
            settle()
            level = min(max(len(heading.group(1)), 2), 6)
            blocks.append(f"<h{level}>{markup(heading.group(2))}</h{level}>")
            continue
        if stripped.startswith("|"):
            if paragraph or bullets:
                settle()
            # The dashed line under a header row is punctuation, not data.
            if not set(stripped) <= set("|-: "):
                table.append([markup(cell) for cell in cells(stripped)])
            continue
        if table:
            settle()
        item = re.match(r"^[-*+]\s+(.*)$", stripped)
        if item:
            if paragraph:
                settle()
            bullets.append(markup(item.group(1)))
            continue
        if bullets:
            settle()
        paragraph.append(markup(stripped))
    settle()
    return "".join(blocks)


def paragraphs(*parts: str) -> str:
    """Generated prose, already plain, as its own paragraphs."""
    return "".join(f"<p>{markup(part)}</p>" for part in parts if part)


def spell(record: dict) -> tuple[str, str, str]:
    school = record.get("school", {}).get("name", "")
    classes = ", ".join(
        item.get("name", "") if isinstance(item, dict) else str(item)
        for item in record.get("classes", [])
    )
    subtitle = f"{record.get('level', 0)} · {school} · {classes}"
    higher = prose(record.get("higher_level"))
    body = document("\n\n".join(filter(None, [prose(record.get("desc", [])), higher])))
    return subtitle, body, ';'.join([f"class={classes}", f"level={record.get('level', 0)}", f"school={school}", f"ritual={int(bool(record.get('ritual')))}", f"concentration={int(bool(record.get('concentration')))}"])


def monster(record: dict) -> tuple[str, str, str]:
    ac = clean(record.get("armor_class"))
    speed = clean(record.get("speed"))
    subtitle = f"{record.get('size')} {record.get('type')} · CR {record.get('challenge_rating')}"
    ability = "  ".join(
        f"{label} {record.get(key, 0)}"
        for label, key in (("STR", "strength"), ("DEX", "dexterity"), ("CON", "constitution"),
                           ("INT", "intelligence"), ("WIS", "wisdom"), ("CHA", "charisma"))
    )
    body = paragraphs(
        f"AC {ac}  HP {record.get('hit_points')} ({record.get('hit_dice')})  Speed {speed}",
        ability,
    )
    for key, title in (("special_abilities", "Traits"), ("actions", "Actions"),
                       ("legendary_actions", "Legendary actions"), ("reactions", "Reactions")):
        entries = record.get(key, [])
        if entries:
            body += f"<h3>{title}</h3>" + "".join(
                f"<p><strong>{markup(clean(entry.get('name')))}.</strong> "
                f"{markup(prose(entry.get('desc')))}</p>"
                for entry in entries
            )
    return subtitle, body, f"type={clean(record.get('type'))};cr={record.get('challenge_rating', '')}"


def named(value: object) -> str:
    """The readable half of a snapshot field that is an object with a name."""
    if isinstance(value, dict):
        return clean(value.get("name", ""))
    return clean(value)


def prose(value: object) -> str:
    """A description field as text with its paragraphs intact.

    Not `clean`, which joins a list with commas: that is right for a list of
    classes and wrong for a list of paragraphs, and it is what ran the
    headings and tables of a dozen spells together into one line nothing
    downstream could recognise.
    """
    if isinstance(value, list):
        return "\n\n".join(prose(item) for item in value)
    return clean(value)


def description(record: dict) -> str:
    """Both spellings the snapshots use. The 2024 files say "description"
    where the 2014 files say "desc", and reading only one of them shipped
    fifteen conditions with nothing under their names."""
    return prose(record.get("desc") or record.get("description") or "")


def item(record: dict) -> tuple[str, str, str]:
    """A magic item, said the way a table asks for one: what it is, how rare
    it is, and whether it needs attunement."""
    parts = [named(record.get("equipment_category")), named(record.get("rarity"))]
    if str(record.get("attunement", "")).lower() == "true":
        parts.append("attunement")
    subtitle = " · ".join(part for part in parts if part)
    return subtitle, document(description(record)), ""


def generic(record: dict, kind: str) -> tuple[str, str, str]:
    # The index is a slug, not a subtitle. A condition says what it is; a rule
    # section carries its own heading in the body.
    subtitle = "Condition" if kind == "condition" else ""
    return subtitle, document(description(record)), ""


def main() -> None:
    records: list[tuple[str, str, str, str, str, str]] = []
    for path in sorted(SOURCE.glob("*.json")):
        edition, stem = path.stem.split("-", 1)
        kind_key = next((key for key in KINDS if stem == key), None)
        if kind_key is None:
            continue
        kind = KINDS[kind_key]
        for record in json.loads(path.read_text()):
            if kind == "spell":
                subtitle, body, tags = spell(record)
            elif kind == "monster":
                subtitle, body, tags = monster(record)
            elif kind == "item":
                subtitle, body, tags = item(record)
            else:
                subtitle, body, tags = generic(record, kind)
            records.append(
                (edition, kind, clean(record["name"]), subtitle, body, tags)
            )
    records.sort(key=lambda row: (row[1], row[0], row[2].casefold()))
    OUT.write_text(
        "# edition\tkind\tname\tsubtitle\tbody\ttags\n"
        + "\n".join("\t".join(escape(field) for field in row) for row in records)
        + "\n"
    )
    print(f"wrote {len(records)} records, {OUT.stat().st_size} bytes")
    if OUT.stat().st_size > 6 * 1024 * 1024:
        raise SystemExit("corpus exceeds 6 MiB limit")


if __name__ == "__main__":
    main()
