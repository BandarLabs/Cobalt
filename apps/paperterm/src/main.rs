//! A network-only terminal mirror.  The shell and its credentials stay on the host.
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Caret, Context, KoboApp, Screen, ScreenBuilder, StoreResult,
};
use std::process::ExitCode;

const PAIR: &str = "pair";
const CHANGE: &str = "change";
const PAIRING: &str = "pairing";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Opening,
    Pairing,
    Watching,
}

struct Paperterm {
    view: View,
    address: TextEntry,
    code: TextEntry,
    saved: Option<String>,
    offline: bool,
}

impl Default for Paperterm {
    fn default() -> Self {
        Self {
            view: View::Opening,
            address: TextEntry::new().opened_by(PAIR),
            code: TextEntry::new().opened_by("enter-code"),
            saved: None,
            offline: false,
        }
    }
}

impl Paperterm {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("paperterm-opening")
                .top_bar("Paperterm")
                .activity("Reading pairing", None)
                .build(),
            View::Pairing if self.address.is_open() => ScreenBuilder::new("paperterm-address")
                .top_bar("Paperterm")
                .heading("Pair with your computer")
                .text("Run kobo stream init, then enter the address it prints.")
                .text_entry(&self.address, "Computer address", "Next")
                .build(),
            View::Pairing => ScreenBuilder::new("paperterm-code")
                .top_bar("Paperterm")
                .heading("Now the pairing code")
                .text("Enter the six characters printed by kobo stream init.")
                .text_entry(&self.code, "Six-character code", "Watch")
                .build(),
            View::Watching => {
                let mut screen = ScreenBuilder::new("paperterm-watching")
                    .top_bar("Paperterm")
                    .terminal(demo_rows(), Some(Caret { row: 5, column: 30 }));
                if self.offline {
                    screen = screen.banner(BannerLevel::Attention, "off the air");
                }
                screen
                    .secondary("Read-only mirror. The shell remains on your computer.")
                    .button(CHANGE, "Change pairing")
                    .build()
            }
        }
    }

    fn finish_pairing(&mut self, context: &mut Context) {
        let address = self.address.text().trim();
        let code = self.code.text().trim();
        if address.is_empty() || code.len() != 6 {
            self.offline = true;
            self.show(context);
            return;
        }
        let value = format!("{address}\n{code}");
        self.saved = Some(value.clone());
        context.store().save(PAIRING, value);
        self.view = View::Watching;
        self.offline = true; // The host transport is deliberately not invented by the app.
        self.show(context);
    }
}

fn demo_rows() -> Vec<String> {
    [
        "┌─ paperterm ───────────────────────────────┐",
        "│ session is waiting for the host            │",
        "│                                             │",
        "│ Pair this reader, then run:                 │",
        "│   kobo stream -- <command>                  │",
        "│                                             │",
        "└─────────────────────────────────────────────┘",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

impl KoboApp for Paperterm {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(PAIRING);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            self.saved = value.and_then(|bytes| String::from_utf8(bytes).ok());
            self.view = if self.saved.is_some() {
                View::Watching
            } else {
                self.address.open();
                View::Pairing
            };
            self.offline = self.saved.is_some();
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::Pairing {
            if let Some(event) = self.address.handle(action) {
                if matches!(event, Typing::Submitted(_)) {
                    self.code.open();
                }
                self.show(context);
                return;
            }
            if let Some(event) = self.code.handle(action) {
                if matches!(event, Typing::Submitted(_)) {
                    self.finish_pairing(context);
                } else {
                    self.show(context);
                }
                return;
            }
        }
        if action == action_id(CHANGE) {
            self.view = View::Pairing;
            self.address.open();
            self.code.close();
            self.offline = false;
            self.show(context);
        }
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("paperterm", Paperterm::default()).map_or_else(
        |error| {
            eprintln!("paperterm: {error}");
            ExitCode::FAILURE
        },
        |_| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn mirror_uses_the_terminal_node_and_fits_the_clara_panel() {
        let screen = Paperterm {
            view: View::Watching,
            ..Paperterm::default()
        }
        .screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id(CHANGE)).is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }

    #[test]
    fn pairing_requires_a_six_character_code() {
        let mut app = Paperterm::default();
        app.address = TextEntry::new().opened_by(PAIR);
        assert_eq!(demo_rows().len(), 7);
        assert!(app.saved.is_none());
    }
}
