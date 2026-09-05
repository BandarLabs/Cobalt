#!/usr/bin/env python3

import sys
import subprocess
from pathlib import Path

import fontTools
from fontTools.ttLib import TTFont


EXPECTED_FONTTOOLS = "4.25.0"
EXPECTED_OUTPUT_SHA256 = (
    "150c82a7b6a4e39645099b3d27c96a00a148a1f57faf523027559910059c2dc0"
)


def japanese_repertoire() -> str:
    characters = {chr(value) for value in range(0x20, 0x100)}
    characters.update("\n\t")
    for value in range(0xA1, 0xE0):
        try:
            characters.add(bytes([value]).decode("shift_jis"))
        except UnicodeDecodeError:
            pass
    for lead in [*range(0x81, 0xA0), *range(0xE0, 0xFD)]:
        for trail in [*range(0x40, 0x7F), *range(0x80, 0xFD)]:
            try:
                value = bytes([lead, trail]).decode("shift_jis")
            except UnicodeDecodeError:
                continue
            if len(value) == 1:
                characters.add(value)
    for row in [*range(1, 16), *range(90, 95)]:
        for cell in range(1, 95):
            try:
                value = bytes([0xA0 + row, 0xA0 + cell]).decode("euc_jis_2004")
            except UnicodeDecodeError:
                continue
            if len(value) == 1:
                characters.add(value)
    return "".join(sorted(characters))


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(
            "usage: scripts/build-flashcards-japanese-font.py SOURCE.otf OUTPUT.otf"
        )
    if fontTools.__version__ != EXPECTED_FONTTOOLS:
        raise SystemExit(
            f"fontTools {EXPECTED_FONTTOOLS} is required; found {fontTools.__version__}"
        )
    source = Path(sys.argv[1])
    output = Path(sys.argv[2])
    charset = output.parent / ".cobalt-japanese-charset.txt"
    charset.write_text(japanese_repertoire())
    try:
        subprocess.run(
            [
                sys.executable,
                "-m",
                "fontTools.subset",
                str(source),
                f"--text-file={charset}",
                f"--output-file={output}",
                "--layout-features=",
                "--no-hinting",
                "--notdef-glyph",
                "--notdef-outline",
                "--name-IDs=0,1,2,4,6",
                "--name-languages=0x409",
            ],
            check=True,
        )
    finally:
        charset.unlink(missing_ok=True)

    renamed = TTFont(output, recalcTimestamp=False)
    renamed.recalcTimestamp = False
    names = {
        1: "Cobalt Japanese",
        2: "Regular",
        4: "Cobalt Japanese Regular",
        6: "CobaltJapanese-Regular",
    }
    for name_id, value in names.items():
        renamed["name"].setName(value, name_id, 3, 1, 0x409)
        renamed["name"].setName(value, name_id, 1, 0, 0)
    renamed.save(output, reorderTables=True)

    import hashlib

    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    if digest != EXPECTED_OUTPUT_SHA256:
        raise SystemExit(f"unexpected output digest: {digest}")


if __name__ == "__main__":
    main()
