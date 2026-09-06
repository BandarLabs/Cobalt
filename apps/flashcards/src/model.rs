#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CardState {
    New,
    Learning,
    Review,
}

impl CardState {
    pub const fn code(self) -> char {
        match self {
            Self::New => 'n',
            Self::Learning => 'l',
            Self::Review => 'r',
        }
    }
    pub const fn from_code(code: char) -> Option<Self> {
        match code {
            'n' => Some(Self::New),
            'l' => Some(Self::Learning),
            'r' => Some(Self::Review),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: i64,
    pub deck: String,
    pub front: String,
    pub back: String,
    pub last_review_day: i32,
    pub due_day: i32,
    pub state: CardState,
    pub reps: u32,
    pub lapses: u32,
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub media: u16,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub cards: Vec<Card>,
    pub reviews_today: u32,
    pub again_today: u32,
    pub transfer_at: Option<String>,
}

impl Library {
    pub fn decks(&self, today: i32) -> Vec<(String, usize, usize, usize)> {
        let mut decks: Vec<(String, usize, usize, usize)> = Vec::new();
        for card in &self.cards {
            if card.due_day > today {
                continue;
            }
            let index = decks
                .iter()
                .position(|(name, ..)| name == &card.deck)
                .unwrap_or_else(|| {
                    decks.push((card.deck.clone(), 0, 0, 0));
                    decks.len() - 1
                });
            match card.state {
                CardState::New => decks[index].1 += 1,
                CardState::Learning => decks[index].2 += 1,
                CardState::Review => decks[index].3 += 1,
            }
        }
        decks.sort_by(|left, right| left.0.cmp(&right.0));
        decks
    }
    pub fn next_due(&self, deck: &str, today: i32) -> Option<usize> {
        self.cards
            .iter()
            .enumerate()
            .filter(|(_, card)| card.deck == deck && card.due_day <= today)
            .min_by_key(|(_, card)| (card.due_day, card.id))
            .map(|(index, _)| index)
    }
}

fn hex(text: &str) -> String {
    use std::fmt::Write as _;
    text.as_bytes()
        .iter()
        .fold(String::new(), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}
fn unhex(text: &str) -> Option<String> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}
pub fn encode(library: &Library) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut out = format!(
        "v1\t{}\t{}\t{}\n",
        library.reviews_today,
        library.again_today,
        hex(library.transfer_at.as_deref().unwrap_or(""))
    );
    for card in &library.cards {
        let memory = card
            .stability
            .zip(card.difficulty)
            .map(|(stability, difficulty)| format!("{stability},{difficulty}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            card.id,
            hex(&card.deck),
            hex(&card.front),
            hex(&card.back),
            card.last_review_day,
            card.due_day,
            card.state.code(),
            card.reps,
            card.lapses,
            memory,
            card.media
        );
    }
    out.into_bytes()
}
pub fn decode(bytes: &[u8]) -> Option<Library> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    let head: Vec<_> = lines.next()?.split('\t').collect();
    if head.len() != 4 || head[0] != "v1" {
        return None;
    }
    let mut library = Library {
        reviews_today: head[1].parse().ok()?,
        again_today: head[2].parse().ok()?,
        transfer_at: {
            let saved = unhex(head[3])?;
            (!saved.is_empty()).then_some(saved)
        },
        ..Library::default()
    };
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 11 {
            return None;
        }
        let memory = if fields[9].is_empty() {
            None
        } else {
            let (stability, difficulty) = fields[9].split_once(',')?;
            let stability = stability.parse::<f32>().ok()?;
            let difficulty = difficulty.parse::<f32>().ok()?;
            Some(
                (stability.is_finite() && difficulty.is_finite())
                    .then_some((stability, difficulty))?,
            )
        };
        let mut state_code = fields[6].chars();
        let state = CardState::from_code(state_code.next()?)?;
        if state_code.next().is_some() {
            return None;
        }
        library.cards.push(Card {
            id: fields[0].parse().ok()?,
            deck: unhex(fields[1])?,
            front: unhex(fields[2])?,
            back: unhex(fields[3])?,
            last_review_day: fields[4].parse().ok()?,
            due_day: fields[5].parse().ok()?,
            state,
            reps: fields[7].parse().ok()?,
            lapses: fields[8].parse().ok()?,
            stability: memory.map(|(stability, _)| stability),
            difficulty: memory.map(|(_, difficulty)| difficulty),
            media: fields[10].parse().ok()?,
        });
    }
    Some(library)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_state_round_trips_unicode_and_memory() {
        let library = Library {
            cards: vec![Card {
                id: 7,
                deck: "日本語".to_owned(),
                front: "犬".to_owned(),
                back: "dog".to_owned(),
                last_review_day: 3,
                due_day: 4,
                state: CardState::Review,
                reps: 2,
                lapses: 0,
                stability: Some(3.2),
                difficulty: Some(5.1),
                media: 1,
            }],
            reviews_today: 2,
            again_today: 1,
            transfer_at: Some("today".to_owned()),
        };
        assert_eq!(decode(&encode(&library)), Some(library));
    }

    #[test]
    fn rejects_non_finite_memory_and_extended_state_codes() {
        let invalid_memory = b"v1\t0\t0\t\n1\t44\t51\t41\t0\t0\tr\t0\t0\tNaN,5\t0\n";
        assert_eq!(decode(invalid_memory), None);
        let extended_state = b"v1\t0\t0\t\n1\t44\t51\t41\t0\t0\trr\t0\t0\t\t0\n";
        assert_eq!(decode(extended_state), None);
    }
}
