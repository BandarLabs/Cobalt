//! Original save/cancellation fixture, hosted beside the real local launcher.
use kobo_sdk::{ActionId, Context, KoboApp, ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Notes {
    phase: u8,
    wakes: u32,
    scheduled: u32,
    cancelled: u32,
    dirty: bool,
    status: String,
}
impl Notes {
    fn show(&self, context: &mut Context) {
        context.set_screen(
            ScreenBuilder::new("power-fixture")
                .top_bar("Garden notes")
                .heading("Save before sleep")
                .text("An original note and its receipt are saved in order.")
                .text(format!(
                    "Resumes: {}. Scheduled: {}. Cancelled tasks: {}.",
                    self.wakes, self.scheduled, self.cancelled
                ))
                .text(&self.status)
                .button("edit", "Edit note")
                .build(),
        );
    }
}
impl KoboApp for Notes {
    fn on_start(&mut self, context: &mut Context) {
        self.status = "Ready to edit".into();
        context.spawn(Task::Sleep { seconds: 300 });
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, _: ActionId) {
        self.dirty = true;
        self.status = "Note changed".into();
        self.show(context);
    }
    fn on_suspend(&mut self, context: &mut Context) {
        self.dirty = true;
        self.phase = 1;
        self.status = "Saving note".into();
        context
            .store()
            .save("note", b"Water the mint after sunset.".to_vec());
        self.show(context);
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        match (self.phase, key, result) {
            (1, "note", StoreResult::Saved { .. }) => {
                self.phase = 2;
                context.store().save("receipt", b"note saved".to_vec());
            }
            (2, "receipt", StoreResult::Saved { .. }) => {
                self.phase = 0;
                self.dirty = false;
                self.status = "Note and receipt saved".into();
            }
            (_, _, StoreResult::Denied(_)) => {
                self.phase = 0;
                self.status = "Note not saved. Try again.".into();
            }
            _ => {}
        }
        self.show(context);
    }
    fn can_suspend(&self) -> bool {
        !self.dirty
    }
    fn on_resume(&mut self, context: &mut Context) {
        self.wakes += 1;
        self.show(context);
    }
    fn on_scheduled_wake(&mut self, context: &mut Context) {
        self.scheduled += 1;
        self.show(context);
    }
    fn on_task(&mut self, context: &mut Context, _: TaskId, outcome: TaskOutcome) {
        if outcome == TaskOutcome::Cancelled {
            self.cancelled += 1;
        }
        self.show(context);
    }
}

fn source(executable: &Path) -> Result<kobo_sim::CaptureSource, Box<dyn std::error::Error>> {
    Ok(kobo_sim::CaptureSource {
        revision: std::env::var("KOBO_FIXTURE_REVISION").ok(),
        dirty: Some(true),
        binary_sha256: Some(kobo_net::sha256::hex_digest(&std::fs::read(executable)?)),
        fixture: Some("original-power-save-barrier".into()),
        seed: Some(0),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("KOBO_SOCKET").is_some() {
        return Ok(kobo_sdk::run("todo", Notes::default())?);
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: power ADDRESS PRIVATE-SOCKET LAUNCHER-BINARY".into());
    }
    let executable = std::env::current_exe()?;
    let launcher = PathBuf::from(&args[2]);
    let programs = BTreeMap::from([
        (
            "launcher".into(),
            kobo_sim::runtime::Program {
                source: source(&launcher)?,
                executable: launcher,
            },
        ),
        (
            "todo".into(),
            kobo_sim::runtime::Program {
                source: source(&executable)?,
                executable,
            },
        ),
    ]);
    let socket = PathBuf::from(&args[1]);
    kobo_sim::runtime::run(
        kobo_sim::AppServer::bind(&args[0], &socket)?,
        &socket,
        &programs,
    )?;
    Ok(())
}
