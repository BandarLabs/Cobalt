//! Board geometry for the original compact puzzles.
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
pub fn slither(cells: &[u8; 16]) -> PencilBoard {
    let mut marks = Vec::new();
    for row in 0..3 {
        for column in 0..3 {
            marks.push(mark(column * 2, row * 2, Ink::Dot));
        }
    }
    for row in 0..2 {
        for column in 0..2 {
            marks.push(mark(column * 2 + 1, row * 2 + 1, Ink::Clue(2)));
        }
    }
    let mut edges = Vec::new();
    for row in 0..3 {
        for column in 0..2 {
            let index = usize::from(row * 2 + column);
            edges.push(PencilEdge {
                from: (column * 2, row * 2),
                to: (column * 2 + 2, row * 2),
                state: if cells[index] == 2 { 3 } else { cells[index] },
                action: Some(action_id(&format!("edge-{index}"))),
            });
        }
    }
    for row in 0..2 {
        for column in 0..3 {
            let index = usize::from(6 + row * 3 + column);
            edges.push(PencilEdge {
                from: (column * 2, row * 2),
                to: (column * 2, row * 2 + 2),
                state: if cells[index] == 2 { 3 } else { cells[index] },
                action: Some(action_id(&format!("edge-{index}"))),
            });
        }
    }
    PencilBoard {
        columns: 5,
        rows: 5,
        cell_tenth_mm: 100,
        marks,
        edges,
    }
}
pub fn hashi(cells: &[u8; 16]) -> PencilBoard {
    let ends = [(2, 0), (0, 2), (4, 2), (2, 4)];
    let mut marks = ends
        .iter()
        .map(|&(x, y)| mark(x, y, Ink::Island(1)))
        .collect::<Vec<_>>();
    marks.push(mark(2, 2, Ink::Island(4)));
    let edges = ends
        .into_iter()
        .enumerate()
        .map(|(index, from)| PencilEdge {
            from,
            to: (2, 2),
            state: cells[index],
            action: Some(action_id(&format!("route-{index}"))),
        })
        .collect();
    PencilBoard {
        columns: 5,
        rows: 5,
        cell_tenth_mm: 100,
        marks,
        edges,
    }
}
pub fn kakuro(cells: &[u8; 16]) -> PencilBoard {
    let mut marks = vec![
        mark(0, 0, Ink::Block),
        mark(1, 0, Ink::Sum { across: 0, down: 3 }),
        mark(2, 0, Ink::Sum { across: 0, down: 7 }),
        mark(0, 1, Ink::Sum { across: 4, down: 0 }),
        mark(0, 2, Ink::Sum { across: 6, down: 0 }),
        mark(
            1,
            1,
            Ink::Digit {
                value: 1,
                given: true,
            },
        ),
    ];
    for (index, (column, row)) in [(2, 1), (1, 2), (2, 2)].into_iter().enumerate() {
        let mut cell = mark(
            column,
            row,
            Ink::Digit {
                value: cells[index],
                given: false,
            },
        );
        cell.action = Some(action_id(&format!("kakuro-{index}")));
        marks.push(cell);
    }
    PencilBoard {
        columns: 3,
        rows: 3,
        cell_tenth_mm: 160,
        marks,
        edges: vec![],
    }
}
