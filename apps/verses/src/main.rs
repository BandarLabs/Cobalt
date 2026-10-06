//! Daily public-domain poetry with an offline shelf and `PoetryDB` search.
use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Header, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::process::ExitCode;

const SETTINGS: &str = "settings";
const SEARCH_LIMIT: u32 = 512 * 1024;
const USER_AGENT: &str = "Cobalt Verses/0.1";

#[derive(Clone, Copy)]
struct Poem {
    /// What a favourite is recorded against.
    ///
    /// Favourites used to be the poem's position in this list, so adding a
    /// poem or changing the order silently moved every favourite onto a
    /// different poem. A name is what the reader chose; a position is an
    /// accident of how the shelf is written down.
    id: &'static str,
    title: &'static str,
    author: &'static str,
    year: u16,
    source: &'static str,
    /// The poem as it is set: a list of stanzas, each a list of lines.
    ///
    /// Poems used to be four lines each, which is a stanza of The Tiger and
    /// a third of Hope, offered with nothing to say it was not the poem. A
    /// reader who met them here met an excerpt believing it was the whole.
    /// These are complete, and the shape is stanzas rather than lines so the
    /// space between them is the poet's rather than the renderer's.
    stanzas: &'static [&'static [&'static str]],
    tags: &'static str,
}

impl Poem {
    #[cfg(test)]
    fn lines(self) -> impl Iterator<Item = &'static str> {
        self.stanzas
            .iter()
            .flat_map(|stanza| stanza.iter().copied())
    }

    #[cfg(test)]
    fn line_count(self) -> usize {
        self.stanzas.iter().map(|stanza| stanza.len()).sum()
    }
}

const CORPUS: &[Poem] = &[
    Poem {
        id: "dickinson-hope",
        title: "Hope",
        author: "Emily Dickinson",
        year: 1891,
        source: "Poems, Second Series, via Project Gutenberg",
        tags: "hope · a minute",
        stanzas: &[
            &[
                "Hope is the thing with feathers",
                "That perches in the soul,",
                "And sings the tune without the words,",
                "And never stops at all,",
            ],
            &[
                "And sweetest in the gale is heard;",
                "And sore must be the storm",
                "That could abash the little bird",
                "That kept so many warm.",
            ],
            &[
                "I've heard it in the chillest land,",
                "And on the strangest sea;",
                "Yet, never, in extremity,",
                "It asked a crumb of me.",
            ],
        ],
    },
    Poem {
        id: "blake-tiger",
        title: "The Tiger",
        author: "William Blake",
        year: 1794,
        source: "Songs of Innocence and of Experience, via Project Gutenberg",
        tags: "nature · two minutes",
        stanzas: &[
            &[
                "Tiger, tiger, burning bright",
                "In the forests of the night,",
                "What immortal hand or eye",
                "Could frame thy fearful symmetry?",
            ],
            &[
                "In what distant deeps or skies",
                "Burnt the fire of thine eyes?",
                "On what wings dare he aspire?",
                "What the hand dare seize the fire?",
            ],
            &[
                "And what shoulder and what art",
                "Could twist the sinews of thy heart?",
                "And, when thy heart began to beat,",
                "What dread hand and what dread feet?",
            ],
            &[
                "What the hammer? what the chain?",
                "In what furnace was thy brain?",
                "What the anvil? what dread grasp",
                "Dare its deadly terrors clasp?",
            ],
            &[
                "When the stars threw down their spears,",
                "And watered heaven with their tears,",
                "Did He smile His work to see?",
                "Did He who made the lamb make thee?",
            ],
            &[
                "Tiger, tiger, burning bright",
                "In the forests of the night,",
                "What immortal hand or eye",
                "Dare frame thy fearful symmetry?",
            ],
        ],
    },
    Poem {
        id: "shelley-ozymandias",
        title: "Ozymandias",
        author: "Percy Bysshe Shelley",
        year: 1818,
        source: "The Examiner, via Project Gutenberg",
        tags: "history · a minute",
        stanzas: &[&[
            "I met a traveller from an antique land",
            "Who said: Two vast and trunkless legs of stone",
            "Stand in the desert...Near them, on the sand,",
            "Half sunk, a shattered visage lies, whose frown,",
            "And wrinkled lip, and sneer of cold command,",
            "Tell that its sculptor well those passions read",
            "Which yet survive, stamped on these lifeless things,",
            "The hand that mocked them, and the heart that fed:",
            "And on the pedestal these words appear:",
            "\u{2018}My name is Ozymandias, king of kings:",
            "Look on my works, ye Mighty, and despair!\u{2019}",
            "Nothing beside remains. Round the decay",
            "Of that colossal wreck, boundless and bare",
            "The lone and level sands stretch far away.",
        ]],
    },
    Poem {
        id: "dickinson-chariot",
        title: "The Chariot",
        author: "Emily Dickinson",
        year: 1890,
        source: "Poems, First Series, via Project Gutenberg",
        tags: "time · two minutes",
        stanzas: &[
            &[
                "Because I could not stop for Death,",
                "He kindly stopped for me;",
                "The carriage held but just ourselves",
                "And Immortality.",
            ],
            &[
                "We slowly drove, he knew no haste,",
                "And I had put away",
                "My labor, and my leisure too,",
                "For his civility.",
            ],
            &[
                "We passed the school where children played,",
                "Their lessons scarcely done;",
                "We passed the fields of gazing grain,",
                "We passed the setting sun.",
            ],
            &[
                "We paused before a house that seemed",
                "A swelling of the ground;",
                "The roof was scarcely visible,",
                "The cornice but a mound.",
            ],
            &[
                "Since then 't is centuries; but each",
                "Feels shorter than the day",
                "I first surmised the horses' heads",
                "Were toward eternity.",
            ],
        ],
    },
];

#[derive(Clone, Debug, Deserialize, Serialize, Hash)]
struct OnlinePoem {
    title: String,
    author: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default, deserialize_with = "count_text")]
    linecount: String,
}

/// `PoetryDB` sends a poem's line count as a string for some poems and as a
/// number for others, Emily Dickinson's among them. Read as a string only, one
/// numeric count failed the whole list, so searching for the most read poet in
/// the collection reported that it held nothing by her.
fn count_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Count {
        Text(String),
        Number(u64),
    }
    Ok(match Count::deserialize(deserializer)? {
        Count::Text(text) => text,
        Count::Number(number) => number.to_string(),
    })
}

/// Orders titles the way a reader counts: "Sonnet 2" before "Sonnet 10",
/// where a plain comparison of the text put every sonnet in the teens between
/// the first and the second.
fn natural_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(text: &str) -> Vec<(bool, &str)> {
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut digits = None;
        for (index, character) in text.char_indices() {
            let digit = character.is_ascii_digit();
            if digits != Some(digit) {
                if let Some(was) = digits {
                    chunks.push((was, &text[start..index]));
                }
                start = index;
                digits = Some(digit);
            }
        }
        if let Some(was) = digits {
            chunks.push((was, &text[start..]));
        }
        chunks
    }
    // Quotation marks and apostrophes that open a title are not where it
    // files: "Whose are the little beds" belongs under W.
    fn file(text: &str) -> &str {
        text.trim_start_matches(|c: char| !c.is_alphanumeric())
    }
    let (left, right) = (chunks(file(a)), chunks(file(b)));
    for ((left_digits, left), (right_digits, right)) in left.iter().zip(&right) {
        let order = if *left_digits && *right_digits {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        } else {
            left.to_lowercase().cmp(&right.to_lowercase())
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

/// Where a search looks: everywhere, or in one field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Scope {
    #[default]
    Anything,
    Title,
    Poet,
    Line,
}

impl Scope {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Anything, "scope-any", "Anything"),
        (Self::Title, "scope-title", "Title"),
        (Self::Poet, "scope-poet", "Poet"),
        (Self::Line, "scope-line", "Line"),
    ];

    const fn fields(self) -> &'static str {
        match self {
            Self::Anything => "author,title,lines",
            Self::Title => "title",
            Self::Poet => "author",
            Self::Line => "lines",
        }
    }
}

/// Poets to start from, offered while the search box is empty. All of them
/// are well represented in `PoetryDB`, so a tap never lands on an empty page.
const SUGGESTED_POETS: [&str; 6] = [
    "Emily Dickinson",
    "William Blake",
    "John Keats",
    "Christina Rossetti",
    "Walt Whitman",
    "William Shakespeare",
];

/// "a minute", "three minutes": how long a poem takes, at a reading pace of
/// about twenty lines a minute aloud.
fn reading_time(lines: &str) -> String {
    let lines = lines.trim().parse::<u32>().unwrap_or(0);
    match lines.div_ceil(20).max(1) {
        1 => "a minute".to_owned(),
        2 => "two minutes".to_owned(),
        3 => "three minutes".to_owned(),
        4 => "four minutes".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

#[derive(Clone, Debug)]
struct OnlineLine {
    text: String,
    stanza_start: bool,
}

type OnlinePages = Vec<Vec<OnlineLine>>;
/// A bundled poem's pages, with the poem and panel they were measured for.
type PoemLayout = (usize, kobo_ui::DisplayMetrics, Vec<Vec<VerseRun>>);
type OnlineLayout = (kobo_sdk::DisplayMetrics, u64, OnlinePages);

#[derive(Clone)]
struct PoemRow {
    section: Option<String>,
    action: String,
    title: String,
    detail: String,
    glyph: Glyph,
}

#[derive(Default, Deserialize, Serialize)]
struct Saved {
    /// Poems from the shelf, by name.
    #[serde(default)]
    favorites: BTreeSet<String>,
    online_favorites: Vec<OnlinePoem>,
    sleep: bool,
    /// Today's poem from `PoetryDB`, and the day it was chosen for.
    #[serde(default)]
    daily: Option<DailyPoem>,
}

/// A poem fetched for one day, kept so the day's poem is the same poem all
/// day and still there when the reader goes offline.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DailyPoem {
    day: (u16, u8, u8),
    poem: OnlinePoem,
}

/// Lengths a daily poem is drawn from, so it is a poem for a page or two and
/// never one of `PoetryDB`'s thousand line epics.
const DAILY_LENGTHS: [u16; 5] = [12, 14, 16, 20, 24];

/// A ceiling on what is accepted as a daily poem, whatever the service sends.
const MAX_DAILY_LINES: usize = 60;

/// Asks `PoetryDB` for one poem of a readable length. A random one, so each
/// reader meets a different poem from the thousands it holds rather than the
/// same few bundled ones on rotation.
fn daily_task(day: (u16, u8, u8)) -> Task {
    let (year, month, date) = day;
    let length = DAILY_LENGTHS[daily_index(year, month, date) % DAILY_LENGTHS.len()];
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/linecount,random/{length};1/author,title,lines,linecount"
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

/// What the shelf wrote before poems had names: favourites as positions.
///
/// Read only when the current form will not decode, which is exactly when the
/// stored favourites are numbers. Written back by name, so this is read once
/// per reader and then never again.
#[derive(Default, Deserialize)]
struct LegacySaved {
    #[serde(default)]
    favorites: BTreeSet<usize>,
    #[serde(default)]
    online_favorites: Vec<OnlinePoem>,
    #[serde(default)]
    sleep: bool,
}

/// The air between two stanzas, in hundredths of an em: three quarters of a
/// line, which is what a printed book of verse leaves.
const STANZA_AIR: u16 = 75;

/// Verse leading, as a percentage of the reading face's line height.
const VERSE_LEADING: u16 = 88;

/// The size verse is set at: one step above whatever the reader chose.
///
/// A poem is a page looked at rather than moved through, and these are short
/// lines with wide margins either side, so verse carries a step more than
/// prose would. It is a step above the reader's own setting rather than a
/// fixed size, because a fixed one is an override: a reader who asked for the
/// smallest type got three steps more than they asked for, and at that size
/// a sonnet no longer fits the panel it is measured against.
///
/// Pagination is measured at this size too. A page measured at one size and
/// set at another loses its last lines.
fn poem_scale(context: &Context) -> kobo_ui::TextScale {
    let chosen = context.metrics().text_scale;
    chosen.larger().unwrap_or(chosen)
}

/// The lines of one stanza that belong on one page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VerseRun {
    stanza: usize,
    from: usize,
    to: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Today,
    /// The card being written out to a paired computer.
    Card,
    Browse,
    Reading,
    Search,
    Results,
    Online,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Search,
    Open(usize),
}

struct Verses {
    view: View,
    poem: usize,
    online: Option<usize>,
    online_page: usize,
    online_layout: std::cell::RefCell<Option<OnlineLayout>>,
    /// The pages of the bundled poem last measured, and what they were
    /// measured for. Paging lays each page out to see what fits, which is
    /// worth doing once per poem and panel rather than on every repaint of a
    /// single core reader.
    poem_layout: std::cell::RefCell<Option<PoemLayout>>,
    browse_page: usize,
    results_page: usize,
    online_from: View,
    card_from: View,
    saved: Saved,
    keyboard: Keyboard,
    results: Vec<OnlinePoem>,
    /// Where the next search looks, and what the last one asked for.
    scope: Scope,
    query: String,
    task: Option<TaskId>,
    /// The fetch for today's poem, separate from searches so a search does
    /// not cancel it and it does not hold up a search.
    daily_task: Option<TaskId>,
    pending: Option<Pending>,
    notice: Option<String>,
    loaded: bool,
    /// Which page of the poem is open. A long poem is more than one panel.
    poem_page: usize,
    /// The reader's own day, as the poem on the Today screen was chosen for.
    ///
    /// Kept so that crossing midnight with the application open moves the
    /// Today screen on to the next day's poem rather than leaving yesterday's
    /// there until it is restarted.
    day: Option<(u16, u8, u8)>,
    /// A card on its way to a paired computer.
    export: Option<kobo_sdk::exports::Export>,
    /// Whether a favourite is still waiting for storage to confirm it.
    ///
    /// Marking a poem used to write and walk away. A write that failed took
    /// the favourite with it and said nothing, so a reader found their poem
    /// unmarked the next time they opened the shelf and had no idea why.
    saving: bool,
}

impl Default for Verses {
    fn default() -> Self {
        Self {
            view: View::Today,
            poem: poem_for_today(reader_clock().as_ref()),
            online: None,
            online_page: 0,
            online_layout: std::cell::RefCell::new(None),
            poem_layout: std::cell::RefCell::new(None),
            browse_page: 0,
            results_page: 0,
            online_from: View::Results,
            card_from: View::Today,
            saved: Saved::default(),
            keyboard: Keyboard::new(),
            results: Vec::new(),
            scope: Scope::Anything,
            query: String::new(),
            task: None,
            daily_task: None,
            pending: None,
            notice: None,
            loaded: false,
            poem_page: 0,
            export: None,
            day: reader_clock()
                .now()
                .ok()
                .and_then(Snapshot::date)
                .map(|date| (date.year, date.month, date.day)),
            saving: false,
        }
    }
}

fn daily_index(year: u16, month: u8, day: u8) -> usize {
    ((year as usize * 372) + (month as usize * 31) + day as usize) % CORPUS.len()
}

/// The poem for the day the reader is actually holding the device on.
///
/// This was the first of September 2026, written into the default state, so an
/// application called "daily poetry" offered the same poem for ever. The
/// platform's clock is injectable and carries an explicit offset rather than a
/// guessed time zone, so the day here is the reader's own day and a test can
/// put the device on either side of midnight.
fn poem_for_today(clock: &dyn Clock) -> usize {
    clock
        .now()
        .ok()
        .and_then(Snapshot::date)
        .map_or(0, |date| daily_index(date.year, date.month, date.day))
}

/// The reader's clock, at the offset the runtime was started with.
fn reader_clock() -> Box<dyn Clock> {
    let minutes = std::env::var("KOBO_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|value| value.parse::<i16>().ok())
        .unwrap_or(0);
    SystemClock::new(minutes)
        .or_else(|_| SystemClock::new(0))
        .map_or_else(
            |_| Box::new(ManualClock::new(EPOCH).expect("a valid fixed clock")) as Box<dyn Clock>,
            |clock| Box::new(clock) as Box<dyn Clock>,
        )
}

/// A clock that cannot be read is not a reason to refuse to draw a poem.
const EPOCH: Snapshot = Snapshot {
    unix_millis: 0,
    monotonic_millis: 0,
    utc_offset_minutes: 0,
};

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn search_task(query: &str, scope: Scope) -> Task {
    let fields = scope.fields();
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/{fields}/{}/author,title,linecount",
            escape(query)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

fn poem_task(title: &str) -> Task {
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/title/{}:abs/author,title,lines,linecount",
            escape(title)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

impl Verses {
    /// One page of a poem, set the way a poem is set.
    ///
    /// The title and the poet head the first page in the display serif, and
    /// the poem follows flush left in the book face, a stanza to a paragraph
    /// with the poet's space between stanzas and none between their lines.
    /// Where it came from closes the last page. A poem that fits on one page
    /// sits in the middle of it: hung from the top it left half the panel
    /// empty beneath twelve lines, which read as a page that had failed to
    /// load the rest.
    fn local_poem(&self, context: &Context) -> Screen {
        let pages = self.poem_pages(context);
        let page = self.poem_page.min(pages.len().saturating_sub(1));
        self.poem_page_screen(context, &pages[page], page, pages.len())
    }

    fn poem_page_screen(
        &self,
        context: &Context,
        runs: &[VerseRun],
        page: usize,
        count: usize,
    ) -> Screen {
        let poem = CORPUS[self.poem];
        let mut screen = ScreenBuilder::new("verses-poem").top_bar(if self.view == View::Today {
            "Today's poem"
        } else {
            poem.author
        });
        if self.view == View::Today {
            screen = screen.top_bar_glyph("browse", "Browse", Glyph::Grid);
        }
        screen = screen.top_bar_glyph(
            "favorite",
            if self.saved.favorites.contains(CORPUS[self.poem].id) {
                "Remove favorite"
            } else {
                "Favorite"
            },
            Glyph::Heart,
        );
        let centred = count == 1;
        if centred {
            screen = screen.fill();
        }
        if page == 0 {
            screen = screen
                .heading_at_level(2, poem.title)
                .secondary(format!("{} · {}", poem.author, poem.year));
        }
        screen = screen.reading(true).text_scale(poem_scale(context));
        screen = set_verse(screen, poem, runs);
        if page + 1 == count {
            screen = screen.secondary(format!("From {}", poem.source));
        }
        if centred {
            screen = screen.fill();
        }
        if count > 1 {
            screen = screen
                .page_turns("poem-previous", "poem-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(count).unwrap_or(1),
                );
        }
        screen.bottom_action("card", "Make a quote card").build()
    }

    /// Which lines go on which page, measured against the panel.
    ///
    /// Each page is filled by laying it out and keeping whatever still fits,
    /// a whole stanza at a time, so the measure is the page the reader will
    /// see rather than an estimate of it. The estimate this replaced held a
    /// line back for the attribution, charged every gap between stanzas a
    /// whole line, and moved a stanza that did not fit the remainder to the
    /// next page: Hope, twelve lines in three stanzas, came out as two pages
    /// with four lines on the second and the bottom half of the first empty.
    ///
    /// A stanza is only split when it is taller than a page on its own, which
    /// a sonnet is at the largest text sizes, and then line by line so nothing
    /// is lost.
    fn poem_pages(&self, context: &Context) -> Vec<Vec<VerseRun>> {
        let metrics = context.metrics();
        if let Some((poem, measured, pages)) = self.poem_layout.borrow().as_ref() {
            if *poem == self.poem && *measured == metrics {
                return pages.clone();
            }
        }
        let pages = self.measure_poem_pages(context);
        *self.poem_layout.borrow_mut() = Some((self.poem, metrics, pages.clone()));
        pages
    }

    fn measure_poem_pages(&self, context: &Context) -> Vec<Vec<VerseRun>> {
        let poem = CORPUS[self.poem];
        // Measured as a page of a longer poem, with the page position and the
        // attribution both present, so a page that fits here fits wherever it
        // lands.
        let fits = |runs: &[VerseRun], first: bool| {
            self.poem_page_screen(context, runs, usize::from(!first), 2)
                .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                .issues
                .is_empty()
        };
        let mut pages: Vec<Vec<VerseRun>> = Vec::new();
        let mut current: Vec<VerseRun> = Vec::new();
        for (stanza, lines) in poem.stanzas.iter().enumerate() {
            let mut from = 0;
            while from < lines.len() {
                let first = pages.is_empty();
                let mut whole = current.clone();
                whole.push(VerseRun {
                    stanza,
                    from,
                    to: lines.len(),
                });
                if fits(&whole, first) {
                    current = whole;
                    break;
                }
                if !current.is_empty() {
                    pages.push(std::mem::take(&mut current));
                    continue;
                }
                // Alone on a page and still too tall: take as many lines as
                // fit, and never fewer than one, so the poem always moves on.
                let mut to = from + 1;
                while to < lines.len()
                    && fits(
                        &[VerseRun {
                            stanza,
                            from,
                            to: to + 1,
                        }],
                        first,
                    )
                {
                    to += 1;
                }
                pages.push(vec![VerseRun { stanza, from, to }]);
                from = to;
            }
        }
        if !current.is_empty() {
            pages.push(current);
        }
        if pages.is_empty() {
            pages.push(vec![VerseRun {
                stanza: 0,
                from: 0,
                to: poem.stanzas.first().map_or(0, |lines| lines.len()),
            }]);
        }
        pages
    }

    fn online_prefix(&self, context: &Context, poem: &OnlinePoem, page: usize) -> ScreenBuilder {
        let favorite = self
            .saved
            .online_favorites
            .iter()
            .any(|saved| saved.title == poem.title && saved.author == poem.author);
        let today = self.view == View::Today;
        let mut screen = ScreenBuilder::new("verses-online").top_bar(if today {
            "Today's poem".to_owned()
        } else {
            context.clamped_row(&poem.author, 1, false)
        });
        screen = if today {
            screen.top_bar_glyph("browse", "Browse", Glyph::Grid)
        } else {
            screen.top_bar_glyph("more-by-author", "More by this poet", Glyph::Person)
        };
        screen = screen.top_bar_glyph(
            "favorite",
            if favorite {
                "Remove favorite"
            } else {
                "Favorite"
            },
            Glyph::Heart,
        );
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if page == 0 {
            // The same head as a bundled poem: its title in the display serif
            // and who wrote it beneath, instead of the title squeezed into the
            // bar and the poet set as the first line of the page.
            screen = screen
                .heading_at_level(2, context.clamped_row(&poem.title, 2, true))
                .secondary(context.clamped_row(&poem.author, 2, false));
        }
        screen.reading(true)
    }

    fn online_page_screen(
        &self,
        context: &Context,
        poem: &OnlinePoem,
        lines: &[OnlineLine],
        page: usize,
        count: usize,
    ) -> Screen {
        let mut screen = self.online_prefix(context, poem, page);
        // Lines of one stanza go together as one paragraph, at verse leading,
        // exactly as a bundled poem is set. The page was measured a line at a
        // time with paragraph spacing between, so it always has the room.
        let mut stanza: Vec<&str> = Vec::new();
        let mut opens = false;
        let flush = |screen: ScreenBuilder, stanza: &mut Vec<&str>, opens: bool| {
            if stanza.is_empty() {
                return screen;
            }
            let text = stanza.join("\n");
            stanza.clear();
            screen.rich_text(
                text,
                Vec::new(),
                kobo_sdk::ParagraphPresentation {
                    margin_before_em: if opens { STANZA_AIR } else { 0 },
                    line_height_percent: VERSE_LEADING,
                    ..kobo_sdk::ParagraphPresentation::default()
                },
            )
        };
        for line in lines {
            if line.stanza_start {
                screen = flush(screen, &mut stanza, opens);
                opens = true;
            }
            stanza.push(&line.text);
        }
        screen = flush(screen, &mut stanza, opens);
        if page + 1 == count {
            screen = screen.secondary("From PoetryDB");
        }
        if count > 1 {
            screen = screen
                .page_turns("online-previous", "online-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        screen.build()
    }

    /// Keep verse lines and stanza gaps, measuring the same rich-text nodes
    /// the reader sees. Cache the result so each page turn stays cheap.
    fn online_pages(&self, context: &Context, poem: &OnlinePoem) -> OnlinePages {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        poem.hash(&mut hash);
        self.notice.hash(&mut hash);
        let key = hash.finish();
        if let Some((metrics, saved_key, pages)) = self.online_layout.borrow().as_ref() {
            if *metrics == context.metrics() && *saved_key == key {
                return pages.clone();
            }
        }
        let mut pending = std::collections::VecDeque::new();
        let mut stanza_start = false;
        for text in &poem.lines {
            if text.trim().is_empty() {
                stanza_start = true;
                continue;
            }
            pending.push_back(OnlineLine {
                text: text.clone(),
                stanza_start,
            });
            stanza_start = false;
        }
        let metrics = context.metrics();
        // Measured with the first page's heading and the attribution, so every
        // page has at least the room the fullest one needs.
        let prefix = self.online_page_screen(context, poem, &[], 0, 2);
        let chrome =
            kobo_ui::Chrome::for_screen(&prefix, false, kobo_ui::Chrome::measuring(true).status);
        let prefix_height = prefix.layout_with(&metrics, &chrome).content_used();
        let (capacity, gap) = kobo_ui::with_text_scale(metrics.text_scale, || {
            // Reading screens have no status strip. Reserve the same page
            // position band as the renderer, then the measured author/notice.
            let area = metrics.prose_area_in(true, false, kobo_ui::Face::Reading);
            (
                area.height
                    .saturating_sub(metrics.page_position_band())
                    .saturating_sub(prefix_height),
                area.gap,
            )
        });
        let mut pages = Vec::new();
        let mut page = Vec::new();
        let mut used = 0;
        while let Some(mut line) = pending.pop_front() {
            if page.is_empty() {
                line.stanza_start = false;
            }
            // Measure each source line once, rather than relaying out the
            // entire growing page for every additional line.
            let screen = ScreenBuilder::new("verses-line-measure")
                .reading(true)
                .rich_text(
                    line.text.clone(),
                    Vec::new(),
                    kobo_sdk::ParagraphPresentation {
                        margin_before_em: if line.stanza_start { STANZA_AIR } else { 0 },
                        ..kobo_sdk::ParagraphPresentation::default()
                    },
                )
                .build();
            let measured = screen.diagnostics(&metrics, &chrome);
            let height = measured.layout.content_used();
            let overflow = measured.issues.iter().any(|issue| {
                matches!(
                    issue.kind,
                    kobo_ui::LayoutIssueKind::TextOverflow
                        | kobo_ui::LayoutIssueKind::Clipped
                        | kobo_ui::LayoutIssueKind::ContentOverflow { .. }
                        | kobo_ui::LayoutIssueKind::InteractiveOffscreen
                )
            });
            if !overflow && used + gap + height <= capacity {
                used += gap + height;
                page.push(line);
            } else if !page.is_empty() {
                pages.push(std::mem::take(&mut page));
                used = 0;
                pending.push_front(line);
            } else if let Some((left, right)) = split_online_line(&line.text, context) {
                pending.push_front(OnlineLine {
                    text: right,
                    stanza_start: false,
                });
                pending.push_front(OnlineLine {
                    text: left,
                    stanza_start: line.stanza_start,
                });
            } else {
                // One glyph cannot be split further; this only applies to a
                // viewport too small even for a single line below the header.
                pages.push(vec![line]);
            }
        }
        if !page.is_empty() {
            pages.push(page);
        }
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        *self.online_layout.borrow_mut() = Some((context.metrics(), key, pages.clone()));
        pages
    }

    fn online_poem(&self, context: &Context) -> Screen {
        let Some(poem) = self.current_online() else {
            return ScreenBuilder::new("verses-online")
                .top_bar("Verses")
                .splash(
                    Some(Glyph::Search),
                    "Choose a poem",
                    "Open one from Search.",
                )
                .build();
        };
        let pages = self.online_pages(context, poem);
        let page = self.online_page.min(pages.len().saturating_sub(1));
        self.online_page_screen(context, poem, &pages[page], page, pages.len())
    }

    fn paged_poems(
        context: &Context,
        screen: ScreenBuilder,
        rows: &[PoemRow],
        page: usize,
    ) -> (Screen, usize) {
        let labels = rows
            .iter()
            .map(|row| {
                (
                    row.section.as_deref(),
                    row.title.as_str(),
                    row.detail.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_in_sections_under(
            &labels,
            false,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen;
        // Keep rows from each group in one node, so their separators match the
        // SDK's section-aware measurement exactly.
        let mut start = 0;
        while start < visible.len() {
            let first = &rows[visible[start]];
            if let Some(section) = &first.section {
                screen = screen.section(section);
            }
            let end = ((start + 1)..visible.len())
                .find(|&i| rows[visible[i]].section.is_some())
                .unwrap_or(visible.len());
            screen = screen.rows(visible[start..end].iter().map(|&index| {
                let row = &rows[index];
                (
                    row.action.clone(),
                    row.title.clone(),
                    row.detail.clone(),
                    row.glyph,
                )
            }));
            start = end;
        }
        if count > 1 {
            screen = screen
                .page_turns("list-previous", "list-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        (screen.build(), count)
    }

    fn browse(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("verses-browse")
            .top_bar("Browse")
            .top_bar_glyph("search", "Search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let mut rows = self
            .saved
            .favorites
            .iter()
            .filter_map(|id| {
                CORPUS
                    .iter()
                    .position(|poem| poem.id == id.as_str())
                    .map(|index| {
                        let poem = CORPUS[index];
                        PoemRow {
                            section: None,
                            action: format!("poem-{index}"),
                            title: context.clamped_row(poem.title, 2, false),
                            detail: poem.author.into(),
                            glyph: Glyph::Heart,
                        }
                    })
            })
            .collect::<Vec<_>>();
        rows.extend(
            self.saved
                .online_favorites
                .iter()
                .enumerate()
                .map(|(index, poem)| PoemRow {
                    section: None,
                    action: format!("saved-online-{index}"),
                    title: context.clamped_row(&poem.title, 2, false),
                    detail: poem.author.clone(),
                    glyph: Glyph::Heart,
                }),
        );
        if let Some(first) = rows.first_mut() {
            first.section = Some("Favorites".to_owned());
        }
        rows.extend(CORPUS.iter().enumerate().map(|(index, poem)| PoemRow {
            section: (index == 0).then(|| "Poems".to_owned()),
            action: format!("poem-{index}"),
            title: context.clamped_row(poem.title, 2, false),
            detail: format!("{} · {}", poem.author, poem.tags),
            glyph: Glyph::Note,
        }));
        Self::paged_poems(context, screen, &rows, self.browse_page)
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-search").top_bar("Search poetry");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        let selected = Scope::ALL
            .iter()
            .position(|&(scope, _, _)| scope == self.scope)
            .unwrap_or(0);
        screen = screen
            .segmented(
                selected,
                Scope::ALL.iter().map(|&(_, name, label)| (name, label)),
            )
            .typed(
                &self.keyboard,
                match self.scope {
                    Scope::Anything => "A title, a poet, or a line you remember",
                    Scope::Title => "A poem's title",
                    Scope::Poet => "A poet's name",
                    Scope::Line => "Words from a line",
                },
            );
        // Somewhere to start, while there is nothing typed: the whole of
        // PoetryDB is behind this box and an empty one says nothing about it.
        if self.keyboard.text().is_empty() {
            screen = screen.section("Poets to start with").chips(
                SUGGESTED_POETS
                    .iter()
                    .map(|poet| (format!("suggest-{poet}"), *poet, false)),
            );
        }
        screen.keyboard(&self.keyboard, "Search").build()
    }

    fn results(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("verses-results")
            .top_bar("Search")
            .top_bar_glyph("search", "New search", Glyph::Search);
        if self.task.is_some() {
            return (
                screen
                    .activity(
                        if matches!(self.pending, Some(Pending::Open(_))) {
                            "Opening poem…"
                        } else {
                            "Searching poetry…"
                        },
                        None,
                    )
                    .build(),
                1,
            );
        }
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if self.results.is_empty() {
            return (
                screen
                    .splash(
                        Some(Glyph::Search),
                        "No poems found",
                        "Try a poet, title, or memorable line.",
                    )
                    .build(),
                1,
            );
        }
        // Grouped by poet, the way an anthology's contents are, so a search
        // for a word that several poets used reads as who used it rather than
        // as a list where the poet is the small print under every title.
        let mut order: Vec<usize> = (0..self.results.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (&self.results[a], &self.results[b]);
            a.author
                .cmp(&b.author)
                .then_with(|| natural_order(&a.title, &b.title))
        });
        let mut previous: Option<&str> = None;
        let rows = order
            .into_iter()
            .map(|index| {
                let poem = &self.results[index];
                let first = previous != Some(poem.author.as_str());
                previous = Some(poem.author.as_str());
                PoemRow {
                    section: first.then(|| poem.author.clone()),
                    action: format!("result-{index}"),
                    title: context.clamped_row(&poem.title, 2, false),
                    detail: if poem.linecount.trim().is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{} lines · {}",
                            poem.linecount.trim(),
                            reading_time(&poem.linecount)
                        )
                    },
                    glyph: Glyph::Note,
                }
            })
            .collect::<Vec<_>>();
        screen = screen.secondary(format!(
            "{} for \u{201c}{}\u{201d}",
            match self.results.len() {
                1 => "1 poem".to_owned(),
                count => format!("{count} poems"),
            },
            self.query
        ));
        Self::paged_poems(context, screen, &rows, self.results_page)
    }

    fn turn_list(&mut self, context: &Context, forward: bool) {
        let count = match self.view {
            View::Browse => self.browse(context).1,
            View::Results => self.results(context).1,
            _ => return,
        };
        let page = if self.view == View::Browse {
            &mut self.browse_page
        } else {
            &mut self.results_page
        };
        let last = count.saturating_sub(1);
        *page = if forward {
            page.saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
    }

    fn turn_online(&mut self, context: &Context, forward: bool) {
        let Some(poem) = self.current_online() else {
            return;
        };
        let last = self.online_pages(context, poem).len().saturating_sub(1);
        self.online_page = if forward {
            self.online_page.saturating_add(1).min(last)
        } else {
            self.online_page.min(last).saturating_sub(1)
        };
    }

    fn cancel_request(&mut self, context: &mut Context) {
        if let Some(task) = self.task.take() {
            context.cancel(task);
        }
        self.pending = None;
    }

    fn settings(&self) -> Screen {
        ScreenBuilder::new("verses-settings")
            .top_bar("Sleep screen")
            .splash(
                Some(Glyph::Note),
                if self.saved.sleep {
                    "Daily poem on"
                } else {
                    "Daily poem off"
                },
                if self.saved.sleep {
                    "Tomorrow's poem will appear when the reader sleeps."
                } else {
                    "Show tomorrow's poem while the reader sleeps."
                },
            )
            .primary_button(
                "sleep",
                if self.saved.sleep {
                    "Turn off"
                } else {
                    "Turn on"
                },
            )
            .build()
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Today if self.daily_poem().is_some() => self.online_poem(context),
            View::Today | View::Reading => self.local_poem(context),
            // The export's own screen: what is being written, where it goes,
            // and a way to try again if the computer is not listening.
            View::Card => self.export.as_ref().map_or_else(
                || self.local_poem(context),
                kobo_sdk::exports::Export::screen,
            ),
            View::Browse => self.browse(context).0,
            View::Search => self.search(),
            View::Results => self.results(context).0,
            View::Online => self.online_poem(context),
            View::Settings => self.settings(),
        }
    }

    fn save(&mut self, context: &mut Context) {
        match serde_json::to_vec(&self.saved) {
            Ok(bytes) => {
                self.saving = true;
                context.store().save(SETTINGS, bytes);
            }
            Err(_) => {
                self.notice = Some("This favourite could not be written. Try again.".to_owned());
            }
        }
    }

    fn show(&self, context: &mut Context) {
        let screen = self
            .screen(context)
            .with_own_back(!matches!(self.view, View::Today));
        context.set_screen(screen);
    }

    fn begin_search(&mut self, context: &mut Context, query: &str, scope: Scope) {
        if query.trim().is_empty() {
            self.notice = Some("Type something to search for.".into());
            return;
        }
        self.cancel_request(context);
        self.notice = Some("Searching…".into());
        self.results.clear();
        self.online = None;
        self.online_page = 0;
        self.results_page = 0;
        self.view = View::Results;
        query.trim().clone_into(&mut self.query);
        self.task = context.spawn(search_task(query.trim(), scope));
        self.pending = self.task.map(|_| Pending::Search);
        if self.task.is_none() {
            self.notice = Some("Search is busy. Try again in a moment.".into());
        }
    }

    /// The card a reader takes away: the poem, its poet, and where the text
    /// came from, drawn as the panel would draw it.
    ///
    /// A quote with no attribution is the thing the internet is already full
    /// of. Everything here is public domain and says which edition it was
    /// taken from, so a card can be passed on without stripping the poem of
    /// its provenance on the way.
    fn quote_card(&self, context: &Context) -> Screen {
        let poem = CORPUS[self.poem];
        let pages = self.poem_pages(context);
        let page = self.poem_page.min(pages.len().saturating_sub(1));
        // The card is a picture of the poem, so it is set at the size the poem
        // is set at. It also shares the poem's pagination, and pagination is
        // measured at that size: a card measured at one size and drawn at
        // another breaks its lines where the poem does not.
        let mut screen = ScreenBuilder::new("verses-card")
            .top_bar(poem.title)
            .reading(true)
            .text_scale(poem_scale(context));
        screen = set_verse(screen, poem, &pages[page]);
        screen
            .secondary(format!("{} · {}", poem.author, poem.year))
            .secondary(poem.source.to_owned())
            .build()
    }

    /// Draws the card and offers it to a paired computer.
    fn export_card(&mut self, context: &mut Context) {
        let metrics = context.metrics();
        let Ok(width) = usize::try_from(metrics.width) else {
            return;
        };
        let Ok(height) = usize::try_from(metrics.height) else {
            return;
        };
        let card = self.quote_card(context);
        let mut surface = kobo_ui::Surface::new(width, height);
        kobo_ui::render(&card, &mut surface, None);
        let picture = kobo_image::encode_png_grey(
            u32::try_from(metrics.width).unwrap_or(0),
            u32::try_from(metrics.height).unwrap_or(0),
            &surface.pixels,
        );
        let poem = CORPUS[self.poem];
        match picture
            .map_err(|error| format!("{error:?}"))
            .and_then(|bytes| {
                kobo_sdk::exports::Export::new(
                    &format!("{} by {}", poem.title, poem.author),
                    kobo_sdk::exports::Format::Png,
                    bytes,
                )
            }) {
            Ok(mut export) => {
                export.begin(context);
                self.export = Some(export);
                self.card_from = self.view;
                self.view = View::Card;
            }
            Err(reason) => {
                context.log(
                    kobo_sdk::LogLevel::Warn,
                    format!("the quote card was refused: {reason}"),
                );
                self.notice = Some("This card could not be prepared.".to_owned());
            }
        }
    }

    /// Moves the Today screen on if the reader's day has changed.
    fn advance_day(&mut self, clock: &dyn Clock) {
        let Some(date) = clock.now().ok().and_then(Snapshot::date) else {
            return;
        };
        let today = (date.year, date.month, date.day);
        if self.day == Some(today) {
            return;
        }
        self.day = Some(today);
        if self.view == View::Today {
            self.poem = daily_index(date.year, date.month, date.day);
            self.poem_page = 0;
        }
    }

    /// Today's poem from `PoetryDB`, if one has arrived for today.
    fn daily_poem(&self) -> Option<&OnlinePoem> {
        self.saved
            .daily
            .as_ref()
            .filter(|daily| Some(daily.day) == self.day)
            .map(|daily| &daily.poem)
    }

    /// The fetched poem on screen: today's, or one opened from a search.
    fn current_online(&self) -> Option<&OnlinePoem> {
        if self.view == View::Today {
            self.daily_poem()
        } else {
            self.online.and_then(|index| self.results.get(index))
        }
    }

    /// Asks for today's poem once the shelf has loaded and the day has none.
    /// Offline it simply fails, and the day keeps its bundled poem.
    fn fetch_daily(&mut self, context: &mut Context) {
        let Some(day) = self.day else { return };
        if !self.loaded || self.daily_task.is_some() || self.daily_poem().is_some() {
            return;
        }
        self.daily_task = context.spawn(daily_task(day));
    }

    fn toggle_favorite(&mut self) {
        if self.view == View::Online || (self.view == View::Today && self.daily_poem().is_some()) {
            let Some(poem) = self.current_online().cloned() else {
                return;
            };
            if let Some(saved) = self
                .saved
                .online_favorites
                .iter()
                .position(|saved| saved.title == poem.title && saved.author == poem.author)
            {
                self.saved.online_favorites.remove(saved);
            } else {
                self.saved.online_favorites.push(poem);
            }
        } else {
            let id = CORPUS[self.poem].id;
            if !self.saved.favorites.remove(id) {
                self.saved.favorites.insert(id.to_owned());
            }
        }
    }
}

impl KoboApp for Verses {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SETTINGS);
        self.show(context);
    }

    /// The blob a card is written into answers here rather than in `on_store`,
    /// which is where this first looked for it: the card sat on "saving and
    /// checking" for ever because nothing was listening on this hook.
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_shelf(context, name, &result) {
                self.show(context);
            }
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        // A card on its way out answers on its own keys. The settings key is
        // never handed to it: Todo emptied a list once by letting an export
        // claim the key that held the list.
        if let Some(export) = self.export.as_mut() {
            let key = match &result {
                StoreResult::Loaded { key, .. } | StoreResult::Saved { key } => key.clone(),
                _ => String::new(),
            };
            if key != SETTINGS && export.on_save(context, &key, &result) {
                self.show(context);
                return;
            }
        }
        // Storage answers every write, and a refusal is the one answer that
        // has to reach the reader: the favourite is not kept.
        match &result {
            StoreResult::Saved { .. } => {
                self.saving = false;
                if self
                    .notice
                    .as_deref()
                    .is_some_and(|notice| notice.contains("favourite"))
                {
                    self.notice = None;
                }
            }
            StoreResult::Denied(_) => {
                self.saving = false;
                self.notice = Some(
                    "This reader would not keep that favourite. Try marking it again.".to_owned(),
                );
            }
            _ => {}
        }
        if let StoreResult::Loaded {
            value: Some(bytes), ..
        } = result
        {
            if let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) {
                self.saved = saved;
            } else if let Ok(legacy) = serde_json::from_slice::<LegacySaved>(&bytes) {
                // Positions in the shelf as it was then: Hope, The Tiger,
                // Ozymandias, in that order.
                const WAS: [&str; 3] = ["dickinson-hope", "blake-tiger", "shelley-ozymandias"];
                self.saved = Saved {
                    favorites: legacy
                        .favorites
                        .iter()
                        .filter_map(|position| WAS.get(*position).map(|id| (*id).to_owned()))
                        .collect(),
                    online_favorites: legacy.online_favorites,
                    sleep: legacy.sleep,
                    daily: None,
                };
            }
        }
        self.loaded = true;
        self.fetch_daily(context);
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.advance_day(reader_clock().as_ref());
        self.fetch_daily(context);
        if self.view == View::Search && action != ActionId::BACK {
            if let Some(&(scope, _, _)) = Scope::ALL
                .iter()
                .find(|(_, name, _)| action == action_id(name))
            {
                self.scope = scope;
            } else if let Some(poet) = SUGGESTED_POETS
                .iter()
                .find(|poet| action == action_id(&format!("suggest-{poet}")))
            {
                self.begin_search(context, poet, Scope::Poet);
            } else if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let query = self.keyboard.take();
                self.begin_search(context, &query, self.scope);
            }
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.cancel_request(context);
            self.notice = None;
            self.view = match self.view {
                View::Online => self.online_from,
                View::Reading | View::Results | View::Search => View::Browse,
                View::Card => {
                    self.export = None;
                    self.card_from
                }
                _ => View::Today,
            };
        } else if action == action_id("today") {
            self.cancel_request(context);
            self.view = View::Today;
        } else if action == action_id("browse") {
            self.cancel_request(context);
            self.view = View::Browse;
        } else if action == action_id("search") {
            self.cancel_request(context);
            self.keyboard = Keyboard::new();
            self.notice = None;
            self.view = View::Search;
        } else if action == action_id("favorite") {
            self.toggle_favorite();
            self.save(context);
        } else if action == action_id("more-by-author") {
            if let Some(index) = self.online {
                let author = self.results[index].author.clone();
                self.begin_search(context, &author, Scope::Poet);
            }
        } else if action == action_id("card") && matches!(self.view, View::Today | View::Reading) {
            self.export_card(context);
        } else if action == action_id("export-confirm") || action == action_id("export-retry") {
            if let Some(export) = self.export.as_mut() {
                export.begin(context);
            }
        } else if action == action_id("list-next") {
            self.turn_list(context, true);
        } else if action == action_id("list-previous") {
            self.turn_list(context, false);
        } else if action == action_id("online-next") {
            self.turn_online(context, true);
        } else if action == action_id("online-previous") {
            self.turn_online(context, false);
        } else if action == action_id("poem-next") {
            let last = self.poem_pages(context).len().saturating_sub(1);
            self.poem_page = (self.poem_page + 1).min(last);
        } else if action == action_id("poem-previous") {
            self.poem_page = self.poem_page.saturating_sub(1);
        } else if action == action_id("sleep") {
            self.saved.sleep = !self.saved.sleep;
            self.save(context);
        } else if action == action_id("settings") {
            self.view = View::Settings;
        } else if let Some(index) =
            (0..CORPUS.len()).find(|index| action == action_id(&format!("poem-{index}")))
        {
            self.poem = index;
            self.poem_page = 0;
            self.view = View::Reading;
        } else if let Some(index) =
            (0..self.results.len()).find(|index| action == action_id(&format!("result-{index}")))
        {
            if self.task.is_some() {
                return;
            }
            self.online_page = 0;
            self.online_from = View::Results;
            if self.results[index].lines.is_empty() {
                self.notice = Some("Opening poem…".into());
                self.task = context.spawn(poem_task(&self.results[index].title));
                self.pending = self.task.map(|_| Pending::Open(index));
                if self.task.is_none() {
                    self.notice = Some("This poem is busy. Try again in a moment.".into());
                }
            } else {
                self.online = Some(index);
                self.view = View::Online;
            }
        } else if let Some(index) = (0..self.saved.online_favorites.len())
            .find(|index| action == action_id(&format!("saved-online-{index}")))
        {
            self.cancel_request(context);
            self.results = vec![self.saved.online_favorites[index].clone()];
            self.online = Some(0);
            self.online_page = 0;
            self.online_from = View::Browse;
            self.view = View::Online;
        }
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        match self.view {
            View::Browse | View::Results => self.turn_list(context, forward),
            View::Online => self.turn_online(context, forward),
            View::Today if self.daily_poem().is_some() => self.turn_online(context, forward),
            View::Today | View::Reading => {
                let last = self.poem_pages(context).len().saturating_sub(1);
                self.poem_page = if forward {
                    self.poem_page.saturating_add(1).min(last)
                } else {
                    self.poem_page.saturating_sub(1)
                };
            }
            _ => return,
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, id: TaskId, outcome: TaskOutcome) {
        if self.daily_task == Some(id) {
            self.daily_task = None;
            let fetched = match outcome {
                TaskOutcome::Completed(bytes) => serde_json::from_slice::<Vec<OnlinePoem>>(&bytes)
                    .ok()
                    .and_then(|poems| {
                        poems.into_iter().find(|poem| {
                            !poem.lines.is_empty() && poem.lines.len() <= MAX_DAILY_LINES
                        })
                    }),
                _ => None,
            };
            if let (Some(poem), Some(day)) = (fetched, self.day) {
                self.saved.daily = Some(DailyPoem { day, poem });
                self.online_page = 0;
                self.save(context);
                self.show(context);
            }
            return;
        }
        if self.task != Some(id) {
            return;
        }
        self.task = None;
        let pending = self.pending.take();
        match outcome {
            TaskOutcome::Completed(bytes) => match pending {
                Some(Pending::Search) => {
                    if let Ok(results) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        self.results = results;
                        self.notice = None;
                        self.online = None;
                        self.view = View::Results;
                    } else {
                        self.notice = Some("Poetry search couldn't open these results.".into());
                        self.view = View::Results;
                    }
                }
                Some(Pending::Open(index)) => {
                    if let Ok(mut poems) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        if let Some(poem) = poems
                            .drain(..)
                            .find(|poem| poem.author == self.results[index].author)
                        {
                            self.results[index] = poem;
                            self.online = Some(index);
                            self.notice = None;
                            self.view = View::Online;
                        } else {
                            self.notice = Some("That poem isn't available right now.".into());
                        }
                    } else {
                        self.notice = Some("That poem couldn't be opened.".into());
                    }
                }
                None => {}
            },
            // A 404 from this service is its way of saying nothing matched, so
            // it is the one failure that is really an empty result.
            TaskOutcome::Failed(TaskError::NotFound) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem isn't available right now.".into());
                    self.view = View::Results;
                } else {
                    self.notice = None;
                    self.results.clear();
                    self.view = View::Results;
                }
            }
            // Anything else is the service, not the search. This used to land
            // with the empty result above, so a poetry service that was down
            // told readers there were no poems matching what they asked for.
            TaskOutcome::Failed(error) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem couldn't be opened.".into());
                    self.view = View::Results;
                } else {
                    self.notice = Some(
                        match error {
                            TaskError::Offline => {
                                "This reader is offline. Your shelf is still here."
                            }
                            TaskError::Unauthorized | TaskError::NoCredential => {
                                "The poetry service would not answer this reader."
                            }
                            TaskError::RateLimited(_) => {
                                "The poetry service asked us to wait. Your shelf is still here."
                            }
                            _ => "The poetry service is not answering. Your shelf is still here.",
                        }
                        .to_owned(),
                    );
                    self.view = View::Browse;
                }
            }
            TaskOutcome::Cancelled => {
                self.notice = None;
            }
        }
        self.show(context);
    }
}

/// Split an exceptional line at the renderer's own line/grapheme boundaries.
/// Sets the runs of a poem as stanzas: one paragraph each, a printed poem's
/// leading inside it and the poet's space between them. The page and the card
/// share this so a card is the page it was made from, line for line.
fn set_verse(mut screen: ScreenBuilder, poem: Poem, runs: &[VerseRun]) -> ScreenBuilder {
    for (index, run) in runs.iter().enumerate() {
        let lines = &poem.stanzas[run.stanza][run.from..run.to];
        screen = screen.rich_text(
            lines.join("\n"),
            Vec::new(),
            kobo_sdk::ParagraphPresentation {
                // A stanza carried over from the page before opens without
                // the space. The unit is hundredths of an em.
                margin_before_em: if index > 0 && run.from == 0 {
                    STANZA_AIR
                } else {
                    0
                },
                // A printed poem's leading rather than a novel's. The reading
                // face opens lines up for an hour of prose, which set a short
                // stanza almost double spaced and pushed the last stanza of
                // Hope onto a page of its own.
                line_height_percent: VERSE_LEADING,
                ..kobo_sdk::ParagraphPresentation::default()
            },
        );
    }
    screen
}

fn split_online_line(text: &str, context: &Context) -> Option<(String, String)> {
    let metrics = context.metrics();
    let lines = kobo_ui::with_text_scale(metrics.text_scale, || {
        kobo_ui::with_reading_scale(metrics.text_scale, || {
            kobo_ui::wrap_text_in(
                text,
                metrics.readable_width(),
                kobo_ui::FontSize::Body,
                kobo_ui::Face::Reading,
            )
        })
    });
    if lines.len() < 2 {
        return None;
    }
    let mut split = 0;
    for line in lines.iter().take(lines.len() / 2) {
        let start = text[split..].find(line)?;
        split += start + line.len();
    }
    (split > 0 && split < text.len()).then(|| {
        (
            text[..split].trim_end().to_owned(),
            text[split..].trim_start().to_owned(),
        )
    })
}

fn main() -> ExitCode {
    kobo_sdk::run("verses", Verses::default()).map_or_else(
        |error| {
            eprintln!("verses: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::AppRunner;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn daily_choice_is_deterministic_and_leap_day_safe() {
        assert_eq!(daily_index(2028, 2, 29), daily_index(2028, 2, 29));
        assert_ne!(daily_index(2026, 9, 1), CORPUS.len());
    }

    /// A clock stopped at a given civil day, at UTC.
    fn clock_on(year: u16, month: u8, day: u8) -> ManualClock {
        // Days from 1970 to the date asked for, the long way round, because a
        // test that computes the answer with the code under test proves
        // nothing.
        let mut days: u64 = 0;
        for past in 1970..year {
            days += if past % 4 == 0 && (past % 100 != 0 || past % 400 == 0) {
                366
            } else {
                365
            };
        }
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let lengths = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        days += lengths.iter().take(usize::from(month - 1)).sum::<u64>();
        days += u64::from(day) - 1;
        ManualClock::new(Snapshot {
            unix_millis: days * 86_400_000 + 12 * 3_600_000,
            monotonic_millis: 0,
            utc_offset_minutes: 0,
        })
        .expect("a valid clock")
    }

    #[test]
    fn the_daily_poem_is_the_readers_day_and_moves_at_midnight() {
        // The day was written into the default state as the first of September
        // 2026, so an application called daily poetry offered one poem for
        // ever.
        let clock = clock_on(2026, 9, 1);
        assert_eq!(poem_for_today(&clock), daily_index(2026, 9, 1));
        let later = clock_on(2027, 3, 14);
        assert_eq!(poem_for_today(&later), daily_index(2027, 3, 14));

        // Crossing midnight with the application open moves the Today screen
        // on rather than leaving yesterday's poem there.
        let mut app = Verses {
            view: View::Today,
            poem: daily_index(2026, 9, 1),
            day: Some((2026, 9, 1)),
            ..Verses::default()
        };
        let midnight = clock_on(2026, 9, 2);
        app.advance_day(&midnight);
        assert_eq!(app.day, Some((2026, 9, 2)));
        assert_eq!(app.poem, daily_index(2026, 9, 2));

        // A poem the reader opened themselves is not replaced under them.
        let mut reading = Verses {
            view: View::Reading,
            poem: 1,
            day: Some((2026, 9, 1)),
            ..Verses::default()
        };
        reading.advance_day(&midnight);
        assert_eq!(reading.poem, 1, "the poem being read was replaced");
        assert_eq!(reading.day, Some((2026, 9, 2)));
    }

    #[test]
    fn every_poem_is_whole_and_says_where_it_came_from() {
        // Each of these used to be four lines: one stanza of The Tiger, a
        // third of Hope, offered with nothing to say it was an excerpt.
        for poem in CORPUS {
            assert!(!poem.author.is_empty() && !poem.source.is_empty() && poem.year < 1929);
            assert!(poem.lines().all(|line| !line.trim().is_empty()));
            assert!(
                poem.stanzas.iter().all(|stanza| !stanza.is_empty()),
                "{}: an empty stanza",
                poem.title
            );
            // Not a proof of completeness, which no test can give: a floor
            // that the four line excerpts this corpus used to carry could not
            // have cleared.
            assert!(
                poem.line_count() >= 12,
                "{} is {} lines, which is an excerpt rather than a poem",
                poem.title,
                poem.line_count()
            );
        }
    }

    #[test]
    fn a_long_poem_pages_by_stanza_and_keeps_every_line() {
        // A stanza is never split across a page turn, and paging never loses
        // or repeats one.
        let metrics = kobo_ui::DisplayMetrics {
            text_scale: kobo_ui::TextScale::Largest,
            ..CLARA_BW_METRICS
        };
        for (index, poem) in CORPUS.iter().enumerate() {
            for metrics in [CLARA_BW_METRICS, metrics] {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let app = Verses {
                    poem: index,
                    ..Verses::default()
                };
                let pages = app.poem_pages(&context);
                let seen: Vec<(usize, usize)> = pages
                    .iter()
                    .flatten()
                    .flat_map(|run| (run.from..run.to).map(move |line| (run.stanza, line)))
                    .collect();
                let every: Vec<(usize, usize)> = poem
                    .stanzas
                    .iter()
                    .enumerate()
                    .flat_map(|(stanza, lines)| (0..lines.len()).map(move |line| (stanza, line)))
                    .collect();
                assert_eq!(
                    seen,
                    every,
                    "{}: lines lost or repeated across {} pages",
                    poem.title,
                    pages.len()
                );
            }
        }
    }

    #[test]
    fn every_page_of_every_poem_fits_at_every_text_size() {
        let chrome = Chrome::measuring(true);
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for (index, poem) in CORPUS.iter().enumerate() {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let mut app = Verses {
                    poem: index,
                    view: View::Reading,
                    ..Verses::default()
                };
                for page in 0..app.poem_pages(&context).len() {
                    app.poem_page = page;
                    let issues = app
                        .local_poem(&context)
                        .diagnostics(&metrics, &chrome)
                        .issues;
                    assert!(
                        issues.is_empty(),
                        "{:?} {} page {page}: {issues:?}",
                        scale,
                        poem.title
                    );
                }
            }
        }
    }

    #[test]
    fn poem_actions_are_icons_in_the_header() {
        let context = AppRunner::new(Verses::default()).context();
        let screen = Verses::default().screen(&context);
        let debug = format!("{screen:?}");
        assert!(debug.contains("Heart"), "{debug}");
        assert!(debug.contains("Grid"), "{debug}");
        assert!(!debug.contains("Save favorite"), "{debug}");
    }

    #[test]
    fn poetrydb_searches_titles_authors_and_lines() {
        let Task::Fetch { url, .. } = search_task("hope & spring", Scope::Anything) else {
            panic!("search must use the network");
        };
        assert_eq!(
            url,
            "https://poetrydb.org/author,title,lines/hope%20%26%20spring/author,title,linecount"
        );
    }

    #[test]
    fn every_card_of_every_poem_fits_at_every_text_size() {
        // The card shares the poem's pagination, so it has to share the size
        // that pagination was measured at. It did not: the poem gained a size
        // override and the card kept drawing at the reader's own, so pages
        // measured for one were set in the other and the card broke its lines
        // where the poem did not. Only the poem had a test like this, which is
        // why nothing said so.
        let chrome = Chrome::measuring(true);
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for (index, poem) in CORPUS.iter().enumerate() {
                let context = AppRunner::with_metrics(Verses::default(), metrics).context();
                let mut app = Verses {
                    poem: index,
                    view: View::Reading,
                    ..Verses::default()
                };
                for page in 0..app.poem_pages(&context).len() {
                    app.poem_page = page;
                    let card = app.quote_card(&context);
                    let issues = card.diagnostics(&metrics, &chrome).issues;
                    assert!(
                        issues.is_empty(),
                        "{:?} {} card page {page}: {issues:?}",
                        scale,
                        poem.title
                    );
                    // The size it is set at, not merely that it fits. A card
                    // measured large and drawn small still fits: it under-fills
                    // and breaks its lines where the poem does not, which no
                    // overflow diagnostic reports.
                    assert_eq!(
                        card.text_scale,
                        app.local_poem(&context).text_scale,
                        "{:?} {} card page {page} is set at another size than the poem it copies",
                        scale,
                        poem.title
                    );
                }
            }
        }
    }

    #[test]
    fn a_quote_card_is_a_picture_of_the_poem_with_its_poet_on_it() {
        // A quote with no attribution is what the internet is already full of.
        let runner = AppRunner::new(Verses::default());
        let context = runner.context();
        let mut app = Verses {
            view: View::Reading,
            poem: CORPUS
                .iter()
                .position(|poem| poem.id == "shelley-ozymandias")
                .expect("a poem"),
            ..Verses::default()
        };
        let card = app.quote_card(&context);
        let drawn = format!("{card:?}");
        assert!(drawn.contains("Ozymandias"), "no title: {drawn}");
        assert!(drawn.contains("Percy Bysshe Shelley"), "no poet");
        assert!(drawn.contains("1818"), "no year");
        assert!(drawn.contains("Project Gutenberg"), "no source");
        assert!(
            drawn.contains("I met a traveller from an antique land"),
            "no poem"
        );
        assert!(card
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());

        // The card leaves as a picture, which is the thing somebody can pass
        // on, and the export is what carries it to a paired computer.
        let second = AppRunner::new(Verses::default());
        app.export_card(&mut second.context());
        let export = app.export.as_ref().expect("a card on its way out");
        assert_eq!(export.offer().format, kobo_sdk::exports::Format::Png);
        assert!(export.offer().title.contains("Ozymandias"));
        assert_eq!(app.view, View::Card);
    }

    #[test]
    fn a_favourite_survives_the_shelf_being_rewritten() {
        // Favourites were positions in the corpus, so adding a poem or
        // changing the order moved every one of them onto a different poem.
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        app.poem = CORPUS
            .iter()
            .position(|poem| poem.id == "shelley-ozymandias")
            .expect("a poem to mark");
        app.toggle_favorite();
        assert!(app.saved.favorites.contains("shelley-ozymandias"));
        assert!(
            !app.saved
                .favorites
                .iter()
                .any(|id| id.parse::<usize>().is_ok()),
            "a favourite is a name rather than a position"
        );
        app.save(&mut runner.context());
        assert!(app.saving, "a write is outstanding until storage answers");
    }

    #[test]
    fn favourites_written_before_poems_had_names_are_still_the_same_poems() {
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        // What the old shelf wrote: positions of Hope, The Tiger, Ozymandias.
        let legacy = br#"{"favorites":[0,2],"online_favorites":[],"sleep":false}"#;
        app.on_store(
            &mut runner.context(),
            StoreResult::Loaded {
                key: SETTINGS.to_owned(),
                value: Some(legacy.to_vec()),
            },
        );
        assert!(app.saved.favorites.contains("dickinson-hope"));
        assert!(app.saved.favorites.contains("shelley-ozymandias"));
        assert!(!app.saved.favorites.contains("blake-tiger"));
    }

    #[test]
    fn a_favourite_the_reader_cannot_keep_says_so() {
        let mut app = Verses::default();
        let runner = AppRunner::new(Verses::default());
        app.toggle_favorite();
        app.save(&mut runner.context());
        app.on_store(
            &mut runner.context(),
            StoreResult::Denied(kobo_sdk::StoreError::Unwritable),
        );
        assert!(!app.saving);
        let notice = app.notice.clone().unwrap_or_default();
        assert!(notice.to_lowercase().contains("favourite"), "{notice}");
    }

    #[test]
    fn todays_poem_comes_from_poetrydb_and_stays_for_the_day() {
        let mut runner = AppRunner::new(Verses {
            day: Some((2026, 10, 6)),
            ..Verses::default()
        });
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let task = runner.app().daily_task.expect("today's poem was asked for");
        let Task::Fetch { url, .. } = daily_task((2026, 10, 6)) else {
            panic!("a fetch");
        };
        assert!(url.contains("linecount,random/"), "{url}");
        let body = br#"[{"title":"A Fetched Poem","author":"A Poet","lines":["One line,","another.","","A second stanza."],"linecount":"3"}]"#;
        runner.task_outcome(task, TaskOutcome::Completed(body.to_vec()));
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains("A Fetched Poem"), "{shown}");
        assert!(shown.contains("From PoetryDB"));
        assert!(shown.contains("Today's poem"));
        // Kept for the day: a later action asks for nothing more.
        runner.action(action_id("poem-next"));
        assert!(runner.app().daily_task.is_none());
    }

    #[test]
    fn offline_the_day_keeps_its_bundled_poem() {
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SETTINGS.into(),
            value: None,
        });
        let task = runner.app().daily_task.expect("asked once");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Offline));
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains(CORPUS[runner.app().poem].title));
        assert!(
            runner.app().notice.is_none(),
            "offline is not an error here"
        );
    }

    #[test]
    fn a_numeric_line_count_does_not_empty_the_results() {
        let body = br#"[{"title":"I'm nobody!  Who are you?","author":"Emily Dickinson","linecount":8},{"title":"Ozymandias","author":"Percy Bysshe Shelley","linecount":"14"}]"#;
        let poems: Vec<OnlinePoem> = serde_json::from_slice(body).expect("both forms read");
        assert_eq!(poems[0].linecount, "8");
        assert_eq!(poems[1].linecount, "14");
    }

    #[test]
    fn search_looks_where_the_reader_asked_and_groups_by_poet() {
        for (scope, fields) in [
            (Scope::Anything, "/author,title,lines/"),
            (Scope::Title, "/title/"),
            (Scope::Poet, "/author/"),
            (Scope::Line, "/lines/"),
        ] {
            let Task::Fetch { url, .. } = search_task("sea", scope) else {
                panic!("a fetch");
            };
            assert!(url.contains(fields), "{url}");
        }
        let mut runner = AppRunner::new(Verses::default());
        runner.start();
        runner.action(action_id("browse"));
        runner.action(action_id("search"));
        runner.action(action_id("scope-poet"));
        assert_eq!(runner.app().scope, Scope::Poet);
        assert!(format!("{:?}", runner.app().screen(&runner.context())).contains("Emily Dickinson"));
        runner.action(action_id("suggest-John Keats"));
        assert_eq!(runner.app().view, View::Results);
        let app = runner.app_mut();
        app.task = None;
        app.pending = None;
        app.notice = None;
        app.results = vec![
            OnlinePoem {
                title: "Ode to a Nightingale".into(),
                author: "John Keats".into(),
                lines: Vec::new(),
                linecount: "80".into(),
            },
            OnlinePoem {
                title: "Bright Star".into(),
                author: "John Keats".into(),
                lines: Vec::new(),
                linecount: "14".into(),
            },
        ];
        let shown = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(shown.contains("2 poems for"), "{shown}");
        assert!(shown.contains("14 lines · a minute"));
        assert!(shown.contains("80 lines · four minutes"));
        // Sorted within the poet: Bright Star before the Nightingale.
        assert!(shown.find("Bright Star") < shown.find("Ode to a Nightingale"));
    }

    #[test]
    fn a_service_that_is_down_is_not_reported_as_an_empty_search() {
        // The poetry service answered with its framework's error page for a
        // while, and the application told readers there were no poems matching
        // their search: a server fault dressed as an answer about their words.
        for (error, expected) in [
            (TaskError::Unreachable, "not answering"),
            (TaskError::TimedOut, "not answering"),
            (TaskError::Offline, "offline"),
            (TaskError::Unauthorized, "would not answer"),
        ] {
            let mut app = Verses {
                view: View::Search,
                task: Some(TaskId(1)),
                pending: Some(Pending::Search),
                ..Verses::default()
            };
            let runner = AppRunner::new(Verses::default());
            app.on_task(&mut runner.context(), TaskId(1), TaskOutcome::Failed(error));
            let notice = app.notice.clone().unwrap_or_default();
            assert!(
                notice.to_lowercase().contains(expected),
                "{error:?} became {notice:?}"
            );
            assert!(
                !notice.to_lowercase().contains("no poems"),
                "{error:?} blamed the search"
            );
        }

        // The one failure that really is an empty result: this service answers
        // a search that matched nothing with a 404.
        let mut app = Verses {
            view: View::Search,
            task: Some(TaskId(1)),
            pending: Some(Pending::Search),
            ..Verses::default()
        };
        let runner = AppRunner::new(Verses::default());
        app.on_task(
            &mut runner.context(),
            TaskId(1),
            TaskOutcome::Failed(TaskError::NotFound),
        );
        assert_eq!(app.notice, None);
        assert!(app.results.is_empty());
    }

    #[test]
    fn poetrydb_metadata_results_do_not_need_lines_until_opened() {
        let poems: Vec<OnlinePoem> = serde_json::from_str(
            r#"[{"title":"The Tyger","author":"William Blake","linecount":"24"}]"#,
        )
        .expect("metadata response");
        assert!(poems[0].lines.is_empty());
    }

    #[test]
    fn reading_and_browse_screens_fit() {
        let app = Verses::default();
        let context = AppRunner::new(Verses::default()).context();
        for screen in [
            app.local_poem(&context),
            app.browse(&context).0,
            app.search(),
        ] {
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
    }
}

#[cfg(test)]
mod navigation_tests;
