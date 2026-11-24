//! What a release says it changes, read from the description it is
//! published with.
//!
//! The description comes in the same reply as the release's version and
//! download links, so knowing what is new costs no extra request. It is
//! written by hand for people reading it on the web, which makes most of it
//! the wrong thing for a reader's panel: the paragraph about what has not
//! been tried on hardware, the commit it was built from, a heading. The
//! changes themselves are written one to a list item with a bold lead, and
//! those items are the only part kept.
//!
//! Anything that does not look like one is passed over without complaint.
//! Older releases carry a sentence of boilerplate and some carry nothing,
//! and the update itself must never be held up by how its notes were
//! written. A description with no such items gives no notes, and the page
//! is then drawn as it was before notes existed.

/// One change, as the release's description names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Note {
    /// The words the description sets bold, without their full stop: "Light
    /// in Settings".
    pub lead: String,
    /// The sentences after the lead, as plain text.
    pub detail: String,
}

/// A space a line may not end at.
const BOUND: char = '\u{a0}';

impl Note {
    /// The change as the update screen sets it, "Light in Settings. A Light
    /// row...", and how many bytes of it are the lead with its full stop,
    /// which is the part set bold.
    ///
    /// Two places in the detail are tied so a line cannot break there, as
    /// the launcher ties them in its sheet: a number stays with the word it
    /// counts, so "7" does not end one line with "inch" opening the next,
    /// and the last two words stay together, so a change never ends on a
    /// line of one word. The lead is left alone; it opens its line.
    #[must_use]
    pub fn set(&self) -> (String, usize) {
        let words: Vec<&str> = self.detail.split_whitespace().collect();
        let mut text = format!("{}. ", self.lead);
        let lead = text.len() - 1;
        for (index, word) in words.iter().enumerate() {
            text.push_str(word);
            if index + 1 == words.len() {
                break;
            }
            // A number opening a bracket counts too: "(6 per cent)" broke
            // after the 6 on a six inch panel.
            let number = word.trim_start_matches(['(', '[']);
            let counts = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
            let last_pair = index + 2 == words.len();
            text.push(if counts || last_pair { BOUND } else { ' ' });
        }
        (text, lead)
    }
}

/// The most changes kept. A description is written for a web page, where a
/// sixth item costs nothing; on a panel the first few are what an owner
/// reads before deciding, and the page gives up the later ones first anyway.
pub const MOST: usize = 5;

/// The most characters one change may run to, lead and detail together.
///
/// A change is passed over whole when it is longer, never shortened: a note
/// cut off at a word says less than no note, and may say something the
/// writer did not. The longest change written so far is about 340
/// characters, the Tailscale change in 0.3.41, so this leaves room for one
/// a little longer while still refusing a pasted log or a paragraph that
/// ran on.
pub const CHARACTERS: usize = 400;

/// The changes a release's description lists, in the order it lists them.
///
/// A change is a list item that opens with a bold lead ending in a full
/// stop, `- **Light in Settings.** A Light row under Battery...`, and goes on
/// to at least one sentence. A writer who wrapped an item by hand is
/// followed onto the lines after it, up to a blank line or the next item, so
/// a wrapped change is read whole rather than up to its first line break.
#[must_use]
pub fn parse(body: &str) -> Vec<Note> {
    let mut notes = Vec::new();
    // `lines` drops the carriage return of a Windows line ending with the
    // line feed, so a description edited on Windows reads the same.
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(opened) = line.trim_end().strip_prefix("- **") else {
            continue;
        };
        let mut item = opened.to_owned();
        while let Some(next) = lines.peek().map(|next| next.trim()) {
            if next.is_empty() || opens_block(next) {
                break;
            }
            item.push(' ');
            item.push_str(next);
            lines.next();
        }
        let Some(note) = note(&item) else {
            continue;
        };
        if note.lead.chars().count() + 2 + note.detail.chars().count() > CHARACTERS {
            continue;
        }
        notes.push(note);
        if notes.len() == MOST {
            break;
        }
    }
    notes
}

/// Whether a line starts something of its own rather than carrying on the
/// list item above it: another item, a heading, a quotation or a fence.
fn opens_block(line: &str) -> bool {
    let numbered = line.split_once(". ").is_some_and(|(number, _)| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    });
    numbered
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("+ ")
        || line.starts_with(['#', '>', '`', '|'])
}

/// The change in one list item, given what follows its opening `- **`.
fn note(item: &str) -> Option<Note> {
    let (lead, detail) = item.split_once("**")?;
    let lead = plain(lead.trim().strip_suffix('.')?);
    let detail = first_sentence(&plain(detail));
    // A detail that does not end a sentence was cut off somewhere on its way
    // here, and half a sentence is not shown.
    let ended = detail
        .trim_end_matches(['"', '\'', ')', '\u{2019}', '\u{201d}'])
        .ends_with(['.', '?', '!']);
    (!lead.is_empty() && !lead.ends_with('.') && ended).then_some(Note { lead, detail })
}

/// The first sentence of `detail`, or all of it when it is one.
///
/// A release's description is written for a page in a browser, and an item
/// there runs to three or four sentences: what changed, how it behaves, what
/// it does not do. On a six inch panel that is six lines a change, two
/// changes to a page, and the button under them. The first sentence says
/// what changed, which is what somebody deciding whether to update now
/// reads; the rest stays on the release page.
///
/// A sentence ends at a full stop, question mark or exclamation mark that is
/// followed by a space and a capital letter, so "6.5 inch" and "e.g. this"
/// do not end one.
fn first_sentence(detail: &str) -> String {
    let characters: Vec<char> = detail.chars().collect();
    for (index, window) in characters.windows(3).enumerate() {
        if matches!(window[0], '.' | '?' | '!') && window[1] == ' ' && window[2].is_uppercase() {
            return characters[..=index].iter().collect();
        }
    }
    detail.to_owned()
}

/// `text` without the markup a description may carry: a link is its words,
/// code is its letters, bold is the words it marks, and every run of spaces
/// is one space.
fn plain(text: &str) -> String {
    let mut words = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let linked = rest[open + 1..]
            .split_once("](")
            .and_then(|(words, after)| {
                let (_, beyond) = after.split_once(')')?;
                (!words.contains(['[', ']'])).then_some((words, beyond))
            });
        if let Some((link, beyond)) = linked {
            // An image is written as a link with `!` in front; its
            // description is the only part a panel of words can show.
            words.push_str(rest[..open].strip_suffix('!').unwrap_or(&rest[..open]));
            words.push_str(link);
            rest = beyond;
        } else {
            words.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
        }
    }
    words.push_str(rest);
    words
        .replace("**", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The description 0.3.41 was published with, as GitHub returns it: the
/// shape descriptions are written in now, and what the update screen's own
/// tests lay out.
#[cfg(test)]
pub const RELEASE_0_3_41: &str = "\
- **Light in Settings.** A Light row under Battery shows the front light's level, and its page sets the brightness: one per cent at a time at or below ten, where a step of ten is the difference between dark and too bright, and five above that. The level lasts until Cobalt closes, when the Kobo reader's own level comes back.
- **A calmer top bar.** The bar is paper on every reader, and each screen's title is set as a title. On a colour reader an app's colour is the line under the bar, where it used to fill the bar. The way back now lines up with the clock and the page.
- **Larger readers.** Everything is drawn a little bigger on the 7 inch Libras (6 per cent) and the 10 inch Elipsa 2E (a quarter), because a larger reader is held further away. Nothing changes on 6 inch readers.
- **What's new.** The first time Cobalt opens after this update it shows these changes once, over the app grid. Done puts them away.
- **Tailscale says how much data the reader has received and sent.** While your tailnet has approved the reader as an exit node or its network as shared, the Tailscale screen has a Traffic line, such as `Rx 67 MB, Tx 324 MB`. Update Tailscale to 0.1.2 from the App Store on the reader to see it. The counts start again whenever Tailscale restarts.

Not yet seen on a reader: the Light page, the new top bar and the larger sizes. Tailscale staying connected outside Cobalt, and the shared network, are still unseen too.

Built from `beta` at 5a4e3381155ecfa4450545c4f73700626be9f483.
";

#[cfg(test)]
mod tests {
    use super::{parse, Note, CHARACTERS, MOST, RELEASE_0_3_41};

    fn leads(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|note| note.lead.as_str()).collect()
    }

    #[test]
    fn the_description_0_3_41_was_published_with_gives_its_five_changes_and_nothing_else() {
        let notes = parse(RELEASE_0_3_41);
        assert_eq!(
            leads(&notes),
            [
                "Light in Settings",
                "A calmer top bar",
                "Larger readers",
                "What's new",
                "Tailscale says how much data the reader has received and sent",
            ]
        );
        // Each change is its first sentence: the rest is for the release
        // page, where there is room for it.
        assert_eq!(
            notes[3].detail,
            "The first time Cobalt opens after this update it shows these changes once, over the app grid."
        );
        // Code is its letters, without the marks that set it as code.
        assert_eq!(
            notes[4].detail,
            "While your tailnet has approved the reader as an exit node or its network as shared, the Tailscale screen has a Traffic line, such as Rx 67 MB, Tx 324 MB."
        );
        let every_word = notes
            .iter()
            .flat_map(|note| [note.lead.as_str(), note.detail.as_str()])
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!every_word.contains("Not yet seen"));
        assert!(!every_word.contains("Built from"));
        assert!(!every_word.contains(['`', '*']));
    }

    #[test]
    fn a_change_keeps_its_first_sentence_and_no_more() {
        let notes = parse(
            "- **Larger readers.** Everything is drawn 6.5 per cent bigger, e.g. on a Libra. Nothing changes on 6 inch readers.\n\
             - **One sentence.** Only this.\n\
             - **A question.** Can it be read? It can.\n",
        );
        let details: Vec<&str> = notes.iter().map(|note| note.detail.as_str()).collect();
        assert_eq!(
            details,
            [
                "Everything is drawn 6.5 per cent bigger, e.g. on a Libra.",
                "Only this.",
                "Can it be read?",
            ]
        );
    }

    #[test]
    fn the_boilerplate_older_releases_carry_gives_no_notes() {
        for body in [
            "Immutable prerelease built from the beta branch at 0123456789abcdef0123456789abcdef01234567.",
            "Promoted verified artifacts from tested beta-v0.3.40.",
        ] {
            assert_eq!(parse(body), [], "{body}");
        }
    }

    #[test]
    fn an_empty_description_gives_no_notes() {
        assert_eq!(parse(""), []);
        assert_eq!(parse("\n\n  \n"), []);
    }

    #[test]
    fn a_heading_is_passed_over_and_a_link_is_read_as_its_words() {
        let body = "\
## What changed

- **Books.** Open a file from [the Books guide](https://example.test/books.md) or ![a cover](cover.png) the shelf.
- **Feeds.** A link left [unfinished stays as it was written.
";
        let notes = parse(body);
        assert_eq!(leads(&notes), ["Books", "Feeds"]);
        assert_eq!(
            notes[0].detail,
            "Open a file from the Books guide or a cover the shelf."
        );
        assert_eq!(
            notes[1].detail,
            "A link left [unfinished stays as it was written."
        );
    }

    #[test]
    fn bold_inside_a_change_is_dropped_and_its_words_kept() {
        let notes = parse("- **Retry.** It now waits for **Try again**.\n");
        assert_eq!(notes[0].detail, "It now waits for Try again.");
    }

    #[test]
    fn an_item_whose_lead_does_not_end_in_a_full_stop_is_not_a_change() {
        // The older shape, and items that are not changes at all.
        let body = "\
- **Reading:** Books opens files up to 32 MB.
- **Setup** turns off password login.
- Plain item with no lead.
* **Starred.** Another list marker.
-  **Spaced.** Two spaces after the dash.
";
        assert_eq!(parse(body), []);
    }

    #[test]
    fn a_change_with_no_sentence_after_its_lead_is_not_a_change() {
        let body = "- **Light.**\n- **Sound.**   \n- **Clock.** It reads the time\n";
        assert_eq!(parse(body), []);
    }

    #[test]
    fn a_change_longer_than_the_ceiling_is_passed_over_whole_and_not_cut() {
        let at_the_ceiling = format!("{}.", "a".repeat(CHARACTERS - "Long. ".len() - 1));
        let past_it = format!("{}.", "a".repeat(CHARACTERS - "Long. ".len()));
        let body =
            format!("- **Long.** {past_it}\n- **Short.** It fits.\n- **Long.** {at_the_ceiling}\n");
        let notes = parse(&body);
        assert_eq!(leads(&notes), ["Short", "Long"]);
        assert_eq!(notes[1].detail, at_the_ceiling);
        assert_eq!(
            notes[1].lead.chars().count() + 2 + notes[1].detail.chars().count(),
            CHARACTERS
        );
    }

    #[test]
    fn only_the_first_five_changes_are_kept() {
        let body = ["One", "Two", "Three", "Four", "Five", "Six", "Seven"]
            .map(|lead| format!("- **{lead}.** It changed."))
            .join("\n");
        let notes = parse(&body);
        assert_eq!(notes.len(), MOST);
        assert_eq!(leads(&notes), ["One", "Two", "Three", "Four", "Five"]);
    }

    #[test]
    fn a_change_is_set_with_its_lead_bold_and_its_numbers_and_last_words_tied() {
        let note = Note {
            lead: "Larger readers".to_owned(),
            detail: "Drawn bigger on the 7 inch Libras (6 per cent) and the rest.".to_owned(),
        };
        let (text, lead) = note.set();
        assert_eq!(&text[..lead], "Larger readers.");
        assert_eq!(
            text,
            "Larger readers. Drawn bigger on the 7\u{a0}inch Libras (6\u{a0}per cent) and the\u{a0}rest."
        );
        let one_word = Note {
            lead: "Light".to_owned(),
            detail: "Brighter.".to_owned(),
        };
        assert_eq!(one_word.set(), ("Light. Brighter.".to_owned(), 6));
    }

    #[test]
    fn windows_line_endings_read_the_same_as_unix_ones() {
        let windows = RELEASE_0_3_41.replace('\n', "\r\n");
        assert_eq!(parse(&windows), parse(RELEASE_0_3_41));
        assert!(parse(&windows)
            .iter()
            .all(|note| !note.detail.contains('\r')));
    }

    #[test]
    fn a_change_wrapped_by_hand_is_read_whole_up_to_the_next_item() {
        let body = "\
- **Light in Settings.** A Light row under Battery
  shows the front light's level.
- **Larger readers.** Everything is drawn
a little bigger.

Built from `beta`.
";
        let notes = parse(body);
        assert_eq!(leads(&notes), ["Light in Settings", "Larger readers"]);
        assert_eq!(
            notes[0].detail,
            "A Light row under Battery shows the front light's level."
        );
        assert_eq!(notes[1].detail, "Everything is drawn a little bigger.");
    }
}
