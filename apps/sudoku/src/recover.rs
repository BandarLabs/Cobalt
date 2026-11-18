//! A way past a saved record this version cannot read.
//!
//! Try again reads the same bytes and fails the same way every time, so on
//! its own the unreadable screen was a dead end. Start over first copies the
//! record to a key of its own, so taking the way out costs the owner nothing
//! a later version might still be able to read.
use kobo_sdk::{action_id, ActionId, Context, DialogAction, Glyph, Screen, ScreenBuilder};
use kobo_sdk::{DisplayMetrics, StoreResult};

pub const START_OVER: &str = "start-over";
pub const CONFIRM: &str = "confirm-start-over";
pub const CANCEL: &str = "cancel-start-over";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Idle,
    Asking,
    Listing,
    Copying(String),
}

/// What the owner is told while the copy is made and before it is asked for.
pub struct Wording<'a> {
    /// The screen's title, as on the failure itself.
    pub title: &'a str,
    /// The sentence saying the save is untouched, after the reason.
    pub kept: &'a str,
    /// What starting over does, on the confirmation.
    pub explain: &'a str,
}

#[derive(Debug, Default)]
pub struct SetAside {
    /// The record as it was read. Only a record that was read and not
    /// understood can be copied; a store that could not be read at all is
    /// left to Try again.
    bytes: Option<Vec<u8>>,
    stage: Stage,
    failed: bool,
}

/// The first copy key not already taken, so a second unreadable record
/// never writes over the copy of the first.
pub fn free_key(base: &str, keys: &[String]) -> String {
    // One more number than there are keys always leaves one free.
    (1..=keys.len() + 1)
        .map(|n| format!("{base}-unreadable-{n}"))
        .find(|key| !keys.contains(key))
        .expect("more numbers than keys")
}

impl SetAside {
    pub fn hold(&mut self, bytes: Vec<u8>) {
        *self = Self {
            bytes: Some(bytes),
            ..Self::default()
        };
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Listing | Stage::Copying(_))
    }
    /// Reads a tap on the unreadable screen, returning whether it was one of
    /// Start over's.
    pub fn action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if self.bytes.is_none() {
            return false;
        }
        match self.stage {
            Stage::Idle if action == action_id(START_OVER) => {
                self.stage = Stage::Asking;
                self.failed = false;
            }
            Stage::Asking if action == action_id(CANCEL) || action == ActionId::BACK => {
                self.stage = Stage::Idle;
            }
            Stage::Asking if action == action_id(CONFIRM) => {
                context.store().list();
                self.stage = Stage::Listing;
            }
            _ => return false,
        }
        true
    }
    /// The list of keys, which arrives through `on_store`. Returns whether it
    /// was the one Start over asked for.
    pub fn on_keys(&mut self, context: &mut Context, base: &str, result: StoreResult) -> bool {
        if self.stage != Stage::Listing {
            return false;
        }
        if let (StoreResult::Keys(keys), Some(bytes)) = (result, &self.bytes) {
            let key = free_key(base, &keys);
            context.store().save(key.clone(), bytes.clone());
            self.stage = Stage::Copying(key);
        } else {
            self.stage = Stage::Idle;
            self.failed = true;
        }
        true
    }
    /// The answer to the copy's write: `Some(true)` once it is kept and the
    /// game may start over, `Some(false)` when nothing was replaced.
    pub fn on_save(&mut self, key: &str, result: &StoreResult) -> Option<bool> {
        match &self.stage {
            Stage::Copying(copy) if copy == key => {}
            _ => return None,
        }
        if matches!(result, StoreResult::Saved { .. }) {
            self.clear();
            Some(true)
        } else {
            self.stage = Stage::Idle;
            self.failed = true;
            Some(false)
        }
    }
    /// The screen in place of the game while its record cannot be read.
    pub fn screen(
        &self,
        head: impl Fn() -> ScreenBuilder,
        metrics: &DisplayMetrics,
        error: &str,
        wording: &Wording<'_>,
    ) -> Screen {
        match self.stage {
            Stage::Asking => head()
                .owns_back(true)
                .confirmation(
                    "Start over?",
                    wording.explain,
                    DialogAction::new(CONFIRM, "Start over"),
                    DialogAction::new(CANCEL, "Go back"),
                )
                .build(),
            Stage::Listing | Stage::Copying(_) => head().text("Keeping a copy…").build(),
            Stage::Idle => {
                let (full, short) = if self.failed {
                    (
                        format!(
                            "A copy could not be made, so nothing was replaced. {}",
                            wording.kept
                        ),
                        "A copy could not be made.".to_owned(),
                    )
                } else {
                    (format!("{error} {}", wording.kept), error.to_owned())
                };
                let build = |glyph, summary: &str| {
                    let screen = head()
                        .splash(glyph, wording.title, summary)
                        .primary_button("retry-load", "Try again");
                    if self.bytes.is_some() {
                        screen.button(START_OVER, "Start over")
                    } else {
                        screen
                    }
                    .build()
                };
                // On a small panel at the largest sizes the mark gives way
                // first and then the longer sentence, so the reason and both
                // ways out are always on the panel together.
                let fitting = [(Some(Glyph::Close), &full), (None, &full), (None, &short)]
                    .into_iter()
                    .map(|(glyph, summary)| build(glyph, summary))
                    .find(|screen| {
                        !screen
                            .diagnostics(metrics, &kobo_sdk::Chrome::measuring(true))
                            .has_errors()
                    });
                fitting.unwrap_or_else(|| build(None, &short))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_takes_the_first_number_no_earlier_copy_holds() {
        assert_eq!(free_key("game", &[]), "game-unreadable-1");
        let keys = ["game".to_owned(), "game-unreadable-1".to_owned()];
        assert_eq!(free_key("game", &keys), "game-unreadable-2");
    }
}
