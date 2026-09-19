//! Plain numbered entry point; choices reuse the existing command handlers.
use std::io::{BufRead, Write};

pub const COMPACT_HELP: &str = "Cobalt - tools for your reader

  kobo setup
    Set up a reader connected by USB
  kobo apps
    Find apps and read setup guides
  kobo frame --help
    Prepare and send photos
  kobo flashcards --help
    Import cards and review progress
  kobo feeds --help
    Check and send feed subscriptions
  kobo stream
    Share a terminal with Paperterm

Run kobo in a terminal for guided choices.
Developer and release commands: kobo --help";

pub fn choose(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<Vec<String>>, String> {
    writeln!(output, "Cobalt\n\n1. Set up a reader over USB\n2. Preview photos for Frame\n3. Check a feed subscription file\n4. Check a Paperterm connection\n5. Developer and release commands\n6. App setup guides\n0. Exit").map_err(|e| e.to_string())?;
    loop {
        let Some(choice) = answer(input, output, "Choose a number: ")? else {
            return Ok(None);
        };
        let command: Vec<String> = match choice.as_str() {
            "0" => return Ok(None),
            "1" => {
                writeln!(output, "Connect the reader by USB and choose Connect on its screen.\nSetup will identify the reader and explain the installation before its approval step.").map_err(|e| e.to_string())?;
                vec!["setup".into()]
            }
            "2" => {
                let Some(source) = answer(input, output, "Photo or folder path (blank cancels): ")?
                else {
                    return Ok(None);
                };
                let Some(dest) =
                    answer(input, output, "New preview folder path (blank cancels): ")?
                else {
                    return Ok(None);
                };
                vec![
                    "frame".into(),
                    "preview".into(),
                    source,
                    "--out".into(),
                    dest,
                ]
            }
            "3" => {
                let Some(source) = answer(input, output, "OPML file path (blank cancels): ")?
                else {
                    return Ok(None);
                };
                vec!["feeds".into(), "check".into(), source]
            }
            "4" => vec!["stream".into(), "demo".into()],
            "5" => vec!["--help".into()],
            "6" => {
                let apps = kobo_catalog::bundled()?;
                let titles: Vec<String> = apps.iter().map(|app| app.title.clone()).collect();
                let Some(index) = crate::console::choose_numbered(
                    input,
                    output,
                    &titles,
                    "App number (blank cancels): ",
                )?
                else {
                    return Ok(None);
                };
                vec!["apps".into(), "setup".into(), apps[index].id.clone()]
            }
            _ => {
                writeln!(output, "Enter a number from 0 to 6.").map_err(|e| e.to_string())?;
                continue;
            }
        };
        return Ok(Some(command));
    }
}

fn answer(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
) -> Result<Option<String>, String> {
    write!(output, "{prompt}")
        .and_then(|()| output.flush())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return Ok(None);
    }
    if line.chars().any(char::is_control) {
        return Err("Use a path without control characters.".into());
    }
    Ok(Some(line.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_choice_dispatches_to_a_real_command() {
        // The guided surface and the typed commands are the same operations:
        // each choice produces argv whose head is a command the binary
        // actually has, so the menu can never offer a path that does not
        // exist outside it.
        let known = ["setup", "frame", "feeds", "stream", "--help", "apps"];
        for answers in ["1\n", "2\n/x\n/y\n", "3\n/x.opml\n", "4\n", "5\n", "6\n1\n"] {
            let mut input = std::io::BufReader::new(answers.as_bytes());
            let mut output = Vec::new();
            let command = choose(&mut input, &mut output)
                .expect("a choice")
                .expect("a command");
            assert!(known.contains(&command[0].as_str()), "{command:?}");
        }
    }

    #[test]
    fn preview_paths_stay_literal_and_cancellation_does_nothing() {
        let mut output = Vec::new();
        let result = choose(
            &mut &b"2\n/photos with spaces/$(touch nope)\n/new preview\n"[..],
            &mut output,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result,
            [
                "frame",
                "preview",
                "/photos with spaces/$(touch nope)",
                "--out",
                "/new preview"
            ]
        );
        for input in ["", "0\n", "2\n\n", "2\n/photo\n\n"] {
            assert!(choose(&mut input.as_bytes(), &mut Vec::new())
                .unwrap()
                .is_none());
        }
    }
    #[test]
    fn app_menu_retries_invalid_numbers_and_cancels_without_dispatch() {
        let apps = kobo_catalog::bundled().unwrap();
        let index = apps.iter().position(|app| app.id == "lichess").unwrap() + 1;
        let input = format!("6\n0\n999999\n{index}\n");
        let mut output = Vec::new();
        assert_eq!(
            choose(&mut input.as_bytes(), &mut output).unwrap().unwrap(),
            ["apps", "setup", "lichess"]
        );
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Enter a number from 1 to"));
        for input in ["6\n", "6\n\n"] {
            assert!(choose(&mut input.as_bytes(), &mut Vec::new())
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn invalid_choices_retry_and_developer_help_is_explicit() {
        let mut output = Vec::new();
        assert_eq!(
            choose(&mut &b"99\n5\n"[..], &mut output).unwrap().unwrap(),
            ["--help"]
        );
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Enter a number"));
        assert_eq!(
            choose(&mut &b"3\n/feeds.opml\n"[..], &mut Vec::new())
                .unwrap()
                .unwrap(),
            ["feeds", "check", "/feeds.opml"]
        );
    }
}
