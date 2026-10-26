//! Original text/image export fixture using the real SDK and paired-receiver format.
use kobo_sdk::exports::{Export, Format};
use kobo_sdk::{action_id, ActionId, Context, KoboApp, StoreResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Copy {
    export: Option<Export>,
    started: bool,
}
impl Copy {
    fn show(&self, context: &mut Context) {
        if let Some(export) = &self.export {
            context.set_screen(export.screen());
        }
    }
}
impl KoboApp for Copy {
    fn on_start(&mut self, context: &mut Context) {
        let (title, format, bytes) = std::env::var_os("KOBO_EXPORT_IMAGE").map_or_else(
            || {
                (
                    "Garden notes",
                    Format::Text,
                    b"Water the mint after sunset.\n".to_vec(),
                )
            },
            |path| {
                (
                    "Garden sketch",
                    Format::Png,
                    std::fs::read(path).expect("original image fixture"),
                )
            },
        );
        self.export = Some(Export::new(title, format, bytes).expect("original export fixture"));
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("export-confirm") || action == action_id("export-retry") {
            if let Some(export) = &mut self.export {
                self.started |= export.begin(context);
            }
        }
        self.show(context);
    }
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = &mut self.export {
            export.on_shelf(context, name, &result);
        }
        self.show(context);
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(export) = &mut self.export {
            export.on_save(context, key, &result);
        }
        self.show(context);
    }
    fn can_suspend(&self) -> bool {
        !self.started || self.export.as_ref().is_some_and(Export::is_ready)
    }
}

fn source(executable: &Path) -> Result<kobo_sim::CaptureSource, Box<dyn std::error::Error>> {
    Ok(kobo_sim::CaptureSource {
        revision: std::env::var("KOBO_FIXTURE_REVISION").ok(),
        dirty: Some(true),
        binary_sha256: Some(kobo_net::sha256::hex_digest(&std::fs::read(executable)?)),
        fixture: Some("original-export-copy".into()),
        seed: Some(0),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("KOBO_SOCKET").is_some() {
        return Ok(kobo_sdk::run("todo", Copy::default())?);
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: export ADDRESS PRIVATE-SOCKET LAUNCHER-BINARY".into());
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
