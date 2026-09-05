use crate::model::{
    Account, Challenge, ChallengeDirection, ChallengeTime, Color, FullGame, GameSummary, Player,
    ServerState,
};
use kobo_json::Value;
use kobo_sdk::{Credential, Header, Task};

pub const SECRET: &str = "lichess";
pub const PUZZLE_URL: &str = "https://lichess.org/api/puzzle/batch/mix?nb=32&difficulty=normal";
pub const ACCOUNT_URL: &str = "https://lichess.org/api/account";
pub const PLAYING_URL: &str = "https://lichess.org/api/account/playing";
pub const EVENT_URL: &str = "https://lichess.org/api/stream/event";

const MAX_JSON: u32 = 256 * 1024;
const MAX_RECORD: u32 = 128 * 1024;
const FORM: &str = "application/x-www-form-urlencoded";
const STREAM_TYPE: &str = "application/x-ndjson";
const STREAM_HEADER: &str = "X-Cobalt-Line-Stream";
const WAIT_HEADER: &str = "X-Cobalt-Wait-Until-Cancelled";
const RATE_HEADER: &str = "X-Cobalt-Rate-Limit";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeekPreset {
    Rapid10_0,
    Rapid10_5,
    Rapid15_10,
    Classical30_0,
    Classical30_20,
}

impl SeekPreset {
    pub const ALL: [Self; 5] = [
        Self::Rapid10_0,
        Self::Rapid10_5,
        Self::Rapid15_10,
        Self::Classical30_0,
        Self::Classical30_20,
    ];

    pub const fn minutes(self) -> u16 {
        match self {
            Self::Rapid10_0 | Self::Rapid10_5 => 10,
            Self::Rapid15_10 => 15,
            Self::Classical30_0 | Self::Classical30_20 => 30,
        }
    }

    pub const fn increment(self) -> u16 {
        match self {
            Self::Rapid10_0 | Self::Classical30_0 => 0,
            Self::Rapid10_5 => 5,
            Self::Rapid15_10 => 10,
            Self::Classical30_20 => 20,
        }
    }

    pub const fn speed(self) -> &'static str {
        match self {
            Self::Rapid10_0 | Self::Rapid10_5 | Self::Rapid15_10 => "rapid",
            Self::Classical30_0 | Self::Classical30_20 => "classical",
        }
    }

    pub const fn speed_label(self) -> &'static str {
        match self {
            Self::Rapid10_0 | Self::Rapid10_5 | Self::Rapid15_10 => "Rapid",
            Self::Classical30_0 | Self::Classical30_20 => "Classical",
        }
    }

    pub const fn action(self) -> &'static str {
        match self {
            Self::Rapid10_0 => "seek-10-0",
            Self::Rapid10_5 => "seek-10-5",
            Self::Rapid15_10 => "seek-15-10",
            Self::Classical30_0 => "seek-30-0",
            Self::Classical30_20 => "seek-30-20",
        }
    }

    pub fn label(self) -> String {
        format!("{}+{}", self.minutes(), self.increment())
    }

    pub fn body(self) -> String {
        format!(
            "rated=true&time={}&increment={}&variant=standard&color=random",
            self.minutes(),
            self.increment()
        )
    }

    pub fn challenge_body(self, color: &str) -> String {
        format!(
            "rated=false&clock.limit={}&clock.increment={}&variant=standard&color={color}",
            u32::from(self.minutes()).saturating_mul(60),
            self.increment()
        )
    }

    pub fn matches_summary(self, game: &GameSummary) -> bool {
        let initial = u32::from(self.minutes()).saturating_mul(60);
        game.supported()
            && game.rated
            && game
                .source
                .as_deref()
                .is_some_and(|source| matches!(source, "pool" | "lobby"))
            && game.speed.as_deref() == Some(self.speed())
            && game
                .seconds_left
                .is_some_and(|seconds| (initial.saturating_sub(60)..=initial).contains(&seconds))
    }

    pub fn matches_full(self, game: &FullGame) -> bool {
        let increment_ms = u64::from(self.increment()).saturating_mul(1000);
        game.rated
            && game.speed == self.speed()
            && game.state.white_increment_ms == increment_ms
            && game.state.black_increment_ms == increment_ms
    }
}

pub fn account() -> Task {
    fetch(ACCOUNT_URL, MAX_JSON)
}

pub fn playing() -> Task {
    fetch(PLAYING_URL, MAX_JSON)
}

pub fn puzzle(credential: bool) -> Task {
    Task::Fetch {
        url: PUZZLE_URL.to_owned(),
        offset: 0,
        max_bytes: MAX_JSON,
        credential: credential.then(|| Credential::bearer(SECRET)),
        headers: vec![Header::new(RATE_HEADER, "1")],
    }
}

pub fn event_stream(action: &str) -> Task {
    stream(EVENT_URL.to_owned(), action)
}

pub fn board_stream(game_id: &str, action: &str) -> Option<Task> {
    valid_id(game_id).then(|| {
        stream(
            format!("https://lichess.org/api/board/game/stream/{game_id}"),
            action,
        )
    })
}

pub fn seek(preset: SeekPreset) -> Task {
    Task::Post {
        url: "https://lichess.org/api/board/seek".to_owned(),
        body: preset.body(),
        content_type: FORM.to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: vec![Header::new(WAIT_HEADER, "1"), Header::new(RATE_HEADER, "1")],
        max_bytes: 4096,
    }
}

pub fn move_piece(game_id: &str, movement: &str) -> Option<Task> {
    (valid_id(game_id) && valid_move(movement)).then(|| {
        post(format!(
            "https://lichess.org/api/board/game/{game_id}/move/{movement}"
        ))
    })
}

pub fn resign(game_id: &str) -> Option<Task> {
    valid_id(game_id).then(|| {
        post(format!(
            "https://lichess.org/api/board/game/{game_id}/resign"
        ))
    })
}

pub fn abort(game_id: &str) -> Option<Task> {
    valid_id(game_id).then(|| {
        post(format!(
            "https://lichess.org/api/board/game/{game_id}/abort"
        ))
    })
}

pub fn claim_victory(game_id: &str) -> Option<Task> {
    valid_id(game_id).then(|| {
        post(format!(
            "https://lichess.org/api/board/game/{game_id}/claim-victory"
        ))
    })
}

pub fn draw(game_id: &str, accept: bool) -> Option<Task> {
    valid_id(game_id).then(|| {
        post(format!(
            "https://lichess.org/api/board/game/{game_id}/draw/{}",
            if accept { "yes" } else { "no" }
        ))
    })
}

pub fn challenge(challenge_id: &str, accept: bool) -> Option<Task> {
    valid_id(challenge_id).then(|| {
        post(format!(
            "https://lichess.org/api/challenge/{challenge_id}/{}",
            if accept { "accept" } else { "decline" }
        ))
    })
}

pub fn challenge_player(username: &str, preset: SeekPreset, color: &str) -> Option<Task> {
    valid_username(username).then(|| Task::Post {
        url: format!("https://lichess.org/api/challenge/{username}"),
        body: preset.challenge_body(color),
        content_type: FORM.to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: vec![Header::new(RATE_HEADER, "1")],
        max_bytes: MAX_RECORD,
    })
}

fn fetch(url: &str, max_bytes: u32) -> Task {
    Task::Fetch {
        url: url.to_owned(),
        offset: 0,
        max_bytes,
        credential: Some(Credential::bearer(SECRET)),
        headers: vec![Header::new(RATE_HEADER, "1")],
    }
}

fn stream(url: String, action: &str) -> Task {
    Task::Fetch {
        url,
        offset: 0,
        max_bytes: MAX_RECORD,
        credential: Some(Credential::bearer(SECRET)),
        headers: vec![
            Header::new("Accept", STREAM_TYPE),
            Header::new(STREAM_HEADER, action),
            Header::new(RATE_HEADER, "1"),
        ],
    }
}

fn post(url: String) -> Task {
    Task::Post {
        url,
        body: String::new(),
        content_type: FORM.to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: vec![Header::new(RATE_HEADER, "1")],
        max_bytes: 16 * 1024,
    }
}

fn valid_username(value: &str) -> bool {
    (2..=30).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    GameStart(GameSummary),
    GameFinish(String),
    Challenge(Challenge),
    ChallengeCanceled(String),
    ChallengeDeclined(String),
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoardRecord {
    Full(FullGame),
    State(ServerState),
    OpponentGone {
        gone: bool,
        claim_win_seconds: Option<u32>,
    },
    Unsupported(String),
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimit {
    Limited(Option<u32>),
}

pub fn rate_limit(bytes: &[u8]) -> Option<RateLimit> {
    let text = std::str::from_utf8(bytes).ok()?;
    let rest = text.strip_prefix("COBALT-HTTP/1 429\nRetry-After: ")?;
    let value = rest.strip_suffix("\n\n")?;
    Some(RateLimit::Limited(if value == "-" {
        None
    } else {
        value.parse::<u32>().ok()
    }))
}

pub fn parse_account(bytes: &[u8]) -> Option<Account> {
    let value = json(bytes)?;
    Some(Account {
        id: user_identifier(value.get("id")?.as_str()?)?,
        username: label(value.get("username")?.as_str()?, 1, 80)?,
    })
}

pub fn parse_playing(bytes: &[u8]) -> Option<Vec<GameSummary>> {
    let value = json(bytes)?;
    let games = value.get("nowPlaying")?.as_array()?;
    Some(
        games
            .iter()
            .filter_map(game_summary)
            .filter(GameSummary::supported)
            .collect(),
    )
}

pub fn parse_event(bytes: &[u8]) -> Option<Event> {
    let value = json(bytes)?;
    match value.get("type")?.as_str()? {
        "gameStart" => {
            let game = value.get("game")?;
            Some(Event::GameStart(game_summary(game)?))
        }
        "gameFinish" => Some(Event::GameFinish(identifier(
            value.get("game")?.get("id")?.as_str()?,
        )?)),
        "challenge" => Some(Event::Challenge(parse_challenge(value.get("challenge")?)?)),
        "challengeCanceled" => Some(Event::ChallengeCanceled(identifier(
            value.get("challenge")?.get("id")?.as_str()?,
        )?)),
        "challengeDeclined" => Some(Event::ChallengeDeclined(identifier(
            value.get("challenge")?.get("id")?.as_str()?,
        )?)),
        _ => Some(Event::Unknown),
    }
}

pub fn parse_board(bytes: &[u8], expected_id: &str) -> Option<BoardRecord> {
    let value = json(bytes)?;
    match value.get("type")?.as_str()? {
        "gameFull" => {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| Some(expected_id.to_owned()), identifier)?;
            if id != expected_id {
                return None;
            }
            let variant = value.get("variant")?.get("key").and_then(Value::as_str)?;
            if variant != "standard" {
                return Some(BoardRecord::Unsupported(variant.to_owned()));
            }
            Some(BoardRecord::Full(FullGame {
                id,
                initial_fen: bounded(
                    value
                        .get("initialFen")
                        .and_then(Value::as_str)
                        .unwrap_or("startpos"),
                    1,
                    256,
                )?,
                rated: value.get("rated").and_then(Value::as_bool).unwrap_or(false),
                speed: value
                    .get("speed")
                    .and_then(Value::as_str)
                    .unwrap_or("rapid")
                    .to_owned(),
                white: parse_player(value.get("white")?)?,
                black: parse_player(value.get("black")?)?,
                state: parse_state(value.get("state")?)?,
            }))
        }
        "gameState" => Some(BoardRecord::State(parse_state(&value)?)),
        "opponentGone" => Some(BoardRecord::OpponentGone {
            gone: value.get("gone")?.as_bool()?,
            claim_win_seconds: value
                .get("claimWinInSeconds")
                .and_then(Value::as_i64)
                .and_then(|seconds| u32::try_from(seconds).ok()),
        }),
        _ => Some(BoardRecord::Ignored),
    }
}

fn game_summary(value: &Value) -> Option<GameSummary> {
    let opponent = value.get("opponent")?;
    Some(GameSummary {
        id: identifier(value.get("gameId").or_else(|| value.get("id"))?.as_str()?)?,
        color: Color::parse(value.get("color")?.as_str()?)?,
        opponent: opponent
            .get("username")
            .or_else(|| opponent.get("name"))
            .and_then(Value::as_str)
            .and_then(|name| label(name, 1, 80))
            .or_else(|| {
                opponent
                    .get("ai")
                    .and_then(unsigned)
                    .map(|level| format!("Lichess AI {level}"))
            })?,
        rated: value.get("rated").and_then(Value::as_bool).unwrap_or(false),
        is_my_turn: value
            .get("isMyTurn")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        last_move: value
            .get("lastMove")
            .and_then(Value::as_str)
            .filter(|movement| valid_move(movement))
            .map(str::to_owned),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .and_then(|source| bounded(source, 1, 32)),
        speed: value
            .get("speed")
            .and_then(Value::as_str)
            .and_then(|speed| bounded(speed, 1, 32)),
        variant: value
            .get("variant")
            .and_then(|variant| variant.get("key"))
            .and_then(Value::as_str)
            .and_then(|variant| bounded(variant, 1, 32)),
        seconds_left: value.get("secondsLeft").and_then(unsigned),
    })
}

fn parse_challenge(value: &Value) -> Option<Challenge> {
    let time = value.get("timeControl")?;
    let time_control = match time.get("type").and_then(Value::as_str) {
        Some("clock") => ChallengeTime::Clock {
            initial_seconds: time.get("limit").and_then(unsigned),
            increment_seconds: time.get("increment").and_then(unsigned),
        },
        Some("correspondence") => ChallengeTime::Correspondence {
            days_per_turn: time.get("daysPerTurn").and_then(unsigned),
        },
        Some("unlimited") => ChallengeTime::Unlimited,
        _ => ChallengeTime::Unknown,
    };
    Some(Challenge {
        id: identifier(value.get("id")?.as_str()?)?,
        challenger: label(
            value
                .get("challenger")?
                .get("name")
                .or_else(|| value.get("challenger")?.get("username"))?
                .as_str()?,
            1,
            80,
        )?,
        direction: match value.get("direction").and_then(Value::as_str) {
            Some("in") => ChallengeDirection::Incoming,
            Some("out") => ChallengeDirection::Outgoing,
            _ => ChallengeDirection::Unknown,
        },
        status: bounded(value.get("status")?.as_str()?, 1, 32)?,
        rated: value.get("rated").and_then(Value::as_bool).unwrap_or(false),
        variant: bounded(value.get("variant")?.get("key")?.as_str()?, 1, 32)?,
        speed: bounded(value.get("speed")?.as_str()?, 1, 32)?,
        time_control,
    })
}

fn parse_player(value: &Value) -> Option<Player> {
    let name = value
        .get("name")
        .or_else(|| value.get("username"))
        .and_then(Value::as_str)
        .unwrap_or("Anonymous");
    Some(Player {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .and_then(user_identifier),
        name: label(name, 1, 80)?,
        rating: value.get("rating").and_then(unsigned),
    })
}

fn parse_state(value: &Value) -> Option<ServerState> {
    Some(ServerState {
        moves: crate::model::strings(value.get("moves"))?,
        white_ms: unsigned64(value.get("wtime")?)?,
        black_ms: unsigned64(value.get("btime")?)?,
        white_increment_ms: value.get("winc").and_then(unsigned64).unwrap_or(0),
        black_increment_ms: value.get("binc").and_then(unsigned64).unwrap_or(0),
        status: bounded(value.get("status")?.as_str()?, 1, 32)?,
        winner: value
            .get("winner")
            .and_then(Value::as_str)
            .and_then(Color::parse),
        white_draw: value.get("wdraw").and_then(Value::as_bool).unwrap_or(false),
        black_draw: value.get("bdraw").and_then(Value::as_bool).unwrap_or(false),
        white_takeback: value
            .get("wtakeback")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        black_takeback: value
            .get("btakeback")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn json(bytes: &[u8]) -> Option<Value> {
    if bytes.len() > MAX_JSON as usize {
        return None;
    }
    kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()
}

fn unsigned(value: &Value) -> Option<u32> {
    u32::try_from(value.as_i64()?).ok()
}

fn unsigned64(value: &Value) -> Option<u64> {
    u64::try_from(value.as_i64()?).ok()
}

fn identifier(value: &str) -> Option<String> {
    valid_id(value).then(|| value.to_owned())
}

fn user_identifier(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| value.to_owned())
}

fn bounded(value: &str, minimum: usize, maximum: usize) -> Option<String> {
    (minimum..=maximum)
        .contains(&value.len())
        .then(|| value.to_owned())
}

fn label(value: &str, minimum: usize, maximum: usize) -> Option<String> {
    (minimum..=maximum)
        .contains(&value.len())
        .then_some(value)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
}

fn valid_id(value: &str) -> bool {
    (8..=16).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn valid_move(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.len(), 4 | 5)
        && matches!(bytes[0], b'a'..=b'h')
        && matches!(bytes[1], b'1'..=b'8')
        && matches!(bytes[2], b'a'..=b'h')
        && matches!(bytes[3], b'1'..=b'8')
        && (bytes.len() == 4 || matches!(bytes[4], b'q' | b'r' | b'b' | b'n'))
}

#[cfg(test)]
mod tests {
    use super::{
        abort, board_stream, challenge_player, event_stream, move_piece, parse_account,
        parse_board, parse_event, parse_playing, rate_limit, seek, BoardRecord, Color, Event,
        FullGame, GameSummary, Player, RateLimit, SeekPreset, ServerState,
    };
    use kobo_sdk::Task;

    #[test]
    fn request_shapes_use_only_the_named_secret_and_runtime_controls() {
        let Task::Post {
            url,
            body,
            credential,
            headers,
            ..
        } = seek(SeekPreset::Rapid10_0)
        else {
            panic!("seek is a post")
        };
        assert_eq!(url, "https://lichess.org/api/board/seek");
        assert_eq!(
            body,
            "rated=true&time=10&increment=0&variant=standard&color=random"
        );
        assert_eq!(credential.expect("credential").secret, "lichess");
        assert!(headers.iter().any(|header| header
            .name
            .eq_ignore_ascii_case("x-cobalt-wait-until-cancelled")));

        let Task::Fetch {
            credential,
            headers,
            ..
        } = event_stream("open")
        else {
            panic!("event stream is a fetch")
        };
        assert_eq!(credential.expect("credential").secret, "lichess");
        assert!(headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("x-cobalt-line-stream") && header.value == "open"
        }));
        assert!(move_piece("abcdEF12", "e2e4").is_some());
        assert!(move_piece("abcdEF12", "e2e9").is_none());
        assert!(abort("../../etc").is_none());
        assert!(board_stream("abcdEF12", "next").is_some());
    }

    #[test]
    fn every_seek_preset_has_the_exact_rated_random_standard_body() {
        let expected = [
            ("10", "0"),
            ("10", "5"),
            ("15", "10"),
            ("30", "0"),
            ("30", "20"),
        ];
        for (preset, (minutes, increment)) in SeekPreset::ALL.into_iter().zip(expected) {
            let Task::Post { body, .. } = seek(preset) else {
                panic!("seek is a post")
            };
            assert_eq!(
                body,
                format!(
                    "rated=true&time={minutes}&increment={increment}&variant=standard&color=random"
                )
            );
        }
    }

    #[test]
    fn player_challenges_are_casual_standard_and_validate_the_username() {
        let Task::Post {
            url,
            body,
            credential,
            ..
        } = challenge_player("Reader-Two", SeekPreset::Rapid10_5, "white")
            .expect("valid challenge")
        else {
            panic!("challenge is a post")
        };
        assert_eq!(url, "https://lichess.org/api/challenge/Reader-Two");
        assert_eq!(
            body,
            "rated=false&clock.limit=600&clock.increment=5&variant=standard&color=white"
        );
        assert_eq!(credential.expect("credential").secret, "lichess");
        assert!(challenge_player("a", SeekPreset::Rapid10_0, "random").is_none());
        assert!(challenge_player("../other", SeekPreset::Rapid10_0, "random").is_none());
    }

    #[test]
    fn every_seek_preset_matches_its_lobby_clock_and_full_increment() {
        for preset in SeekPreset::ALL {
            let summary = GameSummary {
                id: "abcdEF12".to_owned(),
                color: Color::White,
                opponent: "Other".to_owned(),
                rated: true,
                is_my_turn: true,
                last_move: None,
                source: Some("pool".to_owned()),
                speed: Some(preset.speed().to_owned()),
                variant: Some("standard".to_owned()),
                seconds_left: Some(u32::from(preset.minutes()) * 60),
            };
            assert!(preset.matches_summary(&summary), "{}", preset.label());
            let mut legacy_lobby = summary.clone();
            legacy_lobby.source = Some("lobby".to_owned());
            assert!(preset.matches_summary(&legacy_lobby));
            let mut unrelated = summary.clone();
            unrelated.source = Some("friend".to_owned());
            assert!(!preset.matches_summary(&unrelated));
            let mut wrong_minutes = summary.clone();
            wrong_minutes.seconds_left =
                Some(u32::from(preset.minutes().saturating_add(5)).saturating_mul(60));
            assert!(!preset.matches_summary(&wrong_minutes));

            let full = FullGame {
                id: "abcdEF12".to_owned(),
                initial_fen: "startpos".to_owned(),
                rated: true,
                speed: preset.speed().to_owned(),
                white: Player {
                    id: None,
                    name: "Owner".to_owned(),
                    rating: None,
                },
                black: Player {
                    id: None,
                    name: "Other".to_owned(),
                    rating: None,
                },
                state: ServerState {
                    moves: Vec::new(),
                    white_ms: u64::from(preset.minutes()) * 60_000,
                    black_ms: u64::from(preset.minutes()) * 60_000,
                    white_increment_ms: u64::from(preset.increment()) * 1000,
                    black_increment_ms: u64::from(preset.increment()) * 1000,
                    status: "started".to_owned(),
                    winner: None,
                    white_draw: false,
                    black_draw: false,
                    white_takeback: false,
                    black_takeback: false,
                },
            };
            assert!(preset.matches_full(&full));
            let mut wrong_increment = full;
            wrong_increment.state.black_increment_ms = wrong_increment
                .state
                .black_increment_ms
                .saturating_add(1000);
            assert!(!preset.matches_full(&wrong_increment));
        }
    }

    #[test]
    fn parses_account_playing_and_zero_move_game_full() {
        let account = parse_account(br#"{"id":"owner123","username":"Owner"}"#).expect("account");
        assert_eq!(account.username, "Owner");
        let playing = parse_playing(
            br#"{"nowPlaying":[{"gameId":"abcdEF12","color":"white","rated":true,"source":"lobby","speed":"rapid","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":true,"lastMove":"","opponent":{"username":"Other"}}]}"#,
        )
        .expect("playing");
        assert_eq!(playing.len(), 1);
        assert!(playing[0].is_my_turn);
        let board = parse_board(
            br#"{"type":"gameFull","id":"abcdEF12","rated":true,"speed":"rapid","variant":{"key":"standard"},"initialFen":"startpos","white":{"id":"owner123","name":"Owner","rating":1500},"black":{"id":"other123","name":"Other","rating":1510},"state":{"type":"gameState","moves":"","wtime":600000,"btime":600000,"winc":0,"binc":0,"status":"started"}}"#,
            "abcdEF12",
        )
        .expect("board");
        let BoardRecord::Full(full) = board else {
            panic!("full")
        };
        assert!(full.state.moves.is_empty());
        assert!(matches!(
            parse_board(
                br#"{"type":"gameFull","id":"abcdEF12","rated":false,"speed":"rapid","variant":{"key":"chess960"},"initialFen":"startpos","white":{"name":"Owner"},"black":{"name":"Other"},"state":{"moves":"","wtime":600000,"btime":600000,"status":"started"}}"#,
                "abcdEF12",
            ),
            Some(BoardRecord::Unsupported(variant)) if variant == "chess960"
        ));
    }

    #[test]
    fn parses_event_lifecycle_and_board_updates() {
        let started = parse_event(
            br#"{"type":"gameStart","game":{"id":"abcdEF12","color":"black","rated":true,"speed":"rapid","source":"friend","variant":{"key":"standard"},"secondsLeft":600,"isMyTurn":false,"lastMove":"e2e4","opponent":{"username":"Other"}}}"#,
        )
        .expect("event");
        let Event::GameStart(started) = started else {
            panic!("game start")
        };
        assert!(!SeekPreset::Rapid10_0.matches_summary(&started));
        let challenge = parse_event(
            br#"{"type":"challenge","challenge":{"id":"chall123","status":"created","direction":"in","challenger":{"name":"Other"},"rated":false,"variant":{"key":"standard"},"speed":"rapid","timeControl":{"type":"clock","limit":600,"increment":0}}}"#,
        )
        .expect("challenge");
        assert!(matches!(challenge, Event::Challenge(_)));
        let unlimited = parse_event(
            br#"{"type":"challenge","challenge":{"id":"other123","status":"created","direction":"out","challenger":{"name":"Owner"},"rated":false,"variant":{"key":"standard"},"speed":"correspondence","timeControl":{"type":"unlimited"}}}"#,
        )
        .expect("valid unlimited challenge");
        let Event::Challenge(unlimited) = unlimited else {
            panic!("challenge")
        };
        assert!(!unlimited.supported());
        let update = parse_board(
            br#"{"type":"gameState","moves":"e2e4 e7e5","wtime":599000,"btime":598000,"winc":0,"binc":0,"status":"started","wdraw":false,"bdraw":true}"#,
            "abcdEF12",
        );
        assert!(matches!(update, Some(BoardRecord::State(_))));
        assert!(parse_board(
            br#"{"type":"gameState","wtime":599000,"btime":598000,"status":"started"}"#,
            "abcdEF12",
        )
        .is_none());
    }

    #[test]
    fn rate_limit_envelope_never_contains_an_upstream_body() {
        assert_eq!(
            rate_limit(b"COBALT-HTTP/1 429\nRetry-After: 12\n\n"),
            Some(RateLimit::Limited(Some(12)))
        );
        assert_eq!(
            rate_limit(b"COBALT-HTTP/1 429\nRetry-After: -\n\n"),
            Some(RateLimit::Limited(None))
        );
        assert_eq!(rate_limit(b"{\"error\":\"private\"}"), None);
    }
}
