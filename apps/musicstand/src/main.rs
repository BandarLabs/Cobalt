//! Music Stand: stable score pages, a setlist, and half-page turns.

use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::{process::ExitCode, time::Duration};

const STATE: &str = "musicstand-state";
const PAGES: &[&str] = &[
    "J. S. Bach\nCello Suite No. 1\nPrelude\n\nModerato\n\n| G  D  G  B | d  B  G  D |\n| G  D  G  B | d  B  G  D |\n\nThe score page is held still at reading scale.",
    "Prelude\n\n| e  d  B  G | D  G  B  d |\n| e  d  B  G | D  G  B  d |\n\nKeep the line in sight. A half-page turn brings the next half up before the page changes.",
    "Prelude\n\n| g  d  B  G | D  G  B  d |\n| e  d  B  G | D  G  B  d |\n\nFine",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Library,
    Stand,
    Setlist,
    About,
}
struct Stand {
    view: View,
    page: usize,
    top_half: bool,
    marked: bool,
    loaded: bool,
}
impl Default for Stand {
    fn default() -> Self {
        Self {
            view: View::Library,
            page: 0,
            top_half: true,
            marked: false,
            loaded: false,
        }
    }
}
impl Stand {
    fn save(&self, context: &mut Context) {
        context.store().save(
            STATE,
            format!(
                "{}|{}|{}",
                self.page,
                u8::from(self.top_half),
                u8::from(self.marked)
            )
            .into_bytes(),
        );
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen(self));
    }
    fn advance(&mut self) {
        if self.top_half {
            self.top_half = false;
        } else {
            self.top_half = true;
            self.page = (self.page + 1).min(PAGES.len() - 1);
        }
    }
    fn previous(&mut self) {
        if !self.top_half {
            self.top_half = true;
        } else if self.page > 0 {
            self.page -= 1;
            self.top_half = false;
        }
    }
}
fn screen(stand: &Stand) -> Screen {
    match stand.view {
        View::Library => ScreenBuilder::new("music-library").top_bar("Music Stand").heading("Library")
            .rows([("open", "Cello Suite No. 1", "J. S. Bach · 3 pages", Glyph::Note)])
            .button("setlist", "Evening rehearsal setlist").button("about", "Add scores").build(),
        View::Stand => {
            let half = if stand.top_half { "top half" } else { "bottom half" };
            ScreenBuilder::new("music-score").top_bar(format!("Cello Suite No. 1 · {} of {}", stand.page + 1, PAGES.len()))
                .secondary(format!("Half-page mode · {half} · {}", if stand.marked { "marked" } else { "unmarked" }))
                .text(PAGES[stand.page]).button("mark", if stand.marked { "Remove mark" } else { "Mark page corner" })
                .page_turns("previous", "next").reading_menu("library").build()
        }
        View::Setlist => ScreenBuilder::new("music-setlist").top_bar("Evening rehearsal").heading("Setlist")
            .rows([("open", "1. Cello Suite No. 1", "Start page 1 · crop: automatic", Glyph::Note)])
            .secondary("Setlists keep their order for rehearsal and gigs.").button("library", "Library").build(),
        View::About => ScreenBuilder::new("music-about").top_bar("Music Stand").heading("Transfer")
            .text("Add scores from the Cobalt desktop app. Pages are prepared automatically for clear E Ink reading.")
            .text("Only transfer scores you have the right to use. MuPDF is available under the AGPL-3.0 license.")
            .button("library", "Library").build(),
    }
}
impl KoboApp for Stand {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            if let Some(bytes) = value {
                if let Ok(text) = String::from_utf8(bytes) {
                    let v: Vec<_> = text.split('|').collect();
                    self.page = v
                        .first()
                        .and_then(|x| x.parse::<usize>().ok())
                        .unwrap_or(0)
                        .min(PAGES.len() - 1);
                    self.top_half = v.get(1) != Some(&"0");
                    self.marked = v.get(2) == Some(&"1");
                }
            }
            self.loaded = true;
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("open") {
            self.view = View::Stand;
            context.device().keep_awake(Duration::from_secs(14_400));
        } else if action == action_id("setlist") {
            self.view = View::Setlist;
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("library") {
            self.view = View::Library;
            context.device().allow_sleep();
        } else if action == action_id("next") {
            self.advance();
            self.save(context);
        } else if action == action_id("previous") {
            self.previous();
            self.save(context);
        } else if action == action_id("mark") {
            self.marked = !self.marked;
            self.save(context);
        }
        self.show(context);
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("musicstand", Stand::default()).map_or_else(
        |error| {
            eprintln!("musicstand: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn half_page_turn_requires_two_presses() {
        let mut stand = Stand::default();
        stand.advance();
        assert!(!stand.top_half);
        assert_eq!(stand.page, 0);
        stand.advance();
        assert!(stand.top_half);
        assert_eq!(stand.page, 1);
    }
    #[test]
    fn score_turn_zones_and_mark_fit_clara() {
        let stand = Stand {
            view: View::Stand,
            ..Stand::default()
        };
        let screen = screen(&stand);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.page_turns.declared().expect("page turns").next,
            action_id("next")
        );
        assert!(layout.rect_of_action(action_id("mark")).is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
