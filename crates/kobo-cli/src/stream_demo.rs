//! A connection check that never interprets input as a shell command.

use std::io::{BufRead, Write};

pub fn run() -> Result<(), String> {
    conversation(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
        .map_err(|error| format!("Paperterm connection check: {error}"))
}

fn conversation(input: &mut impl BufRead, output: &mut impl Write) -> std::io::Result<()> {
    writeln!(output, "Paperterm connection check")?;
    writeln!(
        output,
        "Type here or on your reader.\nPress Enter to send."
    )?;
    writeln!(
        output,
        "Both screens show the same session.\nType exit to finish."
    )?;
    loop {
        write!(output, "> ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.eq_ignore_ascii_case("exit") {
            break;
        }
        let message: String = line
            .chars()
            .filter(|character| !character.is_control())
            .take(120)
            .collect();
        if !message.is_empty() {
            writeln!(output, "Received: {message}")?;
        }
    }
    writeln!(output, "Connection check finished.")?;
    writeln!(output, "This screen stays open for one minute.")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::conversation;

    #[test]
    fn messages_are_text_and_never_commands() {
        let mut output = Vec::new();
        conversation(
            &mut &b"hello from reader\n$(touch unwanted)\nexit\nnot read\n"[..],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Received: hello from reader"));
        assert!(text.contains("Received: $(touch unwanted)"));
        assert!(!text.contains("not read"));
        assert!(text.contains("Connection check finished."));
    }

    #[test]
    fn input_ending_closes_the_check() {
        let mut output = Vec::new();
        conversation(&mut &b""[..], &mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("finished"));
    }
}
