//! Why a job failed, for somebody who has never opened a terminal.
//!
//! The commands a job runs were written to be read in a terminal, and the
//! sentence one stops on can name a file on this computer, a command to run
//! next, or an error by its number. All three are right there and wrong in a
//! window, whose owner is not going to run anything and should not need to
//! know where this program keeps its files.
//!
//! So the failures an owner can actually walk into are recognised here and
//! said again in the window's own terms: what happened, and what to do about
//! it with the reader or the window. It is done once, in the server, so the
//! Mac app and the page cannot come to say different things about the same
//! failure. Nothing is taken away: the command's own sentence is still in
//! what the job said, which both front ends keep one click from the reason.
//!
//! Anything not recognised has no reason here, and the front ends show the
//! command's sentence as they always have. A sentence is added to this file
//! only when it is truer for the owner than the one it replaces.

use super::jobs::Kind;

/// The owner's reason for a failed job, from everything the job said.
///
/// Only the sentence the command stopped on is read: the last line it wrote
/// in its own name, with the indented lines under it, which is how a
/// sentence too long for one line is carried on.
pub fn reason(kind: Kind, said: &str) -> Option<String> {
    let lines: Vec<&str> = said.lines().collect();
    let last = lines.iter().rposition(|line| line.starts_with("kobo: "))?;
    let mut stopped_on = lines[last]["kobo: ".len()..].trim().to_owned();
    for more in lines[last + 1..]
        .iter()
        .take_while(|line| line.starts_with([' ', '\t']) && !line.trim().is_empty())
    {
        stopped_on.push(' ');
        stopped_on.push_str(more.trim());
    }
    match kind {
        Kind::Reader => writing_to_the_reader(&stopped_on),
        Kind::Send => sending(&stopped_on),
    }
    .map(str::to_owned)
}

/// Failures of a job that writes to a reader plugged in over USB.
fn writing_to_the_reader(stopped_on: &str) -> Option<&'static str> {
    // The release this computer holds is checked against its signed list
    // before a byte of it goes to the reader, so these three mean the copy
    // here is bad and the reader has not been touched.
    if stopped_on.contains(" is truncated: manifest says ")
        || stopped_on.contains(" checksum failed: expected ")
        || stopped_on.starts_with("refusing prebuilt package")
    {
        return Some(
            "The copy of Cobalt kept on this computer is damaged, so nothing was put on the \
             reader.",
        );
    }
    match os_error(stopped_on)? {
        Trouble::Full => Some(
            "The reader ran out of room part way through. Remove some books from it and try \
             again.",
        ),
        Trouble::ReadOnly => Some(
            "The reader's storage can only be read at the moment, so nothing could be written \
             to it. Eject it, unplug it, plug it in again and tap Connect.",
        ),
        Trouble::NotAllowed if cfg!(target_os = "macos") => Some(
            "macOS did not allow writing to the reader. Allow it in System Settings, under \
             Privacy & Security, Files and Folders, and try again.",
        ),
        Trouble::NotAllowed => Some("This computer did not allow writing to the reader."),
    }
}

/// Failures of a job that sends something to a reader over Wi-Fi.
fn sending(stopped_on: &str) -> Option<&'static str> {
    if stopped_on.starts_with("No reader saved.") {
        return Some(
            "No reader has been set up from this computer yet. Plug one in and set it up \
             under Reader.",
        );
    }
    // The four ways the command says a reader stayed silent, with or
    // without its name and with or without having waited for it.
    if stopped_on.contains(" did not answer within ")
        || stopped_on.contains(" is not answering.")
        || stopped_on.starts_with("No reader answered within ")
        || stopped_on.starts_with("No reader is answering.")
    {
        return Some(
            "The reader did not answer. Wake it with its power button and check that it is on \
             the same Wi-Fi as this computer.",
        );
    }
    None
}

/// What the system refused, of the refusals an owner can do something about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Trouble {
    Full,
    ReadOnly,
    NotAllowed,
}

/// The system's refusal named in a sentence, when it is one of those.
///
/// The standard library ends such a sentence with the system's own number
/// for the error, which is the one part of it that does not change with the
/// language the system speaks. The numbers are each system's own. A reader
/// that has gone has numbers too, and they are left alone: a front end knows
/// a reader has gone by looking for it, which is surer than a number.
fn os_error(sentence: &str) -> Option<Trouble> {
    let after = sentence.rsplit_once("(os error ")?.1;
    let number: i32 = after.split(')').next()?.parse().ok()?;
    if cfg!(windows) {
        match number {
            // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
            39 | 112 => Some(Trouble::Full),
            // ERROR_WRITE_PROTECT
            19 => Some(Trouble::ReadOnly),
            // ERROR_ACCESS_DENIED
            5 => Some(Trouble::NotAllowed),
            _ => None,
        }
    } else {
        match number {
            // ENOSPC
            28 => Some(Trouble::Full),
            // EROFS
            30 => Some(Trouble::ReadOnly),
            // EPERM, EACCES
            1 | 13 => Some(Trouble::NotAllowed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a failed job said, ending on `sentence` the way a command ends.
    fn ended_on(sentence: &str) -> String {
        format!("found Kobo Clara BW at /Volumes/KOBOeReader\n\nkobo: {sentence}\n")
    }

    #[test]
    fn a_damaged_release_is_said_without_the_file_it_is_in() {
        for sentence in [
            "/Users/owner/.local/share/kobo/releases/0.3.37-stable/cobalt-0.3.37-KoboRoot.tgz is truncated: manifest says 27699108 bytes, found 9000000",
            "/home/owner/.local/share/kobo/releases/0.3.37-stable/cobalt-0.3.37-KoboRoot.tgz checksum failed: expected aa, found bb",
            "refusing prebuilt package: unexpected member",
        ] {
            let reason = reason(Kind::Reader, &ended_on(sentence)).expect("a reason");
            assert!(reason.contains("nothing was put on the reader"), "{reason}");
            assert!(!reason.contains('/'), "a path reached the owner: {reason}");
        }
    }

    #[test]
    fn a_reader_with_no_room_or_no_leave_to_write_is_told_what_to_do_with_the_reader() {
        if cfg!(windows) {
            return;
        }
        let full = reason(
            Kind::Reader,
            &ended_on("write /Volumes/KOBOeReader/.adds/cobalt/bin/kobod: No space left on device (os error 28)"),
        )
        .expect("a reason");
        assert!(full.contains("ran out of room"), "{full}");
        let locked = reason(
            Kind::Reader,
            &ended_on("create /Volumes/KOBOeReader/.adds: Read-only file system (os error 30)"),
        )
        .expect("a reason");
        assert!(locked.contains("can only be read"), "{locked}");
        for number in [1, 13] {
            let refused = reason(
                Kind::Reader,
                &ended_on(&format!(
                    "create /Volumes/KOBOeReader/.adds: refused (os error {number})"
                )),
            )
            .expect("a reason");
            assert!(refused.contains("did not allow writing"), "{refused}");
            assert_eq!(
                refused.contains("System Settings"),
                cfg!(target_os = "macos"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_send_with_no_reader_or_a_silent_one_never_names_a_command() {
        let none = reason(
            Kind::Send,
            &ended_on("No reader saved. Run kobo setup with the reader plugged in, or kobo pair."),
        )
        .expect("a reason");
        assert!(none.contains("under Reader"), "{none}");
        for sentence in [
            "\"kobo\" did not answer within 60 s. Wake it (press power), check Wi-Fi in Cobalt Settings, and run this again.",
            "\"study\" is not answering. Wake it (press power), check Wi-Fi in Cobalt Settings, and run this again.",
            "No reader answered within 60 s. Wake it (press power), check Wi-Fi in Cobalt Settings, and run this again.",
            "No reader is answering. Wake it (press power), check Wi-Fi in Cobalt Settings, and run this again.",
        ] {
            let silent = reason(Kind::Send, &ended_on(sentence)).expect("a reason");
            assert!(silent.contains("did not answer"), "{silent}");
            assert!(!silent.contains("kobo") && !silent.contains("run "), "{silent}");
        }
    }

    #[test]
    fn the_sentences_recognised_are_the_ones_the_commands_still_write() {
        // If a command's wording changes, the reason above stops being found
        // and the owner is back to the terminal's sentence with nothing to
        // show for it. These hold the two to each other.
        assert!(sending(crate::targets::NO_READER_SAVED).is_some());
        for (name, waited) in [
            (Some("kobo"), true),
            (Some("kobo"), false),
            (None, true),
            (None, false),
        ] {
            let said = crate::targets::not_answering(name, waited);
            // As it is printed, without the word that sorts it for a script.
            let printed = crate::console::display(&said);
            assert!(sending(printed).is_some(), "{printed}");
        }
    }

    #[test]
    fn what_is_not_recognised_is_left_to_the_command() {
        assert_eq!(
            reason(
                Kind::Reader,
                &ended_on("the reader runs firmware 5.1, which is not supported")
            ),
            None
        );
        assert_eq!(
            reason(
                Kind::Send,
                &ended_on("that page is a list of links, not an article")
            ),
            None
        );
        assert_eq!(
            reason(Kind::Reader, "it said nothing in its own name\n"),
            None
        );
        // A reader that has gone is the front end's to notice.
        assert_eq!(
            reason(
                Kind::Reader,
                &ended_on("write /Volumes/KOBOeReader/x: No such file or directory (os error 2)")
            ),
            None
        );
        // One kind's failures are not looked for in the other's words.
        assert_eq!(
            reason(Kind::Reader, &ended_on("No reader saved. Run kobo setup.")),
            None
        );
    }

    #[test]
    fn a_sentence_carried_onto_indented_lines_is_read_whole() {
        let said = "preparing Winter.cbz\nkobo: write /Volumes/KOBOeReader/Books/Winter.cbz:\n  No space left on device\n  (os error 28)\n";
        if cfg!(windows) {
            return;
        }
        let reason = reason(Kind::Reader, said).expect("a reason");
        assert!(reason.contains("ran out of room"), "{reason}");
    }

    #[test]
    fn only_the_sentence_it_stopped_on_is_read() {
        let said = "kobo: \"kobo\" is not answering. Wake it.\nTrying once more.\nkobo: that page is a list of links, not an article\n";
        assert_eq!(reason(Kind::Send, said), None);
    }
}
