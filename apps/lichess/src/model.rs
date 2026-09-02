use crate::chess;
use kobo_json::{ObjectBuilder, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "white" | "w" => Some(Self::White),
            "black" | "b" => Some(Self::Black),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Black => "Black",
        }
    }

    pub const fn fen(self) -> char {
        match self {
            Self::White => 'w',
            Self::Black => 'b',
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Account {
    pub id: String,
    pub username: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Player {
    pub id: Option<String>,
    pub name: String,
    pub rating: Option<u32>,
}

impl Player {
    pub fn display(&self) -> String {
        self.rating.map_or_else(
            || self.name.clone(),
            |rating| format!("{} ({rating})", self.name),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub game_id: String,
    pub color: Color,
    pub opponent: String,
    pub rated: bool,
}

impl Session {
    pub fn encode(&self) -> Vec<u8> {
        ObjectBuilder::new()
            .set("version", 1_u32)
            .set("game_id", self.game_id.clone())
            .set("color", self.color.name().to_ascii_lowercase())
            .set("opponent", self.opponent.clone())
            .set("rated", self.rated)
            .build()
            .to_json()
            .into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 4096 {
            return None;
        }
        let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
        if value.get("version")?.as_i64()? != 1 {
            return None;
        }
        let game_id = bounded(value.get("game_id")?.as_str()?, 8, 16)?;
        if !game_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return None;
        }
        let opponent = bounded_label(value.get("opponent")?.as_str()?, 1, 80)?;
        Some(Self {
            game_id,
            color: Color::parse(value.get("color")?.as_str()?)?,
            opponent,
            rated: value.get("rated")?.as_bool()?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Challenge {
    pub id: String,
    pub challenger: String,
    pub direction: ChallengeDirection,
    pub status: String,
    pub rated: bool,
    pub variant: String,
    pub speed: String,
    pub time_control: ChallengeTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChallengeDirection {
    Incoming,
    Outgoing,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChallengeTime {
    Clock {
        initial_seconds: Option<u32>,
        increment_seconds: Option<u32>,
    },
    Correspondence {
        days_per_turn: Option<u32>,
    },
    Unlimited,
    Unknown,
}

impl Challenge {
    pub fn supported(&self) -> bool {
        self.direction == ChallengeDirection::Incoming
            && self.status == "created"
            && self.variant == "standard"
            && matches!(
                &self.time_control,
                ChallengeTime::Clock {
                    initial_seconds: Some(seconds),
                    increment_seconds: Some(_),
                } if *seconds > 0
            )
    }

    pub fn description(&self) -> String {
        let timing = match &self.time_control {
            ChallengeTime::Clock {
                initial_seconds: Some(initial),
                increment_seconds: Some(increment),
            } => format!("{}+{increment}", initial / 60),
            ChallengeTime::Correspondence {
                days_per_turn: Some(days),
            } => format!("{days} day(s) per turn"),
            ChallengeTime::Correspondence {
                days_per_turn: None,
            } => "Correspondence".to_owned(),
            ChallengeTime::Unlimited => "Unlimited".to_owned(),
            ChallengeTime::Clock { .. } | ChallengeTime::Unknown => {
                "Unknown time control".to_owned()
            }
        };
        format!(
            "{} · {timing} · {}",
            if self.rated { "Rated" } else { "Casual" },
            self.speed
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameSummary {
    pub id: String,
    pub color: Color,
    pub opponent: String,
    pub rated: bool,
    pub is_my_turn: bool,
    pub last_move: Option<String>,
}

impl GameSummary {
    pub fn session(&self) -> Session {
        Session {
            game_id: self.id.clone(),
            color: self.color,
            opponent: self.opponent.clone(),
            rated: self.rated,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullGame {
    pub id: String,
    pub initial_fen: String,
    pub rated: bool,
    pub speed: String,
    pub white: Player,
    pub black: Player,
    pub state: ServerState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the Board API exposes independent draw and takeback flags for both colors"
)]
pub struct ServerState {
    pub moves: Vec<String>,
    pub white_ms: u64,
    pub black_ms: u64,
    pub white_increment_ms: u64,
    pub black_increment_ms: u64,
    pub status: String,
    pub winner: Option<Color>,
    pub white_draw: bool,
    pub black_draw: bool,
    pub white_takeback: bool,
    pub black_takeback: bool,
}

impl ServerState {
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "created" | "started")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyState {
    Changed,
    Unchanged,
    Reopen,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Game {
    pub id: String,
    pub my_color: Color,
    pub rated: bool,
    pub speed: String,
    pub white: Player,
    pub black: Player,
    pub initial_fen: String,
    pub state: ServerState,
    pub fen: String,
    pub last_san: Option<String>,
    pub check: bool,
    pub opponent_gone: bool,
    pub claim_win_seconds: Option<u32>,
}

impl Game {
    pub fn from_full(full: FullGame, color: Color) -> Option<Self> {
        let replayed = chess::replay(&full.initial_fen, &full.state.moves)?;
        Some(Self {
            id: full.id,
            my_color: color,
            rated: full.rated,
            speed: full.speed,
            white: full.white,
            black: full.black,
            initial_fen: chess::normalize_initial(&full.initial_fen)?,
            state: full.state,
            fen: replayed.fen,
            last_san: replayed.last_san,
            check: replayed.check,
            opponent_gone: false,
            claim_win_seconds: None,
        })
    }

    pub fn apply(&mut self, incoming: ServerState) -> Option<ApplyState> {
        if incoming.moves.len() < self.state.moves.len()
            || !incoming.moves.starts_with(&self.state.moves)
        {
            return Some(ApplyState::Reopen);
        }
        let replayed = chess::replay(&self.initial_fen, &incoming.moves)?;
        let unchanged = incoming == self.state
            && replayed.fen == self.fen
            && replayed.last_san == self.last_san
            && replayed.check == self.check;
        self.state = incoming;
        self.fen = replayed.fen;
        self.last_san = replayed.last_san;
        self.check = replayed.check;
        Some(if unchanged {
            ApplyState::Unchanged
        } else {
            ApplyState::Changed
        })
    }

    pub fn opponent(&self) -> &Player {
        match self.my_color {
            Color::White => &self.black,
            Color::Black => &self.white,
        }
    }

    pub fn active(&self) -> bool {
        self.state.active()
    }

    pub fn turn(&self) -> Option<Color> {
        Color::parse(match chess::side_to_move(&self.fen)? {
            'w' => "w",
            'b' => "b",
            _ => return None,
        })
    }

    pub fn my_turn(&self) -> bool {
        self.active() && self.turn() == Some(self.my_color)
    }

    pub fn can_abort(&self) -> bool {
        self.active() && self.state.moves.len() < 2
    }

    pub fn draw_offer_from_opponent(&self) -> bool {
        match self.my_color {
            Color::White => self.state.black_draw,
            Color::Black => self.state.white_draw,
        }
    }

    pub fn takeback_pending(&self) -> bool {
        self.state.white_takeback || self.state.black_takeback
    }

    pub fn clock_ms(&self, color: Color, elapsed_seconds: u64) -> u64 {
        let server = match color {
            Color::White => self.state.white_ms,
            Color::Black => self.state.black_ms,
        };
        if self.active() && self.turn() == Some(color) {
            server.saturating_sub(elapsed_seconds.saturating_mul(1000))
        } else {
            server
        }
    }

    pub fn result(&self) -> String {
        match self.state.status.as_str() {
            "created" | "started" => "Game in progress".to_owned(),
            "mate" => winner_sentence(self.state.winner, "checkmate"),
            "resign" => winner_sentence(self.state.winner, "resignation"),
            "outoftime" | "timeout" => winner_sentence(self.state.winner, "time"),
            "stalemate" => "Draw by stalemate".to_owned(),
            "draw" => "Draw agreed".to_owned(),
            "aborted" | "nostart" => "Game aborted".to_owned(),
            "cheat" => winner_sentence(self.state.winner, "server ruling"),
            other => format!("Game finished: {other}"),
        }
    }
}

fn winner_sentence(winner: Option<Color>, reason: &str) -> String {
    winner.map_or_else(
        || format!("Game finished by {reason}"),
        |winner| format!("{} won by {reason}", winner.name()),
    )
}

fn bounded(value: &str, minimum: usize, maximum: usize) -> Option<String> {
    (minimum..=maximum)
        .contains(&value.len())
        .then(|| value.to_owned())
}

fn bounded_label(value: &str, minimum: usize, maximum: usize) -> Option<String> {
    (minimum..=maximum)
        .contains(&value.len())
        .then_some(value)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
}

pub fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    let text = value?.as_str()?;
    let mut moves = Vec::new();
    for movement in text.split_whitespace() {
        if !{
            let bytes = movement.as_bytes();
            matches!(bytes.len(), 4 | 5)
                && matches!(bytes[0], b'a'..=b'h')
                && matches!(bytes[1], b'1'..=b'8')
                && matches!(bytes[2], b'a'..=b'h')
                && matches!(bytes[3], b'1'..=b'8')
                && (bytes.len() == 4 || matches!(bytes[4], b'q' | b'r' | b'b' | b'n'))
        } {
            return None;
        }
        moves.push(movement.to_owned());
    }
    Some(moves)
}

#[cfg(test)]
mod tests {
    use super::{ApplyState, Color, FullGame, Game, Player, ServerState, Session};

    fn state(moves: &str) -> ServerState {
        ServerState {
            moves: moves.split_whitespace().map(str::to_owned).collect(),
            white_ms: 600_000,
            black_ms: 600_000,
            white_increment_ms: 0,
            black_increment_ms: 0,
            status: "started".to_owned(),
            winner: None,
            white_draw: false,
            black_draw: false,
            white_takeback: false,
            black_takeback: false,
        }
    }

    fn game(moves: &str) -> Game {
        Game::from_full(
            FullGame {
                id: "abcdEF12".to_owned(),
                initial_fen: "startpos".to_owned(),
                rated: true,
                speed: "rapid".to_owned(),
                white: Player {
                    id: Some("me".to_owned()),
                    name: "Me".to_owned(),
                    rating: Some(1500),
                },
                black: Player {
                    id: Some("them".to_owned()),
                    name: "Them".to_owned(),
                    rating: Some(1510),
                },
                state: state(moves),
            },
            Color::White,
        )
        .expect("game")
    }

    #[test]
    fn zero_move_games_are_valid_reconnect_state() {
        let zero = game("");
        assert!(zero.state.moves.is_empty());
        assert!(zero.my_turn());
        assert!(zero.can_abort());
        assert!(game("e2e4").can_abort());
        assert!(!game("e2e4 e7e5").can_abort());
    }

    #[test]
    fn stale_or_divergent_events_request_authoritative_reopen() {
        let mut game = game("e2e4 e7e5");
        assert_eq!(game.apply(state("e2e4")), Some(ApplyState::Reopen));
        assert_eq!(
            game.apply(state("d2d4 d7d5 e2e4")),
            Some(ApplyState::Reopen)
        );
        assert_eq!(
            game.apply(state("e2e4 e7e5 g1f3")),
            Some(ApplyState::Changed)
        );
    }

    #[test]
    fn clocks_decrement_only_for_the_side_to_move() {
        let game = game("e2e4");
        assert_eq!(game.clock_ms(Color::White, 12), 600_000);
        assert_eq!(game.clock_ms(Color::Black, 12), 588_000);
    }

    #[test]
    fn persistence_is_small_sanitized_and_rejects_corruption() {
        let session = Session {
            game_id: "abcdEF12".to_owned(),
            color: Color::Black,
            opponent: "Opponent".to_owned(),
            rated: true,
        };
        let encoded = session.encode();
        assert_eq!(Session::decode(&encoded), Some(session));
        assert!(!String::from_utf8_lossy(&encoded)
            .to_ascii_lowercase()
            .contains("authorization"));
        assert!(Session::decode(b"{broken").is_none());
        assert!(Session::decode(
            br#"{"version":1,"game_id":"../../etc","color":"white","opponent":"x","rated":true}"#
        )
        .is_none());
    }
}
