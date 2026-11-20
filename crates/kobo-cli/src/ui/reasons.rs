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
pub fn reason(kind: Kind, volume: Option<&str>, said: &str) -> Option<String> {
    let stopped_on = stopped_on(said)?;
    match kind {
        Kind::Reader if reader_went(&stopped_on, volume) => {
            Some("The reader was disconnected before this finished.")
        }
        Kind::Reader => writing_to_the_reader(&stopped_on),
        Kind::Send => sending(&stopped_on),
    }
    .map(str::to_owned)
}

/// Whether a job stopped because the reader it was writing to went away.
///
/// Three things have to be true, because each alone is true of other
/// failures. The sentence names a file on that reader. The system said the
/// file, or the device under it, was not there. And the reader is not there
/// now: a file that is merely missing from a reader still on the cable is a
/// different failure, and a reader the job ejected itself is not named in
/// the sentence it stops on afterwards.
fn reader_went(stopped_on: &str, volume: Option<&str>) -> bool {
    let Some(volume) = volume.filter(|volume| !volume.is_empty()) else {
        return false;
    };
    let not_there = os_error_number(stopped_on).is_some_and(|number| {
        if cfg!(windows) {
            // ERROR_PATH_NOT_FOUND, ERROR_NOT_READY, ERROR_DEVICE_NOT_CONNECTED
            matches!(number, 3 | 21 | 1167)
        } else {
            // ENOENT, EIO, ENXIO
            matches!(number, 2 | 5 | 6)
        }
    });
    not_there && stopped_on.contains(volume) && !std::path::Path::new(volume).exists()
}

/// Why a reader could not be ejected, when macOS refused.
///
/// The command reports the tool it ran, where the reader is mounted and the
/// tool's own words, which give the program that still has something open
/// by its number and its path. The owner is told which program, when the
/// tool says, and what to do. What other systems say is an instruction
/// already and is passed on as it is.
pub fn eject(said: &str) -> String {
    if !said.starts_with("diskutil eject ") {
        return said.to_owned();
    }
    let user = said
        .split_once("dissented by PID ")
        .and_then(|(_, after)| after.split_once('('))
        .and_then(|(_, after)| after.split_once(')'))
        .and_then(|(path, _)| program_named(path));
    match user {
        Some(program) => format!(
            "The reader could not be ejected because {program} is still using it. Close what \
             it has open from the reader, then eject again."
        ),
        None => "The reader could not be ejected because something on this computer is still \
                 using it. Close any file or window from the reader, then eject again."
            .to_owned(),
    }
}

/// A program's name as its owner knows it, from where it runs.
///
/// An app runs from inside a folder named for it, and that name is the one
/// in the Dock. Anything else is known by the name of its file.
fn program_named(path: &str) -> Option<&str> {
    let parts = path.split('/').filter(|part| !part.is_empty());
    let app = parts
        .clone()
        .find_map(|part| part.strip_suffix(".app"))
        .filter(|name| !name.is_empty());
    app.or_else(|| parts.last())
}

/// The sentence a command stopped on: the last line it wrote in its own
/// name, with the indented lines under it, which is how a sentence too long
/// for one line is carried on.
fn stopped_on(said: &str) -> Option<String> {
    let lines: Vec<&str> = said.lines().collect();
    let last = lines.iter().rposition(|line| line.starts_with("kobo: "))?;
    let mut sentence = lines[last]["kobo: ".len()..].trim().to_owned();
    for more in lines[last + 1..]
        .iter()
        .take_while(|line| line.starts_with([' ', '\t']) && !line.trim().is_empty())
    {
        sentence.push(' ');
        sentence.push_str(more.trim());
    }
    Some(sentence)
}

/// Why Send to Kobo could not be added to a browser, or taken out of one.
///
/// This is not a job: the command runs for a moment and its outcome is the
/// answer to the request. What it prints when it fails begins with its own
/// name and usually ends with the folder it could not write, and the window
/// used to show all of it. The owner is told which browser and, where the
/// system refused the write, that it did. Anything else the command said is
/// kept after that, without the command's name on it.
pub fn browser(title: &str, adding: bool, said: &str) -> String {
    let what = if adding {
        format!("Send to Kobo could not be added to {title}.")
    } else {
        format!("Send to Kobo could not be taken out of {title}.")
    };
    let Some(stopped_on) = stopped_on(said) else {
        return what;
    };
    match os_error(&stopped_on) {
        Some(Trouble::NotAllowed | Trouble::ReadOnly) => {
            format!("{what} This computer did not allow a change to the folder {title} keeps its extensions in.")
        }
        Some(Trouble::Full) => format!("{what} This computer's disk is full."),
        None => {
            // The commands write some reasons to follow a colon, in lower
            // case and unpunctuated. Here it is a sentence of its own.
            let mut letters = stopped_on.chars();
            let begun: String = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default();
            let ended = if begun.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            format!("{what} {begun}{ended}")
        }
    }
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
/// that has gone has numbers too, and those are read by `reader_went`.
fn os_error(sentence: &str) -> Option<Trouble> {
    let number = os_error_number(sentence)?;
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

/// The system's own number for the error a sentence ends on.
fn os_error_number(sentence: &str) -> Option<i32> {
    let after = sentence.rsplit_once("(os error ")?.1;
    after.split(')').next()?.parse().ok()
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
            let reason = reason(Kind::Reader, None, &ended_on(sentence)).expect("a reason");
            assert!(reason.contains("nothing was put on the reader"), "{reason}");
            assert!(!reason.contains('/'), "a path reached the owner: {reason}");
        }
    }

    #[test]
    fn a_reader_with_no_room_or_no_leave_to_write_is_told_what_to_do_with_the_reader() {
        if cfg!(windows) {
            return;
        }
        let full = reason(Kind::Reader, None,
            &ended_on("write /Volumes/KOBOeReader/.adds/cobalt/bin/kobod: No space left on device (os error 28)"),
        )
        .expect("a reason");
        assert!(full.contains("ran out of room"), "{full}");
        let locked = reason(
            Kind::Reader,
            None,
            &ended_on("create /Volumes/KOBOeReader/.adds: Read-only file system (os error 30)"),
        )
        .expect("a reason");
        assert!(locked.contains("can only be read"), "{locked}");
        for number in [1, 13] {
            let refused = reason(
                Kind::Reader,
                None,
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
            None,
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
            let silent = reason(Kind::Send, None, &ended_on(sentence)).expect("a reason");
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
                None,
                &ended_on("the reader runs firmware 5.1, which is not supported")
            ),
            None
        );
        assert_eq!(
            reason(
                Kind::Send,
                None,
                &ended_on("that page is a list of links, not an article")
            ),
            None
        );
        assert_eq!(
            reason(Kind::Reader, None, "it said nothing in its own name\n"),
            None
        );
        // A reader that has gone is the front end's to notice.
        assert_eq!(
            reason(
                Kind::Reader,
                None,
                &ended_on("write /Volumes/KOBOeReader/x: No such file or directory (os error 2)")
            ),
            None
        );
        // One kind's failures are not looked for in the other's words.
        assert_eq!(
            reason(
                Kind::Reader,
                None,
                &ended_on("No reader saved. Run kobo setup.")
            ),
            None
        );
    }

    #[test]
    fn a_sentence_carried_onto_indented_lines_is_read_whole() {
        let said = "preparing Winter.cbz\nkobo: write /Volumes/KOBOeReader/Books/Winter.cbz:\n  No space left on device\n  (os error 28)\n";
        if cfg!(windows) {
            return;
        }
        let reason = reason(Kind::Reader, None, said).expect("a reason");
        assert!(reason.contains("ran out of room"), "{reason}");
    }

    #[test]
    fn a_browser_that_could_not_be_set_up_is_named_and_its_folder_is_not() {
        if cfg!(windows) {
            return;
        }
        let said = "kobo: write /Users/owner/Library/Application Support/Google/Chrome/NativeMessagingHosts/io.cobalt.clip.json: Permission denied (os error 13)\n";
        let told = browser("Chrome", true, said);
        assert_eq!(
            told,
            "Send to Kobo could not be added to Chrome. This computer did not allow a change to the folder Chrome keeps its extensions in."
        );
        assert!(!told.contains('/'), "{told}");
        // Taking it out says so, and a command that said nothing leaves
        // the plain fact.
        assert_eq!(
            browser("Firefox", false, ""),
            "Send to Kobo could not be taken out of Firefox."
        );
        // What is not recognised is passed on, without the command's name.
        assert_eq!(
            browser("Edge", true, "kobo: its profile is one this cannot read\n"),
            "Send to Kobo could not be added to Edge. Its profile is one this cannot read."
        );
    }

    #[test]
    fn a_reader_pulled_out_under_a_job_is_said_to_have_been_disconnected() {
        if cfg!(windows) {
            return;
        }
        let gone = "/Volumes/a-reader-that-is-not-plugged-in";
        let said = ended_on(&format!(
            "could not write {gone}/.adds/cobalt.staging/bin/kobod: No such file or directory (os error 2)"
        ));
        assert_eq!(
            reason(Kind::Reader, Some(gone), &said).as_deref(),
            Some("The reader was disconnected before this finished.")
        );
        // A file missing from a reader that is still there is not that.
        let here = std::env::temp_dir();
        let here = here.to_string_lossy();
        let missing = ended_on(&format!(
            "read {here}/no-such-file: No such file or directory (os error 2)"
        ));
        assert_eq!(reason(Kind::Reader, Some(&here), &missing), None);
        // Nor is a sentence about some other place, or a job with no reader.
        let elsewhere = ended_on("read /somewhere/else: No such file or directory (os error 2)");
        assert_eq!(reason(Kind::Reader, Some(gone), &elsewhere), None);
        assert_eq!(reason(Kind::Reader, None, &said), None);
    }

    #[test]
    fn a_refused_eject_names_the_program_in_the_way_and_not_the_tool() {
        // What diskutil prints on macOS when a file on the volume is open.
        let app = "diskutil eject /Volumes/KOBOeReader failed: Unmount of disk4 failed: at least one volume could not be unmounted\nUnmount was dissented by PID 623 (/System/Applications/Preview.app/Contents/MacOS/Preview)\nDissenter parent PPID 1 (/sbin/launchd)";
        assert_eq!(
            eject(app),
            "The reader could not be ejected because Preview is still using it. Close what it has open from the reader, then eject again."
        );
        let tool = "diskutil eject /Volumes/KOBOeReader failed: Unmount of disk4 failed: at least one volume could not be unmounted\nUnmount was dissented by PID 63705 (/usr/bin/perl)";
        assert!(
            eject(tool).contains("because perl is still using it"),
            "{}",
            eject(tool)
        );
        let unsaid = eject("diskutil eject /Volumes/KOBOeReader failed: Volume failed to eject");
        assert!(
            unsaid.contains("something on this computer is still using it"),
            "{unsaid}"
        );
        for told in [eject(app), eject(tool), unsaid] {
            assert!(
                !told.contains("diskutil") && !told.contains("/Volumes"),
                "{told}"
            );
        }
        // Another system's instruction is passed on as it is.
        let linux = "eject /media/owner/KOBOeReader yourself, then restart the reader";
        assert_eq!(eject(linux), linux);
    }

    #[test]
    fn only_the_sentence_it_stopped_on_is_read() {
        let said = "kobo: \"kobo\" is not answering. Wake it.\nTrying once more.\nkobo: that page is a list of links, not an article\n";
        assert_eq!(reason(Kind::Send, None, said), None);
    }
}
