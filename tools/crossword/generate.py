#!/usr/bin/env python3
"""Fills crossword grids from `words.tsv` and writes the puzzle data as Rust.

Why a generator rather than hand-written puzzles: the four that shipped were
word squares whose grid equalled its own transpose, so every answer appeared
twice, once across and once down. Solving one direction handed you the other,
which is not a crossword. Filling grids by machine and clueing by word is what
lets every puzzle obey the rules a printed grid obeys: no entry under three
squares, no answer twice, a clue for every entry, and black squares in pairs
about the centre with never four meeting in a block.

The vocabulary is exactly the answers in `words.tsv`, so a generated puzzle is
clued by construction and no grid can ship with a blank clue.

usage: tools/crossword/generate.py [--count N] [--out PATH]
"""
import argparse
import pathlib
import random
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
TABLE = pathlib.Path(__file__).resolve().parent / "words.tsv"

# Black corners, so the rows and columns that remain are three entries of five
# and two of three each way. Legal on every count a paper cares about, and the
# smallest grid that is a crossword rather than a word square.
DIAMOND = [
    "#...#",
    ".....",
    ".....",
    ".....",
    "#...#",
]


def read_table():
    clues = {}
    for line in TABLE.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        answer, clue = line.split("\t", 1)
        clues[answer.strip()] = clue.strip()
    return clues


def slots(pattern):
    side = len(pattern)
    out = []
    for row in range(side):
        column = 0
        while column < side:
            if pattern[row][column] == "#":
                column += 1
                continue
            start = column
            while column < side and pattern[row][column] != "#":
                column += 1
            if column - start >= 3:
                out.append((False, [(row, x) for x in range(start, column)]))
    for column in range(side):
        row = 0
        while row < side:
            if pattern[row][column] == "#":
                row += 1
                continue
            start = row
            while row < side and pattern[row][column] != "#":
                row += 1
            if row - start >= 3:
                out.append((True, [(x, column) for x in range(start, row)]))
    return out


def fill(pattern, clues, rng, taken=frozenset()):
    """One filled diamond, or None when this seed's search runs out.

    Solved by structure rather than by search: the two three-letter columns fix
    the first and last letter of all three long rows, and a five-letter word is
    indexed by that pair, so each choice leaves a handful of candidates instead
    of four hundred. A general backtracking fill over the whole grid spent
    minutes on one puzzle; this takes milliseconds.
    """
    del pattern
    fives = [answer for answer in clues if len(answer) == 5]
    threes = {answer for answer in clues if len(answer) == 3}
    by_ends = {}
    by_middle = {}
    for answer in fives:
        by_ends.setdefault((answer[0], answer[4]), []).append(answer)
        by_middle.setdefault(answer[1:4], []).append(answer)

    left_edges = sorted(threes)
    right_edges = sorted(threes)
    rng.shuffle(left_edges)
    rng.shuffle(right_edges)

    for left in left_edges:
        for right in right_edges:
            if left == right:
                continue
            rows = [
                list(by_ends.get((left[index], right[index]), []))
                for index in range(3)
            ]
            if not all(rows):
                continue
            for row in rows:
                rng.shuffle(row)
            for first in rows[0]:
                for second in rows[1]:
                    if second == first:
                        continue
                    for third in rows[2]:
                        if third in (first, second):
                            continue
                        columns = []
                        for index in (1, 2, 3):
                            middle = first[index] + second[index] + third[index]
                            candidates = [
                                answer
                                for answer in by_middle.get(middle, [])
                                if answer not in (first, second, third)
                            ]
                            if not candidates:
                                columns = None
                                break
                            rng.shuffle(candidates)
                            columns.append(candidates)
                        if columns is None:
                            continue
                        for one in columns[0]:
                            for two in columns[1]:
                                if two == one:
                                    continue
                                for three in columns[2]:
                                    if three in (one, two):
                                        continue
                                    top = one[0] + two[0] + three[0]
                                    bottom = one[4] + two[4] + three[4]
                                    if top not in threes or bottom not in threes:
                                        continue
                                    answers = [
                                        top, first, second, third, bottom,
                                        left, one, two, three, right,
                                    ]
                                    if len(set(answers)) != len(answers):
                                        continue
                                    grid = [
                                        "#" + top + "#",
                                        first,
                                        second,
                                        third,
                                        "#" + bottom + "#",
                                    ]
                                    if "".join(grid) in taken:
                                        continue
                                    entries = [
                                        (False, (0, 1), top),
                                        (False, (1, 0), first),
                                        (False, (2, 0), second),
                                        (False, (3, 0), third),
                                        (False, (4, 1), bottom),
                                        (True, (1, 0), left),
                                        (True, (0, 1), one),
                                        (True, (0, 2), two),
                                        (True, (0, 3), three),
                                        (True, (1, 4), right),
                                    ]
                                    return grid, entries
    return None


def numbering(grid):
    """The number each square carries, in the order a printed grid counts."""
    side = len(grid)
    numbers = {}
    count = 0
    for row in range(side):
        for column in range(side):
            if grid[row][column] == "#":
                continue
            starts_across = column == 0 or grid[row][column - 1] == "#"
            starts_down = row == 0 or grid[row - 1][column] == "#"
            runs_across = starts_across and column + 2 < side and grid[row][column + 1] != "#"
            runs_down = starts_down and row + 2 < side and grid[row + 1][column] != "#"
            if runs_across or runs_down:
                count += 1
                numbers[(row, column)] = count
    return numbers


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--count", type=int, default=8)
    parser.add_argument("--out", type=pathlib.Path,
                        default=ROOT / "apps/crossword/src/puzzles.rs")
    arguments = parser.parse_args()
    clues = read_table()

    made = []
    seen_grids = set()
    seed = 0
    while len(made) < arguments.count and seed < 4000:
        seed += 1
        result = fill(DIAMOND, clues, random.Random(seed), seen_grids)
        if result is None:
            continue
        grid, entries = result
        key = "".join(grid)
        if key in seen_grids:
            continue
        seen_grids.add(key)
        made.append((grid, entries))

    if len(made) < arguments.count:
        print(f"filled {len(made)} of {arguments.count}", file=sys.stderr)
        return 1

    numbers_of = numbering(DIAMOND)
    del numbers_of
    lines = [
        "//! Generated by tools/crossword/generate.py. Do not edit by hand.",
        "//!",
        "//! Grids are filled from tools/crossword/words.tsv, which carries one",
        "//! clue per answer. Every entry is at least three squares, no answer",
        "//! appears twice in a puzzle, and the black squares are in pairs about",
        "//! the centre with never four meeting in a block.",
        "use super::Puzzle;",
        "",
        "pub const GENERATED: &[Puzzle] = &[",
    ]
    for index, (grid, entries) in enumerate(made, start=1):
        numbers = numbering(grid)
        across = [
            (numbers[first], word)
            for down, first, word in entries
            if not down
        ]
        down = [
            (numbers[first], word)
            for down_flag, first, word in entries
            if down_flag
            for down in [down_flag]
        ]
        across.sort()
        down.sort()
        answer = "".join(grid)
        lines.append("    Puzzle {")
        lines.append(f'        id: "mini-{index:03}",')
        lines.append(f'        title: "Mini {index}",')
        lines.append('        level: "Mini",')
        lines.append(f"        side: {len(grid)},")
        lines.append(f'        answer: b"{answer}",')
        lines.append("        across: &[")
        for _, word in across:
            lines.append(f'            "{clues[word]}",')
        lines.append("        ],")
        lines.append("        down: &[")
        for _, word in down:
            lines.append(f'            "{clues[word]}",')
        lines.append("        ],")
        lines.append("    },")
    lines.append("];")
    lines.append("")
    arguments.out.write_text("\n".join(lines))
    print(f"wrote {len(made)} puzzles to {arguments.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
