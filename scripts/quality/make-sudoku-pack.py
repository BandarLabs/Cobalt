#!/usr/bin/env python3
"""Generate original Sudoku puzzles; no external puzzle corpus or source code.

Difficulty is measured by this deliberately limited logical solver: easy uses
single candidates, medium additionally uses single locations in a unit, and
hard needs something beyond those two techniques. Uniqueness is checked by a
separate bounded-at-two exhaustive solver. Seeds reproduce the committed pack.
"""
import random
from pathlib import Path

UNITS = [[r*9+c for c in range(9)] for r in range(9)] + [[r*9+c for r in range(9)] for c in range(9)] + [[(br*3+r)*9+bc*3+c for r in range(3) for c in range(3)] for br in range(3) for bc in range(3)]
PEERS = [set().union(*(set(u) for u in UNITS if i in u)) - {i} for i in range(81)]

def options(board, i):
    return set(range(1, 10)) - {board[p] for p in PEERS[i]}

def solutions(board, rng=None, limit=2):
    blank = [(options(board, i), i) for i, n in enumerate(board) if not n]
    if not blank:
        return [board.copy()]
    choices, i = min(blank, key=lambda x: len(x[0]))
    choices = sorted(choices)
    if rng is not None:
        rng.shuffle(choices)
    found = []
    for n in choices:
        board[i] = n
        found += solutions(board, rng, limit-len(found))
        if len(found) >= limit:
            break
    board[i] = 0
    return found

def grade(source):
    board = source.copy()
    hidden = False
    while 0 in board:
        candidates = {i: options(board, i) for i, n in enumerate(board) if not n}
        single = next(((i, next(iter(ns))) for i, ns in candidates.items() if len(ns) == 1), None)
        if single is None:
            for unit in UNITS:
                for n in range(1, 10):
                    cells = [i for i in unit if n in candidates.get(i, ())]
                    if len(cells) == 1:
                        single = cells[0], n
                        hidden = True
                        break
                if single:
                    break
        if single is None:
            return 'hard'
        board[single[0]] = single[1]
    return 'medium' if hidden else 'easy'

def main():
    rng = random.Random(20260908)
    groups = {level: [] for level in ('easy', 'medium', 'hard')}
    attempts = 0
    while any(len(rows) < 12 for rows in groups.values()):
        attempts += 1
        if attempts > 10000:
            raise RuntimeError('Could not fill all difficulty groups')
        solved = solutions([0]*81, rng, 1)[0]
        board = solved.copy()
        order = list(range(81))
        rng.shuffle(order)
        target = rng.choice((28, 30, 32, 36, 40))
        for i in order:
            value, board[i] = board[i], 0
            if len(solutions(board)) != 1:
                board[i] = value
            if sum(bool(n) for n in board) <= target:
                break
        level = grade(board)
        if len(groups[level]) < 12:
            groups[level].append('|'.join((level, ''.join(map(str, board)), ''.join(map(str, solved)))))
    out = Path(__file__).resolve().parents[2] / 'apps/sudoku/assets/puzzles.txt'
    out.write_text('\n'.join(row for rows in groups.values() for row in rows) + '\n')
    print(f'Wrote 36 original puzzles in {attempts} attempts to {out}')

if __name__ == '__main__':
    main()
