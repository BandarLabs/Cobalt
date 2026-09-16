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
                "I 've heard it in the chillest land,",
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OnlinePoem {
    title: String,
    author: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    linecount: String,
}

#[derive(Default, Deserialize, Serialize)]
struct Saved {
    /// Poems from the shelf, by name.
    #[serde(default)]
    favorites: BTreeSet<String>,
    online_favorites: Vec<OnlinePoem>,
    sleep: bool,
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
    saved: Saved,
    keyboard: Keyboard,
    results: Vec<OnlinePoem>,
    task: Option<TaskId>,
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
            saved: Saved::default(),
            keyboard: Keyboard::new(),
            results: Vec::new(),
            task: None,
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

fn search_task(query: &str, author_only: bool) -> Task {
    let fields = if author_only {
        "author"
    } else {
        "author,title,lines"
    };
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
    /// The whole thing used to be handed over as one block of text with the
    /// poet's name and the word "Today" pushed into the top of it, in the same
    /// face and size as the verse, so the page opened with two lines that were
    /// not part of the poem. The lines are centred on the page now, a stanza
    /// keeps the space the poet put after it, and who wrote it and where it
    /// comes from sit under the poem rather than in it.
    fn local_poem(&self, context: &Context) -> Screen {
        let poem = CORPUS[self.poem];
        let pages = self.poem_pages(context);
        let page = self.poem_page.min(pages.len().saturating_sub(1));
        let mut screen = ScreenBuilder::new("verses-poem").top_bar(poem.title);
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
        if self.view == View::Today {
            screen = screen.secondary("Today");
        }
        screen = screen.reading(true);
        for (index, run) in pages[page].iter().enumerate() {
            for (line, text) in poem.stanzas[run.stanza][run.from..run.to]
                .iter()
                .enumerate()
            {
                screen = screen.rich_text(
                    (*text).to_owned(),
                    Vec::new(),
                    kobo_sdk::ParagraphPresentation {
                        alignment: kobo_sdk::ParagraphAlignment::Center,
                        // The space a poet leaves between stanzas, and none
                        // between the lines inside one. A stanza carried over
                        // from the page before opens without one. The unit is
                        // hundredths of an em, so a whole line of air is 100
                        // and the 1 this first carried was invisible.
                        margin_before_em: if index > 0 && line == 0 && run.from == 0 {
                            STANZA_AIR
                        } else {
                            0
                        },
                        ..kobo_sdk::ParagraphPresentation::default()
                    },
                );
            }
        }
        // Who wrote it, when, and which edition this text is taken from. A
        // reader who meets a poem here can go and find it.
        if page + 1 == pages.len() {
            screen = screen.secondary(format!("{} · {} · {}", poem.author, poem.year, poem.source));
        }
        // The card sits at the foot rather than in the bar: the bar holds two
        // controls at most, for the reason the platform gives, and a card is
        // something a reader does with a poem they have just read rather than
        // chrome that belongs beside its title.
        if pages.len() > 1 {
            screen = screen
                .page_position(
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(pages.len()).unwrap_or(1),
                )
                .action_bar([
                    ("poem-previous", "Previous"),
                    ("card", "Quote card"),
                    ("poem-next", "Next"),
                ]);
        } else {
            screen = screen.action_bar([("card", "Quote card")]);
        }
        screen.build()
    }

    /// Which lines fit on a page, measured against the panel.
    ///
    /// A page is a list of runs, each a stanza and the lines of it that belong
    /// on this page. Breaks are taken at stanza boundaries wherever the panel
    /// allows, because a stanza is the unit a poem is written in. A stanza too
    /// tall for one page is carried over rather than dropped: a fourteen line
    /// sonnet does not fit a six inch panel at the larger text settings, and
    /// Ozymandias was drawn through the bottom edge until this said so.
    ///
    /// Measured a line at a time rather than a stanza at a time. Handing the
    /// poem over as prose and counting paragraphs put four lines of The Tiger
    /// under the bottom edge at the smallest text size, because prose wrapping
    /// says nothing about how many separate verse lines fit: each line here is
    /// its own paragraph on the screen, so each line is its own paragraph in
    /// the measurement too.
    fn poem_pages(&self, context: &Context) -> Vec<Vec<VerseRun>> {
        let poem = CORPUS[self.poem];
        let one_per_paragraph = poem.lines().collect::<Vec<_>>().join("\n\n");
        let measured = context.paginate_reading(&one_per_paragraph, true);
        // The fullest page the panel offered, less the line the day's label or
        // the attribution takes on the first and last pages. Taking the
        // smallest instead read the remainder page as the panel's capacity and
        // put one stanza on each of six pages with four fifths of every page
        // empty.
        let capacity = measured
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1)
            .max(1);

        let mut pages: Vec<Vec<VerseRun>> = Vec::new();
        let mut current: Vec<VerseRun> = Vec::new();
        let mut used = 0usize;
        for (stanza, lines) in poem.stanzas.iter().enumerate() {
            let mut from = 0usize;
            while from < lines.len() {
                let gap = usize::from(!current.is_empty());
                let room = capacity.saturating_sub(used + gap);
                let left = lines.len() - from;
                // Start a new page rather than split a stanza that would fit
                // whole on one. Splitting whenever the current page happened
                // to be nearly full left a single line stranded at the foot.
                let splittable = left > capacity;
                if !current.is_empty() && (room == 0 || (left > room && !splittable)) {
                    pages.push(std::mem::take(&mut current));
                    used = 0;
                    continue;
                }
                let take = left.min(room.max(1));
                used += take + gap;
                current.push(VerseRun {
                    stanza,
                    from,
                    to: from + take,
                });
                from += take;
            }
        }
        if !current.is_empty() {
            pages.push(current);
        }
        if pages.is_empty() {
            vec![vec![VerseRun {
                stanza: 0,
                from: 0,
                to: poem.stanzas.first().map_or(0, |lines| lines.len()),
            }]]
        } else {
            pages
        }
    }

    fn online_poem(&self) -> Screen {
        let Some(index) = self.online else {
            return ScreenBuilder::new("verses-online")
                .top_bar("Verses")
                .splash(
                    Some(Glyph::Search),
                    "Choose a poem",
                    "Open one from Search.",
                )
                .build();
        };
        let poem = &self.results[index];
        let favorite = self
            .saved
            .online_favorites
            .iter()
            .any(|saved| saved.title == poem.title && saved.author == poem.author);
        ScreenBuilder::new("verses-online")
            .top_bar(poem.title.clone())
            .top_bar_glyph("more-by-author", "More by this poet", Glyph::Person)
            .top_bar_glyph(
                "favorite",
                if favorite {
                    "Remove favorite"
                } else {
                    "Favorite"
                },
                Glyph::Heart,
            )
            .secondary(poem.author.clone())
            .reading(true)
            .text(poem.lines.join("\n"))
            .build()
    }

    fn browse(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-browse")
            .top_bar("Browse")
            .top_bar_glyph("search", "Search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        if !self.saved.favorites.is_empty() || !self.saved.online_favorites.is_empty() {
            let mut favorites = self
                .saved
                .favorites
                .iter()
                .filter_map(|id| {
                    CORPUS
                        .iter()
                        .position(|poem| poem.id == id.as_str())
                        .map(|index| {
                            let poem = CORPUS[index];
                            (
                                format!("poem-{index}"),
                                poem.title.to_owned(),
                                poem.author.to_owned(),
                                Glyph::Heart,
                            )
                        })
                })
                .collect::<Vec<_>>();
            favorites.extend(self.saved.online_favorites.iter().enumerate().map(
                |(index, poem)| {
                    (
                        format!("saved-online-{index}"),
                        poem.title.clone(),
                        poem.author.clone(),
                        Glyph::Heart,
                    )
                },
            ));
            screen = screen.section("Favorites").rows(favorites);
        }
        screen
            .section("Poems")
            .rows(CORPUS.iter().enumerate().map(|(index, poem)| {
                (
                    format!("poem-{index}"),
                    poem.title,
                    format!("{} · {}", poem.author, poem.tags),
                    Glyph::Note,
                )
            }))
            .build()
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-search").top_bar("Search poetry");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        screen
            .typed(&self.keyboard, "Title, poet, or a line")
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    fn results(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-results")
            .top_bar("Search")
            .top_bar_glyph("search", "New search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        if self.results.is_empty() {
            screen
                .splash(
                    Some(Glyph::Search),
                    "No poems found",
                    "Try a poet, title, or memorable line.",
                )
                .build()
        } else {
            screen
                .rows(
                    self.results
                        .iter()
                        .take(40)
                        .enumerate()
                        .map(|(index, poem)| {
                            (
                                format!("result-{index}"),
                                poem.title.clone(),
                                poem.author.clone(),
                                Glyph::Note,
                            )
                        }),
                )
                .build()
        }
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
            View::Today | View::Reading => self.local_poem(context),
            // The export's own screen: what is being written, where it goes,
            // and a way to try again if the computer is not listening.
            View::Card => self.export.as_ref().map_or_else(
                || self.local_poem(context),
                kobo_sdk::exports::Export::screen,
            ),
            View::Browse => self.browse(),
            View::Search => self.search(),
            View::Results => self.results(),
            View::Online => self.online_poem(),
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

    fn begin_search(&mut self, context: &mut Context, query: &str, author_only: bool) {
        if query.trim().is_empty() {
            self.notice = Some("Type something to search for.".into());
            return;
        }
        self.notice = Some("Searching…".into());
        self.results.clear();
        self.task = context.spawn(search_task(query.trim(), author_only));
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
        let mut screen = ScreenBuilder::new("verses-card")
            .top_bar(poem.title)
            .reading(true);
        for (index, run) in pages[page].iter().enumerate() {
            for (line, text) in poem.stanzas[run.stanza][run.from..run.to]
                .iter()
                .enumerate()
            {
                screen = screen.rich_text(
                    (*text).to_owned(),
                    Vec::new(),
                    kobo_sdk::ParagraphPresentation {
                        alignment: kobo_sdk::ParagraphAlignment::Center,
                        margin_before_em: if index > 0 && line == 0 && run.from == 0 {
                            STANZA_AIR
                        } else {
                            0
                        },
                        ..kobo_sdk::ParagraphPresentation::default()
                    },
                );
            }
        }
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

    fn toggle_favorite(&mut self) {
        if self.view == View::Online {
            let Some(index) = self.online else { return };
            let poem = self.results[index].clone();
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
                };
            }
        }
        self.loaded = true;
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.advance_day(reader_clock().as_ref());
        if self.view == View::Search {
            if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let query = self.keyboard.take();
                self.begin_search(context, &query, false);
            }
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.view = match self.view {
                View::Online | View::Results | View::Search => View::Browse,
                View::Card => {
                    self.export = None;
                    View::Reading
                }
                _ => View::Today,
            };
        } else if action == action_id("today") {
            self.view = View::Today;
        } else if action == action_id("browse") {
            self.view = View::Browse;
        } else if action == action_id("search") {
            self.keyboard = Keyboard::new();
            self.notice = None;
            self.view = View::Search;
        } else if action == action_id("favorite") {
            self.toggle_favorite();
            self.save(context);
        } else if action == action_id("more-by-author") {
            if let Some(index) = self.online {
                let author = self.results[index].author.clone();
                self.begin_search(context, &author, true);
            }
        } else if action == action_id("card") {
            self.export_card(context);
        } else if action == action_id("export-confirm") || action == action_id("export-retry") {
            if let Some(export) = self.export.as_mut() {
                export.begin(context);
            }
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
            self.results = vec![self.saved.online_favorites[index].clone()];
            self.online = Some(0);
            self.view = View::Online;
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, id: TaskId, outcome: TaskOutcome) {
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
        let Task::Fetch { url, .. } = search_task("hope & spring", false) else {
            panic!("search must use the network");
        };
        assert_eq!(
            url,
            "https://poetrydb.org/author,title,lines/hope%20%26%20spring/author,title,linecount"
        );
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
        for screen in [app.local_poem(&context), app.browse(), app.search()] {
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
    }
}
