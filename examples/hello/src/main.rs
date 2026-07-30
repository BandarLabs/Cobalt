//! What `kobo new` writes into a new application.
//!
//! This file is the template itself, not a copy of it: `kobo-cli` includes
//! these bytes with `include_str!` and writes them out verbatim. It is a
//! workspace member so that `cargo build` compiles it and `cargo test` runs
//! it, which is the only way to be sure the first thing a newcomer does still
//! works. The template used to be a string constant checked by a test that
//! searched it for words. The words were all present on the day the SDK's
//! event enum grew two variants and every generated application stopped
//! compiling.
//!
//! Keep it small. It is read as an example of how these are written.

use kobo_sdk::prelude::*;
use std::process::ExitCode;

/// Applications are a struct holding what they know and a `KoboApp` impl
/// saying what they do when something happens.
#[derive(Default)]
struct Hello {
    battery: Option<String>,
}

impl Hello {
    /// Builds the screen and hands it over. Called after anything that
    /// changes what should be on it.
    ///
    /// Nothing is drawn here. A screen is a description, and the runtime
    /// decides when and how it reaches the panel.
    fn show(&self, context: &mut Context) {
        context.set_screen(
            ScreenBuilder::new("hello")
                .heading("Hello, Kobo")
                .text("Built with the Cobalt SDK.")
                .text(
                    self.battery
                        .clone()
                        .unwrap_or_else(|| "Battery: asking...".to_owned()),
                )
                .buttons([("refresh", "Refresh"), ("close", "Close")])
                .build(),
        );
    }
}

impl KoboApp for Hello {
    fn on_start(&mut self, context: &mut Context) {
        // Hardware is asked for, never touched directly. There is no path
        // from here to /dev or /sys, by design: the runtime owns the device
        // and gives it back. Every request gets exactly one answer in
        // `on_device_result`, including a refusal.
        context.device().read_battery();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("close") {
            context.exit();
        } else if action == action_id("refresh") {
            context.device().read_battery();
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if request != DeviceRequest::ReadBattery {
            return;
        }
        // A refusal is an answer and has to be shown. An application that
        // treats "no" as "nothing happened" leaves the reader waiting.
        self.battery = Some(match result {
            DeviceResult::Battery { percent, charging } => {
                let state = if charging { ", charging" } else { "" };
                format!("Battery: {percent}%{state}")
            }
            DeviceResult::Denied(reason) => format!("Battery unavailable: {reason}"),
            _ => "Battery: unexpected answer".to_owned(),
        });
        self.show(context);
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
    use super::Hello;
    use kobo_sdk::prelude::*;
    use kobo_sdk::CLARA_BW_METRICS;

    /// The screen a newcomer sees first has to fit the panel it is written
    /// for, or the first thing they learn is that the SDK produces something
    /// broken.
    #[test]
    fn the_first_screen_fits_a_clara() {
        let mut app = Hello::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let screen = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("on_start sets a screen");
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }

    /// The point of the example: a refused request still says something.
    #[test]
    fn a_refusal_is_shown_rather_than_swallowed() {
        let mut app = Hello::default();
        let mut context = Context::default();
        app.on_device_result(
            &mut context,
            DeviceRequest::ReadBattery,
            DeviceResult::Denied(DenyReason::Unsupported),
        );
        assert!(app
            .battery
            .as_deref()
            .is_some_and(|line| line.contains("unavailable")));
    }
}
