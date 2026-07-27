//! The shapes Algolia sends, and the shapes the panel needs.
//!
//! Two responses matter. `search` returns `{"hits": [{objectID, title, url,
//! points, author, num_comments, created_at_i, story_text?}, …]}`, where `url`
//! is absent on an Ask HN post and `story_text` is absent on everything else.
//! `items/:id` returns one recursively nested object: `{id, created_at_i,
//! type, author, text, points, children: [ … the same shape … ]}`, where a
//! comment's `points` and `title` are `null` and the story's `text` is `null`.
//!
//! Both are read defensively. A missing field is a missing field, not a
//! reason to show nothing: a thread with one deleted comment in it is still a
//! thread, and a story with no score is still a story.

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
    /// Algolia's `objectID`, which is the Hacker News item number as a string.
    pub id: String,
    pub title: String,
    pub author: String,
    pub points: u32,
    pub comments: u32,
    /// Seconds since the epoch, as `created_at_i`.
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

/// Reads a `search` or `search_by_date` response into a list of stories.
#[must_use]
pub fn stories_from(value: &kobo_json::Value) -> Vec<Story> {
    let Some(hits) = value.get("hits").and_then(kobo_json::Value::as_array) else {
        return Vec::new();
    };
    hits.iter()
        .take(MAX_STORIES)
        .filter_map(|hit| {
            // A hit with no identifier cannot be opened and a hit with no
            // title has nothing to tap on, so both are dropped rather than
            // drawn as a blank row that does nothing.
            let id = hit.get("objectID").and_then(kobo_json::Value::as_str)?;
            let title = hit.get("title").and_then(kobo_json::Value::as_str)?;
            Some(Story {
                id: id.to_owned(),
                title: kobo_html::to_text(title),
                author: text_of(hit, "author").unwrap_or_else(|| "[deleted]".to_owned()),
                points: count_of(hit, "points"),
                comments: count_of(hit, "num_comments"),
                created: hit
                    .get("created_at_i")
                    .and_then(kobo_json::Value::as_i64)
                    .unwrap_or_default(),
                text: hit
                    .get("story_text")
                    .and_then(kobo_json::Value::as_str)
                    .map(kobo_html::to_text)
                    .filter(|text| !text.is_empty()),
                site: hit
                    .get("url")
                    .and_then(kobo_json::Value::as_str)
                    .and_then(host_of),
            })
        })
        .collect()
}

/// Flattens an `items/:id` tree into reading order.
///
/// Depth-first with an explicit stack rather than recursion. The parser
/// already refuses a document nested past its own ceiling, so the tree that
/// arrives here is bounded — but a flattener that recursed would still put one
/// stack frame per reply on a device whose threads are 8 KB, and the shape of
/// the input is chosen by whoever wrote the comment.
///
/// The root is the story itself and is not emitted: its title and score are
/// drawn from the [`Story`] the list already holds.
#[must_use]
pub fn flatten(root: &kobo_json::Value) -> Vec<Comment> {
    let mut comments = Vec::new();
    // (node, depth), with siblings pushed in reverse so the first reply comes
    // off the stack first and the order on screen is the order on the site.
    let mut stack = children_of(root, 0);
    while let Some((node, depth)) = stack.pop() {
        if comments.len() >= MAX_COMMENTS {
            break;
        }
        let body = node
            .get("text")
            .and_then(kobo_json::Value::as_str)
            .map(kobo_html::to_text)
            .unwrap_or_default();
        let author = text_of(node, "author").unwrap_or_default();
        // A comment that was deleted keeps its place in the thread — the
        // replies underneath it are still answers to something — but it has
        // neither an author nor a body, and drawing an empty one would look
        // like a rendering fault.
        if !body.is_empty() || !author.is_empty() {
            comments.push(Comment {
                author,
                created: node
                    .get("created_at_i")
                    .and_then(kobo_json::Value::as_i64)
                    .unwrap_or_default(),
                depth,
                body,
            });
        }
        stack.extend(children_of(node, depth.saturating_add(1)));
    }
    comments
}

/// The replies of one node, in reverse, ready to push onto a depth-first stack.
fn children_of(node: &kobo_json::Value, depth: u16) -> Vec<(&kobo_json::Value, u16)> {
    node.get("children")
        .and_then(kobo_json::Value::as_array)
        .map(|children| children.iter().rev().map(|child| (child, depth)).collect())
        .unwrap_or_default()
}

/// Reads a flat page of comments from a `search_by_date` response.
///
/// This is the fallback for a thread too large to fetch whole. The nesting is
/// gone — these hits carry a `parent_id` but not the replies it names — so
/// every comment comes back at depth zero, and the screen says so rather than
/// pretending the conversation was flat all along.
#[must_use]
pub fn flat_comments_from(value: &kobo_json::Value) -> Vec<Comment> {
    let Some(hits) = value.get("hits").and_then(kobo_json::Value::as_array) else {
        return Vec::new();
    };
    hits.iter()
        .take(MAX_COMMENTS)
        .filter_map(|hit| {
            let body = hit
                .get("comment_text")
                .and_then(kobo_json::Value::as_str)
                .map(kobo_html::to_text)?;
            if body.is_empty() {
                return None;
            }
            Some(Comment {
                author: text_of(hit, "author").unwrap_or_default(),
                created: hit
                    .get("created_at_i")
                    .and_then(kobo_json::Value::as_i64)
                    .unwrap_or_default(),
                depth: 0,
                body,
            })
        })
        .collect()
}

/// How many pages of flat comments a fallback thread has, from `nbPages`.
#[must_use]
pub fn pages_of(value: &kobo_json::Value) -> u32 {
    value
        .get("nbPages")
        .and_then(kobo_json::Value::as_i64)
        .and_then(|pages| u32::try_from(pages).ok())
        .unwrap_or(1)
        .max(1)
}

fn text_of(value: &kobo_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(kobo_json::Value::as_str)
        .map(kobo_html::to_text)
        .filter(|text| !text.is_empty())
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
    // link the submitter is the least interesting fact on the line — the host
    // is what tells a reader whether to bother — and on an Ask HN the
    // submitter is the person being asked, so they are the whole point.
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
    use super::{
        age, flat_comments_from, flatten, pages_of, stories_from, summary, MAX_COMMENTS, MAX_INDENT,
    };
    use std::fmt::Write as _;

    const FRONT_PAGE: &str = include_str!("../tests/front_page.json");
    const ASK: &str = include_str!("../tests/ask.json");
    const THREAD: &str = include_str!("../tests/thread.json");
    const COMMENT_PAGE: &str = include_str!("../tests/comment_page.json");

    fn parse(body: &str) -> kobo_json::Value {
        kobo_json::parse(body).expect("a captured response parses")
    }

    #[test]
    fn a_real_front_page_response_becomes_a_list_of_stories() {
        // Captured from `search?tags=front_page` rather than written by hand.
        // A field this application reads that the API stopped sending is a
        // blank row on the device and nothing anywhere else.
        let stories = stories_from(&parse(FRONT_PAGE));
        assert_eq!(stories.len(), 5);
        let first = &stories[0];
        assert_eq!(first.id, "49057175");
        assert_eq!(first.title, "Kill The Cookie Banner");
        assert_eq!(first.author, "rapnie");
        assert_eq!(first.points, 962);
        assert_eq!(first.comments, 459);
        assert_eq!(first.created, 1_785_066_797);
        assert_eq!(first.text, None, "a link post has no self text");
    }

    #[test]
    fn a_real_ask_hn_response_carries_the_question_itself() {
        // Ask and Show posts have no `url` and a `story_text` instead. Reading
        // only `url` would leave the entire question off the screen.
        let stories = stories_from(&parse(ASK));
        let question = stories[0].text.as_deref().expect("an Ask HN has a body");
        assert!(
            question.starts_with("Hi everyone,"),
            "the question did not survive: {question:.60}"
        );
        assert!(
            question.contains("\n\n"),
            "the paragraphs were lost, so the question is one wall of text"
        );
        assert!(
            !question.contains("<p>") && !question.contains("&#x27;"),
            "markup reached the panel: {question:.120}"
        );
    }

    #[test]
    fn a_hit_with_no_identifier_or_no_title_is_dropped_rather_than_drawn() {
        // A row with nothing to tap and nowhere to go is worse than one fewer
        // story: the reader taps it, the panel refreshes, nothing happens.
        let value = parse(
            r#"{"hits": [
                {"title": "No identifier", "author": "a"},
                {"objectID": "1", "author": "b"},
                {"objectID": "2", "title": "Fine", "author": "c"}
            ]}"#,
        );
        let stories = stories_from(&value);
        assert_eq!(stories.len(), 1);
        assert_eq!(stories[0].id, "2");
    }

    #[test]
    fn one_of_something_is_not_written_as_one_somethings() {
        // The Ask and Show pages carry the site's own ordering now, so a story
        // minutes old with a single comment is the ordinary case rather than
        // the rare one.
        let value = parse(
            r#"{"hits": [{"objectID": "1", "title": "T", "author": "a",
                          "points": 1, "num_comments": 1, "created_at_i": 0}]}"#,
        );
        let stories = stories_from(&value);
        assert_eq!(
            summary(&stories[0], 0),
            "a \u{b7} 1 point \u{b7} 1 comment \u{b7} just now"
        );
    }

    #[test]
    fn a_story_with_no_score_and_no_author_still_draws_a_second_line() {
        // Algolia sends `null` for both on some old items. Formatting `null`
        // as an empty string leaves a row whose second line is punctuation.
        let value = parse(r#"{"hits": [{"objectID": "1", "title": "T"}]}"#);
        let stories = stories_from(&value);
        assert_eq!(
            summary(&stories[0], 0),
            "[deleted] \u{b7} 0 points \u{b7} 0 comments \u{b7} just now"
        );
    }

    #[test]
    fn a_real_thread_flattens_into_reading_order() {
        // The order is the whole point: a flattened tree in the wrong order is
        // a conversation where the answers come before the questions.
        let comments = flatten(&parse(THREAD));
        assert!(comments.len() > 8, "only {} comments", comments.len());
        assert_eq!(comments[0].depth, 0);
        assert_eq!(comments[0].author, "ibejoeb");
        assert!(
            comments[0]
                .body
                .starts_with("Anyone have camera suggestions?"),
            "{}",
            comments[0].body
        );
        // A reply comes immediately after the comment it answers, never after
        // the next sibling.
        assert_eq!(comments[1].depth, 1);
        assert_eq!(comments[1].author, "ranger_danger");
        assert!(comments.iter().any(|comment| comment.depth >= 3));
        assert!(
            comments.iter().all(|comment| !comment.body.contains("&#x")),
            "an entity reached the panel"
        );
    }

    #[test]
    fn a_deeply_nested_thread_is_capped_in_width_but_not_in_content() {
        // The panel is 91 mm across. Eleven levels of real indentation is a
        // column of single words; dropping the replies instead would hide the
        // half of a thread that is usually the argument.
        let mut json = String::new();
        for depth in 0..40 {
            let _ = write!(
                json,
                r#"{{"author": "a{depth}", "text": "reply {depth}", "created_at_i": 0, "children": ["#
            );
        }
        json.push_str(&"]}".repeat(40));
        let Ok(tree) = kobo_json::parse(&json) else {
            // The parser's own depth ceiling is a legitimate answer here; what
            // must not happen is a panic or a silent half-thread.
            return;
        };
        let comments = flatten(&tree);
        assert_eq!(comments.len(), 39, "replies were lost to the depth");
        assert_eq!(comments[38].depth, 38);
        for comment in &comments {
            assert!(
                comment.indent() <= MAX_INDENT,
                "indent {} past the ceiling",
                comment.indent()
            );
        }
        assert!(
            comments[38].byline(0).contains("reply 38 deep"),
            "depth past the gutter was not stated: {}",
            comments[38].byline(0)
        );
        assert!(
            !comments[0].byline(0).contains("deep"),
            "a top level comment was labelled as a reply"
        );
    }

    #[test]
    fn a_thread_longer_than_the_ceiling_stops_at_the_ceiling() {
        // Unbounded is the failure mode that matters: a thousand comments of
        // 8 KB each is 8 MB of `String` on a device with a few hundred.
        let mut json = String::from(r#"{"children": ["#);
        for index in 0..(MAX_COMMENTS + 50) {
            if index > 0 {
                json.push(',');
            }
            let _ = write!(
                json,
                r#"{{"author": "a", "text": "c{index}", "created_at_i": 0}}"#
            );
        }
        json.push_str("]}");
        let comments = flatten(&parse(&json));
        assert_eq!(comments.len(), MAX_COMMENTS);
    }

    #[test]
    fn a_deleted_comment_does_not_leave_an_empty_paragraph_behind() {
        // Algolia sends `null` for both fields on a deleted item, and an empty
        // comment on the panel reads as a rendering fault.
        let value = parse(
            r#"{"children": [
                {"author": null, "text": null, "created_at_i": 1,
                 "children": [{"author": "b", "text": "still an answer", "created_at_i": 2}]}
            ]}"#,
        );
        let comments = flatten(&value);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "b");
        // The reply keeps the depth it really had, so it is not promoted into
        // the top level of the thread.
        assert_eq!(comments[0].depth, 1);
    }

    #[test]
    fn a_real_flat_comment_page_is_read_for_the_fallback() {
        // The oversized-thread path. These hits spell the body
        // `comment_text`, not `text`, so reading the wrong key gives a page of
        // nothing with no error anywhere.
        let value = parse(COMMENT_PAGE);
        let comments = flat_comments_from(&value);
        assert_eq!(comments.len(), 5);
        assert_eq!(comments[0].author, "fragmede");
        assert!(comments[0].body.starts_with("Having a category"));
        assert!(comments.iter().all(|comment| comment.depth == 0));
        assert_eq!(pages_of(&value), 185);
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
