#!/usr/bin/env python3
"""Rebuild or verify the Logic Pack collection.

The collection is now built by apps/logicpack/tools/generate.py, which keeps
the original twenty puzzles first and proves every puzzle after them. This
name stays so older notes and checklists still work; it passes its arguments
straight through, so `--check` verifies the shipped file and changes nothing.
"""
import runpy
import sys
from pathlib import Path

GENERATOR = Path(__file__).resolve().parents[2] / "apps/logicpack/tools/generate.py"

if __name__ == "__main__":
    sys.argv[0] = str(GENERATOR)
    runpy.run_path(str(GENERATOR), run_name="__main__")
