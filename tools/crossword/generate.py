#!/usr/bin/env python3
"""Fills crossword grids from `words.tsv` and writes the puzzle data as Rust.

Why a generator rather than hand-written puzzles: the four that shipped first
were three word squares, whose grid equals its own transpose so that solving
one direction hands you the other, and a single small crossword. Filling grids
by machine and clueing by word is what lets every puzzle obey the rules a
printed grid obeys: no entry under three squares, no answer twice, a clue for
every entry, and black squares in pairs about the centre with never four
meeting in a block.

The vocabulary is exactly the answers in `words.tsv`, so a generated puzzle is
clued by construction and no grid can ship with a blank clue. No answer is
used more than `CAP` allows across the whole collection, so a solver working
through it keeps meeting new words rather than the same handful of short ones.

Puzzles already in the output file are kept, in order, with their grids
unchanged, and new ones are added after them. A saved game names its puzzle
by id and refuses a grid that changed under it, so regenerating must never
move an answer. Clues are always taken afresh from the table, which is how a
clue gets corrected. `--fresh` ignores the existing file, for use only before
a collection has ever shipped.

Standard library only, and deterministic: the same table and the same
existing file always produce the same output.

usage: tools/crossword/generate.py [--count N] [--out PATH] [--fresh] [--check]
"""
import argparse
import pathlib
import random
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
TABLE = pathlib.Path(__file__).resolve().parent / "words.tsv"
OUT = ROOT / "apps/crossword/src/puzzles.rs"

# The most times one answer may appear across every crossword in the app. A
# three-letter word is the glue between longer ones, and the few that start
# and end with vowels are what lets a corner close at all; held to two uses
# each, the table stops filling grids after about forty puzzles.
CAP = {3: 3, 5: 2}

# Answers of "Odds and ends", the hand-written crossword that ships before the
# generated ones. They count towards the cap like any other use.
HAND_WRITTEN = "PUT MINOR ALIKE NOTED TEN PILOT UNITE TOKEN MAN RED".split()

# The 5 by 5: three full rows joined by short words, with the steps mirrored
# in alternate puzzles. Black squares at the ends of the two short rows leave
# four squares that belong to one word only, as in a British blocked grid.
# The tighter shape, a diamond with black corners and a three by three core
# where every square is crossed by two five-letter words, has exactly one fill
# in this table and its mirror image, which is "Odds and ends".
MINI = [
    [
        ".....",
        "...##",
        ".....",
        "##...",
        ".....",
    ],
    [
        ".....",
        "##...",
        ".....",
        "...##",
        ".....",
    ],
]

# The 7 by 7: six rows of five held together by short words down the middle.
# It needs no word longer than five, which is what lets a table of short
# everyday words fill it.
MIDI = [
    [
        "#.....#",
        ".....##",
        "#.....#",
        "#.###.#",
        "#.....#",
        "##.....",
        "#.....#",
    ],
    [
        "#.....#",
        "##.....",
        "#.....#",
        "#.###.#",
        "#.....#",
        ".....##",
        "#.....#",
    ],
]

SHAPES = {"mini": MINI, "midi": MIDI}
TITLES = {"mini": "Mini", "midi": "Midi"}


def kind_of(index):
    """Every fifth puzzle is the larger grid, so the two sizes alternate."""
    return "midi" if index % 5 == 2 else "mini"


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


def read_table():
    clues = {}
    for number, line in enumerate(TABLE.read_text().splitlines(), start=1):
        if line.startswith("#") or not line.strip():
            continue
        if line.count("\t") != 1:
            fail(f"words.tsv:{number}: expected ANSWER<tab>clue")
        answer, clue = (part.strip() for part in line.split("\t"))
        where = f"words.tsv:{number}: {answer}"
        if not re.fullmatch(r"[A-Z]{3}|[A-Z]{5}", answer):
            fail(f"{where}: answers are three or five capital letters")
        if answer in clues:
            fail(f"{where}: answer listed twice")
        # Longer than this and a clue no longer fits above the keyboard at the
        # largest text size on the smaller reader.
        if not clue or len(clue) > 36:
            fail(f"{where}: a clue is between 1 and 36 characters")
        if any(mark in clue for mark in '"\\\u2014\u2013'):
            fail(f"{where}: no double quotes, backslashes or dashes in a clue")
        if re.search(rf"\b{answer}", clue, re.IGNORECASE):
            fail(f"{where}: the clue gives the answer away")
        clues[answer] = clue
    return clues


def slots(pattern):
    """Every entry of a pattern: which way it reads and the squares it covers."""
    side = len(pattern)
    out = []
    for down in (False, True):
        for line in range(side):
            run = []
            for step in range(side + 1):
                cell = (step, line) if down else (line, step)
                white = step < side and pattern[cell[0]][cell[1]] != "#"
                if white:
                    run.append(cell)
                    continue
                if len(run) >= 3:
                    out.append((down, run))
                run = []
    return out


def numbering(pattern):
    """The number each square carries, in the order a printed grid counts."""
    starts = {cells[0] for _, cells in slots(pattern)}
    return {cell: number for number, cell in enumerate(sorted(starts), start=1)}


def preferred(words, usage, rng):
    """Least used first, loosely, in an order the seed decides.

    Strictly least used first starves the search: once the fresh words left
    are the awkward ones, every seed tries only those and runs out. A little
    noise lets a word used once compete with one not used yet.
    """
    noise = {word: rng.random() * 1.5 for word in sorted(words)}
    return sorted(noise, key=lambda word: usage.get(word, 0) + noise[word])


def fill(pattern, clues, usage, rng, budget=20000):
    """One filled grid, or None when this seed's search runs out.

    A general fill, taking the most constrained entry first and trying the
    least used answers first. The shapes are chosen so that the search rarely
    backs up far; the budget stops a seed that has wandered somewhere hopeless
    so that the next seed can start clean.
    """
    entries = slots(pattern)
    rank = {}
    by_length = {}
    by_letter = {}
    for answer in preferred([a for a in clues if usage.get(a, 0) < CAP[len(a)]], usage, rng):
        rank[answer] = len(rank)
        by_length.setdefault(len(answer), set()).add(answer)
        for position, letter in enumerate(answer):
            by_letter.setdefault((len(answer), position, letter), set()).add(answer)
    grid = {}
    nodes = [0]

    def search(placed):
        nodes[0] += 1
        if nodes[0] > budget:
            return False
        best = None
        for index, (_, cells) in enumerate(entries):
            if index in placed:
                continue
            length = len(cells)
            fits = by_length.get(length, set())
            for position, cell in enumerate(cells):
                if cell in grid:
                    fits = fits & by_letter.get((length, position, grid[cell]), set())
            candidates = sorted(fits - set(placed.values()), key=rank.get)
            if best is None or len(candidates) < len(best[1]):
                best = (index, candidates)
            if not candidates:
                return False
        if best is None:
            return True
        index, candidates = best
        cells = entries[index][1]
        for answer in candidates:
            before = {cell: grid.get(cell) for cell in cells}
            for cell, letter in zip(cells, answer):
                grid[cell] = letter
            placed[index] = answer
            if search(placed):
                return True
            del placed[index]
            for cell, letter in before.items():
                if letter is None:
                    grid.pop(cell, None)
                else:
                    grid[cell] = letter
        return False

    if not search({}):
        return None
    return [
        "".join("#" if pattern[r][c] == "#" else grid[(r, c)] for c in range(len(pattern)))
        for r in range(len(pattern))
    ]


def answers_of(grid):
    pattern = ["".join("#" if ch == "#" else "." for ch in row) for row in grid]
    return [
        (down, cells, "".join(grid[r][c] for r, c in cells))
        for down, cells in slots(pattern)
    ]


def verify(grid, clues):
    """Every rule a printed grid keeps, checked on the finished grid."""
    side = len(grid)
    problems = []
    if any(len(row) != side for row in grid):
        problems.append("not square")
    words = [word for _, _, word in answers_of(grid)]
    if len(set(words)) != len(words):
        problems.append("an answer appears twice")
    problems += [f"{word} has no clue" for word in words if word not in clues]
    covered = {cell for _, cells, _ in answers_of(grid) for cell in cells}
    for r in range(side):
        for c in range(side):
            black = grid[r][c] == "#"
            if black != (grid[side - 1 - r][side - 1 - c] == "#"):
                problems.append(f"square {r},{c} has no partner")
            if not black and (r, c) not in covered:
                problems.append(f"square {r},{c} is in no entry")
            if r + 1 < side and c + 1 < side and all(
                grid[r + dr][c + dc] == "#" for dr in (0, 1) for dc in (0, 1)
            ):
                problems.append(f"four black squares meet at {r},{c}")
    # A run of two is an entry no paper prints; slots() skips it, so look for
    # it directly.
    for line in grid + ["".join(row[c] for row in grid) for c in range(side)]:
        for run in line.split("#"):
            if len(run) == 2:
                problems.append(f"{run} is a two-letter entry")
    return problems


def kept(path):
    """The id and grid of every puzzle the existing output already ships."""
    if not path.exists():
        return []
    found = re.findall(r'id: "([a-z]+-\d+)".*?side: (\d+), answer: b"([A-Z#]+)"', path.read_text())
    return [
        (ident, [answer[r * int(side):(r + 1) * int(side)] for r in range(int(side))])
        for ident, side, answer in found
    ]


def rust_text(text):
    return '"' + text + '"'


def render(made, clues):
    lines = [
        "//! Generated by `tools/crossword/generate.py` from `tools/crossword/words.tsv`.",
        "//! Do not edit by hand: change the table and run the script again from the",
        "//! repository root. Puzzles already here keep their grids and new ones go on",
        "//! the end, because a saved game names its puzzle by id and expects the same",
        "//! grid behind it.",
        "use crate::game::Puzzle;",
        "",
        "#[rustfmt::skip]",
        f"pub const GENERATED: [Puzzle; {len(made)}] = [",
    ]
    counts = {"mini": 0, "midi": 0}
    for ident, grid in made:
        kind = ident.split("-")[0]
        counts[kind] += 1
        numbers = numbering(["".join("#" if ch == "#" else "." for ch in row) for row in grid])
        entries = sorted(
            (numbers[cells[0]], down, word) for down, cells, word in answers_of(grid)
        )
        across = ", ".join(rust_text(clues[word]) for _, down, word in entries if not down)
        down = ", ".join(rust_text(clues[word]) for _, down, word in entries if down)
        lines.append(
            f'    Puzzle {{ id: "{ident}", title: "{TITLES[kind]} {counts[kind]}", '
            f'level: "Crossword", side: {len(grid)}, answer: b"{"".join(grid)}",'
        )
        lines.append(f"        across: &[{across}],")
        lines.append(f"        down: &[{down}] }},")
    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--count", type=int, default=57)
    parser.add_argument("--out", type=pathlib.Path, default=OUT)
    parser.add_argument("--fresh", action="store_true")
    parser.add_argument("--check", action="store_true",
                        help="fail if the output file is not what the table makes")
    arguments = parser.parse_args()
    clues = read_table()

    usage = {}
    for answer in HAND_WRITTEN:
        usage[answer] = usage.get(answer, 0) + 1
    made = [] if arguments.fresh else kept(arguments.out)
    for ident, grid in made:
        problems = verify(grid, clues)
        if problems:
            fail(f"{ident}: {'; '.join(problems)}")
        for _, _, word in answers_of(grid):
            usage[word] = usage.get(word, 0) + 1

    while len(made) < arguments.count:
        index = len(made)
        kind = kind_of(index)
        number = sum(1 for ident, _ in made if ident.startswith(kind)) + 1
        ident = f"{kind}-{number:03}"
        shapes = SHAPES[kind]
        pattern = shapes[(number - 1) % len(shapes)]
        grid = None
        for attempt in range(400):
            grid = fill(pattern, clues, usage, random.Random(f"{ident}/{attempt}"))
            if grid is not None and "".join(grid) not in {"".join(g) for _, g in made}:
                break
            grid = None
        if grid is None:
            fail(f"could not fill {ident}; add answers to words.tsv or lower --count")
        print(f"filled {ident}", file=sys.stderr)
        problems = verify(grid, clues)
        if problems:
            fail(f"{ident}: {'; '.join(problems)}")
        for _, _, word in answers_of(grid):
            usage[word] = usage.get(word, 0) + 1
        made.append((ident, grid))

    over = sorted(word for word, count in usage.items() if count > CAP[len(word)])
    if over:
        fail(f"used more often than the cap allows: {', '.join(over)}")

    text = render(made, clues)
    if arguments.check:
        if not arguments.out.exists() or arguments.out.read_text() != text:
            fail(f"{arguments.out} is out of date; run tools/crossword/generate.py")
        print(f"{arguments.out} matches the table")
        return 0
    arguments.out.write_text(text)
    used = sum(1 for count in usage.values() if count)
    print(f"wrote {len(made)} puzzles to {arguments.out} using {used} answers")
    return 0


if __name__ == "__main__":
    sys.exit(main())
