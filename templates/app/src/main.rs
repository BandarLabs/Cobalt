use kobo_sdk::prelude::*;
use std::process::ExitCode;

#[derive(Default)]
struct Example;

impl KoboApp for Example {
    fn on_start(&mut self, context: &mut Context) {
        context.set_screen(
            ScreenBuilder::new("example")
                .heading("Example")
                .text("Built with the Cobalt SDK.")
                .build(),
        );
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("example", Example).map_or_else(
        |error| {
            eprintln!("example: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}
