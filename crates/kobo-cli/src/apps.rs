//! Offline owner setup guides from the same metadata used by the Store.
use std::fmt::Write;

pub const HELP: &str = "App setup guides

  kobo apps                    List bundled apps
  kobo apps search WORD        Find an app by name or purpose
  kobo apps setup APP           Show its setup steps and capabilities

Examples:
  kobo apps search chess
  kobo apps setup lichess

Guides describe this CLI's bundled catalog, not what is installed on a reader.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    let apps = kobo_catalog::bundled()?;
    match arguments {
        [] => print_list(&apps, ""),
        [help] if matches!(help.as_str(), "--help" | "-h" | "help") => println!("{HELP}"),
        [verb, query] if verb == "search" => print_list(&apps, query),
        [verb, id] if verb == "setup" => {
            let app = apps
                .iter()
                .find(|app| app.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| {
                    format!("No bundled app named {id:?}. Find it with kobo apps search WORD.")
                })?;
            print!("{}", card(app));
        }
        _ => return Err(HELP.to_owned()),
    }
    Ok(())
}

fn print_list(apps: &[kobo_catalog::App], query: &str) {
    let query = query.to_lowercase();
    let matches: Vec<_> = apps
        .iter()
        .filter(|app| {
            format!("{} {} {}", app.id, app.title, app.summary)
                .to_lowercase()
                .contains(&query)
        })
        .collect();
    if matches.is_empty() {
        println!("No matching apps. Run kobo apps to see the bundled catalog.");
        return;
    }
    for app in matches {
        println!(
            "{} ({})\n  {}\n",
            plain(&app.title),
            app.id,
            plain(&app.summary)
        );
    }
    println!("Setup instructions: kobo apps setup APP");
}

fn plain(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn capability(value: &str) -> String {
    match value {
        "network" => "Connects to online services".into(),
        "hold-wifi" => "Can keep Wi-Fi connected while in use".into(),
        "keep-awake" => "Can keep the reader awake while in use".into(),
        "frontlight-control" => "Can adjust the frontlight".into(),
        "scheduled-wake" => "Can request scheduled wake-ups where supported".into(),
        other => format!("Additional capability: {}", plain(other)),
    }
}

fn card(app: &kobo_catalog::App) -> String {
    let mut output = format!(
        "{}\n{}\n\nBundled guide: version {}\n",
        plain(&app.title),
        plain(&app.summary),
        plain(&app.version)
    );
    if app.capabilities.is_empty() {
        output.push_str("No special capabilities requested.\n");
    } else {
        output.push_str("\nWhat the app can use:\n");
        for value in &app.capabilities {
            writeln!(output, "  - {}", capability(value)).expect("write to string");
        }
    }
    output.push_str("\nSetup:\n");
    let steps = app
        .setup
        .as_ref()
        .and_then(|value| value.get("steps"))
        .and_then(kobo_json::Value::as_array);
    if let Some(steps) = steps.filter(|steps| !steps.is_empty()) {
        for (index, step) in steps.iter().enumerate() {
            if let Some(text) = step.get("text").and_then(kobo_json::Value::as_str) {
                writeln!(output, "{}. {}", index + 1, plain(text)).expect("write to string");
            }
            if let Some(link) = step.get("link") {
                if let Some(url) = link.get("url").and_then(kobo_json::Value::as_str) {
                    writeln!(output, "   {}", plain(url)).expect("write to string");
                }
            }
            if let Some(command) = step.get("command").and_then(kobo_json::Value::as_str) {
                writeln!(output, "   {command}", command = plain(command))
                    .expect("write to string");
            }
        }
    } else {
        output.push_str("Install the app from Cobalt's Store, then open it on your reader.\nNo additional setup steps are listed in its manifest.\n");
    }
    output.push_str("\nThis guide does not check installation or account access.\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lichess_guide_uses_catalog_steps_and_explains_wake_permissions() {
        let apps = kobo_catalog::bundled().unwrap();
        let guide = card(apps.iter().find(|app| app.id == "lichess").unwrap());
        assert!(guide.contains("board:play"));
        assert!(guide.contains("kobo secret set lichess --from <token-file> --device <address>"));
        assert!(guide.contains("Can keep the reader awake"));
        assert!(guide.contains("does not check installation"));
    }

    #[test]
    fn plain_guides_escape_terminal_controls_and_cover_apps_without_setup() {
        let mut app = kobo_catalog::bundled().unwrap().remove(0);
        app.title = "Example\u{1b}[2J".into();
        app.capabilities.clear();
        app.setup = None;
        let guide = card(&app);
        assert!(!guide.contains('\u{1b}'));
        assert!(guide.contains("No special capabilities"));
        assert!(guide.contains("No additional setup steps"));
    }
}
