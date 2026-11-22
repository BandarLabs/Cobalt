//! The lines that close password login for root in the reader's SSH server.
//!
//! Two things write them. `kobo setup` does, from a computer, once it has
//! proved its key opens the reader. The runtime does, on the reader, before it
//! puts the reader on a tailnet, when a key is already there to keep the owner
//! in. Each has to recognise a block the other left, so the words are kept in
//! one place: a marker spelt two ways would be a second block appended to a
//! file that already had one.

/// What a line is searched for to decide whether a block is present.
///
/// A prefix and not the whole line, because a block written by hand carries
/// whatever note its author left on the marker line.
pub const FAMILY: &str = "# cobalt-hardening:";

/// First line of the block.
pub const BEGIN: &str = "# cobalt-hardening: key-only SSH for root.";

/// Last line of the block, so taking one out by hand is a bounded range and
/// not a guess at how many lines it had.
pub const END: &str = "# cobalt-hardening: end.";

/// Who the block is about.
pub const WHO: &str = "Match User root";

/// No password login.
pub const PASSWORDS_OFF: &str = "PasswordAuthentication no";

/// No question-and-answer login either.
///
/// The server calls it keyboard-interactive, and where the server was built
/// with PAM it is a password by another name: [`PASSWORDS_OFF`] alone leaves
/// root's password, or its absence, working through it. A server built
/// without PAM has nothing behind the questions and loses nothing by the
/// line.
pub const QUESTIONS_OFF: &str = "KbdInteractiveAuthentication no";

/// The block as it is appended to the server's file, after a blank line.
///
/// `questions` leaves [`QUESTIONS_OFF`] out for a server too old to know the
/// word. That server still gets the first line, because half of this is not
/// a reason to leave the password door open.
#[must_use]
pub fn block(questions: bool) -> String {
    let mut lines = vec!["", BEGIN, WHO, PASSWORDS_OFF];
    if questions {
        lines.push(QUESTIONS_OFF);
    }
    lines.push(END);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// `settings` with [`QUESTIONS_OFF`] added to a block written before that
/// line existed, or `None` when there is no such block to add it to.
///
/// A block somebody wrote by hand without the closing marker is left exactly
/// as they wrote it, and so is one that already has the line.
#[must_use]
pub fn with_questions_off(settings: &str) -> Option<String> {
    let has = |wanted: &str| settings.lines().any(|line| line.starts_with(wanted));
    if !has(END) || has(QUESTIONS_OFF) {
        return None;
    }
    let mut upgraded = String::with_capacity(settings.len() + QUESTIONS_OFF.len() + 1);
    for line in settings.split_inclusive('\n') {
        if line.starts_with(END) {
            upgraded.push_str(QUESTIONS_OFF);
            upgraded.push('\n');
        }
        upgraded.push_str(line);
    }
    Some(upgraded)
}

#[cfg(test)]
mod tests {
    use super::{block, with_questions_off, BEGIN, END, FAMILY, QUESTIONS_OFF};

    #[test]
    fn both_markers_are_found_by_the_prefix_that_looks_for_a_block() {
        assert!(BEGIN.starts_with(FAMILY));
        assert!(END.starts_with(FAMILY));
    }

    #[test]
    fn the_block_is_set_off_by_a_blank_line_and_ends_with_a_line_break() {
        assert_eq!(
            block(true),
            "\n# cobalt-hardening: key-only SSH for root.\nMatch User root\n\
             PasswordAuthentication no\nKbdInteractiveAuthentication no\n\
             # cobalt-hardening: end.\n"
        );
        assert!(!block(false).contains(QUESTIONS_OFF));
    }

    #[test]
    fn an_earlier_block_gains_the_second_line_just_before_its_end() {
        let earlier = format!("Port 22\n{}", block(false));
        assert_eq!(
            with_questions_off(&earlier).as_deref(),
            Some(format!("Port 22\n{}", block(true)).as_str())
        );
    }

    #[test]
    fn a_block_that_is_whole_or_was_written_by_hand_is_left_alone() {
        assert_eq!(with_questions_off(&block(true)), None);
        assert_eq!(with_questions_off("Port 22\n"), None);
        let by_hand = "# cobalt-hardening: mine\nMatch User root\nPasswordAuthentication no\n";
        assert_eq!(with_questions_off(by_hand), None);
    }
}
