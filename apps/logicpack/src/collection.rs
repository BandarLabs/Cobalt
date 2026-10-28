//! Original, generated puzzle definitions; IDs are stable save identities.
use crate::Kind;
use kobo_json::Value;
use std::sync::OnceLock;

pub struct Run {
    pub cells: Vec<usize>,
    pub clue: (u8, u8),
    pub down: bool,
    pub sum: u8,
}
pub enum Rules {
    Loop {
        side: usize,
        clues: Vec<Option<u8>>,
    },
    Bridges {
        width: u8,
        height: u8,
        islands: Vec<(u8, u8, u8)>,
        routes: Vec<(usize, usize)>,
    },
    CrossSum {
        mask: Vec<String>,
        runs: Vec<Run>,
        givens: Vec<u8>,
    },
    Mines {
        side: usize,
        mines: u64,
        /// A square the app opens before the first tap. Fields that have one
        /// were checked to clear from it without a guess; the five original
        /// fields have none and keep their safe first reveal instead.
        open: Option<usize>,
    },
}
/// The grades in the order the collection teaches them.
pub const LEVELS: [&str; 4] = ["Starter", "Easy", "Medium", "Hard"];
pub struct Puzzle {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub guide: String,
    pub kind: Kind,
    pub rules: Rules,
    /// The intended answer, which only the collection test reads: the app
    /// judges a board by the rules, never by comparing it with this.
    #[cfg(test)]
    pub solution: Vec<u8>,
}
fn number(v: &Value) -> u8 {
    u8::try_from(v.as_i64().expect("integer")).expect("small number")
}
fn numbers(v: &Value) -> Vec<u8> {
    v.as_array().expect("array").iter().map(number).collect()
}
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).expect("collection field")
}
fn string(v: &Value, key: &str) -> String {
    field(v, key).as_str().expect("text").into()
}
pub fn puzzles() -> &'static [Puzzle] {
    static COLLECTION: OnceLock<Vec<Puzzle>> = OnceLock::new();
    COLLECTION.get_or_init(|| {
        let value = kobo_json::parse(include_str!("../assets/collection.json"))
            .expect("original collection");
        let puzzles: Vec<Puzzle> = value
            .as_array()
            .expect("puzzles")
            .iter()
            .map(parse_puzzle)
            .collect();
        // Only the bounds that indexing relies on are checked here. Proving
        // every answer on each start would cost a reader with one slow core
        // for something the collection test already proves before release.
        for puzzle in &puzzles {
            assert!(puzzle.cell_count() <= 64, "bounded collection");
            assert!(
                puzzle
                    .opening()
                    .is_none_or(|open| open < puzzle.cell_count()),
                "opening on the field"
            );
        }
        puzzles
    })
}
fn parse_puzzle(v: &Value) -> Puzzle {
    let (kind, rules) = match field(v, "kind").as_str().expect("kind") {
        "slither" => (
            Kind::Slither,
            Rules::Loop {
                side: usize::from(number(field(v, "side"))),
                clues: field(v, "clues")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_i64().map(|n| u8::try_from(n).unwrap()))
                    .collect(),
            },
        ),
        "hashi" => (
            Kind::Hashi,
            Rules::Bridges {
                width: number(field(v, "width")),
                height: number(field(v, "height")),
                islands: field(v, "islands")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| {
                        let n = numbers(i);
                        (n[0], n[1], n[2])
                    })
                    .collect(),
                routes: field(v, "routes")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| {
                        let n = numbers(e);
                        (usize::from(n[0]), usize::from(n[1]))
                    })
                    .collect(),
            },
        ),
        "kakuro" => cross_sum(v),
        "mines" => (
            Kind::Mines,
            Rules::Mines {
                side: usize::from(number(field(v, "side"))),
                mines: numbers(field(v, "mines"))
                    .into_iter()
                    .fold(0, |mask, cell| mask | (1 << cell)),
                open: v.get("open").map(|open| usize::from(number(open))),
            },
        ),
        _ => panic!("collection kind"),
    };
    Puzzle {
        id: string(v, "id"),
        title: string(v, "title"),
        difficulty: string(v, "difficulty"),
        guide: string(v, "guide"),
        kind,
        rules,
        #[cfg(test)]
        solution: v.get("solution").map(numbers).unwrap_or_default(),
    }
}
fn cross_sum(v: &Value) -> (Kind, Rules) {
    let mask: Vec<String> = field(v, "mask")
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().into())
        .collect();
    // The original five spell every run out. Later puzzles carry
    // only the sums, in the order the runs are read from the mask,
    // which keeps four hundred puzzles small enough to parse quickly
    // on the reader.
    let runs = match v.get("runs") {
        Some(runs) => runs
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let clue = numbers(field(r, "clue"));
                Run {
                    cells: numbers(field(r, "cells"))
                        .into_iter()
                        .map(usize::from)
                        .collect(),
                    clue: (clue[0], clue[1]),
                    down: field(r, "down").as_bool().unwrap(),
                    sum: number(field(r, "sum")),
                }
            })
            .collect(),
        None => cross_runs(&mask)
            .into_iter()
            .zip(numbers(field(v, "sums")))
            .map(|((cells, clue, down), sum)| Run {
                cells,
                clue,
                down,
                sum,
            })
            .collect(),
    };
    (
        Kind::Kakuro,
        Rules::CrossSum {
            mask,
            givens: numbers(field(v, "givens")),
            runs,
        },
    )
}
pub fn loop_edges(side: usize) -> Vec<(usize, usize)> {
    let width = side + 1;
    (0..width)
        .flat_map(|r| (0..side).map(move |c| (r * width + c, r * width + c + 1)))
        .chain(
            (0..side).flat_map(|r| (0..width).map(move |c| (r * width + c, (r + 1) * width + c))),
        )
        .collect()
}
/// The four edges around one square, in the app's edge numbering.
pub fn square_edges(side: usize, square: usize) -> [usize; 4] {
    let (r, c) = (square / side, square % side);
    let left = side * (side + 1) + r * (side + 1) + c;
    [r * side + c, (r + 1) * side + c, left, left + 1]
}
/// The runs of a cross-sum mask: white squares in reading order, each one
/// starting an across run and then a down run where it can. This is the order
/// the collection generator writes sums in.
fn cross_runs(mask: &[String]) -> Vec<(Vec<usize>, (u8, u8), bool)> {
    let white: Vec<(usize, usize)> = mask
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.bytes()
                .enumerate()
                .filter(|(_, square)| *square == b'.')
                .map(move |(c, _)| (r, c))
        })
        .collect();
    let at = |r: usize, c: usize| white.iter().position(|p| *p == (r, c));
    let mut runs = Vec::new();
    for &(r, c) in &white {
        for (dr, dc) in [(0, 1), (1, 0)] {
            if r.checked_sub(dr)
                .zip(c.checked_sub(dc))
                .is_some_and(|(r, c)| at(r, c).is_some())
            {
                continue;
            }
            let mut cells = Vec::new();
            let (mut rr, mut cc) = (r, c);
            while let Some(index) = at(rr, cc) {
                cells.push(index);
                rr += dr;
                cc += dc;
            }
            let clue = (
                u8::try_from(c - dc).expect("small mask"),
                u8::try_from(r - dr).expect("small mask"),
            );
            runs.push((cells, clue, dr == 1));
        }
    }
    runs
}
fn connected(edges: &[(usize, usize)], weights: &[u8], vertices: usize, all: bool) -> bool {
    let Some(start) = edges
        .iter()
        .zip(weights)
        .find(|(_, w)| **w != 0)
        .map(|(e, _)| e.0)
    else {
        return false;
    };
    let mut seen = vec![false; vertices];
    seen[start] = true;
    loop {
        let mut changed = false;
        for (&(a, b), &w) in edges.iter().zip(weights) {
            if w != 0 && (seen[a] || seen[b]) && !(seen[a] && seen[b]) {
                seen[a] = true;
                seen[b] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if all {
        seen.iter().all(|s| *s)
    } else {
        edges
            .iter()
            .zip(weights)
            .all(|(&(a, b), &w)| w == 0 || seen[a] && seen[b])
    }
}
impl Puzzle {
    // At most 64 cells; avoid another dependency for this bounded count.
    #[allow(clippy::naive_bytecount)]
    pub fn cell_count(&self) -> usize {
        match &self.rules {
            Rules::Loop { side, .. } => 2 * side * (side + 1),
            Rules::Bridges { routes, .. } => routes.len(),
            Rules::CrossSum { givens, .. } => givens.iter().filter(|n| **n == 0).count(),
            Rules::Mines { side, .. } => side * side,
        }
    }
    pub fn mines(&self) -> u64 {
        if let Rules::Mines { mines, .. } = self.rules {
            mines
        } else {
            0
        }
    }
    pub fn side(&self) -> usize {
        if let Rules::Mines { side, .. } = self.rules {
            side
        } else {
            0
        }
    }
    pub fn opening(&self) -> Option<usize> {
        if let Rules::Mines { open, .. } = self.rules {
            open
        } else {
            None
        }
    }
    /// Where this puzzle sits in [`LEVELS`].
    pub fn level(&self) -> usize {
        LEVELS
            .iter()
            .position(|level| *level == self.difficulty)
            .unwrap_or(0)
    }
    /// How big the puzzle is, in the words a player sizes it up by.
    pub fn size(&self) -> String {
        match &self.rules {
            Rules::Loop { side, .. } => format!("{side} × {side}"),
            Rules::Bridges { islands, .. } => format!("{} islands", islands.len()),
            Rules::CrossSum { .. } => format!("{} blanks", self.cell_count()),
            Rules::Mines { side, mines, .. } => {
                format!("{side} × {side}, {} mines", mines.count_ones())
            }
        }
    }
    pub fn solved(&self, cells: &[u8; 64], mines: u64) -> bool {
        match &self.rules {
            Rules::Loop { side, clues } => {
                let edges = loop_edges(*side);
                let weights = cells[..edges.len()]
                    .iter()
                    .map(|c| u8::from(*c == 1))
                    .collect::<Vec<_>>();
                let mut degree = vec![0; (side + 1) * (side + 1)];
                for (&(a, b), &n) in edges.iter().zip(&weights) {
                    degree[a] += n;
                    degree[b] += n;
                }
                degree.iter().all(|d| *d == 0 || *d == 2)
                    && clues.iter().enumerate().all(|(i, clue)| {
                        clue.is_none_or(|n| {
                            square_edges(*side, i)
                                .iter()
                                .map(|e| weights[*e])
                                .sum::<u8>()
                                == n
                        })
                    })
                    && connected(&edges, &weights, degree.len(), false)
            }
            Rules::Bridges {
                islands, routes, ..
            } => {
                let weights = &cells[..routes.len()];
                islands.iter().enumerate().all(|(v, (_, _, n))| {
                    routes
                        .iter()
                        .zip(weights)
                        .filter(|((a, b), _)| *a == v || *b == v)
                        .map(|(_, n)| u16::from(*n))
                        .sum::<u16>()
                        == u16::from(*n)
                }) && connected(routes, weights, islands.len(), true)
                    && routes.iter().enumerate().all(|(i, &(a, b))| {
                        weights[i] == 0
                            || routes.iter().enumerate().all(|(j, &(c, d))| {
                                if j >= i || weights[j] == 0 {
                                    return true;
                                }
                                let (ax, ay, _) = islands[a];
                                let (bx, by, _) = islands[b];
                                let (cx, cy, _) = islands[c];
                                let (dx, dy, _) = islands[d];
                                !(ax == bx
                                    && cy == dy
                                    && cx.min(dx) < ax
                                    && ax < cx.max(dx)
                                    && ay.min(by) < cy
                                    && cy < ay.max(by)
                                    || ay == by
                                        && cx == dx
                                        && ax.min(bx) < cx
                                        && cx < ax.max(bx)
                                        && cy.min(dy) < ay
                                        && ay < cy.max(dy))
                            })
                    })
            }
            Rules::CrossSum { runs, givens, .. } => {
                let mut free = cells.iter();
                let digits = givens
                    .iter()
                    .map(|n| if *n == 0 { *free.next().unwrap() } else { *n })
                    .collect::<Vec<_>>();
                runs.iter().all(|run| {
                    let mut seen = 0_u16;
                    let mut sum = 0_u16;
                    for &i in &run.cells {
                        let n = digits[i];
                        if !(1..=9).contains(&n) || seen & (1 << n) != 0 {
                            return false;
                        }
                        seen |= 1 << n;
                        sum += u16::from(n);
                    }
                    sum == u16::from(run.sum)
                })
            }
            Rules::Mines { side, .. } => {
                (0..side * side).all(|i| mines & (1 << i) != 0 || cells[i] == 1)
            }
        }
    }
    #[cfg(test)]
    pub fn answer_cells(&self) -> [u8; 64] {
        let mut cells = [0; 64];
        let values = if let Rules::CrossSum { givens, .. } = &self.rules {
            self.solution
                .iter()
                .zip(givens)
                .filter(|(_, g)| **g == 0)
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
        } else {
            self.solution.clone()
        };
        cells[..values.len()].copy_from_slice(&values);
        cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A puzzle as a search problem: what each cell may hold, sums that
    /// groups of cells must reach, groups that may not repeat a digit, and
    /// pairs of bridges that would cross.
    struct Model {
        domains: Vec<u16>,
        sums: Vec<(Vec<usize>, u64)>,
        distinct: Vec<Vec<usize>>,
        exclusive: Vec<(usize, usize)>,
    }

    fn crosses(islands: &[(u8, u8, u8)], first: (usize, usize), second: (usize, usize)) -> bool {
        let (ax, ay, _) = islands[first.0];
        let (bx, by, _) = islands[first.1];
        let (cx, cy, _) = islands[second.0];
        let (dx, dy, _) = islands[second.1];
        ax == bx
            && cy == dy
            && cx.min(dx) < ax
            && ax < cx.max(dx)
            && ay.min(by) < cy
            && cy < ay.max(by)
    }

    fn model(p: &Puzzle) -> Model {
        match &p.rules {
            Rules::Loop { side, clues } => {
                let edges = loop_edges(*side);
                let mut sums: Vec<_> = (0..(side + 1) * (side + 1))
                    .map(|v| {
                        let at = edges.iter().enumerate();
                        let cells = at.filter(|(_, e)| e.0 == v || e.1 == v).map(|(i, _)| i);
                        (cells.collect(), 0b101)
                    })
                    .collect();
                for (square, clue) in clues.iter().enumerate() {
                    if let Some(n) = clue {
                        sums.push((square_edges(*side, square).to_vec(), 1 << n));
                    }
                }
                Model {
                    domains: vec![0b11; edges.len()],
                    sums,
                    distinct: vec![],
                    exclusive: vec![],
                }
            }
            Rules::Bridges {
                islands, routes, ..
            } => {
                let sums = islands
                    .iter()
                    .enumerate()
                    .map(|(v, (_, _, n))| {
                        let at = routes.iter().enumerate();
                        let cells = at.filter(|(_, r)| r.0 == v || r.1 == v).map(|(i, _)| i);
                        (cells.collect(), 1 << n)
                    })
                    .collect();
                let mut exclusive = vec![];
                for (i, &a) in routes.iter().enumerate() {
                    for (j, &b) in routes.iter().enumerate() {
                        if crosses(islands, a, b) {
                            exclusive.extend([(i, j), (j, i)]);
                        }
                    }
                }
                Model {
                    domains: vec![0b111; routes.len()],
                    sums,
                    distinct: vec![],
                    exclusive,
                }
            }
            Rules::CrossSum { runs, givens, .. } => {
                let mut blank = vec![None; givens.len()];
                let mut next = 0;
                for (white, given) in givens.iter().enumerate() {
                    if *given == 0 {
                        blank[white] = Some(next);
                        next += 1;
                    }
                }
                let mut domains = vec![0b11_1111_1110_u16; next];
                let mut sums = vec![];
                let mut distinct = vec![];
                for run in runs {
                    let printed: Vec<u8> = run
                        .cells
                        .iter()
                        .map(|c| givens[*c])
                        .filter(|n| *n != 0)
                        .collect();
                    let cells: Vec<usize> = run.cells.iter().filter_map(|c| blank[*c]).collect();
                    for &cell in &cells {
                        for n in &printed {
                            domains[cell] &= !(1 << n);
                        }
                    }
                    let left = run.sum - printed.iter().sum::<u8>();
                    sums.push((cells.clone(), 1 << left));
                    distinct.push(cells);
                }
                Model {
                    domains,
                    sums,
                    distinct,
                    exclusive: vec![],
                }
            }
            Rules::Mines { .. } => unreachable!("fields have no single solution"),
        }
    }

    /// Every total a run of cells can reach, as bits.
    fn reach(totals: u64, domain: u16) -> u64 {
        (0..10)
            .filter(|v| (domain >> v) & 1 == 1)
            .fold(0, |out, v| out | (totals << v))
    }

    fn join(a: u64, b: u64) -> u64 {
        (0..64)
            .filter(|s| (a >> s) & 1 == 1)
            .fold(0, |out, s| out | (b << s))
    }

    /// Removes every value no completion of a sum could use, until nothing
    /// changes. Returns false when some cell has nothing left.
    fn settle(m: &Model, d: &mut [u16]) -> bool {
        loop {
            let mut changed = false;
            for (cells, allowed) in &m.sums {
                let mut before = vec![1_u64];
                for cell in cells {
                    before.push(reach(*before.last().unwrap(), d[*cell]));
                }
                let mut after = vec![1_u64; cells.len() + 1];
                for k in (0..cells.len()).rev() {
                    after[k] = reach(after[k + 1], d[cells[k]]);
                }
                for (k, cell) in cells.iter().enumerate() {
                    let others = join(before[k], after[k + 1]);
                    let keep = (0..10)
                        .filter(|v| (d[*cell] >> v) & 1 == 1 && (others << v) & allowed != 0)
                        .fold(0, |keep, v| keep | (1 << v));
                    if keep == 0 {
                        return false;
                    }
                    changed |= keep != d[*cell];
                    d[*cell] = keep;
                }
            }
            for group in &m.distinct {
                for &a in group {
                    if d[a].is_power_of_two() {
                        for &b in group {
                            if b != a && d[b] & d[a] != 0 {
                                d[b] &= !d[a];
                                changed = true;
                                if d[b] == 0 {
                                    return false;
                                }
                            }
                        }
                    }
                }
            }
            for &(a, b) in &m.exclusive {
                if d[a] & 1 == 0 && d[b] != 1 {
                    if d[b] & 1 == 0 {
                        return false;
                    }
                    d[b] = 1;
                    changed = true;
                }
            }
            if !changed {
                return true;
            }
        }
    }

    /// Counts solutions up to `limit`. Each finished board is judged by
    /// [`Puzzle::solved`], the same check the app makes, so one solution here
    /// is one solution as far as a player is concerned.
    fn count(p: &Puzzle, m: &Model, mut d: Vec<u16>, limit: usize) -> usize {
        if !settle(m, &mut d) {
            return 0;
        }
        let open = (0..d.len())
            .filter(|i| d[*i].count_ones() > 1)
            .min_by_key(|i| d[*i].count_ones());
        let Some(cell) = open else {
            let mut cells = [0; 64];
            for (out, domain) in cells.iter_mut().zip(&d) {
                *out = u8::try_from(domain.trailing_zeros()).unwrap();
            }
            return usize::from(p.solved(&cells, 0));
        };
        let mut found = 0;
        for value in (0..10).filter(|v| (d[cell] >> v) & 1 == 1) {
            let mut branch = d.clone();
            branch[cell] = 1 << value;
            found += count(p, m, branch, limit - found);
            if found >= limit {
                break;
            }
        }
        found
    }

    fn around(cell: usize, side: usize) -> impl Iterator<Item = usize> {
        let (r, c) = (cell / side, cell % side);
        (r.saturating_sub(1)..=(r + 1).min(side - 1))
            .flat_map(move |rr| {
                (c.saturating_sub(1)..=(c + 1).min(side - 1)).map(move |cc| rr * side + cc)
            })
            .filter(move |x| *x != cell)
    }

    /// Plays a field from its opening the way a careful player would: open
    /// whatever the numbers and the mine count prove safe, and nothing else.
    fn clears_without_guessing(p: &Puzzle) -> bool {
        let Rules::Mines {
            side,
            mines,
            open: Some(open),
        } = p.rules
        else {
            return false;
        };
        let mine = |c: usize| (mines >> c) & 1 == 1;
        let mut shown = vec![false; side * side];
        let reveal = |shown: &mut Vec<bool>, start: usize| {
            let mut todo = vec![start];
            while let Some(c) = todo.pop() {
                if !shown[c] {
                    shown[c] = true;
                    if !around(c, side).any(mine) {
                        todo.extend(around(c, side));
                    }
                }
            }
        };
        reveal(&mut shown, open);
        loop {
            let hidden: Vec<usize> = (0..side * side).filter(|c| !shown[*c]).collect();
            if hidden.len() == mines.count_ones() as usize {
                return true;
            }
            let index = |c: usize| hidden.iter().position(|h| *h == c);
            let mut sums: Vec<_> = (0..side * side)
                .filter(|c| shown[*c])
                .map(|c| {
                    let near: Vec<usize> = around(c, side).filter_map(index).collect();
                    (near, 1_u64 << around(c, side).filter(|x| mine(*x)).count())
                })
                .filter(|(near, _)| !near.is_empty())
                .collect();
            let mut edge = vec![false; hidden.len()];
            for (near, _) in &sums {
                for k in near {
                    edge[*k] = true;
                }
            }
            sums.push(((0..hidden.len()).collect(), 1 << mines.count_ones()));
            let m = Model {
                domains: vec![0b11; hidden.len()],
                sums,
                distinct: vec![],
                exclusive: vec![],
            };
            let mut d = m.domains.clone();
            assert!(settle(&m, &mut d), "{} contradicts itself", p.id);
            let mut safe: Vec<usize> = (0..hidden.len()).filter(|k| d[*k] == 1).collect();
            if safe.is_empty() {
                safe.extend((0..hidden.len()).filter(|k| d[*k] == 0b11).find(|k| {
                    let mut suppose = d.clone();
                    suppose[*k] = 0b10;
                    !possible(&m, suppose, &edge)
                }));
            }
            if safe.is_empty() {
                return false;
            }
            for k in safe {
                assert!(!mine(hidden[k]), "{} opened a mine", p.id);
                reveal(&mut shown, hidden[k]);
            }
        }
    }

    /// Whether any arrangement fits. Only squares next to a number are tried
    /// both ways: the rest touch nothing but the mine count, which the sum
    /// already settles, and trying them one by one would take forever.
    fn possible(m: &Model, mut d: Vec<u16>, edge: &[bool]) -> bool {
        if !settle(m, &mut d) {
            return false;
        }
        let Some(cell) = (0..d.len()).find(|i| edge[*i] && d[*i].count_ones() > 1) else {
            return true;
        };
        [0b01, 0b10].into_iter().any(|value| {
            let mut branch = d.clone();
            branch[cell] = value;
            possible(m, branch, edge)
        })
    }

    #[test]
    fn every_puzzle_fits_the_app_and_has_exactly_one_solution_or_a_guess_free_field() {
        let puzzles = puzzles();
        assert!(puzzles.len() >= 400, "{}", puzzles.len());
        let originals = [
            "slither-four-twos",
            "hashi-cross",
            "kakuro-three-cells",
            "mines-four-square",
            "slither-01",
            "slither-02",
            "slither-03",
            "slither-04",
            "hashi-01",
            "hashi-02",
            "hashi-03",
            "hashi-04",
            "kakuro-01",
            "kakuro-02",
            "kakuro-03",
            "kakuro-04",
            "mines-01",
            "mines-02",
            "mines-03",
            "mines-04",
        ];
        for (p, id) in puzzles.iter().zip(originals) {
            assert_eq!(p.id, id, "saved games name the originals by place");
        }
        let mut ids = std::collections::BTreeSet::new();
        for (index, p) in puzzles.iter().enumerate() {
            assert!(ids.insert(&p.id), "{} twice", p.id);
            assert!(p.cell_count() <= 64, "{}", p.id);
            assert!(LEVELS.contains(&p.difficulty.as_str()), "{}", p.id);
            assert!(!p.title.is_empty() && !p.guide.is_empty(), "{}", p.id);
            if p.kind == Kind::Mines {
                assert!(p.mines().count_ones() >= 3, "{}", p.id);
                assert_eq!(
                    p.mines()
                        .checked_shr(u32::try_from(p.side() * p.side()).unwrap())
                        .unwrap_or(0),
                    0,
                    "{}",
                    p.id
                );
                if index >= originals.len() {
                    assert!(clears_without_guessing(p), "{} needs a guess", p.id);
                }
                continue;
            }
            assert!(p.solved(&p.answer_cells(), 0), "{} answer", p.id);
            let m = model(p);
            assert_eq!(count(p, &m, m.domains.clone(), 2), 1, "{} solutions", p.id);
            if let Rules::CrossSum { runs, givens, .. } = &p.rules {
                for white in 0..givens.len() {
                    for down in [false, true] {
                        let run = runs
                            .iter()
                            .find(|r| r.down == down && r.cells.contains(&white));
                        assert!(
                            run.is_some_and(|r| r.cells.len() >= 2),
                            "{} square {white}",
                            p.id
                        );
                    }
                }
            }
        }
    }
}
