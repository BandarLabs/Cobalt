use super::{push_u16, push_u32, ActionId, Node, NodeId, ProtocolError, Reader};
use kobo_ui::{BoardCell, BoardClue, BoardMark, BoardSurface};

pub(super) fn encoded_len(board: &BoardSurface, version: u8) -> Result<usize, ProtocolError> {
    if version < 14 || !board.is_valid() {
        return Err(ProtocolError::InvalidValue("board viewport"));
    }
    let cells: usize = board
        .cells
        .iter()
        .map(|cell| {
            6 + match cell.mark {
                BoardMark::Value(_) => 2,
                BoardMark::Notes(_) => 4,
                _ => 0,
            }
        })
        .sum();
    let clues: usize = board
        .row_clues
        .iter()
        .chain(&board.column_clues)
        .map(|clue| 5 + clue.values.len())
        .sum();
    Ok(13 + cells + clues)
}

pub(super) fn push(
    output: &mut Vec<u8>,
    id: NodeId,
    board: &BoardSurface,
    version: u8,
) -> Result<(), ProtocolError> {
    encoded_len(board, version)?;
    output.push(32);
    push_u32(output, id.0);
    output.extend_from_slice(&[board.columns, board.row_start, board.column_start]);
    push_u16(output, board.cell_tenth_mm);
    output.push(u8::try_from(board.cells.len()).map_err(|_| ProtocolError::TooManyNodes)?);
    for cell in &board.cells {
        push_u32(output, cell.action.0);
        match cell.mark {
            BoardMark::Empty => output.push(0),
            BoardMark::Filled => output.push(1),
            BoardMark::Crossed => output.push(2),
            BoardMark::Dot => output.push(3),
            BoardMark::Value(value) => {
                output.push(4);
                push_u16(output, value);
            }
            BoardMark::Notes(bits) => {
                output.push(5);
                push_u32(output, bits);
            }
        }
        output.push(u8::from(cell.given) | (u8::from(cell.selected) << 1));
    }
    for clues in [&board.row_clues, &board.column_clues] {
        output.push(u8::try_from(clues.len()).map_err(|_| ProtocolError::TooManyNodes)?);
        for clue in clues {
            push_u32(output, clue.action.0);
            output.push(u8::try_from(clue.values.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            output.extend_from_slice(&clue.values);
        }
    }
    Ok(())
}

pub(super) fn read(reader: &mut Reader<'_>, id: NodeId) -> Result<Node, ProtocolError> {
    let columns = reader.u8()?;
    let row_start = reader.u8()?;
    let column_start = reader.u8()?;
    let cell_tenth_mm = reader.u16()?;
    let count = usize::from(reader.u8()?);
    if count > 81 {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count {
        let action = ActionId(reader.u32()?);
        let mark = match reader.u8()? {
            0 => BoardMark::Empty,
            1 => BoardMark::Filled,
            2 => BoardMark::Crossed,
            3 => BoardMark::Dot,
            4 => BoardMark::Value(reader.u16()?),
            5 => BoardMark::Notes(reader.u32()?),
            _ => return Err(ProtocolError::InvalidValue("board mark")),
        };
        let flags = reader.u8()?;
        if flags > 3 {
            return Err(ProtocolError::InvalidValue("board cell flags"));
        }
        cells.push(BoardCell {
            action,
            mark,
            given: flags & 1 != 0,
            selected: flags & 2 != 0,
        });
    }
    let row_clues = read_clues(reader)?;
    let column_clues = read_clues(reader)?;
    let surface = BoardSurface {
        columns,
        row_start,
        column_start,
        cell_tenth_mm,
        cells,
        row_clues,
        column_clues,
    };
    if !surface.is_valid() {
        return Err(ProtocolError::InvalidValue("board viewport"));
    }
    Ok(Node::Board { id, surface })
}

fn read_clues(reader: &mut Reader<'_>) -> Result<Vec<BoardClue>, ProtocolError> {
    let count = usize::from(reader.u8()?);
    if count > 12 {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut clues = Vec::with_capacity(count);
    for _ in 0..count {
        let action = ActionId(reader.u32()?);
        let count = usize::from(reader.u8()?);
        if count > 32 {
            return Err(ProtocolError::TooManyNodes);
        }
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(reader.u8()?);
        }
        clues.push(BoardClue { action, values });
    }
    Ok(clues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        decode, encode, Cell, Frame, Message, Screen, FOLIO_VERSION, HEADER_LEN, LEGACY_VERSION,
        SELECTED_GRID_VERSION, VERSION,
    };
    fn sample() -> BoardSurface {
        BoardSurface {
            columns: 3,
            row_start: 8,
            column_start: 9,
            cell_tenth_mm: 120,
            cells: [
                BoardMark::Empty,
                BoardMark::Filled,
                BoardMark::Crossed,
                BoardMark::Dot,
                BoardMark::Value(19),
                BoardMark::Notes(0x8000_0001),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, mark)| BoardCell {
                action: ActionId(u32::try_from(index).unwrap() + 10),
                mark,
                selected: index == 1,
                given: index == 4,
            })
            .collect(),
            row_clues: vec![
                BoardClue {
                    action: ActionId(101),
                    values: vec![1, 2, 3, 4],
                },
                BoardClue {
                    action: ActionId(102),
                    values: vec![],
                },
            ],
            column_clues: (0..3)
                .map(|index| BoardClue {
                    action: ActionId(index + 200),
                    values: vec![2],
                })
                .collect(),
        }
    }
    #[test]
    fn board_round_trip_is_bounded_and_versioned() {
        let board = sample();
        let id = NodeId(7);
        let mut bytes = Vec::new();
        push(&mut bytes, id, &board, VERSION).unwrap();
        assert_eq!(bytes.len(), encoded_len(&board, VERSION).unwrap());
        assert_eq!(
            read(&mut Reader::new(&bytes[5..]), id).unwrap(),
            Node::Board {
                id,
                surface: board.clone()
            }
        );
        for end in 5..bytes.len() {
            assert!(read(&mut Reader::new(&bytes[5..end]), id).is_err());
        }
        for version in [LEGACY_VERSION, FOLIO_VERSION, SELECTED_GRID_VERSION] {
            assert!(encoded_len(&board, version).is_err());
        }
        let frame = Frame {
            version: VERSION,
            request_id: 12,
            message: Message::SetScreen(Screen::new(1, vec![Node::Board { id, surface: board }])),
        };
        assert_eq!(decode(&encode(&frame).unwrap()).unwrap(), frame);
    }
    #[test]
    fn installed_grid_payloads_keep_their_version_13_bytes() {
        let mut frame = Frame {
            version: SELECTED_GRID_VERSION,
            request_id: 10,
            message: Message::SetScreen(Screen::new(
                1,
                vec![Node::Grid {
                    id: NodeId(7),
                    columns: 1,
                    square: true,
                    cells: vec![Cell::new(ActionId(9), "7").with_selected(true)],
                }],
            )),
        };
        let old = encode(&frame).unwrap();
        assert_eq!(decode(&old).unwrap(), frame);
        frame.version = VERSION;
        let current = encode(&frame).unwrap();
        assert_eq!(old[HEADER_LEN..], current[HEADER_LEN..]);
        frame.version = FOLIO_VERSION;
        let decoded = decode(&encode(&frame).unwrap()).unwrap();
        let Message::SetScreen(screen) = decoded.message else {
            panic!("screen");
        };
        let Node::Grid { cells, .. } = &screen.nodes[0] else {
            panic!("grid");
        };
        assert!(!cells[0].selected);
    }

    #[test]
    fn malformed_board_counts_flags_marks_and_actions_are_refused() {
        let mut board = sample();
        let mut bytes = Vec::new();
        push(&mut bytes, NodeId(7), &board, VERSION).unwrap();
        for (offset, value) in [(5, 0), (6, 64), (10, 255), (15, 255), (16, 4)] {
            let mut bad = bytes.clone();
            bad[offset] = value;
            assert!(
                read(&mut Reader::new(&bad[5..]), NodeId(7)).is_err(),
                "offset {offset}"
            );
        }
        board.cells[0].action = board.cells[1].action;
        assert!(encoded_len(&board, VERSION).is_err());
        board = sample();
        board.row_clues[0].values = vec![1; 33];
        assert!(encoded_len(&board, VERSION).is_err());
        board = sample();
        board.cells[0].mark = BoardMark::Notes(0);
        assert!(encoded_len(&board, VERSION).is_err());
    }
}
