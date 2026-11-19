//! Offline owner setup guides from the same metadata used by the Store.
use std::fmt::Write;

pub const HELP: &str = "App setup guides

  kobo apps                    List bundled apps
  kobo apps search WORD        Find an app by name or purpose
  kobo apps setup APP          Show its setup steps and capabilities
  kobo apps connect APP        Show the safest connection path

APP is the app's name as the Store shows it (Music Stand, Feeds), its short
label, its id, or the kobo command that serves it (sync, stream).

Examples:
  kobo apps search chess
  kobo apps setup lichess
  kobo apps setup music stand

Guides describe this CLI's bundled catalog, not what is installed on a reader.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        || arguments.first().is_some_and(|first| first == "help")
    {
        println!("{HELP}");
        return Ok(());
    }
    let apps = kobo_catalog::bundled()?;
    match arguments {
        [] => print_list(&apps, ""),
        [verb, query @ ..] if verb == "search" && !query.is_empty() => {
            print_list(&apps, &query.join(" "));
        }
        [verb, name @ ..] if matches!(verb.as_str(), "setup" | "connect") && !name.is_empty() => {
            // Joined, so `kobo apps setup music stand` works unquoted. The
            // explanation goes to stderr above a short usage line, so the
            // owner reads the suggestion rather than "usage:" in front of it.
            let app = find(&apps, &name.join(" ")).map_err(|why| {
                eprintln!("{why}");
                crate::console::usage(format!("kobo apps {verb} APP"))
            })?;
            if verb == "connect" {
                print!("{}", connection_card(app));
            } else {
                print!("{}", card(app));
            }
        }
        _ => {
            return Err(crate::console::usage(
                "kobo apps [search WORD | setup APP | connect APP]",
            ))
        }
    }
    Ok(())
}

/// Companion commands whose name is not the name of the app they serve.
///
/// `kobo sync` serves Sync, whose id is `syncthing`; `kobo stream` serves
/// Paperterm; `kobo feeds` serves Feeds, whose id is `rss`. An owner who has
/// just typed the command reaches for the same word to read its setup.
const COMMANDS: &[(&str, &str)] = &[
    ("feeds", "rss"),
    ("sync", "syncthing"),
    ("stream", "paperterm"),
    ("library", "calibre-web"),
    ("digest", "rss-miniflux"),
    ("stacks", "zotero-reader"),
];

/// A name with case, spaces and punctuation taken out, so "Music Stand",
/// "music-stand" and "musicstand" are one name.
fn squeeze(word: &str) -> String {
    word.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// The app an owner means by `name`.
///
/// Only the id used to match, so `kobo apps setup feeds`, `sync` and
/// `library` all failed although each is the name the Store shows. The
/// order is most exact first: the id, the Store name, the short label, then
/// the companion command. A name two apps share is listed rather than
/// guessed at, and a miss suggests the closest names there are.
fn find<'a>(apps: &'a [kobo_catalog::App], name: &str) -> Result<&'a kobo_catalog::App, String> {
    let wanted = squeeze(name);
    let tiers: [&dyn Fn(&kobo_catalog::App) -> bool; 4] = [
        &|app| squeeze(&app.id) == wanted,
        &|app| squeeze(&app.title) == wanted,
        &|app| squeeze(&app.label) == wanted,
        &|app| {
            COMMANDS
                .iter()
                .any(|(command, id)| *command == wanted && app.id == *id)
        },
    ];
    for matches in tiers {
        let found: Vec<&kobo_catalog::App> = apps.iter().filter(|app| matches(app)).collect();
        match found.as_slice() {
            [] => {}
            [only] => return Ok(only),
            several => {
                return Err(format!(
                    "{name:?} is the name of more than one app: {}. Name one by its id.",
                    several
                        .iter()
                        .map(|app| format!("{} ({})", plain(&app.title), app.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    let near = suggestions(apps, &wanted);
    if near.is_empty() {
        return Err(format!(
            "No bundled app is called {name:?}. Find it with kobo apps search WORD."
        ));
    }
    Err(format!(
        "No bundled app is called {name:?}. Did you mean {}?",
        near.join(" or ")
    ))
}

/// The apps whose names are close to a miss, as `Title (kobo apps setup id)`.
///
/// Close means one name contains the other, or they are a couple of typing
/// mistakes apart, which covers a plural, a dropped letter and a swapped pair.
fn suggestions(apps: &[kobo_catalog::App], wanted: &str) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let close = |candidate: &str| {
        let candidate = squeeze(candidate);
        (wanted.len() >= 3 && (candidate.contains(wanted) || wanted.contains(&candidate)))
            || distance(&candidate, wanted) <= 2
    };
    apps.iter()
        .filter(|app| close(&app.id) || close(&app.title) || close(&app.label))
        .take(3)
        .map(|app| format!("{} (kobo apps setup {})", plain(&app.title), app.id))
        .collect()
}

/// The number of single-letter edits between two names.
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left != *right);
            current.push(
                substitution
                    .min(previous[column + 1] + 1)
                    .min(current[column] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
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
    kobo_policy::Capability::parse(value).map_or_else(
        || format!("Additional capability: {}", plain(value)),
        |capability| capability.plain_wording().to_owned(),
    )
}

fn connection_card(app: &kobo_catalog::App) -> String {
    let mut output = format!(
        "Connect {}
{}
",
        plain(&app.title),
        plain(&app.summary)
    );
    let setup = app.setup.as_ref();
    let steps = setup
        .and_then(|value| value.get("steps"))
        .and_then(kobo_json::Value::as_array);
    let mut found = false;
    if let Some(steps) = steps {
        for step in steps {
            let link = step
                .get("link")
                .and_then(|link| link.get("url"))
                .and_then(kobo_json::Value::as_str);
            let command = step.get("command").and_then(kobo_json::Value::as_str);
            if let Some(url) = link {
                writeln!(
                    output,
                    "
Open in your browser:
  {}",
                    plain(url)
                )
                .expect("string");
                found = true;
            }
            if let Some(command) = command {
                let command = plain(command);
                writeln!(
                    output,
                    "
Then run:
  {command}"
                )
                .expect("string");
                if command.contains("secret set") {
                    output.push_str(
                        "  The secret comes from a private file and is never shown here.
",
                    );
                }
                found = true;
            }
        }
    }
    if !found {
        output.push_str(
            "
No computer-side sign-in is declared. Open the app and follow its on-reader connection steps.
",
        );
    }
    output.push_str(
        "
Full setup: kobo apps setup ",
    );
    output.push_str(&plain(&app.id));
    output.push('\n');
    output
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
    fn connect_card_uses_browser_link_or_labeled_private_file_command() {
        let apps = kobo_catalog::bundled().unwrap();
        let lichess = connection_card(apps.iter().find(|app| app.id == "lichess").unwrap());
        assert!(lichess.contains("Open in your browser"));
        assert!(lichess.contains("board:play"));
        assert!(lichess.contains("private file"));
        assert!(!lichess.contains("actual token"));
        let mut plain = apps.iter().find(|app| app.id == "panels").unwrap().clone();
        plain.setup = None;
        assert!(connection_card(&plain).contains("on-reader connection steps"));
    }

    #[test]
    fn lichess_guide_uses_catalog_steps_and_explains_wake_permissions() {
        let apps = kobo_catalog::bundled().unwrap();
        let guide = card(apps.iter().find(|app| app.id == "lichess").unwrap());
        assert!(guide.contains("board:play"));
        assert!(guide.contains("kobo secret set lichess --from token.txt"));
        assert!(guide.contains("Can keep the reader awake"));
        assert!(guide.contains("does not check installation"));
    }

    #[test]
    fn an_app_is_found_by_the_name_the_store_shows_its_label_and_its_command() {
        let apps = kobo_catalog::bundled().unwrap();
        for (typed, id) in [
            ("feeds", "rss"),
            ("Feeds", "rss"),
            ("sync", "syncthing"),
            ("library", "calibre-web"),
            ("Music Stand", "musicstand"),
            ("music-stand", "musicstand"),
            ("music", "musicstand"),
            ("home panel", "homepanel"),
            ("Read Later", "readlater"),
            ("stream", "paperterm"),
            ("LICHESS", "lichess"),
            ("digest", "rss-miniflux"),
        ] {
            assert_eq!(
                find(&apps, typed).map(|app| app.id.as_str()),
                Ok(id),
                "{typed}"
            );
        }
    }

    #[test]
    fn a_near_miss_suggests_the_app_it_was_close_to() {
        let apps = kobo_catalog::bundled().unwrap();
        let error = find(&apps, "lichss").unwrap_err();
        assert!(
            error.contains("Did you mean Lichess (kobo apps setup lichess)"),
            "{error}"
        );
        let error = find(&apps, "musicstands").unwrap_err();
        assert!(error.contains("kobo apps setup musicstand"), "{error}");
        let error = find(&apps, "zzzzzz").unwrap_err();
        assert!(error.contains("kobo apps search WORD"), "{error}");
    }

    #[test]
    fn an_unknown_name_is_a_usage_mistake_for_scripts() {
        let error = command(&["setup".to_owned(), "lichss".to_owned()]).unwrap_err();
        assert_eq!(
            crate::console::category_of(&error),
            crate::console::EXIT_USAGE
        );
    }

    #[test]
    fn help_is_answered_after_any_verb() {
        command(&["setup".to_owned(), "--help".to_owned()]).expect("help");
        command(&["--help".to_owned()]).expect("help");
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
