#!/usr/bin/env python3
"""Builds the Sudoku pack with the app's own generator.

The pack now lives with the app: apps/sudoku/tools/generate.py keeps the 36
original puzzles in their places and adds 200 per difficulty after them. This
script used to write only the original 36, so running it would have dropped
every later puzzle and broken saved games that name one. It hands its
arguments on instead, so `--check` still verifies the shipped file.
"""
import subprocess
import sys
from pathlib import Path

GENERATOR = Path(__file__).resolve().parents[2] / 'apps/sudoku/tools/generate.py'

if __name__ == '__main__':
    sys.exit(subprocess.call([sys.executable, '-I', str(GENERATOR), *sys.argv[1:]]))
