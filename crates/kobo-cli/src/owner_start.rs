//! Plain numbered entry point; choices reuse the existing command handlers.
use std::io::{BufRead, Write};

pub const COMPACT_HELP: &str = "Cobalt — tools for your reader

  kobo setup                 Set up a reader connected by USB
  kobo frame --help          Prepare and send photos
  kobo flashcards --help     Import flashcards and review progress
  kobo feeds --help          Check and send a feed subscription list
  kobo stream                Share a computer terminal with Paperterm

Run kobo in a terminal for guided choices.
Developer and release commands: kobo --help";

pub fn choose(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<Option<Vec<String>>, String> {
    writeln!(output, "Cobalt\n\n1. Set up a reader over USB\n2. Preview photos for Frame\n3. Check a feed subscription file\n4. Check a Paperterm connection\n5. Developer and release commands\n0. Exit").map_err(|e| e.to_string())?;
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
            _ => {
                writeln!(output, "Enter a number from 0 to 5.").map_err(|e| e.to_string())?;
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
