use super::{push_u16, push_u32, ActionId, Node, NodeId, ProtocolError, Reader};
use kobo_ui::{PencilBoard, PencilEdge, PencilMark, PencilMarkKind};

pub(super) fn encoded_len(board: &PencilBoard, version: u8) -> Result<usize, ProtocolError> {
    if version < 14 || !board.is_valid() {
        return Err(ProtocolError::InvalidValue("pencil board"));
    }
    Ok(11 + board.marks.len() * 10 + board.edges.len() * 9)
}
pub(super) fn push(
    out: &mut Vec<u8>,
    id: NodeId,
    board: &PencilBoard,
    version: u8,
) -> Result<(), ProtocolError> {
    encoded_len(board, version)?;
    out.push(34);
    push_u32(out, id.0);
    out.extend_from_slice(&[board.columns, board.rows]);
    push_u16(out, board.cell_tenth_mm);
    out.push(u8::try_from(board.marks.len()).map_err(|_| ProtocolError::TooManyNodes)?);
    for mark in &board.marks {
        out.extend_from_slice(&[mark.column, mark.row]);
        out.extend_from_slice(&match mark.kind {
            PencilMarkKind::Dot => [0, 0, 0],
            PencilMarkKind::Clue(n) => [1, n, 0],
            PencilMarkKind::Island(n) => [2, n, 0],
            PencilMarkKind::Block => [3, 0, 0],
            PencilMarkKind::Sum { across, down } => [4, across, down],
            PencilMarkKind::Digit { value, given } => [5, value, u8::from(given)],
        });
        out.push(u8::from(mark.selected));
        push_u32(out, mark.action.map_or(0, |a| a.0));
    }
    out.push(u8::try_from(board.edges.len()).map_err(|_| ProtocolError::TooManyNodes)?);
    for edge in &board.edges {
        out.extend_from_slice(&[edge.from.0, edge.from.1, edge.to.0, edge.to.1, edge.state]);
        push_u32(out, edge.action.map_or(0, |a| a.0));
    }
    Ok(())
}
fn action(reader: &mut Reader<'_>) -> Result<Option<ActionId>, ProtocolError> {
    Ok(match reader.u32()? {
        0 => None,
        n => Some(ActionId(n)),
    })
}
pub(super) fn read(reader: &mut Reader<'_>, id: NodeId) -> Result<Node, ProtocolError> {
    let columns = reader.u8()?;
    let rows = reader.u8()?;
    let cell_tenth_mm = reader.u16()?;
    let count = usize::from(reader.u8()?);
    if count > 81 {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut marks = Vec::with_capacity(count);
    for _ in 0..count {
        let column = reader.u8()?;
        let row = reader.u8()?;
        let kind = match (reader.u8()?, reader.u8()?, reader.u8()?) {
            (0, 0, 0) => PencilMarkKind::Dot,
            (1, n, 0) => PencilMarkKind::Clue(n),
            (2, n, 0) => PencilMarkKind::Island(n),
            (3, 0, 0) => PencilMarkKind::Block,
            (4, across, down) => PencilMarkKind::Sum { across, down },
            (5, value, given @ 0..=1) => PencilMarkKind::Digit {
                value,
                given: given == 1,
            },
            _ => return Err(ProtocolError::InvalidValue("pencil mark")),
        };
        let selected = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(ProtocolError::InvalidValue("pencil selection")),
        };
        marks.push(PencilMark {
            column,
            row,
            kind,
            action: action(reader)?,
            selected,
        });
    }
    let count = usize::from(reader.u8()?);
    if count > 144 {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut edges = Vec::with_capacity(count);
    for _ in 0..count {
        edges.push(PencilEdge {
            from: (reader.u8()?, reader.u8()?),
            to: (reader.u8()?, reader.u8()?),
            state: reader.u8()?,
            action: action(reader)?,
        });
    }
    let board = PencilBoard {
        columns,
        rows,
        cell_tenth_mm,
        marks,
        edges,
    };
    if !board.is_valid() {
        return Err(ProtocolError::InvalidValue("pencil board"));
    }
    Ok(Node::PencilBoard { id, board })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, Frame, Message, Screen, FOLIO_VERSION, VERSION};
    fn sample() -> PencilBoard {
        PencilBoard {
            columns: 3,
            rows: 3,
            cell_tenth_mm: 100,
            marks: vec![
                PencilMark {
                    column: 0,
                    row: 0,
                    kind: PencilMarkKind::Island(2),
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 2,
                    row: 0,
                    kind: PencilMarkKind::Island(2),
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 0,
                    row: 1,
                    kind: PencilMarkKind::Sum {
                        across: 12,
                        down: 9,
                    },
                    action: None,
                    selected: false,
                },
                PencilMark {
                    column: 1,
                    row: 1,
                    kind: PencilMarkKind::Digit {
                        value: 3,
                        given: false,
                    },
                    action: Some(ActionId(9)),
                    selected: true,
                },
            ],
            edges: vec![PencilEdge {
                from: (0, 0),
                to: (2, 0),
                state: 2,
                action: Some(ActionId(8)),
            }],
        }
    }
    #[test]
    fn pencil_payload_round_trips_and_refuses_truncation_and_old_versions() {
        let board = sample();
        let id = NodeId(12);
        let mut bytes = Vec::new();
        push(&mut bytes, id, &board, VERSION).unwrap();
        assert_eq!(bytes.len(), encoded_len(&board, VERSION).unwrap());
        assert_eq!(
            read(&mut Reader::new(&bytes[5..]), id).unwrap(),
            Node::PencilBoard {
                id,
                board: board.clone()
            }
        );
        for end in 5..bytes.len() {
            assert!(read(&mut Reader::new(&bytes[5..end]), id).is_err());
        }
        assert!(encoded_len(&board, FOLIO_VERSION).is_err());
        let frame = Frame {
            version: VERSION,
            request_id: 1,
            message: Message::SetScreen(Screen::new(1, vec![Node::PencilBoard { id, board }])),
        };
        assert_eq!(decode(&encode(&frame).unwrap()).unwrap(), frame);
    }
    #[test]
    fn malformed_pencil_marks_counts_and_edges_are_refused() {
        let mut bytes = Vec::new();
        push(&mut bytes, NodeId(1), &sample(), VERSION).unwrap();
        for (offset, value) in [(9, 82), (12, 99), (15, 2), (55, 9)] {
            let mut bad = bytes.clone();
            bad[offset] = value;
            assert!(
                read(&mut Reader::new(&bad[5..]), NodeId(1)).is_err(),
                "offset {offset}"
            );
        }
        let mut board = sample();
        board.edges[0].action = Some(ActionId(9));
        assert!(encoded_len(&board, VERSION).is_err());
        board = sample();
        board.edges[0].to = (1, 1);
        assert!(encoded_len(&board, VERSION).is_err());
        board = sample();
        board.marks[0].action = Some(ActionId::BACK);
        assert!(encoded_len(&board, VERSION).is_err());
    }
}
