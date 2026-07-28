//! The shapes Hacker News sends, and the shapes the panel needs.
//!
//! One response matters. `item/<number>.json` returns a single object: `{id,
//! type, by, time, text?, title?, url?, score?, descendants?, kids?}`, where
//! `title` and `url` belong to stories, `text` to comments and self-posts, and
//! `kids` lists the replies in the order the site draws them.
//!
//! Read defensively. A missing field is a missing field, not a reason to show
//! nothing: a thread with one deleted comment in it is still a thread, and a
//! story with no score is still a story.

/// The most stories kept from one response.
///
/// The API is asked for thirty. Anything past this is a response that did not
/// come from where it was supposed to, and paging through it would be paging
/// through somebody else's idea of what to show.
pub const MAX_STORIES: usize = 30;

/// The most comments kept from one thread.
///
/// A thread with more than this is one nobody reads to the end of on a panel
/// that turns a page a second. The cap is what stops a popular story becoming
/// several megabytes of `String` on a device with 512 MB of memory, and it is
/// stated on screen when it bites rather than being a silent cut.
pub const MAX_COMMENTS: usize = 400;

/// How many levels of reply are drawn as indentation.
///
/// The UI layer owns this number, because it is the one that knows what a
/// reply indented eleven times would do to a 91 mm measure. Past the cap the
/// depth is written out in the byline instead, which costs no width at all.
pub const MAX_INDENT: u8 = kobo_sdk::MAX_QUOTE_DEPTH;

/// One entry in a tab's list.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Story {
    /// The Hacker News item number, as a string.
    pub id: String,
    pub title: String,
    pub author: String,
    pub points: u32,
    pub comments: u32,
    /// Seconds since the epoch, as the API's `time`.
    pub created: i64,
    /// The self-post body of an Ask HN or a Show HN, already plain text.
    pub text: Option<String>,
    /// Where a link story points, reduced to a host worth reading.
    ///
    /// A whole URL is unreadable in a summary line and a bare title says
    /// nothing about whether the thing behind it is a paper, a blog or a
    /// press release. Hacker News itself has shown the host beside every
    /// title since the beginning, which is the evidence that it is the part
    /// people actually use.
    pub site: Option<String>,
}

/// One comment, already flattened out of the tree and ready to draw.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Comment {
    pub author: String,
    pub created: i64,
    /// How deep the reply really is, before any clamping.
    pub depth: u16,
    /// The body, converted from HTML, split into paragraphs.
    pub body: String,
}

impl Comment {
    /// How many levels of indent this comment is drawn with.
    #[must_use]
    pub fn indent(&self) -> u8 {
        u8::try_from(self.depth).unwrap_or(u8::MAX).min(MAX_INDENT)
    }

    /// The line above the body: who wrote it, when, and how deep it sits.
    ///
    /// Depth up to the cap is drawn as real indentation by the renderer, so it
    /// is not spelled out here. Past the cap the indent has stopped moving,
    /// and then the number is the only thing that still distinguishes a reply
    /// four deep from one forty deep.
    #[must_use]
    pub fn byline(&self, now: i64) -> String {
        let author = if self.author.is_empty() {
            "[deleted]"
        } else {
            self.author.as_str()
        };
        let mut line = format!("{author} \u{b7} {}", age(now, self.created));
        if self.depth > u16::from(MAX_INDENT) {
            // The reply is further in than the gutter can show. Saying so is
            // the difference between a reader who knows they are deep in a
            // sub-thread and one who thinks the conversation changed subject.
            line.push_str(" \u{b7} reply ");
            line.push_str(&self.depth.to_string());
            line.push_str(" deep");
        }
        line.trim().to_owned()
    }
}

/// The host part of a URL, with `www.` dropped, or nothing worth showing.
///
/// Deliberately not a URL parser. The only question being asked is what to
/// print beside a title, so anything that does not look like an ordinary host
/// is answered with nothing rather than with a guess: a summary line is not
/// the place to find out that a stranger's `url` field was a sentence.
#[must_use]
fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .split('@')
        .next_back()?
        .split(':')
        .next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let plausible = !host.is_empty()
        && host.len() <= 60
        && host.contains('.')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
    plausible.then(|| host.to_ascii_lowercase())
}

/// One item exactly as Hacker News' own API gives it.
///
/// `item/<number>.json` is the site's own record of a story, a comment, a job
/// or a poll. It is the only source that is never behind: the search index
/// this application used to read from lags by minutes, which on a front page
/// that turns over in minutes meant a story could be on the site and not in
/// the list, and every score and comment count was slightly wrong.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Item {
    pub id: i64,
    /// `story`, `comment`, `job`, `poll` or `pollopt`.
    pub kind: String,
    pub by: String,
    pub time: i64,
    /// The body, already converted out of Hacker News' HTML.
    pub text: String,
    pub title: String,
    pub url: Option<String>,
    pub score: u32,
    /// How many comments the whole thread holds, on a story.
    pub descendants: u32,
    /// The replies, in the order the site draws them.
    ///
    /// This is the field that makes exact ordering possible. Hacker News ranks
    /// siblings by its own scoring, which is neither chronological nor
    /// anything a client can recompute, and `kids` is that ranking.
    pub kids: Vec<i64>,
    pub dead: bool,
    pub deleted: bool,
}

impl Item {
    /// This item as a list row, when it is something a list can show.
    #[must_use]
    pub fn story(&self) -> Option<Story> {
        if self.deleted || self.dead || self.title.is_empty() {
            return None;
        }
        Some(Story {
            id: self.id.to_string(),
            title: self.title.clone(),
            author: if self.by.is_empty() {
                "[deleted]".to_owned()
            } else {
                self.by.clone()
            },
            points: self.score,
            comments: self.descendants,
            created: self.time,
            text: (!self.text.is_empty()).then(|| self.text.clone()),
            site: self.url.as_deref().and_then(host_of),
        })
    }

    /// This item as a comment at `depth`, when it is one worth drawing.
    ///
    /// A deleted comment with replies under it keeps its place, because the
    /// replies are still answers to something and the site draws it the same
    /// way. A deleted comment with nothing under it is not drawn at all, which
    /// is also what the site does.
    #[must_use]
    pub fn comment(&self, depth: u16) -> Option<Comment> {
        let gone = self.deleted || self.dead;
        if gone && self.kids.is_empty() {
            return None;
        }
        if !gone && self.text.is_empty() && self.by.is_empty() {
            return None;
        }
        Some(Comment {
            author: if gone { String::new() } else { self.by.clone() },
            created: self.time,
            depth,
            body: if gone {
                "[deleted]".to_owned()
            } else {
                self.text.clone()
            },
        })
    }
}

/// Reads one `item/<number>.json` answer.
///
/// `null` is a real answer here: it is what the API says about an item number
/// that does not exist, and it has to be told apart from a request that failed.
#[must_use]
pub fn item_from(value: &kobo_json::Value) -> Option<Item> {
    let id = value.get("id").and_then(kobo_json::Value::as_i64)?;
    Some(Item {
        id,
        kind: value
            .get("type")
            .and_then(kobo_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        by: value
            .get("by")
            .and_then(kobo_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        time: value
            .get("time")
            .and_then(kobo_json::Value::as_i64)
            .unwrap_or_default(),
        text: value
            .get("text")
            .and_then(kobo_json::Value::as_str)
            .map(kobo_html::to_text)
            .unwrap_or_default(),
        title: value
            .get("title")
            .and_then(kobo_json::Value::as_str)
            .map(kobo_html::to_text)
            .unwrap_or_default(),
        url: value
            .get("url")
            .and_then(kobo_json::Value::as_str)
            .map(str::to_owned),
        score: count_of(value, "score"),
        descendants: count_of(value, "descendants"),
        kids: value
            .get("kids")
            .and_then(kobo_json::Value::as_array)
            .map(|kids| {
                kids.iter()
                    .filter_map(kobo_json::Value::as_i64)
                    .filter(|id| *id > 0)
                    .take(MAX_COMMENTS)
                    .collect()
            })
            .unwrap_or_default(),
        dead: flag(value, "dead"),
        deleted: flag(value, "deleted"),
    })
}

fn flag(value: &kobo_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(kobo_json::Value::as_bool)
        .unwrap_or(false)
}

/// Reads a count, clamping anything negative or absurd to something drawable.
fn count_of(value: &kobo_json::Value, key: &str) -> u32 {
    value
        .get(key)
        .and_then(kobo_json::Value::as_i64)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or_default()
}

/// How long ago something happened, in the coarsest unit that still says it.
///
/// Hacker News writes ages this way and readers of it expect the same. The
/// units stop at days because a story older than a year is not something this
/// application shows: the front page is hours old, and a search result's exact
/// date matters less than the fact that it is old.
#[must_use]
pub fn age(now: i64, then: i64) -> String {
    let seconds = now.saturating_sub(then);
    if seconds < 0 {
        // A clock that disagrees with the server. Saying "in 3 hours" about a
        // comment that already exists is worse than saying nothing.
        return "just now".to_owned();
    }
    match seconds {
        ..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        86_400..=2_591_999 => format!("{}d ago", seconds / 86_400),
        2_592_000..=31_535_999 => format!("{}mo ago", seconds / 2_592_000),
        _ => format!("{}y ago", seconds / 31_536_000),
    }
}

/// The second line of a story row: score, author, replies, age.
#[must_use]
pub fn summary(story: &Story, now: i64) -> String {
    // The author is dropped for a link story and kept for a self-post. On a
    // link the submitter is the least interesting fact on the line (the host
    // is what tells a reader whether to bother) and on an Ask HN the submitter
    // is the person being asked, so they are the whole point.
    let who = story.site.clone().unwrap_or_else(|| story.author.clone());
    format!(
        "{who} \u{b7} {} \u{b7} {} \u{b7} {}",
        plural(story.points, "point"),
        plural(story.comments, "comment"),
        age(now, story.created)
    )
}

/// `n thing`, or `n things`.
///
/// A brand new story is the common case on the Ask and Show pages now that
/// they carry the site's own ordering rather than the best of all time, and
/// "1 comments" beside it is the kind of small wrongness that makes a screen
/// look machine-made.
fn plural(count: u32, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{age, host_of, item_from, summary, Item, MAX_COMMENTS, MAX_INDENT};

    const STORY: &str = include_str!("../tests/item_story.json");
    const ASK: &str = include_str!("../tests/item_ask.json");
    const COMMENT: &str = include_str!("../tests/item_comment.json");

    fn parse(body: &str) -> kobo_json::Value {
        kobo_json::parse(body).expect("a captured response parses")
    }

    fn read(body: &str) -> Item {
        item_from(&parse(body)).expect("a captured item reads")
    }

    #[test]
    fn a_real_story_item_becomes_a_list_row() {
        // Captured from the site's own API rather than written by hand. A
        // field this application reads that the API stopped sending is a blank
        // row on the device and nothing anywhere else.
        let item = read(STORY);
        assert_eq!(item.kind, "story");
        let story = item.story().expect("a story is a row");
        assert_eq!(story.id, "49076057");
        assert_eq!(story.title, "Our position on open-weights models");
        assert_eq!(story.author, "surprisetalk");
        assert_eq!(story.points, 826);
        assert_eq!(story.comments, 1187);
        assert_eq!(story.created, 1_785_189_829);
        assert_eq!(story.site.as_deref(), Some("anthropic.com"));
        assert_eq!(story.text, None, "a link post has no self text");
    }

    #[test]
    fn a_real_ask_hn_carries_the_question_itself() {
        // An Ask post has no `url` and a `text` instead. Reading only `url`
        // would leave the entire question off the screen.
        let story = read(ASK).story().expect("an Ask post is a row");
        assert_eq!(story.site, None);
        let question = story.text.as_deref().expect("an Ask HN has a body");
        assert!(
            question.starts_with("Anthropic has released"),
            "the question did not survive: {question:.60}"
        );
        assert!(
            !question.contains("<p>") && !question.contains("&#x27;"),
            "markup reached the panel: {question:.120}"
        );
    }

    #[test]
    fn a_real_comment_item_keeps_the_order_the_site_draws_its_replies_in() {
        // `kids` is the whole reason this application reads the site's API
        // rather than a search index. The site ranks siblings by its own
        // scoring, which is neither chronological nor anything a client can
        // recompute, so the order has to survive parsing exactly.
        let item = read(COMMENT);
        let numbers = &item.kids;
        assert!(numbers.len() > 8, "only {} replies", numbers.len());
        assert_eq!(numbers[0], 49_079_084);
        assert_eq!(numbers[1], 49_076_830);
        assert!(
            numbers.windows(2).any(|pair| pair[0] > pair[1]),
            "the replies came back in ascending order, which is a sort, not \
             the site's ranking"
        );
        let comment = item.comment(1).expect("a comment is drawable");
        assert_eq!(comment.author, "vhantz");
        assert_eq!(comment.depth, 1);
        assert!(
            !comment.body.contains("&#x") && !comment.body.contains("<p>"),
            "an entity reached the panel: {:.120}",
            comment.body
        );
    }

    #[test]
    fn an_item_number_that_does_not_exist_is_told_apart_from_a_failure() {
        // The API answers `null` for a number that was never an item, and
        // that is a real answer. Treating it as a broken request would retry
        // it forever.
        assert!(item_from(&parse("null")).is_none());
        assert!(item_from(&parse(r#"{"by": "a", "type": "comment"}"#)).is_none());
    }

    #[test]
    fn a_deleted_comment_with_replies_keeps_its_place_and_one_without_does_not() {
        // The site draws a deleted comment that has answers under it, because
        // the answers are still answers to something. One with nothing under
        // it is not drawn at all.
        let orphan = read(r#"{"id": 1, "deleted": true, "type": "comment"}"#);
        assert!(orphan.comment(0).is_none());

        let parent = read(r#"{"id": 2, "deleted": true, "type": "comment", "kids": [3]}"#);
        let drawn = parent.comment(0).expect("a deleted parent still shows");
        assert_eq!(drawn.body, "[deleted]");
        assert!(drawn.author.is_empty());
    }

    #[test]
    fn a_dead_comment_is_treated_the_same_way_a_deleted_one_is() {
        // Flagged items come back with `dead: true` and their text intact.
        // Drawing that text would put on the panel the one thing the site
        // took off it.
        let dead = read(
            r#"{"id": 4, "dead": true, "type": "comment", "by": "a",
                            "text": "flagged", "kids": [5]}"#,
        );
        let drawn = dead.comment(0).expect("a dead parent still shows");
        assert_eq!(drawn.body, "[deleted]");
        assert!(
            read(r#"{"id": 6, "dead": true, "type": "story", "title": "T"}"#)
                .story()
                .is_none()
        );
    }

    #[test]
    fn an_empty_comment_is_dropped_rather_than_drawn_as_a_blank_row() {
        // An empty row on the panel reads as a rendering fault.
        let blank = read(r#"{"id": 7, "type": "comment"}"#);
        assert!(blank.comment(0).is_none());
    }

    #[test]
    fn a_story_with_no_identifier_or_no_title_is_dropped_rather_than_drawn() {
        // A row with nothing to tap and nowhere to go is worse than one fewer
        // story: the reader taps it, the panel refreshes, nothing happens.
        assert!(read(r#"{"id": 8, "type": "story", "by": "a"}"#)
            .story()
            .is_none());
    }

    #[test]
    fn a_reply_list_longer_than_the_ceiling_stops_at_the_ceiling() {
        // Unbounded is the failure mode that matters: one request per reply,
        // and a thread with fifteen hundred of them is fifteen hundred trips
        // over a radio that manages a few a second.
        let numbers: Vec<String> = (1..=MAX_COMMENTS + 50)
            .map(|number| number.to_string())
            .collect();
        let body = format!(
            r#"{{"id": 9, "type": "comment", "by": "a", "text": "t", "kids": [{}]}}"#,
            numbers.join(",")
        );
        assert_eq!(read(&body).kids.len(), MAX_COMMENTS);
    }

    #[test]
    fn a_reply_number_that_is_not_a_number_does_not_become_a_request() {
        // Every entry in `kids` turns into a URL this device asks for. A
        // string or a negative in there would be a request for something that
        // is not an item.
        let item = read(
            r#"{"id": 10, "type": "comment", "by": "a", "text": "t",
                            "kids": [1, "2", -3, null, 4]}"#,
        );
        assert_eq!(item.kids, vec![1, 4]);
    }

    #[test]
    fn a_deeply_nested_reply_is_capped_in_width_but_still_says_how_deep_it_is() {
        // The panel is 91 mm across. Eleven levels of real indentation is a
        // column of single words; dropping the replies instead would hide the
        // half of a thread that is usually the argument.
        let item = read(r#"{"id": 11, "type": "comment", "by": "a", "text": "deep"}"#);
        let deep = item.comment(38).expect("a deep reply is still a reply");
        assert!(
            deep.indent() <= MAX_INDENT,
            "indent {} past the ceiling",
            deep.indent()
        );
        assert!(
            deep.byline(0).contains("38 deep"),
            "depth past the gutter was not stated: {}",
            deep.byline(0)
        );
        let top = item.comment(0).expect("a top level comment draws");
        assert!(
            !top.byline(0).contains("deep"),
            "a top level comment was labelled as a reply"
        );
    }

    #[test]
    fn one_of_something_is_not_written_as_one_somethings() {
        let item = read(
            r#"{"id": 12, "type": "story", "title": "T", "by": "a",
                            "score": 1, "descendants": 1, "time": 0}"#,
        );
        assert_eq!(
            summary(&item.story().expect("a row"), 0),
            "a \u{b7} 1 point \u{b7} 1 comment \u{b7} just now"
        );
    }

    #[test]
    fn a_story_with_no_score_and_no_author_still_draws_a_second_line() {
        // Old items come back without either. Formatting a missing field as an
        // empty string leaves a row whose second line is punctuation.
        let item = read(r#"{"id": 13, "type": "story", "title": "T"}"#);
        assert_eq!(
            summary(&item.story().expect("a row"), 0),
            "[deleted] \u{b7} 0 points \u{b7} 0 comments \u{b7} just now"
        );
    }

    #[test]
    fn a_host_is_shown_the_way_the_site_shows_it() {
        assert_eq!(
            host_of("https://www.example.com/a/b"),
            Some("example.com".into())
        );
        assert_eq!(
            host_of("http://EXAMPLE.co.uk"),
            Some("example.co.uk".into())
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn ages_are_written_the_way_the_site_writes_them() {
        assert_eq!(age(1_000_000, 1_000_000), "just now");
        assert_eq!(age(1_000_000, 999_970), "just now");
        assert_eq!(age(1_000_000, 999_400), "10m ago");
        assert_eq!(age(1_000_000, 985_600), "4h ago");
        assert_eq!(age(1_000_000, 740_800), "3d ago");
        assert_eq!(age(100_000_000, 90_000_000), "3mo ago");
        assert_eq!(age(100_000_000, 1_000_000), "3y ago");
        // A device clock behind the server's, which happens on every Kobo that
        // has been asleep for a week.
        assert_eq!(age(1_000, 1_000_000), "just now");
        // No arithmetic overflow at the ends of the range.
        assert_eq!(age(i64::MAX, i64::MIN), "292471208677y ago");
    }
}
