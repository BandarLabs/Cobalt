#!/usr/bin/env python3
"""Builds the Logic Pack collection and proves every puzzle in it.

The first twenty puzzles are the original collection. Saved games name a
puzzle by its place in the file, so those twenty are copied through exactly as
they are and everything new goes after them.

Every new puzzle is solved twice. A complete search proves that Slitherlink,
Hashi and Kakuro puzzles have exactly one solution. Then the puzzle is solved
the way a person would, one kind of reasoning at a time, and graded by the
hardest kind it needed:

- Easy needs only one clue at a time (Starter is the same on the smallest
  boards, with every clue shown where the game has clues to hide).
- Medium needs the puzzle's own global rule: one loop, every island
  connected, or where the digits of a sum have to go.
- Hard needs trying a move and following it to a contradiction.

Minesweeper fields open with a safe patch already cleared, and a field is kept
only when the rest can be cleared from there without ever guessing. Its grade
counts the same way: one number at a time, two numbers together, or every
number and the mine count at once.

    python3 apps/logicpack/tools/generate.py           # rebuild the collection
    python3 apps/logicpack/tools/generate.py --check   # verify it, change nothing

Standard library only, with fixed seeds, so the same file comes out every time.
"""

import argparse
import json
import random
import sys
from pathlib import Path

COLLECTION = Path(__file__).resolve().parents[1] / "assets" / "collection.json"
ORIGINAL_IDS = [
    "slither-four-twos", "hashi-cross", "kakuro-three-cells", "mines-four-square",
    "slither-01", "slither-02", "slither-03", "slither-04",
    "hashi-01", "hashi-02", "hashi-03", "hashi-04",
    "kakuro-01", "kakuro-02", "kakuro-03", "kakuro-04",
    "mines-01", "mines-02", "mines-03", "mines-04",
]
TIER = {"Starter": 1, "Easy": 1, "Medium": 2, "Hard": 3}
LEVELS = list(TIER)
DIRECTIONS = ((1, 0), (0, 1), (-1, 0), (0, -1))


def bits(mask):
    return bin(mask).count("1")


def trial(settle, state):
    """Tries each open edge both ways on a copy and keeps the way that survives.

    This is the one step a person does with a pencil held above the page:
    suppose a line, follow the easier rules, and see whether the board breaks.
    Only one supposition is ever open, so a puzzle graded Hard never needs a
    chain of guesses nested inside each other.
    """
    while True:
        if not settle(state):
            return False
        progress = False
        for index, value in enumerate(state):
            if value >= 0:
                continue
            for guess in (1, 0):
                attempt = state[:]
                attempt[index] = guess
                if not settle(attempt):
                    state[index] = 1 - guess
                    progress = True
                    break
            if progress:
                break
        if not progress:
            return True


# Slitherlink. Edges are numbered as the app numbers them: every across edge
# row by row, then every down edge.


def loop_edges(side):
    width = side + 1
    across = [(r * width + c, r * width + c + 1) for r in range(width) for c in range(side)]
    down = [(r * width + c, (r + 1) * width + c) for r in range(side) for c in range(width)]
    return across + down


def square_edges(side, square):
    row, column = divmod(square, side)
    left = side * (side + 1) + row * (side + 1) + column
    return [row * side + column, (row + 1) * side + column, left, left + 1]


class Loop:
    def __init__(self, side, clues):
        self.side = side
        self.edges = loop_edges(side)
        self.points = (side + 1) ** 2
        self.at = [[] for _ in range(self.points)]
        for index, (a, b) in enumerate(self.edges):
            self.at[a].append(index)
            self.at[b].append(index)
        self.clues = [(square_edges(side, i), n) for i, n in enumerate(clues) if n is not None]
        self.corners = []
        for cells, _ in self.clues:
            ends = [self.edges[e] for e in cells]
            corners = []
            for point in sorted({p for end in ends for p in end}):
                inside = [k for k, end in enumerate(ends) if point in end]
                outside = [e for e in self.at[point] if e not in cells]
                corners.append((inside, outside))
            self.corners.append(corners)

    def counted(self, state):
        """Reads each clue together with the four dots at its corners.

        That is how the first rules anyone learns are seen: a 3 in a corner
        fills both outer edges because the corner dot cannot hold a dead end,
        and a 1 in a corner rules both of them out.
        """
        while True:
            changed = False
            for (cells, need), corners in zip(self.clues, self.corners):
                fits = []
                for pattern in range(16):
                    lines = [pattern >> k & 1 for k in range(4)]
                    if sum(lines) != need or any(state[e] >= 0 and state[e] != v for e, v in zip(cells, lines)):
                        continue
                    if all(any(sum(lines[k] for k in inside) + sum(state[e] == 1 for e in outside) <= d
                               <= sum(lines[k] for k in inside) + sum(state[e] != 0 for e in outside)
                               for d in (0, 2)) for inside, outside in corners):
                        fits.append(lines)
                if not fits:
                    return False
                for k, e in enumerate(cells):
                    if state[e] < 0 and len({lines[k] for lines in fits}) == 1:
                        state[e] = fits[0][k]
                        changed = True
            for cells in self.at:
                on = sum(state[e] == 1 for e in cells)
                open_ = [e for e in cells if state[e] < 0]
                if on > 2 or (on == 1 and not open_):
                    return False
                if open_ and (on == 2 or len(open_) == 1):
                    for e in open_:
                        state[e] = int(on == 1)
                    changed = True
            if not changed:
                return True

    def one_loop(self, state):
        """Applies the rule that the line is a single loop.

        Returns None when the board already breaks it, otherwise whether it
        removed anything.
        """
        parent = list(range(self.points))

        def find(x):
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        degree = [0] * self.points
        for index, (a, b) in enumerate(self.edges):
            if state[index] == 1:
                parent[find(a)] = find(b)
                degree[a] += 1
                degree[b] += 1
        pieces = {find(a) for i, (a, _) in enumerate(self.edges) if state[i] == 1}
        ends = {find(v) for v in range(self.points) if degree[v] == 1}
        if pieces - ends:
            if len(pieces) > 1:
                return None
            open_ = [i for i, v in enumerate(state) if v < 0]
            for i in open_:
                state[i] = 0
            return bool(open_)
        changed = False
        for index, (a, b) in enumerate(self.edges):
            if state[index] < 0 and degree[a] == 1 and degree[b] == 1 and find(a) == find(b):
                if len(pieces) > 1 or not self.closes(state, index):
                    state[index] = 0
                    changed = True
        return changed

    def closes(self, state, index):
        final = [1 if i == index else max(v, 0) for i, v in enumerate(state)]
        return all(sum(final[e] for e in cells) == need for cells, need in self.clues)

    def settle(self, state, level):
        while True:
            if not self.counted(state):
                return False
            if level < 2:
                break
            changed = self.one_loop(state)
            if changed is None:
                return False
            if not changed:
                break
        return min(state) >= 0 and self.valid(state) or min(state) < 0

    def valid(self, state):
        degree = [0] * self.points
        for index, (a, b) in enumerate(self.edges):
            degree[a] += state[index]
            degree[b] += state[index]
        if any(d not in (0, 2) for d in degree) or not any(state):
            return False
        if any(sum(state[e] for e in cells) != need for cells, need in self.clues):
            return False
        start = next(v for v in range(self.points) if degree[v])
        seen, todo = {start}, [start]
        while todo:
            v = todo.pop()
            for e in self.at[v]:
                if state[e]:
                    w = self.edges[e][0] + self.edges[e][1] - v
                    if w not in seen:
                        seen.add(w)
                        todo.append(w)
        return len(seen) == sum(1 for d in degree if d)

    def tier(self, highest=3):
        for level in range(1, highest + 1):
            state = [-1] * len(self.edges)
            if level < 3:
                solved = self.settle(state, level)
            else:
                solved = trial(lambda s: self.settle(s, 2), state)
            if solved and min(state) >= 0:
                return level
        return None

    def solutions(self, limit=2):
        found = []

        def visit(state):
            if not self.settle(state, 2):
                return
            open_ = [i for i, v in enumerate(state) if v < 0]
            if not open_:
                found.append(state)
                return
            index = open_[0]
            for value in (1, 0):
                branch = state[:]
                branch[index] = value
                visit(branch)
                if len(found) >= limit:
                    return

        visit([-1] * len(self.edges))
        return found


def region_loop(side, region):
    answer = []
    for r in range(side + 1):
        for c in range(side):
            answer.append(int(((r - 1, c) in region) != ((r, c) in region)))
    for r in range(side):
        for c in range(side + 1):
            answer.append(int(((r, c - 1) in region) != ((r, c) in region)))
    return answer


def symmetries(side, cells):
    """The eight turns and reflections of a set of squares, as sorted tuples."""
    last = side - 1
    shapes = []
    for flip in (False, True):
        for turn in range(4):
            moved = []
            for r, c in cells:
                if flip:
                    c = last - c
                for _ in range(turn):
                    r, c = c, last - r
                moved.append((r, c))
            shapes.append(tuple(sorted(moved)))
    return shapes


def slither(rng, side, level, seen):
    full = side * side
    for _ in range(4000):
        region = {(rng.randrange(side), rng.randrange(side))}
        size = rng.randint(2, full - 2)
        while len(region) < size:
            r, c = rng.choice(sorted(region))
            dr, dc = rng.choice(DIRECTIONS)
            if 0 <= r + dr < side and 0 <= c + dc < side:
                region.add((r + dr, c + dc))
        key = min(symmetries(side, region))
        if key in seen:
            continue
        answer = region_loop(side, region)
        if not Loop(side, [None] * full).valid(answer):
            continue
        clues = [sum(answer[e] for e in square_edges(side, i)) for i in range(full)]
        target = TIER[level]
        if level != "Starter":
            order = list(range(full))
            rng.shuffle(order)
            for square in order:
                kept, clues[square] = clues[square], None
                if Loop(side, clues).tier(target) is None:
                    clues[square] = kept
        if Loop(side, clues).tier() != target:
            continue
        seen.add(key)
        return clues, answer
    raise RuntimeError(f"no {level} Slitherlink on {side} x {side}")


def slither_puzzles(rng):
    seen = set()
    plan = [("Starter", 3, 8), ("Easy", 3, 12), ("Easy", 4, 22), ("Medium", 4, 30), ("Hard", 4, 28)]
    made = []
    for level, side, count in plan:
        for _ in range(count):
            clues, answer = slither(rng, side, level, seen)
            shown = sum(n is not None for n in clues)
            guide = {
                "Starter": "Every clue is shown.",
                "Easy": "Each line follows from one clue and its corners.",
                "Medium": "Some lines follow only from keeping one loop.",
                "Hard": "Some lines need you to try one and follow it through.",
            }[level]
            made.append((level, side, shown, dict(
                kind="slither", difficulty=level,
                guide=f"{side} × {side} cells, {shown} clues. {guide}",
                side=side, clues=clues, solution=answer)))
    return made


# Hashi.


class Bridges:
    """Routes hold a set of bridge counts as bits: 1 for none, 2 for one, 4 for two."""

    def __init__(self, islands, routes):
        self.islands = islands
        self.routes = routes
        self.at = [[] for _ in islands]
        for index, (a, b) in enumerate(routes):
            self.at[a].append(index)
            self.at[b].append(index)
        self.crosses = [[] for _ in routes]
        for i, (a, b) in enumerate(routes):
            for j, (c, d) in enumerate(routes[:i]):
                if self.cross(a, b, c, d) or self.cross(c, d, a, b):
                    self.crosses[i].append(j)
                    self.crosses[j].append(i)

    def cross(self, a, b, c, d):
        (ax, ay, _), (bx, by, _) = self.islands[a], self.islands[b]
        (cx, cy, _), (dx, dy, _) = self.islands[c], self.islands[d]
        return (ax == bx and cy == dy and min(cx, dx) < ax < max(cx, dx)
                and min(ay, by) < cy < max(ay, by))

    def counted(self, state):
        while True:
            changed = False
            for island, (_, _, need) in enumerate(self.islands):
                routes = self.at[island]
                for route in routes:
                    reach = {0}
                    for other in routes:
                        if other != route:
                            reach = {t + v for t in reach for v in range(3) if state[other] >> v & 1}
                    keep = sum(1 << v for v in range(3) if state[route] >> v & 1 and need - v in reach)
                    if not keep:
                        return False
                    if keep != state[route]:
                        state[route] = keep
                        changed = True
            for route, values in enumerate(state):
                if not values & 1:
                    for other in self.crosses[route]:
                        if not state[other] & 1:
                            return False
                        if state[other] != 1:
                            state[other] = 1
                            changed = True
            if not changed:
                return True

    def joined(self, state, usable):
        seen, todo = {0}, [0]
        while todo:
            v = todo.pop()
            for route in self.at[v]:
                if usable(route):
                    a, b = self.routes[route]
                    w = a + b - v
                    if w not in seen:
                        seen.add(w)
                        todo.append(w)
        return seen

    def connected(self, state):
        """Removes any bridge that would close off a group of islands from the rest."""
        if len(self.joined(state, lambda r: state[r] & 6)) < len(self.islands):
            return None
        least = [(v & -v).bit_length() - 1 for v in state]
        changed = False
        for route, (a, b) in enumerate(self.routes):
            for count in (1, 2):
                if not state[route] >> count & 1:
                    continue
                group, todo = {a}, [a]
                while todo:
                    v = todo.pop()
                    for r in self.at[v]:
                        if r == route or least[r] >= 1:
                            x, y = self.routes[r]
                            w = x + y - v
                            if w not in group:
                                group.add(w)
                                todo.append(w)
                if len(group) == len(self.islands):
                    continue
                if all(sum(count if r == route else least[r] for r in self.at[v]) == self.islands[v][2]
                       for v in group):
                    state[route] &= ~(1 << count)
                    changed = True
                    if not state[route]:
                        return None
        return changed

    def settle(self, state, level):
        while True:
            if not self.counted(state):
                return False
            if level < 2:
                break
            changed = self.connected(state)
            if changed is None:
                return False
            if not changed:
                break
        if all(bits(v) == 1 for v in state):
            return len(self.joined(state, lambda r: state[r] & 6)) == len(self.islands)
        return True

    @staticmethod
    def choices(state):
        return [(i, [v for v in range(3) if s >> v & 1]) for i, s in enumerate(state) if bits(s) > 1]

    def tier(self, highest=3):
        for level in range(1, highest + 1):
            state = [7] * len(self.routes)
            if level < 3:
                solved = self.settle(state, level)
            else:
                solved = self.hard(state)
            if solved and all(bits(v) == 1 for v in state):
                return level
        return None

    def hard(self, state):
        while True:
            if not self.settle(state, 2):
                return False
            progress = False
            for route, values in self.choices(state):
                for value in values:
                    attempt = state[:]
                    attempt[route] = 1 << value
                    if not self.settle(attempt, 2):
                        state[route] &= ~(1 << value)
                        progress = True
                if progress:
                    break
            if not progress:
                return True

    def solutions(self, limit=2):
        found = []

        def visit(state):
            if not self.settle(state, 2):
                return
            open_ = [i for i, v in enumerate(state) if bits(v) > 1]
            if not open_:
                found.append([v.bit_length() - 1 for v in state])
                return
            route = min(open_, key=lambda i: bits(state[i]))
            for value in range(3):
                if state[route] >> value & 1:
                    branch = state[:]
                    branch[route] = 1 << value
                    visit(branch)
                    if len(found) >= limit:
                        return

        visit([7] * len(self.routes))
        return found


def bridge_routes(islands):
    routes = []
    for i, (x, y, _) in enumerate(islands):
        right = [j for j, (a, b, _) in enumerate(islands) if b == y and a > x]
        down = [j for j, (a, b, _) in enumerate(islands) if a == x and b > y]
        if right:
            routes.append((i, min(right, key=lambda j: islands[j][0])))
        if down:
            routes.append((i, min(down, key=lambda j: islands[j][1])))
    return routes


def hashi_layout(rng, width, height, count, extra):
    """Grows a connected network one bridge at a time, so a solution exists by construction."""
    spots = [(rng.randrange(width), rng.randrange(height))]
    placed = {spots[0]}
    under = set()
    bridges = {}
    for _ in range(4000):
        if len(spots) == count:
            break
        a = rng.randrange(len(spots))
        x, y = spots[a]
        dx, dy = rng.choice(DIRECTIONS)
        length = rng.randint(2, max(width, height) - 1)
        end = (x + dx * length, y + dy * length)
        path = [(x + dx * k, y + dy * k) for k in range(1, length)]
        if not (0 <= end[0] < width and 0 <= end[1] < height):
            continue
        if end in placed or end in under or any(p in placed or p in under for p in path):
            continue
        if any((end[0] + ex, end[1] + ey) in placed for ex, ey in DIRECTIONS):
            continue
        placed.add(end)
        spots.append(end)
        under.update(path)
        bridges[(a, len(spots) - 1)] = rng.choice((1, 1, 2))
    if len(spots) < count:
        return None
    order = sorted(range(len(spots)), key=lambda i: (spots[i][1], spots[i][0]))
    rank = {old: new for new, old in enumerate(order)}
    bridges = {tuple(sorted((rank[a], rank[b]))): n for (a, b), n in bridges.items()}
    spots = [spots[i] for i in order]
    routes = bridge_routes([(x, y, 0) for x, y in spots])
    for a, b in routes:
        if (a, b) in bridges or rng.random() >= extra:
            continue
        (ax, ay), (bx, by) = spots[a], spots[b]
        path = [(x, y) for x in range(min(ax, bx), max(ax, bx) + 1)
                for y in range(min(ay, by), max(ay, by) + 1)][1:-1]
        if any(p in under for p in path):
            continue
        under.update(path)
        bridges[(a, b)] = rng.choice((1, 2))
    left = min(x for x, _ in spots)
    top = min(y for _, y in spots)
    islands = [[x - left, y - top, 0] for x, y in spots]
    answer = [bridges.get((a, b), 0) for a, b in routes]
    for (a, b), n in zip(routes, answer):
        islands[a][2] += n
        islands[b][2] += n
    width = max(x for x, _, _ in islands) + 1
    height = max(y for _, y, _ in islands) + 1
    return width, height, islands, [list(r) for r in routes], answer


def hashi(rng, level, board, counts, seen):
    target = TIER[level]
    for _ in range(6000):
        layout = hashi_layout(rng, board, board, rng.randint(*counts), rng.choice((0.2, 0.4, 0.6)))
        if layout is None:
            continue
        width, height, islands, routes, answer = layout
        key = json.dumps([width, height, islands])
        if key in seen or len(routes) > 64:
            continue
        model = Bridges([tuple(i) for i in islands], [tuple(r) for r in routes])
        if model.tier() != target:
            continue
        seen.add(key)
        return width, height, islands, routes, answer
    raise RuntimeError(f"no {level} Hashi on {board}")


def hashi_puzzles(rng):
    seen = set()
    plan = [("Starter", 5, (4, 6), 8), ("Easy", 7, (7, 11), 30),
            ("Medium", 7, (9, 14), 16), ("Medium", 9, (12, 18), 16), ("Hard", 9, (14, 22), 30)]
    made = []
    for level, board, counts, count in plan:
        for _ in range(count):
            width, height, islands, routes, answer = hashi(rng, level, board, counts, seen)
            guide = {
                "Starter": "Each bridge follows from one island's number.",
                "Easy": "Each bridge follows from one island's number.",
                "Medium": "Some bridges follow only from keeping every island connected.",
                "Hard": "Some bridges need you to try one and follow it through.",
            }[level]
            made.append((level, len(islands), len(routes), dict(
                kind="hashi", difficulty=level, guide=f"{len(islands)} islands. {guide}",
                width=width, height=height, islands=islands, routes=routes, solution=answer)))
    return made


# Kakuro. Runs are listed the way the app lists them: white squares in reading
# order, each one starting an across run and then a down run where it can.

COMBOS = {}
for _mask in range(1, 512):
    _digits = [d for d in range(1, 10) if _mask >> (d - 1) & 1]
    COMBOS.setdefault((len(_digits), sum(_digits)), []).append(_mask)


def cross_runs(mask):
    white = [(r, c) for r, row in enumerate(mask) for c, square in enumerate(row) if square == "."]
    lookup = {p: i for i, p in enumerate(white)}
    runs = []
    for r, c in white:
        for dr, dc in ((0, 1), (1, 0)):
            if (r - dr, c - dc) in lookup:
                continue
            cells = []
            rr, cc = r, c
            while (rr, cc) in lookup:
                cells.append(lookup[(rr, cc)])
                rr += dr
                cc += dc
            if len(cells) < 2:
                raise ValueError("a run of one square")
            runs.append({"cells": cells, "clue": [c - dc, r - dr], "down": bool(dr)})
    return white, runs


class CrossSum:
    def __init__(self, mask, sums, givens):
        self.white, runs = cross_runs(mask)
        self.runs = [(run["cells"], total) for run, total in zip(runs, sums)]
        self.combos = [COMBOS.get((len(cells), total), []) for cells, total in self.runs]
        self.givens = givens

    def start(self):
        return [1 << (g - 1) if g else 511 for g in self.givens]

    def counted(self, state, level):
        while True:
            changed = False
            for (cells, _), combos in zip(self.runs, self.combos):
                placed = [state[c] for c in cells if bits(state[c]) == 1]
                fixed = 0
                for p in placed:
                    fixed |= p
                if bits(fixed) < len(placed):
                    return False
                union = 0
                for c in cells:
                    union |= state[c]
                live = [m for m in combos
                        if m & fixed == fixed and m & union == m and all(state[c] & m for c in cells)]
                if not live:
                    return False
                allowed = 0
                needed = 511
                for m in live:
                    allowed |= m
                    needed &= m
                for c in cells:
                    keep = state[c] & allowed
                    if bits(state[c]) > 1:
                        keep &= ~fixed
                    if not keep:
                        return False
                    if keep != state[c]:
                        state[c] = keep
                        changed = True
                if level >= 2:
                    for d in range(9):
                        if needed >> d & 1:
                            holders = [c for c in cells if state[c] >> d & 1]
                            if not holders:
                                return False
                            if len(holders) == 1 and state[holders[0]] != 1 << d:
                                state[holders[0]] = 1 << d
                                changed = True
            if not changed:
                return True

    def settle(self, state, level):
        if not self.counted(state, level):
            return False
        if all(bits(v) == 1 for v in state):
            return self.valid([v.bit_length() for v in state])
        return True

    def valid(self, digits):
        return all(len({digits[c] for c in cells}) == len(cells) and sum(digits[c] for c in cells) == total
                   for cells, total in self.runs)

    @staticmethod
    def choices(state):
        return [(i, [d for d in range(9) if v >> d & 1]) for i, v in enumerate(state) if bits(v) > 1]

    def hard(self, state):
        while True:
            if not self.settle(state, 2):
                return False
            progress = False
            for cell, digits in self.choices(state):
                for d in digits:
                    attempt = state[:]
                    attempt[cell] = 1 << d
                    if not self.settle(attempt, 2):
                        state[cell] &= ~(1 << d)
                        progress = True
                if progress:
                    break
            if not progress:
                return True

    def tier(self, highest=3):
        for level in range(1, highest + 1):
            state = self.start()
            solved = self.settle(state, level) if level < 3 else self.hard(state)
            if solved and all(bits(v) == 1 for v in state):
                return level
        return None

    def solutions(self, limit=2):
        found = []

        def visit(state):
            if not self.settle(state, 2):
                return
            open_ = [i for i, v in enumerate(state) if bits(v) > 1]
            if not open_:
                found.append([v.bit_length() for v in state])
                return
            cell = min(open_, key=lambda i: bits(state[i]))
            for d in range(9):
                if state[cell] >> d & 1:
                    branch = state[:]
                    branch[cell] = 1 << d
                    visit(branch)
                    if len(found) >= limit:
                        return

        visit(self.start())
        return found


def kakuro_mask(rng, rows, columns, blacks):
    for _ in range(1000):
        grid = [["#"] * columns] + [["#"] + ["."] * (columns - 1) for _ in range(rows - 1)]
        inside = [(r, c) for r in range(1, rows) for c in range(1, columns)]
        for r, c in rng.sample(inside, blacks):
            grid[r][c] = "#"
        changed = True
        while changed:
            changed = False
            for r, c in inside:
                if grid[r][c] != ".":
                    continue
                across = (grid[r][c - 1] == ".") + (c + 1 < columns and grid[r][c + 1] == ".")
                down = (grid[r - 1][c] == ".") + (r + 1 < rows and grid[r + 1][c] == ".")
                if not across or not down:
                    grid[r][c] = "#"
                    changed = True
        white = [(r, c) for r, c in inside if grid[r][c] == "."]
        if not white or len(white) > 64:
            continue
        if any(all(grid[r][c] == "#" for c in range(columns)) for r in (1, rows - 1)):
            continue
        if any(all(grid[r][c] == "#" for r in range(rows)) for c in (1, columns - 1)):
            continue
        seen, todo = {white[0]}, [white[0]]
        while todo:
            r, c = todo.pop()
            for dr, dc in DIRECTIONS:
                if (r + dr, c + dc) in white and (r + dr, c + dc) not in seen:
                    seen.add((r + dr, c + dc))
                    todo.append((r + dr, c + dc))
        if len(seen) == len(white):
            return ["".join(row) for row in grid]
    return None


def kakuro_fill(rng, size, runs):
    member = [[] for _ in range(size)]
    for cells, _ in runs:
        for c in cells:
            member[c].append(cells)
    digits = [0] * size

    def visit(cell):
        if cell == size:
            return True
        used = {digits[c] for cells in member[cell] for c in cells}
        options = [d for d in range(1, 10) if d not in used]
        rng.shuffle(options)
        for d in options:
            digits[cell] = d
            if visit(cell + 1):
                return True
        digits[cell] = 0
        return False

    return digits if visit(0) else None


def kakuro(rng, level, shape, seen):
    rows, columns, blacks, most_givens = shape
    target = TIER[level]
    for _ in range(3000):
        mask = kakuro_mask(rng, rows, columns, rng.randint(*blacks))
        if mask is None:
            continue
        white, runs = cross_runs(mask)
        model = CrossSum(mask, [0] * len(runs), [0] * len(white))
        digits = kakuro_fill(rng, len(white), model.runs)
        if digits is None:
            continue
        sums = [sum(digits[c] for c in run["cells"]) for run in runs]
        givens = [0] * len(white)
        while True:
            found = CrossSum(mask, sums, givens).solutions(2)
            if len(found) == 1:
                break
            cell = rng.choice([c for c in range(len(white)) if found[0][c] != found[1][c]])
            givens[cell] = digits[cell]
        order = [c for c in range(len(white)) if givens[c]]
        rng.shuffle(order)
        for cell in order:
            givens[cell] = 0
            if len(CrossSum(mask, sums, givens).solutions(2)) != 1:
                givens[cell] = digits[cell]
        if sum(bool(g) for g in givens) > most_givens:
            continue
        key = (tuple(mask), tuple(sums))
        if key in seen or CrossSum(mask, sums, givens).tier() != target:
            continue
        seen.add(key)
        return mask, sums, givens, digits
    raise RuntimeError(f"no {level} Kakuro on {rows} x {columns}")


def kakuro_puzzles(rng):
    seen = set()
    plan = [("Starter", (3, 3, (0, 0), 2), 2), ("Starter", (3, 4, (0, 0), 2), 3),
            ("Starter", (4, 4, (0, 2), 2), 5),
            ("Easy", (5, 5, (2, 5), 3), 14), ("Easy", (6, 6, (5, 9), 4), 16),
            ("Medium", (6, 6, (4, 8), 4), 14), ("Medium", (7, 7, (8, 14), 5), 16),
            ("Hard", (7, 7, (6, 12), 5), 14), ("Hard", (8, 8, (10, 18), 6), 16)]
    made = []
    for level, shape, count in plan:
        for _ in range(count):
            mask, sums, givens, digits = kakuro(rng, level, shape, seen)
            printed = sum(bool(g) for g in givens)
            blanks = len(givens) - printed
            guide = {
                "Starter": "Each digit follows from one sum.",
                "Easy": "Each digit follows from one sum.",
                "Medium": "Some digits follow from where a sum's digits must go.",
                "Hard": "Some digits need you to try one and follow it through.",
            }[level]
            given = f", {printed} printed" if printed else ""
            made.append((level, blanks, printed, dict(
                kind="kakuro", difficulty=level, guide=f"{blanks} blanks{given}. {guide}",
                mask=mask, sums=sums, givens=givens, solution=digits)))
    return made


# Minesweeper.


def around(cell, side):
    row, column = divmod(cell, side)
    return [r * side + c for r in range(max(0, row - 1), min(side, row + 2))
            for c in range(max(0, column - 1), min(side, column + 2)) if (r, c) != (row, column)]


def by_one(rules):
    safe, mined = set(), set()
    for cells, left in rules:
        if left == 0:
            safe.update(cells)
        elif left == len(cells):
            mined.update(cells)
    return safe, mined


def by_two(rules):
    safe, mined = set(), set()
    for a, left_a in rules:
        for b, left_b in rules:
            shared = a & b
            if a is b or not shared:
                continue
            only_a, only_b = a - shared, b - shared
            least = max(0, left_a - len(only_a), left_b - len(only_b))
            most = min(left_a, left_b, len(shared))
            if only_a and left_a - least == 0:
                safe |= only_a
            if only_a and left_a - most == len(only_a):
                mined |= only_a
            if most == 0:
                safe |= shared
            if least == len(shared):
                mined |= shared
    return safe, mined


def by_all(rules, hidden, mines_left):
    """Every arrangement of mines that fits every number and the mine count.

    Squares touching no number are counted together, since nothing can tell
    them apart, so the enumeration only grows with the edge of the cleared
    area.
    """
    edge = sorted({c for cells, _ in rules for c in cells})
    rest = len(hidden) - len(edge)
    links = {c: [i for i, (cells, _) in enumerate(rules) if c in cells] for c in edge}
    groups, placed = [], set()
    for c in edge:
        if c in placed:
            continue
        group, todo = [], [c]
        placed.add(c)
        while todo:
            x = todo.pop()
            group.append(x)
            for i in links[x]:
                for y in rules[i][0]:
                    if y not in placed:
                        placed.add(y)
                        todo.append(y)
        groups.append(sorted(group))
    outcomes = []
    for group in groups:
        found = {}
        chosen = {}

        def fits(rule):
            cells, left = rule
            placed = sum(chosen.get(c, 0) for c in cells)
            return placed <= left <= placed + sum(c not in chosen for c in cells)

        def visit(index):
            if index == len(group):
                can = found.setdefault(sum(chosen.values()), (set(), set()))
                for c, v in chosen.items():
                    can[v].add(c)
                return
            cell = group[index]
            for v in (0, 1):
                chosen[cell] = v
                if all(fits(rules[i]) for i in links[cell]):
                    visit(index + 1)
            del chosen[cell]

        visit(0)
        outcomes.append(found)
    safe, mined = set(), set()
    for index, (group, mine) in enumerate(zip(groups, outcomes)):
        others = {0}
        for other, counts in enumerate(outcomes):
            if other != index:
                others = {a + b for a in others for b in counts}
        can_mine, can_clear = set(), set()
        for count, (clear, bomb) in mine.items():
            if any(0 <= mines_left - count - o <= rest for o in others):
                can_mine |= bomb
                can_clear |= clear
        safe |= set(group) - can_mine
        mined |= set(group) - can_clear
    if rest:
        totals = {0}
        for counts in outcomes:
            totals = {a + b for a in totals for b in counts}
        leftover = [mines_left - t for t in totals if 0 <= mines_left - t <= rest]
        interior = [c for c in hidden if c not in links]
        if leftover and max(leftover) == 0:
            safe.update(interior)
        if leftover and min(leftover) == rest:
            mined.update(interior)
    return safe, mined


def mines_tier(side, mines, start):
    """The hardest step a player needs from the opening, or None if they must guess."""
    total = side * side
    mine = [c in mines for c in range(total)]
    count = [sum(mine[x] for x in around(c, side)) for c in range(total)]
    if mine[start] or count[start]:
        return None
    shown = [False] * total
    flagged = [False] * total

    def reveal(cell):
        todo = [cell]
        while todo:
            c = todo.pop()
            if shown[c]:
                continue
            shown[c] = True
            if not count[c]:
                todo.extend(x for x in around(c, side) if not shown[x])

    reveal(start)
    hardest = 1
    while sum(shown) < total - len(mines):
        rules = []
        for c in range(total):
            if shown[c] and count[c]:
                cells = frozenset(x for x in around(c, side) if not shown[x] and not flagged[x])
                if cells:
                    rules.append((cells, count[c] - sum(flagged[x] for x in around(c, side))))
        hidden = [c for c in range(total) if not shown[c] and not flagged[c]]
        for step, deduce in enumerate((by_one, by_two, by_all), start=1):
            if deduce is by_all:
                safe, mined = deduce(rules, hidden, len(mines) - sum(flagged))
            else:
                safe, mined = deduce(rules)
            if safe or mined:
                break
        else:
            return None
        if any(mine[c] for c in safe) or not all(mine[c] for c in mined):
            raise AssertionError("an unsound Minesweeper deduction")
        hardest = max(hardest, step)
        for c in mined:
            flagged[c] = True
        for c in safe:
            reveal(c)
    return hardest


def minefield(rng, level, side, counts, seen):
    target = TIER[level]
    for _ in range(20000):
        mines = sorted(rng.sample(range(side * side), rng.randint(*counts)))
        zeros = [c for c in range(side * side)
                 if c not in mines and not any(x in mines for x in around(c, side))]
        if not zeros:
            continue
        start = rng.choice(zeros)
        key = (side, tuple(mines), start)
        if key in seen:
            continue
        if mines_tier(side, set(mines), start) != target:
            continue
        seen.add(key)
        return mines, start
    raise RuntimeError(f"no {level} field on {side} x {side}")


def mines_puzzles(rng):
    seen = set()
    plan = [("Starter", 5, (3, 4), 8), ("Easy", 6, (5, 7), 30),
            ("Medium", 7, (8, 11), 32), ("Hard", 8, (11, 15), 30)]
    made = []
    for level, side, counts, count in plan:
        for _ in range(count):
            mines, start = minefield(rng, level, side, counts, seen)
            guide = {
                "Starter": "Each square follows from one number.",
                "Easy": "Each square follows from one number.",
                "Medium": "Some squares follow from two numbers together.",
                "Hard": "Some squares need several numbers at once, or the mine count.",
            }[level]
            made.append((level, side, len(mines), dict(
                kind="mines", difficulty=level,
                guide=f"{side} × {side} cells with {len(mines)} mines. {guide} No guessing needed.",
                side=side, mines=mines, open=start)))
    return made


# The collection.


def originals():
    pack = json.loads(COLLECTION.read_text(encoding="utf-8"))
    first = pack[:len(ORIGINAL_IDS)]
    if [p["id"] for p in first] != ORIGINAL_IDS:
        raise SystemExit("The original twenty puzzles are missing or out of order.")
    return first


def build():
    pack = originals()
    for kind, make, seed in (("slither", slither_puzzles, 1), ("hashi", hashi_puzzles, 2),
                             ("kakuro", kakuro_puzzles, 3), ("mines", mines_puzzles, 4)):
        made = make(random.Random(20261028 * 10 + seed))
        made.sort(key=lambda item: (LEVELS.index(item[0]), item[1:-1]))
        number = sum(p["kind"] == kind for p in pack)
        for _, *_, puzzle in made:
            number += 1
            pack.append(dict(id=f"{kind}-{number - 1:02}", title=f"Puzzle {number}", **puzzle))
    return [dict(id=p["id"], kind=p["kind"], title=p["title"], difficulty=p["difficulty"],
                 guide=p["guide"], **{k: v for k, v in p.items()
                                      if k not in ("id", "kind", "title", "difficulty", "guide")})
            for p in pack]


def verify(pack):
    """Checks every puzzle against the app's limits and proves the claims made for it."""
    ids = [p["id"] for p in pack]
    assert ids[:len(ORIGINAL_IDS)] == ORIGINAL_IDS, "original order"
    assert len(set(ids)) == len(ids), "duplicate id"
    for index, p in enumerate(pack):
        new = index >= len(ORIGINAL_IDS)
        kind = p["kind"]
        if kind == "slither":
            assert p["side"] <= 4, p["id"]
            model = Loop(p["side"], p["clues"])
            assert model.solutions() == [p["solution"]], p["id"]
            grade = model.tier()
        elif kind == "hashi":
            assert max(p["width"], p["height"]) <= 9 and len(p["routes"]) <= 64, p["id"]
            assert all(1 <= n <= 8 for _, _, n in p["islands"]), p["id"]
            model = Bridges([tuple(i) for i in p["islands"]], [tuple(r) for r in p["routes"]])
            assert model.solutions() == [p["solution"]], p["id"]
            grade = model.tier()
        elif kind == "kakuro":
            assert len(p["mask"]) <= 9 and len(p["mask"][0]) <= 9, p["id"]
            sums = p.get("sums") or [run["sum"] for run in p["runs"]]
            assert max(sums) <= 45 and p["givens"].count(0) <= 64, p["id"]
            model = CrossSum(p["mask"], sums, p["givens"])
            assert model.solutions() == [p["solution"]], p["id"]
            grade = model.tier()
        else:
            assert p["side"] <= 8 and len(set(p["mines"])) == len(p["mines"]), p["id"]
            if not new:
                continue
            grade = mines_tier(p["side"], set(p["mines"]), p["open"])
        if new:
            assert grade == TIER[p["difficulty"]], (p["id"], grade)


def write(pack):
    lines = [json.dumps(p, ensure_ascii=False, separators=(",", ":")) for p in pack]
    COLLECTION.write_text("[\n" + ",\n".join(lines) + "\n]\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="verify the shipped collection")
    arguments = parser.parse_args()
    if arguments.check:
        pack = json.loads(COLLECTION.read_text(encoding="utf-8"))
        verify(pack)
        print(f"Verified {len(pack)} puzzles.")
        return
    pack = build()
    verify(pack)
    write(pack)
    kinds = {}
    for p in pack:
        kinds.setdefault(p["kind"], {}).setdefault(p["difficulty"], 0)
        kinds[p["kind"]][p["difficulty"]] += 1
    print(f"Wrote {len(pack)} puzzles: {kinds}", file=sys.stderr)


if __name__ == "__main__":
    main()
