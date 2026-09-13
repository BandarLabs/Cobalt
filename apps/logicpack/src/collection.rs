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
    },
}
pub struct Puzzle {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub guide: String,
    pub kind: Kind,
    pub rules: Rules,
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
        for puzzle in &puzzles {
            assert!(puzzle.cell_count() <= 64, "bounded collection");
            if puzzle.kind != Kind::Mines {
                assert!(
                    puzzle.solved(&puzzle.answer_cells(), 0),
                    "valid bundled answer"
                );
            }
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
        "kakuro" => (
            Kind::Kakuro,
            Rules::CrossSum {
                mask: field(v, "mask")
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r.as_str().unwrap().into())
                    .collect(),
                givens: numbers(field(v, "givens")),
                runs: field(v, "runs")
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
            },
        ),
        "mines" => (
            Kind::Mines,
            Rules::Mines {
                side: usize::from(number(field(v, "side"))),
                mines: numbers(field(v, "mines"))
                    .into_iter()
                    .fold(0, |mask, cell| mask | (1 << cell)),
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
        solution: v.get("solution").map(numbers).unwrap_or_default(),
    }
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
                        let r = i / side;
                        let c = i % side;
                        clue.is_none_or(|n| {
                            [
                                r * side + c,
                                (r + 1) * side + c,
                                side * (side + 1) + r * (side + 1) + c,
                                side * (side + 1) + r * (side + 1) + c + 1,
                            ]
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
    #[test]
    fn all_twenty_original_puzzles_have_valid_rules_and_answers() {
        let puzzles = puzzles();
        assert_eq!(puzzles.len(), 20);
        let mut ids = std::collections::BTreeSet::new();
        for p in puzzles {
            assert!(ids.insert(&p.id));
            assert!(p.cell_count() <= 64);
            if p.kind == Kind::Mines {
                assert!(p.mines().count_ones() >= 3);
                assert_eq!(p.mines() >> (p.side() * p.side()), 0);
            } else {
                let cells = p.answer_cells();
                assert!(p.solved(&cells, 0), "{}", p.id);
                for i in 0..p.cell_count() {
                    let mut changed = cells;
                    changed[i] = (changed[i] + 1) % 3;
                    assert!(!p.solved(&changed, 0), "{} cell {i}", p.id);
                }
            }
        }
        assert_eq!(puzzles[0].id, "slither-four-twos");
        assert_eq!(puzzles[1].id, "hashi-cross");
        assert_eq!(puzzles[2].id, "kakuro-three-cells");
        assert_eq!(puzzles[3].id, "mines-four-square");
    }
}
