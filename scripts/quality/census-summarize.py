#!/usr/bin/env python3
"""Aggregate census matrix cells into docs/quality/census.md + census.json.

Reads OUT_ROOT/<profile>-<scale>/out/results.json for the 27 cells
(9 supported profiles x 3 text scales) and writes a per-cell pass/fail
table plus a per-app failure index. Commit the output; the matrix itself
is reproducible via scripts/quality/census-matrix.sh.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
PROFILES = ["clara-bw-391", "clara-bw-395", "clara-hd-376", "clara-colour-393",
            "elipsa-2e-389", "libra-2-388", "libra-colour-390",
            "libra-colour-390-4.46.23836", "libra-h2o-384"]
SCALES = ["default", "large", "extra-large"]

def main(out_root):
    out_root = Path(out_root)
    cells = {}
    apps = []
    missing = []
    for profile in PROFILES:
        for scale in SCALES:
            path = out_root / f"{profile}-{scale}" / "out" / "results.json"
            key = f"{profile}-{scale}"
            if not path.is_file():
                missing.append(key)
                continue
            report = json.loads(path.read_text())
            results = report["results"]
            cells[key] = {
                "source_sha": report["source_sha"],
                "dirty": report["dirty"],
                "passed": sum(1 for r in results if r["status"] == "pass"),
                "total": len(results),
                "failed": [r["app"] for r in results if r["status"] != "pass"],
            }
            for r in results:
                if r["app"] not in apps:
                    apps.append(r["app"])
    census = {"profiles": PROFILES, "scales": SCALES, "cells": cells,
              "missing_cells": missing}
    lines = ["# Device census: all apps x supported profiles x text scales", ""]
    lines.append("27 cells = 9 supported profiles (kobo-profile SUPPORTED_PROFILES) "
                 "x 3 text scales. Reproduce: `scripts/quality/census-matrix.sh`. "
                 "Portrait cells; landscape is app-owned via SetOrientation. "
                 "758x1024 is not a supported profile.")
    lines.append("")
    lines.append("| cell | pass/total | failed apps |")
    lines.append("|---|---|---|")
    for key in [f"{p}-{s}" for p in PROFILES for s in SCALES]:
        cell = cells.get(key)
        if cell is None:
            lines.append(f"| {key} | MISSING | |")
        else:
            failed = ", ".join(cell["failed"]) or "-"
            lines.append(f"| {key} | {cell['passed']}/{cell['total']} | {failed} |")
    failures = {}
    for key, cell in cells.items():
        for app in cell["failed"]:
            failures.setdefault(app, []).append(key)
    lines += ["", "## Failure index by app", ""]
    for app in sorted(failures):
        lines.append(f"- {app}: {', '.join(failures[app])}")
    (ROOT / "docs/quality/census.json").write_text(json.dumps(census, indent=2) + "\n")
    (ROOT / "docs/quality/census.md").write_text("\n".join(lines) + "\n")
    print(f"cells complete: {len(cells)}/27; missing: {len(missing)}")
    return 0 if not missing else 1

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "/tmp/census/matrix"))
