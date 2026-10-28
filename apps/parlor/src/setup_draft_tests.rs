//! The setup screen is a form: nothing on it reaches the game until Start
//! game. These tests drive it through the app's own taps and read only what
//! the panel shows and what the app writes to its store, so they say the
//! same thing about any way of holding the choices.
use super::*;
use kobo_sdk::{AppRunner, Command, StoreRequest};
use kobo_ui::Node;

/// The filled chip of every choice on a screen, in order.
fn chosen(screen: &Screen) -> Vec<String> {
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Node::Chips { chips, .. } => out.extend(
                    chips
                        .iter()
                        .filter(|chip| chip.selected)
                        .map(|chip| chip.label.clone()),
                ),
                Node::Band { slots, .. } => {
                    for slot in slots {
                        walk(&slot.nodes, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&screen.nodes, &mut out);
    out
}

/// Every save the app asked for in these commands, read back as a game.
fn saved(commands: &[Command]) -> Vec<Parlor> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == SAVE => {
                Some(decode(std::str::from_utf8(value).expect("a save is text")).expect("opens"))
            }
            _ => None,
        })
        .collect()
}

/// A game against the computer one move in, on the board.
fn solo_game_under_way(title: Title) -> AppRunner<Parlor> {
    let mut app = Parlor::default();
    app.start(title);
    assert!(tests::play_one_move(&mut app));
    let mut runner = AppRunner::new(app);
    runner.app_mut().loaded = true;
    runner
}

fn open_new_game(runner: &mut AppRunner<Parlor>) {
    runner.action(action_id(chrome::MORE));
    runner.action(action_id(chrome::NEW_GAME));
    assert!(matches!(runner.app().view, View::Setup(_)));
}

#[test]
fn choosing_pass_and_play_then_back_keeps_a_game_against_the_computer_that_still_replies() {
    for title in GAMES {
        let mut runner = solo_game_under_way(title);
        let before = encode(runner.app(), runner.app().position.as_ref().unwrap());
        open_new_game(&mut runner);
        runner.action(action_id(PLAY_PEOPLE));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Board, "{}", title.name());
        assert_eq!(
            encode(runner.app(), runner.app().position.as_ref().unwrap()),
            before,
            "{}: Back changed the game",
            title.name()
        );
        let words = tests::shown(&runner.app().screen()).join(" | ");
        assert!(words.contains("Your move"), "{}: {words}", title.name());
        assert!(
            words.contains("You 0 · Computer 0"),
            "{}: {words}",
            title.name()
        );

        // The computer answers the next move in the same tap, so a line of
        // its own lands in the record within a move or two (a Kalah sowing
        // that ends in the player's store, or a Morris mill, is still the
        // player's turn).
        let moves = runner.app().record.len();
        let app = runner.app_mut();
        let replied = |app: &Parlor| {
            app.record[moves..]
                .iter()
                .any(|line| line.starts_with("Computer:"))
        };
        for _ in 0..6 {
            if replied(app) || !tests::play_one_move(app) {
                break;
            }
        }
        assert!(
            replied(app),
            "{}: the computer did not reply: {:?}",
            title.name(),
            &app.record[moves..]
        );
    }
}

#[test]
fn side_strength_and_board_choices_then_back_change_nothing_in_the_game() {
    for title in GAMES {
        // Against the computer: every choice the form offers, then Back.
        let mut runner = solo_game_under_way(title);
        let before = encode(runner.app(), runner.app().position.as_ref().unwrap());
        let board = tests::shown(&runner.app().screen());
        let strength = runner.app().strength;
        open_new_game(&mut runner);
        for name in [SIDE_SECOND, STRENGTHS[2].0, PLAY_PEOPLE, ROTATE_OFF] {
            runner.action(action_id(name));
        }
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Board);
        assert_eq!(
            encode(runner.app(), runner.app().position.as_ref().unwrap()),
            before,
            "{}",
            title.name()
        );
        assert_eq!(runner.app().strength, strength, "{}", title.name());
        assert_eq!(
            tests::shown(&runner.app().screen()),
            board,
            "{}",
            title.name()
        );

        // Between two people, with the board turning: the other way round.
        let mut app = Parlor {
            mode: Mode::PassAndPlay,
            ..Parlor::default()
        };
        app.start(title);
        assert!(tests::play_one_move(&mut app));
        let mut runner = AppRunner::new(app);
        runner.app_mut().loaded = true;
        let before = encode(runner.app(), runner.app().position.as_ref().unwrap());
        let board = tests::shown(&runner.app().screen());
        open_new_game(&mut runner);
        for name in [ROTATE_OFF, PLAY_COMPUTER, SIDE_SECOND, STRENGTHS[0].0] {
            runner.action(action_id(name));
        }
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Board);
        assert_eq!(
            encode(runner.app(), runner.app().position.as_ref().unwrap()),
            before,
            "{}",
            title.name()
        );
        assert_eq!(
            tests::shown(&runner.app().screen()),
            board,
            "{}",
            title.name()
        );
    }
}

#[test]
fn a_game_resumed_from_the_shelf_is_untouched_by_choices_made_on_another_games_setup() {
    let mut runner = solo_game_under_way(Title::Reversi);
    let before = encode(runner.app(), runner.app().position.as_ref().unwrap());
    runner.action(action_id(chrome::MORE));
    runner.action(action_id(CHANGE_GAME));
    runner.action(action_id(Title::Kalah.key()));
    for name in [PLAY_PEOPLE, ROTATE_OFF] {
        runner.action(action_id(name));
    }
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Menu);
    runner.action(action_id("resume"));
    assert_eq!(
        encode(runner.app(), runner.app().position.as_ref().unwrap()),
        before
    );
}

#[test]
fn reopening_new_game_shows_the_settings_of_the_game_in_progress() {
    let mut runner = solo_game_under_way(Title::Draughts);
    open_new_game(&mut runner);
    let fresh = chosen(&runner.app().screen());
    assert_eq!(fresh, ["Computer", "Black, moves first", "Club"]);
    for name in [SIDE_SECOND, STRENGTHS[2].0, PLAY_PEOPLE] {
        runner.action(action_id(name));
    }
    assert_ne!(chosen(&runner.app().screen()), fresh, "a tap shows at once");
    runner.action(ActionId::BACK);
    open_new_game(&mut runner);
    assert_eq!(chosen(&runner.app().screen()), fresh);
}

#[test]
fn a_choice_not_yet_started_is_never_written_to_the_saved_game() {
    let mut runner = solo_game_under_way(Title::Morris);
    let human = runner.app().human_side;
    let strength = runner.app().strength;
    let mut writes = Vec::new();
    writes.extend(saved(&runner.action(action_id(chrome::MORE))));
    writes.extend(saved(&runner.action(action_id(chrome::NEW_GAME))));
    for name in [SIDE_SECOND, STRENGTHS[0].0, PLAY_PEOPLE, ROTATE_OFF] {
        writes.extend(saved(&runner.action(action_id(name))));
    }
    writes.extend(saved(&runner.action(ActionId::BACK)));
    assert!(!writes.is_empty(), "every tap saves the game");
    for game in writes {
        assert_eq!(game.mode, Mode::Solo);
        assert_eq!(game.human_side, human);
        assert_eq!(game.strength, strength);
        assert!(game.rotate);
    }
}

#[test]
fn start_game_applies_every_choice_on_the_setup_screen() {
    // Against the computer, second, at the strongest: the computer opens.
    let mut runner = AppRunner::new(Parlor::default());
    runner.app_mut().loaded = true;
    runner.action(action_id(Title::Reversi.key()));
    for name in [SIDE_SECOND, STRENGTHS[2].0] {
        runner.action(action_id(name));
    }
    let writes = saved(&runner.action(action_id("start")));
    let app = runner.app();
    assert_eq!(app.view, View::Board);
    assert_eq!(app.mode, Mode::Solo);
    assert_eq!(app.human_side, -1);
    assert_eq!(app.strength, Strength::Strong);
    assert_eq!(app.record.len(), 1, "the computer has opened");
    let game = writes.last().expect("Start game is saved");
    assert_eq!(
        (game.mode, game.human_side, game.strength),
        (Mode::Solo, -1, Strength::Strong)
    );

    // Between two people with the board still: New game over that game.
    open_new_game(&mut runner);
    for name in [PLAY_PEOPLE, ROTATE_OFF] {
        runner.action(action_id(name));
    }
    // Rules from the setup screen and back keeps what was chosen.
    runner.action(action_id(chrome::RULES));
    runner.action(ActionId::BACK);
    assert_eq!(
        chosen(&runner.app().screen()),
        ["Pass-and-play", "Stays still"]
    );
    let writes = saved(&runner.action(action_id("start")));
    let app = runner.app();
    assert_eq!(app.mode, Mode::PassAndPlay);
    assert!(!app.rotate);
    assert!(app.record.is_empty());
    let game = writes.last().expect("Start game is saved");
    assert_eq!((game.mode, game.rotate), (Mode::PassAndPlay, false));
    let words = tests::shown(&runner.app().screen()).join(" | ");
    assert!(words.contains("Black to move"), "{words}");
}
