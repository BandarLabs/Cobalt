//! The controls every game keeps in the same place, under the same name.
//!
//! Before this, each game put Undo, its rules and a fresh start wherever its
//! author had room: Undo was a key under the board in one, a row inside a
//! menu screen in another and a bar entry in a third, and the rules were
//! called "Help", "How to play" or "Rules and difficulty" depending on the
//! box. Somebody who learned one game had learned nothing about the next.
//!
//! The arrangement here is built only from controls the runtime already
//! draws, so it reaches a game with the game's own release and needs nothing
//! new on the wire:
//!
//! - **Rules** is a word in the top bar, never more than one tap away, because
//!   the moment a player wants the rules is the moment they are stuck.
//! - **More** is the three dots beside it. **New game** is always its first
//!   entry; anything a game adds goes after it.
//! - **Undo** is the first entry of the bar at the foot of the panel, the
//!   same corner in every game, with up to two of the game's own verbs beside
//!   it.
//!
//! A game that has no undo, such as a quiz, leaves it out rather than showing
//! one that never does anything.

use crate::{action_id, ActionId, ScreenBuilder};

/// The action raised by Undo.
pub const UNDO: &str = "game-undo";
/// The action raised by Rules.
pub const RULES: &str = "game-rules";
/// The three dots that open the menu.
pub const MORE: &str = "game-more";
/// The menu's first entry.
pub const NEW_GAME: &str = "game-new";

/// What Undo is called in every game.
pub const UNDO_LABEL: &str = "Undo";
/// What the rules are called in every game.
pub const RULES_LABEL: &str = "Rules";
/// What starting again is called in every game.
pub const NEW_GAME_LABEL: &str = "New game";
/// What to say when Undo is tapped with nothing behind it.
///
/// The bar has no greyed-out state, and an Undo that comes and goes moves the
/// game's other verbs under the player's finger, so it stays put and answers
/// in words instead.
pub const NOTHING_TO_UNDO: &str = "Nothing to undo yet.";

/// A tap the shared controls recognised.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tap {
    /// The menu opened or closed and nothing else happened. Redraw.
    Menu,
    Undo,
    Rules,
    NewGame,
}

/// Whether the More menu is showing.
///
/// The menu is a popover, and a tap anywhere outside a popover arrives as
/// [`ActionId::BACK`]. A game that had to notice that itself would sometimes
/// treat it as "leave the game", so [`Self::read`] takes it first.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Menu {
    open: bool,
}

impl Menu {
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    /// Closes the menu, for a game that changes screen on its own.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Reads a tap before the game does.
    ///
    /// Returns `None` for a tap the game should handle itself. While the menu
    /// is open every tap closes it, because the only things a player can touch
    /// then are its entries and the scrim around it: an entry the game added
    /// comes back as `None` with the menu already shut, ready for the game to
    /// act on.
    pub fn read(&mut self, action: ActionId) -> Option<Tap> {
        let was_open = self.open;
        self.open = false;
        if action == action_id(MORE) {
            self.open = !was_open;
            return Some(Tap::Menu);
        }
        if action == action_id(UNDO) {
            return Some(Tap::Undo);
        }
        if action == action_id(RULES) {
            return Some(Tap::Rules);
        }
        if action == action_id(NEW_GAME) {
            return Some(Tap::NewGame);
        }
        if was_open && action == ActionId::BACK {
            return Some(Tap::Menu);
        }
        None
    }
}

impl ScreenBuilder {
    /// The top bar of a game screen: the title, Rules, and the menu.
    ///
    /// `more` is what the game adds to the menu after New game, such as
    /// "Restart puzzle" or "Use landscape". Replaces any top bar the screen
    /// already had, since a game screen has room for exactly these two.
    #[must_use]
    pub fn game_top_bar<I, N, L>(self, title: impl Into<String>, menu: Menu, more: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let items = std::iter::once((NEW_GAME.to_owned(), NEW_GAME_LABEL.to_owned())).chain(
            more.into_iter()
                .map(|(name, label)| (name.as_ref().to_owned(), label.into())),
        );
        self.top_bar(title)
            .top_bar_action(RULES, RULES_LABEL)
            .top_bar_overflow(MORE, menu.is_open(), items)
    }

    /// The bar at the foot of a game screen: Undo first, then up to two of the
    /// game's own verbs.
    ///
    /// `undo` is whether this game has an undo at all, not whether there is
    /// anything to undo right now; see [`NOTHING_TO_UNDO`]. A game with one
    /// verb and no undo gets a single pinned control, because a bar of one
    /// entry is not drawn.
    #[must_use]
    pub fn game_actions<I, N, L>(self, undo: bool, actions: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let mut entries = Vec::new();
        if undo {
            entries.push((UNDO.to_owned(), UNDO_LABEL.to_owned()));
        }
        entries.extend(
            actions
                .into_iter()
                .take(2)
                .map(|(name, label)| (name.as_ref().to_owned(), label.into())),
        );
        match entries.len() {
            0 => self,
            1 => {
                let (name, label) = entries.remove(0);
                self.bottom_action(name, label)
            }
            _ => self.action_bar(entries),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::Node;

    fn menu_labels(screen: &kobo_ui::Screen) -> Vec<String> {
        screen
            .overlay
            .as_ref()
            .map(|overlay| {
                overlay
                    .nodes
                    .iter()
                    .filter_map(|node| match node {
                        Node::Button { label, .. } => Some(label.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn every_game_top_bar_carries_rules_and_the_menu_in_the_same_order() {
        let screen = ScreenBuilder::new("game")
            .game_top_bar("Sudoku", Menu::default(), [("restart", "Restart puzzle")])
            .build();
        let bar = screen.top_bar.expect("a game screen has a top bar");
        assert_eq!(bar.title, "Sudoku");
        let labels: Vec<_> = bar.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, [RULES_LABEL, "More"]);
        assert_eq!(bar.actions[0].action, action_id(RULES));
        assert_eq!(bar.actions[1].action, action_id(MORE));
        assert!(
            screen.overlay.is_none(),
            "a closed menu draws only its dots"
        );
    }

    #[test]
    fn new_game_is_always_the_first_entry_in_an_open_menu() {
        let mut menu = Menu::default();
        assert_eq!(menu.read(action_id(MORE)), Some(Tap::Menu));
        let screen = ScreenBuilder::new("game")
            .game_top_bar("Parlor", menu, [("display", "Use landscape")])
            .build();
        assert_eq!(menu_labels(&screen), [NEW_GAME_LABEL, "Use landscape"]);
    }

    #[test]
    fn undo_comes_first_at_the_foot_and_a_lone_verb_is_pinned() {
        let screen = ScreenBuilder::new("game")
            .game_actions(
                true,
                [("notes", "Notes"), ("erase", "Erase"), ("x", "Dropped")],
            )
            .build();
        let bar = screen.nav_bar.expect("undo and two verbs make a bar");
        let labels: Vec<_> = bar.destinations.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, [UNDO_LABEL, "Notes", "Erase"]);

        let lone = ScreenBuilder::new("game")
            .game_actions(false, [("next", "Next question")])
            .build();
        assert!(lone.nav_bar.is_none());
        assert_eq!(
            lone.bottom_action.expect("one verb is pinned").action.label,
            "Next question"
        );

        let undo_only = ScreenBuilder::new("game")
            .game_actions(true, std::iter::empty::<(&str, &str)>())
            .build();
        assert_eq!(
            undo_only.bottom_action.expect("pinned").action.label,
            UNDO_LABEL
        );
    }

    #[test]
    fn a_tap_outside_the_open_menu_closes_it_instead_of_leaving_the_game() {
        let mut menu = Menu::default();
        assert_eq!(
            menu.read(ActionId::BACK),
            None,
            "closed: back is the game's"
        );
        menu.read(action_id(MORE));
        assert!(menu.is_open());
        assert_eq!(menu.read(ActionId::BACK), Some(Tap::Menu));
        assert!(!menu.is_open());
    }

    #[test]
    fn choosing_any_entry_shuts_the_menu_and_hands_the_game_its_own_entries() {
        let mut menu = Menu::default();
        menu.read(action_id(MORE));
        assert_eq!(menu.read(action_id(NEW_GAME)), Some(Tap::NewGame));
        assert!(!menu.is_open());

        menu.read(action_id(MORE));
        assert_eq!(menu.read(action_id("restart")), None);
        assert!(!menu.is_open());

        menu.read(action_id(MORE));
        assert_eq!(menu.read(action_id(MORE)), Some(Tap::Menu));
        assert!(!menu.is_open(), "the dots close what they opened");
        assert_eq!(menu.read(action_id(UNDO)), Some(Tap::Undo));
        assert_eq!(menu.read(action_id(RULES)), Some(Tap::Rules));
    }
}
