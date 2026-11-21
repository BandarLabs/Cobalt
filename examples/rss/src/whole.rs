//! Telling an article from a teaser, and remembering what the page behind a
//! teaser turned out to hold.
//!
//! Most feeds carry a sentence or two of each story and a link to the rest.
//! Feeds used to show exactly that, so most subscriptions an owner added read
//! as one paragraph and an address they could not follow on a reader with no
//! browser. The page behind such an item is now fetched and cut to its
//! article, the way the sibling applications already do for the stories they
//! link to.
//!
//! Two things are decided here and nowhere else, so that opening an article
//! and Refresh all cannot come to disagree about them: whether an item is a
//! teaser at all ([`is_teaser`]), and whether what came back from its page is
//! the article or only the same teaser dressed as a web page
//! ([`article_in`]).
//!
//! What was found is written down in [`Notes`]. A page that gave no article
//! is remembered, so it is not asked for again on every opening and every
//! refresh. The words of a page that did are kept beside it, so a search of
//! saved articles finds them: the article itself lives in the shared keeper's
//! file with its pictures, and reading four hundred of those back to answer
//! one search is not something a reader with one core should be asked to do.

use super::{cache, feed};
use kobo_sdk::{Context, StoreResult};
use std::collections::BTreeMap;

/// How many words a feed has to give of an item before the item is taken to
/// be the article rather than a teaser for it.
///
/// Measured against two dozen feeds as they were served in November 2026. The
/// summaries of news services ran from ten words to about a hundred and
/// forty, the longest being the ones that give two opening paragraphs and an
/// invitation to continue. Feeds that carry whole articles gave three hundred
/// words or more for nearly every item, and two hundred and thirty for the
/// shortest post among them. The line sits between the two, nearer the
/// summaries, because the mistakes do not cost the same. A short post taken
/// for a teaser costs one request, which finds no more behind the link and is
/// never made again. A teaser taken for the article leaves the owner with the
/// paragraph and the unfollowable address this exists to end.
pub const WHOLE_WORDS: usize = 200;

/// How long a body may run and still be a teaser when it ends by saying so.
///
/// One technology site's feed gives up to two hundred and sixty words of each
/// story and then "Read full article", and the previews of paid newsletter
/// issues run longer still before their "Read more". A body that trails off
/// that way is a teaser however long it has run so far, up to the point where
/// the feed's own text is cut for length and every body ends in an ellipsis.
const TRAILING_WORDS: usize = 1000;

/// What the end of a body says when the rest of it is somewhere else.
///
/// Matched in the last line or two only, and in lower case. They are English
/// because the feeds measured were; a feed in another language is judged on
/// its length and on the ellipsis, which is the same in all of them.
///
/// "The post … appeared first on …" is not among them. A site's software
/// adds that line to every item it sends, the whole ones as well as the cut
/// ones, so it says where an item came from and nothing about how much of it
/// arrived. Counting it sent every article of a feed that carries them whole
/// off to its site for a page that had no more to give.
const TRAILS_OFF: [&str; 8] = [
    "[\u{2026}]",
    "[...]",
    "continue reading",
    "read more",
    "read full article",
    "read the full",
    "read the rest",
    "keep reading",
];

/// How much of the end of a body is searched for [`TRAILS_OFF`], in bytes.
const TRAIL: usize = 160;

/// The most words a picture post carries besides its picture.
///
/// A comic or a photograph a day is the feed's whole offering, not a teaser
/// for something longer, and the page behind it holds the same picture under
/// a site menu. Such an item is a picture with at most a caption beside it.
/// A news summary with a thumbnail has a sentence or more, which is longer
/// than this.
const PICTURE_POST_WORDS: usize = 8;

/// The most words an item may say for itself and still be read as a bare
/// pointer to somebody else's page.
///
/// A link aggregator's item is a headline, an address on another site and the
/// word "Comments", and the article behind the address is what its owner came
/// for. A link blog's item points at another site too, but its paragraph of
/// commentary is the post, and fetching the page would replace the writer the
/// owner follows with the one being written about. The two are told apart by
/// whether the item says anything of its own.
const BARE_WORDS: usize = 12;

/// The fewest words a fetched page must cut to before it is read in the
/// teaser's place.
///
/// A page that will not give its article still gives something. The refusals
/// measured, two requests to enable scripts and one puzzle to prove the
/// reader is a person, came to between nine and thirty-three words. The
/// shortest real article behind a teaser was three hundred and sixty.
pub const ARTICLE_WORDS: usize = 120;

/// How many notes may wait in memory before they are written down.
///
/// The file is rewritten whole, and during a refresh a note arrives with
/// every page. Writing each one would be forty rewrites of a file that can
/// run to megabytes; writing none until the end would lose them all to a
/// reader that went to sleep part way.
const WRITE_EVERY: usize = 8;

/// The most the notes may weigh on the shelf.
///
/// The words of one article are held under the ceiling a feed's own body is
/// read under, and the keeper holds four hundred articles at most, which comes
/// to a little over three megabytes when every one of them is long.
const NOTES_BYTES: usize = 4 * 1024 * 1024;

/// The name the notes are kept under. Not an address, and so never the name
/// of a feed's own saved copy.
const NOTES: &str = "rss-whole-articles-v1";

const MAGIC: &str = "whole-articles-v1\n";

/// Where a feed lives: its own address and the site it says it belongs to.
///
/// Feeds are often served from a different host than the site they describe,
/// so an item's link is the feed's own page when it matches either.
#[derive(Clone, Copy, Debug)]
pub struct Home<'a> {
    pub feed: &'a str,
    pub site: &'a str,
}

impl Home<'_> {
    /// Whether an address is a page of this feed's own site.
    fn owns(&self, link: &str) -> bool {
        let name = site_name(link);
        !name.is_empty() && (name == site_name(self.feed) || name == site_name(self.site))
    }
}

/// The name a site goes by in its address: `bbc` in both `www.bbc.co.uk` and
/// `feeds.bbc.com`.
///
/// The exact rule needs the list of every public suffix, which is larger than
/// this application. What is here is the label before the suffix, where the
/// suffix is the last label, or the last two when the address ends in a
/// country code after one of the handful of labels countries put in front of
/// theirs. It is wrong for two writers on one hosting platform, who then
/// count as one site, and that costs a link blog there the commentary on a
/// neighbour's article and nobody anything else.
fn site_name(address: &str) -> String {
    let rest = address
        .trim()
        .split_once("://")
        .map_or(address.trim(), |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').filter(|label| !label.is_empty()).collect();
    let name = match labels.as_slice() {
        [.., name, second, country]
            if country.len() == 2
                && [
                    "co", "com", "org", "net", "ac", "gov", "edu", "or", "ne", "go",
                ]
                .contains(second) =>
        {
            name
        }
        [.., name, _] | [name] => name,
        [] => "",
    };
    name.to_owned()
}

/// How many words are in a piece of plain text.
#[must_use]
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Markup with its pictures taken out.
///
/// Turning markup into text writes each picture as its description, which is
/// right for a row in a list and wrong for counting how much there is to
/// read: a comic's description is the joke said again at length, and it is
/// not prose the page is offering.
fn without_pictures(markup: &str) -> String {
    let lower = markup.to_ascii_lowercase();
    let mut out = String::with_capacity(markup.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find("<img") {
        let start = at + found;
        out.push_str(&markup[at..start]);
        at = lower[start..]
            .find('>')
            .map_or(markup.len(), |end| start + end + 1);
    }
    out.push_str(&markup[at..]);
    out
}

/// The words of an item that are prose rather than a picture's description.
fn prose_words(item: &feed::Item) -> usize {
    let all = words(&item.body);
    // The body is the upper bound, so an item with no pictures, or with far
    // more words than any threshold here, is not converted a second time.
    if !item.html.contains("<img") || all >= 4 * TRAILING_WORDS {
        return all;
    }
    words(&kobo_html::to_text(&without_pictures(&item.html)))
}

/// Whether a body ends by saying the rest of it is elsewhere.
fn trails_off(body: &str) -> bool {
    let body = body.trim_end();
    if body.ends_with('\u{2026}') || body.ends_with("...") {
        return true;
    }
    let mut from = body.len().saturating_sub(TRAIL);
    while !body.is_char_boundary(from) {
        from += 1;
    }
    let tail = body[from..].to_lowercase();
    TRAILS_OFF.iter().any(|mark| tail.contains(mark))
}

/// Whether the feed itself says there is more of this item than it carries.
///
/// A short post and a cut one are both sent for their page, because by
/// length alone they are the same thing. They part when the page has no more
/// to give: the cut one was promised more and its reader is told it did not
/// come, and the short one was whole all along, so there is nothing to say.
#[must_use]
pub fn promises_more(item: &feed::Item) -> bool {
    trails_off(&item.body)
}

/// Whether the feed gave only a teaser for this item, with the article behind
/// its link.
///
/// The link has to be one the runtime will fetch, which means HTTPS. Without
/// one there is nowhere to look, and the item is whatever the feed gave.
#[must_use]
pub fn is_teaser(item: &feed::Item, home: Home<'_>) -> bool {
    let link = item.link.trim();
    if !link.starts_with("https://") {
        return false;
    }
    let prose = prose_words(item);
    if item.html.contains("<img") && prose <= PICTURE_POST_WORDS {
        return false;
    }
    if !home.owns(link) {
        return prose <= BARE_WORDS;
    }
    prose < WHOLE_WORDS || (prose < TRAILING_WORDS && trails_off(&item.body))
}

/// How many words a page must cut to before it counts as the article behind
/// this teaser.
///
/// At least [`ARTICLE_WORDS`], and at least twice what the feed already gave.
/// A page that repeats the teaser and stops, which is what a paywall and a
/// short post that was whole all along both look like, has nothing to add.
#[must_use]
pub fn article_floor(item: &feed::Item) -> usize {
    ARTICLE_WORDS.max(prose_words(item) * 2)
}

/// The words of the article in a page already cut to its main content, when
/// there are at least `floor` of them.
///
/// The words come back because they are what a search of saved articles
/// reads. They are held to the ceiling a feed's own body is held to, so a
/// fetched article is searched exactly as far in as one the feed carried.
#[must_use]
pub fn article_in(cut: &str, floor: usize) -> Option<String> {
    let text = kobo_html::to_text(cut);
    let prose = if cut.contains("<img") || cut.contains("<IMG") {
        words(&kobo_html::to_text(&without_pictures(cut)))
    } else {
        words(&text)
    };
    (prose >= floor).then_some(text)
}

/// What the page behind one teaser turned out to hold.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Found {
    /// No more than the feed gave. The teaser is all this reader can have.
    Nothing,
    /// The article. These are its words, for searching.
    Words(String),
}

/// What is known about the pages behind teasers, kept across restarts.
///
/// One verified file for all of it rather than a store value per article: an
/// application has two hundred and fifty-six store keys and Feeds already
/// spends one on every feed and one on every picture it has shown.
pub struct Notes {
    saved: cache::Cache,
    found: BTreeMap<String, Found>,
    /// Notes made since the file was last written.
    unwritten: usize,
}

impl Default for Notes {
    fn default() -> Self {
        Self {
            saved: cache::Cache::new(NOTES).at_most(NOTES_BYTES),
            found: BTreeMap::new(),
            unwritten: 0,
        }
    }
}

impl Notes {
    /// Asks for the notes. Call once, at start.
    pub fn start(&self, context: &mut Context) {
        self.saved.start(context);
    }

    /// The store key the notes' pointer lives under.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.saved.key
    }

    /// Whether a shelf answer is about the notes' file.
    #[must_use]
    pub fn owns_file(&self, name: &str) -> bool {
        self.saved.owns_file(name)
    }

    /// Takes a store answer for the notes' pointer.
    pub fn stored(&mut self, context: &mut Context, result: &StoreResult) {
        let event = self.saved.stored(context, result);
        self.heard(context, event);
    }

    /// Takes a shelf answer for the notes' file.
    pub fn shelf(&mut self, context: &mut Context, result: &StoreResult) {
        let event = self.saved.shelf(context, result);
        self.heard(context, event);
    }

    fn heard(&mut self, context: &mut Context, event: Option<cache::Event>) {
        match event {
            Some(cache::Event::Loaded) => {
                // Anything noted before the file arrived is newer than the
                // file, so it is laid over what was read rather than under.
                let noted = std::mem::take(&mut self.found);
                self.found = self
                    .saved
                    .bytes
                    .as_deref()
                    .and_then(decode)
                    .unwrap_or_default();
                self.found.extend(noted);
                self.write(context);
            }
            // A write that was asked for while another was under way.
            Some(cache::Event::Saved) => self.write(context),
            // Notes that cannot be written cost a request made twice and a
            // search that misses a fetched article. Neither is worth a
            // banner over somebody's reading, so they stay in memory and the
            // next note tries the file again.
            Some(cache::Event::Failed) | None => {}
        }
    }

    /// Whether the page behind this article is known to hold no more than
    /// the feed gave.
    #[must_use]
    pub fn found_nothing(&self, id: &str) -> bool {
        self.found.get(id) == Some(&Found::Nothing)
    }

    /// The words of a fetched article, for searching.
    #[must_use]
    pub fn words_of(&self, id: &str) -> Option<&str> {
        match self.found.get(id) {
            Some(Found::Words(text)) => Some(text),
            _ => None,
        }
    }

    /// Writes down that the page behind this article gave no more than the
    /// feed did.
    pub fn found_no_article(&mut self, context: &mut Context, id: &str) {
        self.note(context, id, Found::Nothing);
    }

    /// Writes down the words of an article fetched from its page.
    pub fn found_article(&mut self, context: &mut Context, id: &str, text: String) {
        self.note(context, id, Found::Words(text));
    }

    fn note(&mut self, context: &mut Context, id: &str, found: Found) {
        if id.is_empty() || self.found.get(id) == Some(&found) {
            return;
        }
        self.found.insert(id.to_owned(), found);
        self.unwritten += 1;
        if self.unwritten >= WRITE_EVERY {
            self.write(context);
        }
    }

    /// Forgets every article `keep` answers no for.
    ///
    /// For the end of a refresh, so the notes about articles no feed carries
    /// any more do not pile up for ever.
    pub fn retain(&mut self, context: &mut Context, keep: impl Fn(&str) -> bool) {
        let before = self.found.len();
        self.found.retain(|id, _| keep(id));
        if self.found.len() != before {
            self.unwritten += 1;
            self.write(context);
        }
    }

    /// Writes the notes down, if any have been made since they last were.
    ///
    /// Called when the work that makes notes has gone quiet, and by the notes
    /// themselves every few. While the file is busy the notes stay counted,
    /// and its acknowledgement brings the write round again.
    pub fn write(&mut self, context: &mut Context) {
        if self.unwritten == 0 || self.saved.busy() {
            return;
        }
        if self.saved.retryable() {
            self.saved.retry(context);
            return;
        }
        if self.saved.save(context, encode(&self.found)) {
            self.unwritten = 0;
        }
    }
}

/// The notes as bytes: one article to a line, its identity, whether its page
/// held the article, and the words when it did.
///
/// Text rather than a binary format for the reason the subscription list is:
/// a file somebody can read in a hex dump is one somebody can mend by hand.
fn encode(found: &BTreeMap<String, Found>) -> Vec<u8> {
    let mut out = String::from(MAGIC);
    for (id, found) in found {
        let line = match found {
            Found::Nothing => format!("{id}\t-\n"),
            Found::Words(text) => {
                format!("{id}\t+\t{}\n", text.replace(['\t', '\n', '\r'], " "))
            }
        };
        // Words past the budget are left out and the article stays kept: it
        // still opens, and only a search stops short of it.
        if out.len() + line.len() > NOTES_BYTES {
            continue;
        }
        out.push_str(&line);
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Option<BTreeMap<String, Found>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut found = BTreeMap::new();
    for line in text.strip_prefix(MAGIC)?.lines() {
        let mut fields = line.splitn(3, '\t');
        let id = fields.next().filter(|id| !id.is_empty())?;
        let note = match (fields.next()?, fields.next()) {
            ("-", None) => Found::Nothing,
            ("+", Some(text)) => Found::Words(text.to_owned()),
            _ => return None,
        };
        found.insert(id.to_owned(), note);
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::{
        article_floor, article_in, decode, encode, is_teaser, promises_more, site_name, Found,
        Home, Notes, ARTICLE_WORDS, WHOLE_WORDS,
    };
    use crate::feed::Item;
    use kobo_sdk::{Command, Context, StoreRequest, StoreResult};
    use std::collections::BTreeMap;

    const HOME: Home<'static> = Home {
        feed: "https://feeds.example.com/news.xml",
        site: "https://www.example.com/",
    };

    fn item(link: &str, body: &str) -> Item {
        Item {
            title: "A story".to_owned(),
            link: link.to_owned(),
            body: body.to_owned(),
            html: format!("<p>{body}</p>"),
            ..Item::default()
        }
    }

    #[test]
    fn a_sentence_and_a_link_is_a_teaser_and_a_long_body_is_the_article() {
        let summary = "The council voted on Tuesday to rebuild the harbour wall.";
        assert!(is_teaser(
            &item("https://example.com/harbour", summary),
            HOME
        ));
        let whole = "word ".repeat(WHOLE_WORDS);
        assert!(!is_teaser(
            &item("https://example.com/harbour", &whole),
            HOME
        ));
        let nearly = "word ".repeat(WHOLE_WORDS - 1);
        assert!(is_teaser(
            &item("https://example.com/harbour", &nearly),
            HOME
        ));
    }

    #[test]
    fn an_item_with_no_link_the_runtime_will_fetch_is_whatever_the_feed_gave() {
        let summary = "The council voted on Tuesday to rebuild the harbour wall.";
        for link in ["", "http://example.com/harbour", "/harbour"] {
            assert!(!is_teaser(&item(link, summary), HOME), "{link}");
        }
    }

    #[test]
    fn the_line_a_site_signs_every_item_with_does_not_make_a_whole_article_a_teaser() {
        let whole = format!(
            "{}The post Harbour appeared first on Example.",
            "word ".repeat(WHOLE_WORDS + 40)
        );
        let article = item("https://example.com/harbour", &whole);
        assert!(!is_teaser(&article, HOME));
        assert!(!promises_more(&article));
        assert!(promises_more(&item(
            "https://example.com/harbour",
            "The council voted on Tuesday [\u{2026}]"
        )));
    }

    #[test]
    fn a_long_body_that_ends_by_pointing_at_the_rest_is_still_a_teaser() {
        let opening = "word ".repeat(WHOLE_WORDS + 40);
        for ending in [
            "Read full article Comments",
            "Continue reading...",
            "and then the tide [\u{2026}] The post Harbour appeared first on Example.",
            "and then the tide\u{2026}",
        ] {
            let body = format!("{opening}{ending}");
            assert!(
                is_teaser(&item("https://example.com/harbour", &body), HOME),
                "{ending}"
            );
        }
        let complete = format!("{opening}And that was the end of it.");
        assert!(!is_teaser(
            &item("https://example.com/harbour", &complete),
            HOME
        ));
    }

    #[test]
    fn a_picture_with_no_more_than_a_caption_is_the_post_and_not_a_teaser_for_one() {
        let comic = Item {
            title: "Tide tables".to_owned(),
            link: "https://example.com/1234/".to_owned(),
            body: "[[Later] A long description of the drawing, which is the joke said \
                   again for somebody who cannot see it, and runs to a good many words.]"
                .to_owned(),
            html: "<img src=\"/1234.png\" alt=\"[Later] A long description of the drawing, \
                   which is the joke said again for somebody who cannot see it, and runs \
                   to a good many words.\">"
                .to_owned(),
            ..Item::default()
        };
        assert!(!is_teaser(&comic, HOME));
        // A thumbnail beside a sentence is still a summary of something.
        let news = Item {
            body: "[The harbour] The council voted on Tuesday to rebuild the harbour wall \
                   after the winter storms."
                .to_owned(),
            html: "<img src=\"/harbour.jpg\" alt=\"The harbour\"><p>The council voted on \
                   Tuesday to rebuild the harbour wall after the winter storms.</p>"
                .to_owned(),
            ..comic
        };
        assert!(is_teaser(&news, HOME));
    }

    #[test]
    fn a_link_to_another_site_is_followed_only_when_the_item_says_nothing_of_its_own() {
        // An aggregator: the article on somebody else's site is the point.
        let pointer = item("https://elsewhere.net/story", "Comments");
        assert!(is_teaser(&pointer, HOME));
        // A link blog: the paragraph is the post, and the page behind the
        // link is the thing being written about.
        let commentary = item(
            "https://elsewhere.net/story",
            "This is the clearest account of the harbour inquiry so far, and the \
             table in the third section is the part everybody will quote.",
        );
        assert!(!is_teaser(&commentary, HOME));
        assert!(is_teaser(
            &Item {
                link: "https://www.example.com/story".to_owned(),
                ..commentary
            },
            HOME
        ));
    }

    #[test]
    fn a_site_is_known_by_its_name_whichever_host_and_suffix_serve_it() {
        for (address, name) in [
            ("https://www.bbc.co.uk/news/articles/1", "bbc"),
            ("https://feeds.bbc.com/news/rss.xml", "bbc"),
            ("https://www.npr.org/2026/11/19/story", "npr"),
            ("https://rss.nytimes.com/services/xml/rss", "nytimes"),
            ("https://example.com.au:8443/a?b#c", "example"),
            ("example.org", "example"),
            ("https://localhost/feed", "localhost"),
            ("", ""),
        ] {
            assert_eq!(site_name(address), name, "{address}");
        }
    }

    #[test]
    fn a_page_counts_as_the_article_only_when_it_holds_well_more_than_the_teaser() {
        let teaser = item("https://example.com/harbour", &"word ".repeat(20));
        assert_eq!(article_floor(&teaser), ARTICLE_WORDS);
        let stub = format!("<p>{}</p>", "word ".repeat(ARTICLE_WORDS - 1));
        assert_eq!(article_in(&stub, article_floor(&teaser)), None);
        let story = format!("<h1>Harbour</h1><p>{}</p>", "word ".repeat(ARTICLE_WORDS));
        assert!(article_in(&story, article_floor(&teaser)).is_some());
        // A short post the feed carried whole: its page has the same words,
        // which is no reason to read the page in its place.
        let post = item("https://example.com/note", &"word ".repeat(150));
        let same = format!("<p>{}</p>", "word ".repeat(160));
        assert_eq!(article_in(&same, article_floor(&post)), None);
    }

    #[test]
    fn a_picture_on_the_page_does_not_count_towards_its_words() {
        let page = format!(
            "<img src=\"a.png\" alt=\"{}\"><p>Sign in to read this.</p>",
            "word ".repeat(ARTICLE_WORDS)
        );
        assert_eq!(article_in(&page, ARTICLE_WORDS), None);
    }

    #[test]
    fn notes_read_back_as_they_were_written_and_a_damaged_file_is_no_notes() {
        let found = BTreeMap::from([
            ("a1".to_owned(), Found::Nothing),
            (
                "b2".to_owned(),
                Found::Words("Two lines\nof words\tand a tab.".to_owned()),
            ),
        ]);
        let read = decode(&encode(&found)).expect("the notes decode");
        assert_eq!(read.get("a1"), Some(&Found::Nothing));
        assert_eq!(
            read.get("b2"),
            Some(&Found::Words("Two lines of words and a tab.".to_owned()))
        );
        assert_eq!(decode(b"something else"), None);
        assert_eq!(decode(b"whole-articles-v1\nid\t?\n"), None);
    }

    fn loaded(context: &mut Context) -> Notes {
        let mut notes = Notes::default();
        notes.start(context);
        let key = notes.key().to_owned();
        notes.stored(context, &StoreResult::Loaded { key, value: None });
        notes
    }

    fn writes(context: &Context) -> usize {
        context
            .commands()
            .iter()
            .filter(|command| matches!(command, Command::Store(StoreRequest::ShelfWrite { .. })))
            .count()
    }

    #[test]
    fn notes_are_written_every_few_rather_than_one_file_for_each() {
        let mut context = Context::default();
        let mut notes = loaded(&mut context);
        for index in 0..super::WRITE_EVERY - 1 {
            notes.found_no_article(&mut context, &format!("id-{index}"));
        }
        assert_eq!(writes(&context), 0, "a note was written on its own");
        notes.found_article(&mut context, "last", "Some words.".to_owned());
        assert_eq!(writes(&context), 1);
        assert!(notes.found_nothing("id-0"));
        assert_eq!(notes.words_of("last"), Some("Some words."));
        assert_eq!(notes.words_of("id-0"), None);
    }

    #[test]
    fn a_note_made_before_the_file_arrived_survives_its_arrival() {
        let mut context = Context::default();
        let mut notes = Notes::default();
        notes.start(&mut context);
        notes.found_no_article(&mut context, "early");
        let key = notes.key().to_owned();
        notes.stored(&mut context, &StoreResult::Loaded { key, value: None });
        assert!(notes.found_nothing("early"));
        assert_eq!(writes(&context), 1, "the early note was never written");
    }

    #[test]
    fn letting_go_of_articles_no_feed_carries_rewrites_the_notes_once() {
        let mut context = Context::default();
        let mut notes = loaded(&mut context);
        notes.found_no_article(&mut context, "old");
        notes.found_no_article(&mut context, "new");
        notes.retain(&mut context, |id| id == "new");
        assert!(!notes.found_nothing("old"));
        assert!(notes.found_nothing("new"));
        assert_eq!(writes(&context), 1);
        notes.retain(&mut context, |id| id == "new");
        assert_eq!(
            writes(&context),
            1,
            "nothing changed and it was written again"
        );
    }
}
