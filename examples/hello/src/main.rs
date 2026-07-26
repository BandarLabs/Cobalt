//! The smallest useful Kobo application.
//!
//! Everything here is deliberate. There is no window, no colour, no font, no
//! pixel measurement and no event loop, because none of those are an
//! application author's problem. What is left is the part that is.

use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

#[derive(Default)]
struct Hello {
    greeted: bool,
}

/// Builds the one screen this application has.
///
/// The body text deliberately does not change with the state, and the heading
/// carries the change instead.
///
/// Text above a control that grows by one wrapped line pushes that control
/// down. On a panel that takes a moment to refresh, the button then moves out
/// from under the finger that just tapped it, and the next tap lands on
/// nothing. This example got that wrong first: a greeted message one line
/// longer moved the button 27 pixels down, which is more than enough to miss.
///
/// Tuning wording to sit just inside the wrap point is not the fix, because the
/// wrap point moves with the font. Keeping the varying text out of the flow
/// above an action is.
fn screen(greeted: bool) -> Screen {
    ScreenBuilder::new("hello")
        .top_bar("Hello")
        .heading(if greeted {
            "Hello, Kobo"
        } else {
            "Ready when you are"
        })
        .text("Tap the button below.")
        .button("greet", if greeted { "Say it again" } else { "Say hello" })
        .build()
}

impl KoboApp for Hello {
    fn on_start(&mut self, context: &mut Context) {
        context.set_screen(screen(self.greeted));
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("greet") {
            self.greeted = true;
            context.set_screen(screen(self.greeted));
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("hello", Hello::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hello: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::screen;
    use kobo_sdk::action_id;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn button(greeted: bool) -> kobo_ui::Rect {
        screen(greeted)
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
            .rect_of_action(action_id("greet"))
            .expect("the greeting button is always present")
    }

    /// Tapping must not move the thing that was tapped.
    #[test]
    fn the_button_does_not_move_when_the_screen_changes() {
        assert_eq!(
            button(false),
            button(true),
            "the button moved between states, so a second tap can miss it"
        );
    }

    #[test]
    fn the_button_is_larger_than_a_finger() {
        let button = button(false);
        assert!(button.height >= CLARA_BW_METRICS.touch_target_minimum());
    }
}
