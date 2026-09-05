#!/usr/bin/env python3
"""Build Grimoire's deterministic, device-readable SRD index.

The checked-in JSON snapshots are the reviewable source.  This program never
contacts a service: refreshing a snapshot is an explicit maintainer action.
Fields are deliberately limited to data that the Kobo UI renders.
"""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "data" / "source"
OUT = ROOT / "data" / "corpus.tsv"
KINDS = {
    "Spells": "spell", "Monsters": "monster", "Conditions": "condition",
    "Rules": "rule", "Rule-Sections": "rule", "Magic-Items": "item",
}


def clean(value: object) -> str:
    if isinstance(value, list):
        return ", ".join(clean(item) for item in value)
    if isinstance(value, dict):
        return ", ".join(f"{key}: {clean(item)}" for key, item in value.items())
    return str(value or "").replace("\\", "\\\\").replace("\t", " ").replace("\n", "\\n")


def spell(record: dict) -> tuple[str, str, str]:
    school = record.get("school", {}).get("name", "")
    classes = ", ".join(
        item.get("name", "") if isinstance(item, dict) else str(item)
        for item in record.get("classes", [])
    )
    subtitle = f"{record.get('level', 0)} · {school} · {classes}"
    body = "\n\n".join(filter(None, [clean(record.get("desc", [])), clean(record.get("higher_level"))]))
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
    body = f"AC {ac}  HP {record.get('hit_points')} ({record.get('hit_dice')})  Speed {speed}\n{ability}"
    for key, title in (("special_abilities", "Traits"), ("actions", "Actions"),
                       ("legendary_actions", "Legendary actions"), ("reactions", "Reactions")):
        entries = record.get(key, [])
        if entries:
            body += "\n\n" + title + "\n" + "\n".join(
                f"{entry.get('name')}. {clean(entry.get('desc'))}" for entry in entries
            )
    return subtitle, body, f"type={clean(record.get('type'))};cr={record.get('challenge_rating', '')}"


def generic(record: dict) -> tuple[str, str, str]:
    subtitle = clean(record.get("rarity") or record.get("index") or "")
    return subtitle, clean(record.get("desc", [])), ""


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
            else:
                subtitle, body, tags = generic(record)
            records.append(
                (edition, kind, clean(record["name"]), clean(subtitle), clean(body), clean(tags))
            )
    records.sort(key=lambda row: (row[1], row[0], row[2].casefold()))
    OUT.write_text(
        "# edition\tkind\tname\tsubtitle\tbody\ttags\n"
        + "\n".join("\t".join(row) for row in records)
        + "\n"
    )
    print(f"wrote {len(records)} records, {OUT.stat().st_size} bytes")
    if OUT.stat().st_size > 6 * 1024 * 1024:
        raise SystemExit("corpus exceeds 6 MiB limit")


if __name__ == "__main__":
    main()
