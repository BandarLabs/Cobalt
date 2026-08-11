//! A brief that is ready before you open it.
//!
//! This exists to demonstrate the one lifecycle e-readers actually need, and
//! the one a mobile framework would call backgrounding. It is not a feed
//! reader with extra steps: the whole point is what happens when you *leave*.
//!
//! ## What it demonstrates
//!
//! Tap Refresh and it starts fetching. Go back to the launcher and open
//! something else. The fetch keeps running, because leaving an application no
//! longer stops it: the runtime keeps the process, the work in flight and the
//! memory, and tells the application it is no longer being looked at. Come back
//! and the brief is finished and drawn, with no reload and no second fetch.
//!
//! Under the previous design, leaving killed the process and returning started
//! it again from nothing, so this application could not have existed.
//!
//! ## Why it saves the moment it goes to the background
//!
//! [`KoboApp::on_background`] is the last certain moment. A reader closes an
//! e-reader by shutting a cover and may not open it for a week, and the device
//! may run its battery flat in between. So the brief is written then, and on
//! every arrival, rather than on the way out.
//!
//! ## Why the cached copy is shown before the fetch finishes
//!
//! The panel holds an image at zero power, so there is nothing to cover and no
//! reason to show a spinner. Yesterday's brief with an honest "as of" line is
//! more use than a blank screen, and it means the application is readable with
//! no network at all.

use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, Failure, Glyph, KoboApp, LogLevel, Screen,
    ScreenBuilder, SlotWidth, Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// Where the list of story ids comes from.
const TOP: &str = "https://hacker-news.firebaseio.com/v0/topstories.json";

/// How many stories a brief holds.
///
/// One panel's worth. A brief that needs a page turn is a feed, and a feed is
/// something a reader has to manage rather than glance at.
const STORIES: usize = 6;

/// The largest reply worth reading for either request.
///
/// The index is a few thousand ids and an item is a few hundred bytes. Asking
/// for less than the runtime's ceiling is what keeps a background refresh from
/// costing radio time nobody asked for.
const CEILING: u32 = 64 * 1024;

const REFRESH: &str = "refresh";
const STORED: &str = "brief";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Story {
    title: String,
    site: String,
}

/// What the application is waiting for, if anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fetching {
    Nothing,
    /// The list of ids.
    Index(TaskId),
    /// One story. The position is where it goes in the brief, so replies that
    /// arrive out of order still land in the right place.
    Story(TaskId, usize),
}

struct Brief {
    stories: Vec<Story>,
    /// Ids still to be fetched, in order.
    queue: Vec<u64>,
    /// The brief being assembled. Kept apart from `stories` so a failed refresh
    /// leaves the previous brief on the panel rather than half of a new one.
    building: Vec<Option<Story>>,
    fetching: Fetching,
    /// Set while the reader is not looking. Nothing is drawn then, because
    /// nothing drawn would be seen, and a repaint the reader cannot see is a
    /// refresh charged to the battery for nothing.
    background: bool,
    loaded: bool,
    note: Option<String>,
}

impl Default for Brief {
    fn default() -> Self {
        Self {
            stories: Vec::new(),
            queue: Vec::new(),
            building: Vec::new(),
            fetching: Fetching::Nothing,
            background: false,
            loaded: false,
            note: None,
        }
    }
}

/// Encodes the brief for the store: one story a line, title and site tabbed.
///
/// A tab is used rather than a comma because a title can contain a comma and
/// cannot contain a tab; anything that arrives with one has it replaced when the
/// story is built, so this cannot be ambiguous.
fn encode(stories: &[Story]) -> Vec<u8> {
    let mut out = String::new();
    for story in stories {
        out.push_str(&story.title);
        out.push('\t');
        out.push_str(&story.site);
        out.push('\n');
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Vec<Story> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (title, site) = line.split_once('\t')?;
            if title.is_empty() {
                return None;
            }
            Some(Story {
                title: title.to_string(),
                site: site.to_string(),
            })
        })
        .take(STORIES)
        .collect()
}

/// The host part of a URL, which is as much of a link as this panel can use.
///
/// Deliberately not the whole address: there is no browser to open it in, so
/// the only question a reader has is where the story is from.
fn site_of(url: &str) -> String {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn clean(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

impl Brief {
    fn show(&self, context: &mut Context) {
        if self.background {
            return;
        }
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("brief").top_bar("Daily brief");
        if !self.loaded {
            return screen.skeleton(5).build();
        }
        if let Some(note) = &self.note {
            screen = screen.text(note.clone());
        }
        if self.stories.is_empty() && self.fetching == Fetching::Nothing {
            // Centred in what is left rather than stacked at the top, so a
            // brief that has not been fetched yet reads as a page waiting for
            // a tap instead of a page that failed to load.
            screen = screen.splash(
                Some(Glyph::News),
                "Nothing yet",
                "Tap Refresh once the device is online.",
            );
        } else if !self.stories.is_empty() {
            // Two honest counts, both read off the brief in hand: how many
            // headlines, and how many places they came from. A brief drawn
            // from one site is a different thing from one drawn from six, and
            // that was invisible when the sites were only a line under each
            // title.
            let sources = self.sources();
            // Side by side rather than stacked: two counts of one or two
            // digits each, given a full line apiece, read as a list of
            // findings rather than the one-line summary they are.
            let stories = self.stories.len().to_string();
            let sources = sources.to_string();
            screen = screen.band(
                BandAlign::Top,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(move |slot: ScreenBuilder| slot.facts([("Stories", stories)]))
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(move |slot: ScreenBuilder| slot.facts([("Sources", sources)])),
                    ),
                ],
            );
            // Numbered rather than illustrated: the same note icon beside
            // every headline is decoration, and a briefing is ordered, so the
            // position is the one thing the well can usefully say.
            screen = screen
                .section("Top stories")
                .rows(self.stories.iter().enumerate().map(|(index, story)| {
                    (
                        "story",
                        story.title.clone(),
                        story.site.clone(),
                        u16::try_from(index + 1).unwrap_or(u16::MAX),
                    )
                }));
        }
        if self.fetching == Fetching::Nothing {
            // Pinned to the foot of the panel rather than set after the last
            // story. Placed inline it was drawn wherever the list happened to
            // end, and with a full brief that was past the bottom edge: the
            // one control on the screen, off the screen.
            screen = screen.bottom_action_marked(REFRESH, "Refresh", Glyph::Refresh);
        } else {
            // A bar against a known total, not a spinner: the count of stories
            // is fixed, so an indeterminate animation would be claiming the end
            // is unknowable when it is six. Every frame of movement is a panel
            // refresh besides, so the bar is redrawn only as each story lands.
            let done = u64::try_from(self.building.iter().filter(|slot| slot.is_some()).count())
                .unwrap_or(0);
            // A count, not a byte count: `transfer` captions itself in bytes
            // and would print "3 B of 6 B" for three stories out of six.
            let percent = u8::try_from(done.saturating_mul(100) / STORIES as u64).unwrap_or(100);
            screen = screen
                .spacer(Space::Medium)
                .activity(
                    format!("Collecting stories, {done} of {STORIES}"),
                    Some(percent),
                )
                .text("You can leave this open. It keeps going.");
        }
        screen.build()
    }

    /// How many distinct sites the brief drew from.
    ///
    /// Counted off the stories on hand rather than tracked as the fetch runs,
    /// so it can never disagree with the sites actually printed under the
    /// titles.
    fn sources(&self) -> usize {
        let mut seen: Vec<&str> = Vec::new();
        for story in &self.stories {
            if !seen.contains(&story.site.as_str()) {
                seen.push(&story.site);
            }
        }
        seen.len()
    }

    fn start_refresh(&mut self, context: &mut Context) {
        if self.fetching != Fetching::Nothing {
            return;
        }
        self.note = None;
        self.building = vec![None; STORIES];
        self.queue.clear();
        match context.spawn(Task::Fetch {
            url: TOP.to_owned(),
            offset: 0,
            max_bytes: CEILING,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.fetching = Fetching::Index(task),
            None => self.note = Some("Too much already in flight.".to_owned()),
        }
    }

    /// Starts the next story, or finishes the brief when there are none left.
    fn advance(&mut self, context: &mut Context) {
        let position = self.building.iter().position(Option::is_none);
        let (Some(position), Some(id)) = (position, self.queue.first().copied()) else {
            self.complete(context);
            return;
        };
        self.queue.remove(0);
        let url = format!("https://hacker-news.firebaseio.com/v0/item/{id}.json");
        if let Some(task) = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: CEILING,
            credential: None,
            headers: Vec::new(),
        }) {
            self.fetching = Fetching::Story(task, position);
        } else {
            self.note = Some("Too much already in flight.".to_owned());
            self.complete(context);
        }
    }

    /// Publishes whatever was collected and writes it down.
    fn complete(&mut self, context: &mut Context) {
        self.fetching = Fetching::Nothing;
        let collected: Vec<Story> = self.building.iter().flatten().cloned().collect();
        if collected.is_empty() {
            if self.note.is_none() {
                self.note = Some("The refresh brought back nothing.".to_owned());
            }
        } else {
            self.stories = collected;
            context.store().save(STORED, encode(&self.stories));
        }
        self.building.clear();
        self.queue.clear();
    }

    fn on_index(&mut self, context: &mut Context, body: &[u8]) {
        let Ok(text) = std::str::from_utf8(body) else {
            self.fail(context, "The index was not text.");
            return;
        };
        let Ok(value) = kobo_json::parse(text) else {
            self.fail(context, "The index could not be read.");
            return;
        };
        let Some(ids) = value.as_array() else {
            self.fail(context, "The index was not a list.");
            return;
        };
        self.queue = ids
            .iter()
            .filter_map(Value::as_i64)
            .map(u64::try_from)
            .filter_map(Result::ok)
            .take(STORIES)
            .collect();
        if self.queue.is_empty() {
            self.fail(context, "The index was empty.");
            return;
        }
        self.building = vec![None; self.queue.len().min(STORIES)];
        self.advance(context);
    }

    fn on_story(&mut self, context: &mut Context, position: usize, body: &[u8]) {
        if let Some(story) = parse_story(body) {
            if let Some(slot) = self.building.get_mut(position) {
                *slot = Some(story);
            }
        }
        self.advance(context);
    }

    fn fail(&mut self, context: &mut Context, why: &str) {
        self.note = Some(why.to_owned());
        self.complete(context);
    }
}

fn parse_story(body: &[u8]) -> Option<Story> {
    let text = std::str::from_utf8(body).ok()?;
    let value = kobo_json::parse(text).ok()?;
    let title = value.get("title")?.as_str()?;
    if title.is_empty() {
        return None;
    }
    let site = value
        .get("url")
        .and_then(Value::as_str)
        .map_or_else(|| "news.ycombinator.com".to_owned(), site_of);
    Some(Story {
        title: clean(title),
        site: clean(&site),
    })
}

impl KoboApp for Brief {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STORED);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { value, .. } => {
                self.stories = value.map(|bytes| decode(&bytes)).unwrap_or_default();
                self.loaded = true;
                self.show(context);
            }
            StoreResult::Denied(reason) => {
                self.loaded = true;
                context.log(
                    LogLevel::Warn,
                    format!("the brief could not be kept: {reason}"),
                );
                self.show(context);
            }
            // Listed rather than wildcarded, so adding a store answer to the
            // protocol makes every application decide what it means here.
            // This one keeps nothing on the shelf.
            StoreResult::Saved { .. }
            | StoreResult::Forgotten { .. }
            | StoreResult::Keys(_)
            | StoreResult::ShelfWritten { .. }
            | StoreResult::ShelfRead { .. }
            | StoreResult::ShelfRemoved { .. }
            | StoreResult::Shelf(_) => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(REFRESH) {
            self.start_refresh(context);
            self.show(context);
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        self.background = true;
        // Written now, because this is the last certain moment. Nothing here
        // stops: the fetch in flight keeps running and its answer will still
        // arrive.
        if !self.stories.is_empty() {
            context.store().save(STORED, encode(&self.stories));
        }
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.background = false;
        // Whatever arrived while nobody was looking is drawn now, in one
        // refresh rather than one per story.
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        // Only the one thing this application is actually waiting for. An
        // outcome for anything else is ignored rather than mistaken for the
        // answer to what is outstanding.
        let stage = self.fetching;
        let waiting = match stage {
            Fetching::Nothing => return,
            Fetching::Index(waiting) | Fetching::Story(waiting, _) => waiting,
        };
        if waiting != task {
            return;
        }
        match outcome {
            TaskOutcome::Completed(body) => match stage {
                Fetching::Index(_) => self.on_index(context, &body),
                Fetching::Story(_, position) => self.on_story(context, position, &body),
                Fetching::Nothing => {}
            },
            TaskOutcome::Failed(error) => {
                // The SDK owns the wording, so every application says the
                // same thing about the same failure and a new TaskError
                // variant does not need five edits.
                let why = Failure::of(error).advice;
                // A story that fails is skipped; an index that fails ends the
                // refresh, because there is nothing to skip to.
                if matches!(stage, Fetching::Story(_, _)) {
                    self.note = Some(why.to_owned());
                    self.advance(context);
                } else {
                    self.fail(context, why);
                }
            }
            TaskOutcome::Cancelled => self.fail(context, "The refresh was stopped."),
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("brief", Brief::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("brief: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, parse_story, site_of, Brief, Fetching, Story, REFRESH, STORIES};
    use kobo_sdk::{action_id, Command, Context, KoboApp, Task, TaskId, TaskOutcome};

    fn spawned(commands: &[Command]) -> TaskId {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("nothing was started")
    }

    fn url_of(commands: &[Command]) -> String {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
            .expect("nothing was fetched")
    }

    fn ready() -> Brief {
        Brief {
            loaded: true,
            ..Brief::default()
        }
    }

    fn refreshing(context: &mut Context) -> (Brief, TaskId, Vec<Command>) {
        let mut brief = ready();
        brief.on_action(context, action_id(REFRESH));
        let commands = context.take_commands();
        let task = spawned(&commands);
        (brief, task, commands)
    }

    #[test]
    fn a_brief_survives_being_written_and_read_back() {
        let stories = vec![Story {
            title: "A title, with a comma".into(),
            site: "example.org".into(),
        }];
        assert_eq!(decode(&encode(&stories)), stories);
    }

    #[test]
    fn a_story_reports_where_it_is_from_rather_than_its_whole_address() {
        assert_eq!(site_of("https://www.bbc.co.uk/news/1234"), "bbc.co.uk");
        assert_eq!(site_of("https://example.org"), "example.org");
    }

    #[test]
    fn a_story_with_no_link_is_attributed_to_the_site_itself() {
        let story = parse_story(br#"{"title":"Ask HN: anything","by":"someone"}"#)
            .expect("the story was dropped");
        assert_eq!(story.site, "news.ycombinator.com");
    }

    #[test]
    fn a_tab_in_a_title_cannot_break_the_stored_format() {
        // Otherwise a title containing a tab would split into a title and a
        // site on the way back in, and the brief would be quietly wrong.
        let story =
            parse_story(b"{\"title\":\"one\\ttwo\",\"url\":\"https://e.org/\"}").expect("dropped");
        assert_eq!(decode(&encode(&[story])).len(), 1);
    }

    #[test]
    fn the_index_is_read_and_the_first_story_is_asked_for() {
        let mut context = Context::default();
        let (mut brief, task, started) = refreshing(&mut context);
        assert_eq!(url_of(&started), super::TOP);
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863,8864,8865,8866,8867,8868,8869]".to_vec()),
        );
        let commands = context.take_commands();
        assert_eq!(
            url_of(&commands),
            "https://hacker-news.firebaseio.com/v0/item/8863.json"
        );
        assert_eq!(brief.building.len(), STORIES);
    }

    #[test]
    fn work_started_before_leaving_still_finishes_afterwards() {
        // This is the whole point of the example. Nothing about being in the
        // background stops the fetch, and the answer is taken exactly as it
        // would have been.
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_background(&mut context);
        let _ignored = context.take_commands();
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        assert!(
            matches!(brief.fetching, Fetching::Story(_, 0)),
            "leaving stopped the work"
        );
        assert!(
            context
                .take_commands()
                .iter()
                .all(|command| !matches!(command, Command::SetScreen(_))),
            "a background application drew to a panel nobody was looking at"
        );
    }

    #[test]
    fn coming_back_draws_what_arrived_while_nobody_was_looking() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_background(&mut context);
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        let story = spawned(&context.take_commands());
        brief.on_task(
            &mut context,
            story,
            TaskOutcome::Completed(
                br#"{"title":"Something happened","url":"https://e.org/x"}"#.to_vec(),
            ),
        );
        let _ignored = context.take_commands();
        brief.on_foreground(&mut context);
        let commands = context.take_commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::SetScreen(_))),
            "coming back drew nothing"
        );
        assert_eq!(brief.stories.len(), 1);
        assert_eq!(brief.stories[0].title, "Something happened");
    }

    #[test]
    fn a_finished_brief_is_written_down_without_being_asked() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        let story = spawned(&context.take_commands());
        brief.on_task(
            &mut context,
            story,
            TaskOutcome::Completed(br#"{"title":"Kept","url":"https://e.org/x"}"#.to_vec()),
        );
        let saved = context.take_commands().iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == super::STORED
            )
        });
        assert!(saved, "the brief was not kept");
    }

    #[test]
    fn a_failed_refresh_leaves_the_previous_brief_alone() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.stories = vec![Story {
            title: "yesterday".into(),
            site: "e.org".into(),
        }];
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_eq!(brief.stories.len(), 1, "a failure emptied the brief");
        assert!(brief.note.is_some(), "a failure said nothing");
    }
}
