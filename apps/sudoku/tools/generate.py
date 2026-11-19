#!/usr/bin/env python3
"""Builds the Sudoku pack in assets/puzzles.txt and checks every line of it.

The first 36 lines are the original pack and are copied through untouched,
because a saved game names its puzzle by line number. New puzzles are added
after them, one difficulty at a time, from a fixed seed, so running this again
writes the same file.

Difficulty follows the techniques the in-game rules describe, judged by a
logical solver rather than by counting givens, since a sparse grid can be easy
and a full one hard:

- easy: solvable with naked singles alone (a square with one possible number);
- medium: needs hidden singles (a number with one possible place in a row,
  column or box) and is solvable with singles;
- hard: needs naked or hidden pairs, pointing or box-line reduction at least
  once, and is solvable with those and singles, never by guessing.

A grid the logical solver fills has exactly one solution, but every line is
still counted by a separate backtracking search that stops at two, so a bug in
the solver cannot ship a puzzle with two answers.

Usage: generate.py [--check] [--per-level N]
"""

import argparse
import random
import sys
from pathlib import Path

SEED = 20261028
ORIGINAL = 36
PER_LEVEL = 200
LEVELS = ("easy", "medium", "hard")
# Givens left in the grid before digging stops. Hard digs until nothing more
# can go, because the techniques it needs only appear in sparse grids.
TARGETS = {"easy": (30, 38), "medium": (25, 31), "hard": (0, 0)}
ALL = 0x1FF

ROW = [i // 9 for i in range(81)]
COL = [i % 9 for i in range(81)]
BOX = [(i // 27) * 3 + (i % 9) // 3 for i in range(81)]
ROWS = [[r * 9 + c for c in range(9)] for r in range(9)]
COLS = [[r * 9 + c for r in range(9)] for c in range(9)]
BOXES = [[i for i in range(81) if BOX[i] == b] for b in range(9)]
UNITS = ROWS + COLS + BOXES
PEERS = [
    sorted({p for u in (ROWS[ROW[i]], COLS[COL[i]], BOXES[BOX[i]]) for p in u} - {i})
    for i in range(81)
]
BIT_DIGIT = {1 << d: d + 1 for d in range(9)}


def popcount(mask):
    return bin(mask).count("1")


def digits_of(mask):
    return [d for d in range(9) if mask >> d & 1]


def count_solutions(board, limit=2, rng=None):
    """Counts solutions up to `limit`; with `rng`, also returns the first one."""
    rows, cols, boxes = [0] * 9, [0] * 9, [0] * 9
    grid = list(board)
    empty = []
    for i, v in enumerate(grid):
        if v:
            bit = 1 << (v - 1)
            if (rows[ROW[i]] | cols[COL[i]] | boxes[BOX[i]]) & bit:
                return 0, None
            rows[ROW[i]] |= bit
            cols[COL[i]] |= bit
            boxes[BOX[i]] |= bit
        else:
            empty.append(i)
    found = [0, None]

    def search(open_cells):
        if not open_cells:
            found[0] += 1
            if found[1] is None:
                found[1] = list(grid)
            return found[0] >= limit
        best, best_mask, best_n = -1, 0, 10
        for k, i in enumerate(open_cells):
            mask = ALL & ~(rows[ROW[i]] | cols[COL[i]] | boxes[BOX[i]])
            n = popcount(mask)
            if n < best_n:
                best, best_mask, best_n = k, mask, n
                if n <= 1:
                    break
        if best_n == 0:
            return False
        i = open_cells[best]
        rest = open_cells[:best] + open_cells[best + 1 :]
        choices = digits_of(best_mask)
        if rng is not None:
            rng.shuffle(choices)
        for d in choices:
            bit = 1 << d
            grid[i] = d + 1
            rows[ROW[i]] |= bit
            cols[COL[i]] |= bit
            boxes[BOX[i]] |= bit
            done = search(rest)
            rows[ROW[i]] &= ~bit
            cols[COL[i]] &= ~bit
            boxes[BOX[i]] &= ~bit
            grid[i] = 0
            if done:
                return True
        return False

    search(empty)
    return found[0], found[1]


class Stuck(Exception):
    pass


class Logic:
    """Pencil marks plus the eliminations a person makes on paper."""

    def __init__(self, board):
        self.value = list(board)
        self.cand = [0] * 81
        for i in range(81):
            if not self.value[i]:
                used = 0
                for p in PEERS[i]:
                    if self.value[p]:
                        used |= 1 << (self.value[p] - 1)
                self.cand[i] = ALL & ~used
                if not self.cand[i]:
                    raise Stuck

    def place(self, i, digit):
        self.value[i] = digit
        self.cand[i] = 0
        bit = 1 << (digit - 1)
        for p in PEERS[i]:
            if self.cand[p] & bit:
                self.cand[p] &= ~bit
                if not self.cand[p] and not self.value[p]:
                    raise Stuck

    def solved(self):
        return all(self.value)

    def naked_single(self):
        for i in range(81):
            if not self.value[i] and self.cand[i] in BIT_DIGIT:
                self.place(i, BIT_DIGIT[self.cand[i]])
                return True
        return False

    def hidden_single(self):
        for unit in UNITS:
            for d in range(9):
                bit = 1 << d
                if any(self.value[i] == d + 1 for i in unit):
                    continue
                cells = [i for i in unit if self.cand[i] & bit]
                if not cells:
                    raise Stuck
                if len(cells) == 1:
                    self.place(cells[0], d + 1)
                    return True
        return False

    def eliminate(self, cells, mask):
        changed = False
        for i in cells:
            if self.cand[i] & mask:
                self.cand[i] &= ~mask
                changed = True
                if not self.cand[i]:
                    raise Stuck
        return changed

    def naked_pair(self):
        for unit in UNITS:
            pairs = [i for i in unit if popcount(self.cand[i]) == 2]
            for a in range(len(pairs)):
                for b in range(a + 1, len(pairs)):
                    mask = self.cand[pairs[a]]
                    if self.cand[pairs[b]] != mask:
                        continue
                    others = [i for i in unit if i not in (pairs[a], pairs[b])]
                    if self.eliminate(others, mask):
                        return True
        return False

    def hidden_pair(self):
        for unit in UNITS:
            places = {}
            for d in range(9):
                cells = tuple(i for i in unit if self.cand[i] >> d & 1)
                if len(cells) == 2:
                    places.setdefault(cells, []).append(d)
            for cells, ds in places.items():
                if len(ds) != 2:
                    continue
                keep = (1 << ds[0]) | (1 << ds[1])
                if self.eliminate(cells, ALL & ~keep):
                    return True
        return False

    def pointing(self):
        for box in BOXES:
            for d in range(9):
                bit = 1 << d
                cells = [i for i in box if self.cand[i] & bit]
                if len(cells) < 2:
                    continue
                for line_of, lines in ((ROW, ROWS), (COL, COLS)):
                    if len({line_of[i] for i in cells}) == 1:
                        line = lines[line_of[cells[0]]]
                        if self.eliminate([i for i in line if i not in box], bit):
                            return True
        return False

    def box_line(self):
        for line in ROWS + COLS:
            for d in range(9):
                bit = 1 << d
                cells = [i for i in line if self.cand[i] & bit]
                if len(cells) < 2 or len({BOX[i] for i in cells}) != 1:
                    continue
                box = BOXES[BOX[cells[0]]]
                if self.eliminate([i for i in box if i not in line], bit):
                    return True
        return False


ADVANCED = ("naked_pair", "hidden_pair", "pointing", "box_line")


def solve(board, techniques):
    """Returns the techniques used if `techniques` alone solve the grid, else None.

    Cheaper steps always go first, so a technique shows up in the answer only
    when everything below it had run dry: that is what "needs" means here.
    """
    try:
        state = Logic(board)
        used = set()
        while not state.solved():
            for name in techniques:
                if getattr(state, name)():
                    used.add(name)
                    break
            else:
                return None
        return used
    except Stuck:
        return None


SINGLES = ("naked_single", "hidden_single")
ALLOWED = {
    "easy": ("naked_single",),
    "medium": SINGLES,
    "hard": SINGLES + ADVANCED,
}


def grade(board):
    for level in LEVELS:
        used = solve(board, ALLOWED[level])
        if used is not None:
            return level, used
    return None, set()


def dig(level, rng):
    solution = count_solutions([0] * 81, 1, rng)[1]
    board = list(solution)
    order = list(range(81))
    rng.shuffle(order)
    low, high = TARGETS[level]
    target = rng.randint(low, high)
    for i in order:
        if 81 - board.count(0) <= target:
            break
        value, board[i] = board[i], 0
        if solve(board, ALLOWED[level]) is None:
            board[i] = value
    return board, solution


def text(cells):
    return "".join(map(str, cells))


def parse(line):
    parts = line.split("|")
    if len(parts) != 3 or parts[0] not in LEVELS:
        raise ValueError(f"bad line: {line!r}")
    level, clues, solution = parts
    if len(clues) != 81 or len(solution) != 81 or not (clues + solution).isdigit():
        raise ValueError(f"bad grid: {line!r}")
    return level, [int(c) for c in clues], [int(c) for c in solution]


def check(lines):
    """Checks every line and returns the originals that miss their grade."""
    seen = set()
    misgraded = []
    counts = {level: 0 for level in LEVELS}
    for number, line in enumerate(lines, 1):
        level, clues, solution = parse(line)
        counts[level] += 1
        if text(clues) in seen:
            raise ValueError(f"line {number}: repeated puzzle")
        seen.add(text(clues))
        if any(c and c != s for c, s in zip(clues, solution)):
            raise ValueError(f"line {number}: givens disagree with the solution")
        if count_solutions(solution, 2)[0] != 1 or 0 in solution:
            raise ValueError(f"line {number}: solution breaks a rule")
        total, first = count_solutions(clues, 2)
        if total != 1 or first != solution:
            raise ValueError(f"line {number}: {total} solutions")
        actual, used = grade(clues)
        if actual != level:
            if number > ORIGINAL:
                raise ValueError(f"line {number}: graded {actual}, labelled {level}")
            misgraded.append((number, level, actual))
    return counts, misgraded


def generate(per_level):
    path = Path(__file__).resolve().parent.parent / "assets" / "puzzles.txt"
    original = path.read_text().splitlines()[:ORIGINAL]
    lines = list(original)
    seen_clues = {text(parse(line)[1]) for line in lines}
    seen_solutions = {text(parse(line)[2]) for line in lines}
    for index, level in enumerate(LEVELS):
        rng = random.Random(SEED * 10 + index)
        have = sum(1 for line in original if line.startswith(level + "|"))
        attempts = 0
        while have < per_level:
            attempts += 1
            board, solution = dig(level, rng)
            if grade(board)[0] != level:
                continue
            if text(board) in seen_clues or text(solution) in seen_solutions:
                continue
            seen_clues.add(text(board))
            seen_solutions.add(text(solution))
            lines.append("|".join((level, text(board), text(solution))))
            have += 1
        print(f"{level}: {per_level} puzzles after {attempts} tries", file=sys.stderr)
    path.write_text("\n".join(lines) + "\n")
    return path, lines


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--check", action="store_true", help="check the pack without writing it")
    parser.add_argument("--per-level", type=int, default=PER_LEVEL)
    args = parser.parse_args()
    path = Path(__file__).resolve().parent.parent / "assets" / "puzzles.txt"
    if args.check:
        lines = path.read_text().splitlines()
    else:
        path, lines = generate(args.per_level)
    counts, misgraded = check(lines)
    print(f"{path.name}: {len(lines)} puzzles, " + ", ".join(f"{n} {lvl}" for lvl, n in counts.items()))
    for number, labelled, actual in misgraded:
        print(f"original line {number}: labelled {labelled}, graded {actual or 'beyond the solver'}")


if __name__ == "__main__":
    main()
