#!/usr/bin/env python3

import hashlib
import json
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
WORK = ROOT / "target" / "flashcards-licenses"
CONFIG = ROOT / "licenses" / "flashcards-about.toml"

TARGETS = [
    (
        ROOT / "apps" / "flashcards" / "Cargo.toml",
        ROOT / "licenses" / "LICENSE-Flashcards-device-dependencies.txt",
        "Flashcards device application dependency notices",
    ),
    (
        ROOT / "crates" / "kobo-flashcards-import" / "Cargo.toml",
        ROOT / "licenses" / "LICENSE-Flashcards-host-dependencies.txt",
        "Flashcards host helper dependency notices",
    ),
]


def generate(manifest: Path, output: Path, title: str) -> None:
    WORK.mkdir(parents=True, exist_ok=True)
    raw = WORK / f"{manifest.parent.name}.json"
    environment = os.environ.copy()
    environment.update(
        {
            "CARGO_NET_OFFLINE": "true",
            "HTTP_PROXY": "http://127.0.0.1:9",
            "HTTPS_PROXY": "http://127.0.0.1:9",
            "ALL_PROXY": "http://127.0.0.1:9",
            "NO_PROXY": "",
        }
    )
    subprocess.run(
        [
            "cargo",
            "about",
            "generate",
            "--format",
            "json",
            "--config",
            str(CONFIG),
            "--manifest-path",
            str(manifest),
            "--output-file",
            str(raw),
            "--threshold",
            "0.6",
        ],
        cwd=ROOT,
        check=True,
        env=environment,
    )
    report = json.loads(raw.read_text())
    sections = [
        f"{title}\n",
        f"Generated from Cargo.lock with cargo-about 0.6.4 for {manifest.relative_to(ROOT)}.\n",
    ]
    licences = sorted(
        report["licenses"],
        key=lambda licence: (
            licence.get("name", ""),
            licence.get("id", ""),
            hashlib.sha256((licence.get("text") or "").encode()).hexdigest(),
            tuple(
                sorted(
                    f"{item['crate']['name']} {item['crate']['version']}"
                    for item in licence.get("used_by", [])
                )
            ),
        ),
    )
    for licence in licences:
        used_by = sorted(
            {
                f"{item['crate']['name']} {item['crate']['version']}"
                for item in licence.get("used_by", [])
            }
        )
        text = (licence.get("text") or "").replace("\r\n", "\n").replace("\r", "\n")
        text = "\n".join(line.rstrip() for line in text.splitlines()).rstrip()
        sections.extend(
            [
                "\n" + "=" * 79 + "\n",
                f"{licence.get('name', 'Unknown')} [{licence.get('id', '')}]\n",
                f"Used by: {', '.join(used_by)}\n",
                "-" * 79 + "\n",
                text + "\n",
            ]
        )
    output.write_text("".join(sections))


for manifest, output, title in TARGETS:
    generate(manifest, output, title)
