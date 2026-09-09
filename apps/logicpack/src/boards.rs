//! Board geometry derived from the same collection rules used for checking.
use crate::collection::{loop_edges, Puzzle, Rules};
use kobo_sdk::{action_id, PencilBoard, PencilEdge, PencilMark, PencilMarkKind as Ink};
fn mark(column: u8, row: u8, kind: Ink) -> PencilMark {
    PencilMark {
        column,
        row,
        kind,
        action: None,
        selected: false,
    }
}
fn byte(n: usize) -> u8 {
    u8::try_from(n).expect("bounded board")
}
pub fn pencil(puzzle: &Puzzle, cells: &[u8; 64]) -> PencilBoard {
    match &puzzle.rules {
        Rules::Loop { side, clues } => {
            let mut marks = Vec::new();
            for row in 0..=(*side) {
                for column in 0..=(*side) {
                    marks.push(mark(byte(column * 2), byte(row * 2), Ink::Dot));
                }
            }
            for (i, clue) in clues.iter().enumerate() {
                if let Some(n) = clue {
                    marks.push(mark(
                        byte(i % side * 2 + 1),
                        byte(i / side * 2 + 1),
                        Ink::Clue(*n),
                    ));
                }
            }
            let edges = loop_edges(*side)
                .iter()
                .enumerate()
                .map(|(i, &(a, b))| PencilEdge {
                    from: (byte(a % (side + 1) * 2), byte(a / (side + 1) * 2)),
                    to: (byte(b % (side + 1) * 2), byte(b / (side + 1) * 2)),
                    state: if cells[i] == 2 { 3 } else { cells[i] },
                    action: Some(action_id(&format!("edge-{i}"))),
                })
                .collect();
            PencilBoard {
                columns: byte(side * 2 + 1),
                rows: byte(side * 2 + 1),
                cell_tenth_mm: if *side == 2 { 100 } else { 80 },
                marks,
                edges,
            }
        }
        Rules::Bridges {
            width,
            height,
            islands,
            routes,
        } => {
            let marks = islands
                .iter()
                .map(|&(x, y, n)| mark(x, y, Ink::Island(n)))
                .collect();
            let edges = routes
                .iter()
                .enumerate()
                .map(|(i, &(a, b))| PencilEdge {
                    from: (islands[a].0, islands[a].1),
                    to: (islands[b].0, islands[b].1),
                    state: cells[i],
                    action: Some(action_id(&format!("route-{i}"))),
                })
                .collect();
            PencilBoard {
                columns: *width,
                rows: *height,
                cell_tenth_mm: if *width <= 5 && *height <= 5 { 100 } else { 80 },
                marks,
                edges,
            }
        }
        Rules::CrossSum { mask, runs, givens } => cross_sum(mask, runs, givens, cells),
        Rules::Mines { .. } => unreachable!("mine board uses square controls"),
    }
}

fn cross_sum(
    mask: &[String],
    runs: &[crate::collection::Run],
    givens: &[u8],
    cells: &[u8; 64],
) -> PencilBoard {
    let mut marks = Vec::new();
    let mut white = 0;
    let mut free = 0;
    for (row, line) in mask.iter().enumerate() {
        for (column, square) in line.bytes().enumerate() {
            let x = byte(column);
            let y = byte(row);
            if square == b'.' {
                let given = givens[white] != 0;
                let value = if given { givens[white] } else { cells[free] };
                let mut m = mark(x, y, Ink::Digit { value, given });
                if !given {
                    m.action = Some(action_id(&format!("kakuro-{free}")));
                    free += 1;
                }
                white += 1;
                marks.push(m);
            } else {
                let across = runs
                    .iter()
                    .find(|r| r.clue == (x, y) && !r.down)
                    .map_or(0, |r| r.sum);
                let down = runs
                    .iter()
                    .find(|r| r.clue == (x, y) && r.down)
                    .map_or(0, |r| r.sum);
                marks.push(mark(
                    x,
                    y,
                    if across == 0 && down == 0 {
                        Ink::Block
                    } else {
                        Ink::Sum { across, down }
                    },
                ));
            }
        }
    }
    PencilBoard {
        columns: byte(mask[0].len()),
        rows: byte(mask.len()),
        cell_tenth_mm: if mask.len() <= 3 { 160 } else { 100 },
        marks,
        edges: vec![],
    }
}
