//! Kitchen Card is a deliberately narrow, read-only Mealie companion.

mod amounts;
mod cache;
mod mealie;

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, DeviceRequest, DeviceResult, FieldKind,
    Glyph, KoboApp, Position, RowState, Screen, ScreenBuilder, StoreResult, Task, TaskId,
    TaskOutcome,
};
use mealie::Recipe;
use std::{
    process::ExitCode,
    time::{Duration, SystemTime},
};

const CONFIG: &str = "config";
const TONIGHT: &str = "tonight";
const CACHE_ERROR: &str = "Recipes could not be saved or opened. Retry saving before closing.";
/// What Check again says when the computer has sent nothing. The banner that
/// says it carries Check again itself, so the sentence need not tell the
/// owner to check again.
const NO_ACCOUNT_YET: &str = "No account has arrived yet.";

/// The most a recipe can be scaled to, and the least is one.
const MAX_SERVINGS: u32 = 12;

/// How tonight's recipe is laid out, the roomiest first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TonightShape {
    /// Start cooking and Pick recipe as buttons under the recipe.
    Stacked,
    /// The two in the bar at the foot.
    Barred,
    /// The two in the bar, and the counts and description on a second page.
    Paged,
}

/// How much of the first-run screen the panel has room for, most first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    Marked,
    Plain,
    Compact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Tonight,
    Browse,
    Cook,
    Ingredients,
    Finished,
    Settings,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting {
    Server,
}
/// A cooking timer, counted against the clock rather than by the ticks that
/// redraw it, so time spent asleep or in another app still counts.
///
/// The ticks are short sleeps that end on each minute boundary while the app
/// is in front. A scheduled wake is asked for as well, for a reader that goes
/// to sleep mid-cook; the runtime may grant it later than asked, so whichever
/// comes first ends the timer and the clock says how late that was.
#[derive(Clone, Debug)]
struct Timer {
    step: usize,
    minutes: u32,
    ends: SystemTime,
    tick: Option<TaskId>,
    /// The reader would not wake for the timer, so it only ends while
    /// Kitchen Card is open.
    wake_refused: bool,
}

impl Timer {
    fn left(&self, now: SystemTime) -> Duration {
        self.ends.duration_since(now).unwrap_or_default()
    }

    /// Seconds to the next whole minute left, or to the end, whichever is
    /// first, so the banner changes when its number does.
    fn next_tick(&self, now: SystemTime) -> u32 {
        let left = u32::try_from(self.left(now).as_secs()).unwrap_or(u32::MAX);
        (left.saturating_sub(1) % 60) + 1
    }

    fn banner(&self, now: SystemTime) -> String {
        let left = self.left(now).as_secs();
        let left = if left < 60 {
            "under a minute left".to_owned()
        } else {
            format!("{} min left", left.div_ceil(60))
        };
        let mut text = format!("Timer: {left}, set on step {}.", self.step + 1);
        if self.wake_refused {
            text.push_str(" Keep Kitchen Card open: this reader cannot wake itself for it.");
        }
        text
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingTask {
    List,
    Detail(usize),
}

#[derive(Default)]
struct Kitchen {
    snapshot: Option<Snapshot>,
    cache_dirty: bool,
    server: String,
    credential: String,
    recipes: Vec<Recipe>,
    recipes_origin: Option<(String, String)>,
    task_origin: Option<(String, String)>,
    importing: Option<(Vec<mealie::Stub>, usize)>,
    imported: Vec<Recipe>,
    dropped: usize,
    task: Option<(TaskId, PendingTask)>,
    tonight: Option<String>,
    servings: u32,
    step: usize,
    browse_page: usize,
    ingredients_page: usize,
    /// Which half of tonight's recipe is shown where it takes two pages.
    tonight_page: usize,
    checked: Vec<usize>,
    timer: Option<Timer>,
    /// What the last timer did, said on the cooking pages where the timer
    /// was, since the note at the top of the home page is not seen there.
    timer_note: Option<(bool, String)>,
    view: Option<View>,
    note: Option<(bool, String)>,
    offline: bool,
    /// The setting being typed on the runtime's keyboard.
    entry: TextEntry,
    editing: Option<Setting>,
    /// The owner tapped Check again and is owed an answer either way.
    checking: bool,
}

impl Kitchen {
    fn ready(&self) -> bool {
        self.server.starts_with("https://") && !self.credential.is_empty()
    }
    fn recipe(&self) -> Option<&Recipe> {
        self.tonight
            .as_deref()
            .and_then(|slug| self.recipes.iter().find(|recipe| recipe.slug == slug))
    }
    fn info(&mut self, text: impl Into<String>) {
        self.note = Some((false, text.into()));
    }
    fn problem(&mut self, text: impl Into<String>) {
        self.note = Some((true, text.into()));
    }
    /// Takes down the note that no account had arrived, once one has. Left
    /// up, it sat over the empty shelf of an app that was ready to sync.
    fn clear_no_account_note(&mut self) {
        if self
            .note
            .as_ref()
            .is_some_and(|(_, note)| note == NO_ACCOUNT_YET)
        {
            self.note = None;
        }
    }
    /// Whether the list of recipes is the page showing: picked from the bar,
    /// or the home page itself while nothing is chosen for tonight.
    fn browsing(&self) -> bool {
        match self.view {
            Some(View::Browse) => true,
            None | Some(View::Tonight) => {
                self.editing.is_none()
                    && self.ready()
                    && !self.recipes.is_empty()
                    && self.recipe().is_none()
            }
            _ => false,
        }
    }

    fn open_cache(&mut self, context: &mut Context) {
        if !self.ready() {
            return;
        }
        let snapshot = Snapshot::new(&format!(
            "kitchencard-v1:{}\n{}",
            self.server, self.credential
        ))
        .at_most(cache::LIMIT);
        snapshot.start(context);
        self.snapshot = Some(snapshot);
        self.cache_dirty = false;
    }

    fn keep_recipes(&mut self, context: &mut Context) {
        self.cache_dirty = true;
        self.flush_cache(context);
    }

    fn flush_cache(&mut self, context: &mut Context) {
        if !self.cache_dirty {
            return;
        }
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        if snapshot.busy() {
            return;
        }
        let Some(bytes) = cache::encode(&self.recipes) else {
            self.problem(
                "These recipes exceed the offline storage limit. Keep fewer and sync again.",
            );
            return;
        };
        if snapshot.save(context, bytes) {
            self.cache_dirty = false;
        }
    }

    fn cache_event(&mut self, context: &mut Context, event: Option<SnapshotEvent>) {
        match event {
            Some(SnapshotEvent::Loaded) => {
                if let Some(bytes) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.bytes.as_deref())
                {
                    match cache::decode(bytes) {
                        Some(recipes) => {
                            self.recipes = recipes;
                            self.recipes_origin =
                                Some((self.server.clone(), self.credential.clone()));
                            self.reconcile(context);
                        }
                        None => self.problem(
                            "Saved recipes could not be opened. Sync to import them again.",
                        ),
                    }
                }
            }
            Some(SnapshotEvent::Failed) => self.problem(CACHE_ERROR),
            Some(SnapshotEvent::Saved)
                if self.note.as_ref().is_some_and(|note| note.1 == CACHE_ERROR) =>
            {
                self.note = None;
            }
            _ => {}
        }
        self.flush_cache(context);
        self.show(context);
    }

    /// Drops tonight's card when its recipe is gone, and clamps what stays.
    fn reconcile(&mut self, context: &mut Context) {
        let sizes = self
            .recipe()
            .map(|recipe| (recipe.steps.len(), recipe.ingredients.len()));
        match sizes {
            Some((steps, ingredients)) => {
                self.servings = self.servings.clamp(1, 12);
                self.step = self.step.min(steps.saturating_sub(1));
                self.checked.retain(|index| *index < ingredients);
            }
            None => {
                if self.tonight.is_some() {
                    self.tonight = None;
                    self.servings = 2;
                    self.step = 0;
                    self.checked.clear();
                    self.persist_tonight(context);
                }
            }
        }
    }

    fn persist_config(&self, context: &mut Context) {
        context
            .store()
            .save(CONFIG, format!("{}\n{}", self.server, self.credential));
    }

    fn persist_tonight(&self, context: &mut Context) {
        context.store().save(
            TONIGHT,
            self.tonight.as_deref().map_or_else(String::new, |slug| {
                format!(
                    "{}|{}|{}|{}",
                    slug,
                    self.servings,
                    self.step,
                    self.checked
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }),
        );
    }

    fn select(&mut self, context: &mut Context, slug: &str) {
        let Some((slug, servings)) = self
            .recipes
            .iter()
            .find(|recipe| recipe.slug == slug)
            .map(|recipe| (recipe.slug.clone(), recipe.servings))
        else {
            return;
        };
        self.stop_timer(context);
        self.tonight = Some(slug);
        self.servings = servings;
        self.step = 0;
        self.ingredients_page = 0;
        self.tonight_page = 0;
        self.checked.clear();
        self.view = Some(View::Tonight);
        self.info("Tonight's card is saved for offline cooking.");
        self.persist_tonight(context);
    }

    fn sync(&mut self, context: &mut Context) {
        if !self.ready() {
            self.view = Some(View::Settings);
            return;
        }
        if self.task.is_some() {
            self.info("Recipes are already updating.");
            return;
        }
        self.info("Asking Mealie for the recipe list…");
        if let Some(id) = context.spawn_retrying(Task::Fetch {
            url: mealie::list_url(&self.server),
            offset: 0,
            max_bytes: 256 * 1024,
            credential: Some(Credential::bearer(&self.credential)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::List));
            self.task_origin = Some((self.server.clone(), self.credential.clone()));
        }
    }

    fn fetch_detail(&mut self, context: &mut Context) {
        let Some((slug, index, total)) = self.importing.as_ref().and_then(|(stubs, index)| {
            stubs
                .get(*index)
                .map(|stub| (stub.slug.clone(), *index, stubs.len()))
        }) else {
            if self.importing.is_some() {
                self.finish_import(context);
            }
            return;
        };
        self.info(format!("Fetching recipe {} of {}…", index + 1, total));
        if let Some(id) = context.spawn_retrying(Task::Fetch {
            url: mealie::detail_url(&self.server, &slug),
            offset: 0,
            max_bytes: 256 * 1024,
            credential: Some(Credential::bearer(&self.credential)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::Detail(index)));
            self.task_origin = Some((self.server.clone(), self.credential.clone()));
        }
    }

    fn finish_import(&mut self, context: &mut Context) {
        let total = self.importing.take().map_or(0, |(stubs, _)| stubs.len());
        self.recipes = std::mem::take(&mut self.imported);
        self.recipes_origin = Some((self.server.clone(), self.credential.clone()));
        self.reconcile(context);
        self.keep_recipes(context);
        let kept = self.recipes.len();
        if kept == 0 && total > 0 {
            self.problem("Mealie's recipes arrived unreadable. Nothing was replaced.");
        } else if self.dropped > 0 {
            self.problem(format!(
                "Kept {kept} of {total} recipes; {} arrived unreadable.",
                self.dropped
            ));
        } else {
            self.info(format!(
                "{kept} recipe{} from Mealie.",
                if kept == 1 { "" } else { "s" }
            ));
        }
        self.dropped = 0;
    }

    /// The real cause, never "check the address" for a refused token: a
    /// wrong address cannot answer 401, and an owner sent to retype a correct
    /// address never finds the expired token.
    fn fail(&mut self, error: kobo_sdk::TaskError) {
        if error == kobo_sdk::TaskError::Offline {
            self.offline = true;
        }
        let problem = kobo_sdk::failure::Service::new("Mealie")
            .at(&self.server)
            .fixed_by("Run kobo connect kitchencard on your computer.")
            .explain(error);
        self.problem(if self.recipes.is_empty() {
            problem.message
        } else {
            format!("{} Saved recipes still cook.", problem.message)
        });
    }

    /// Reads the saved address again, which `kobo connect kitchencard` may
    /// have written while the app was away.
    fn reload_config(&mut self, context: &mut Context) {
        if self.editing.is_none() && self.task.is_none() {
            context.store().load(CONFIG);
        }
    }

    fn start_timer(&mut self, context: &mut Context, minutes: u32, now: SystemTime) {
        let length = Duration::from_secs(u64::from(minutes) * 60);
        context.device().schedule_wake(length);
        self.timer = Some(Timer {
            step: self.step,
            minutes,
            ends: now + length,
            tick: None,
            wake_refused: false,
        });
        self.timer_note = None;
        self.arm_tick(context, now);
    }

    fn arm_tick(&mut self, context: &mut Context, now: SystemTime) {
        if let Some(timer) = self.timer.as_mut().filter(|timer| timer.tick.is_none()) {
            timer.tick = context.spawn(Task::Sleep {
                seconds: timer.next_tick(now),
            });
        }
    }

    /// Ends the timer once its time has come, or keeps it ticking.
    fn check_timer(&mut self, context: &mut Context, now: SystemTime) {
        let Some(timer) = &self.timer else {
            return;
        };
        if timer.left(now) > Duration::ZERO {
            self.arm_tick(context, now);
            return;
        }
        let late = now.duration_since(timer.ends).unwrap_or_default().as_secs() / 60;
        let what = format!(
            "the {} min timer from step {}",
            timer.minutes,
            timer.step + 1
        );
        self.timer_note = Some((
            true,
            if late >= 2 {
                format!("Time was up {late} min ago: {what}.")
            } else {
                format!("Time is up: {what}.")
            },
        ));
        self.stop_timer(context);
    }

    fn finish(&mut self, context: &mut Context) {
        self.stop_timer(context);
        self.view = Some(View::Finished);
        self.step = 0;
        self.persist_tonight(context);
    }

    /// Back in front, the timer may have run out while away.
    fn resume_timer(&mut self, context: &mut Context) {
        if self.timer.is_some() {
            self.check_timer(context, SystemTime::now());
            self.show(context);
        }
    }

    fn stop_timer(&mut self, context: &mut Context) {
        if let Some(timer) = self.timer.take() {
            context.device().cancel_wake();
            if let Some(tick) = timer.tick {
                context.cancel(tick);
            }
        }
    }

    /// The timer's banner on the cooking pages: how long is left while it
    /// runs, then what it did.
    fn timer_banner(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match &self.timer {
            Some(timer) => screen.banner(BannerLevel::Info, timer.banner(SystemTime::now())),
            None => Self::banner(screen, self.timer_note.as_ref()),
        }
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context).with_own_back(
            self.editing.is_some() || self.view.is_some_and(|view| view != View::Tonight),
        ));
    }

    fn banner(screen: ScreenBuilder, note: Option<&(bool, String)>) -> ScreenBuilder {
        match note {
            Some((true, text)) => screen.banner(BannerLevel::Attention, text),
            Some((false, text)) => screen.banner(BannerLevel::Info, text),
            None => screen,
        }
    }

    fn page_controls(screen: ScreenBuilder, page: usize, count: usize) -> ScreenBuilder {
        if count <= 1 {
            return screen;
        }
        screen
            .page_turns("list-previous", "list-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(count).unwrap_or(u16::MAX),
            )
    }

    fn browse_prefix(&self) -> ScreenBuilder {
        // Sync then Settings, in the order tonight's page has them.
        let screen = if self.view == Some(View::Browse) {
            ScreenBuilder::new("kitchencard")
                .top_bar("Pick a recipe")
                .top_bar_action("sync", "Sync")
        } else {
            ScreenBuilder::new("kitchencard")
                .top_bar("Kitchen Card")
                .top_bar_action("sync", "Sync")
                .top_bar_glyph("settings", "Settings", Glyph::Settings)
        };
        Self::banner(screen, self.note.as_ref())
    }

    fn browse_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.recipes.len()).collect();
        order.sort_by_key(|&index| &self.recipes[index].category);
        order
    }

    fn browse_slice(&self, indices: &[usize], page: usize, count: usize) -> Screen {
        let mut screen = self.browse_prefix();
        let mut group = Vec::new();
        let mut heading = None;
        for &index in indices {
            let recipe = &self.recipes[index];
            if heading != Some(recipe.category.as_str()) {
                if let Some(heading) = heading {
                    screen = screen.section_rows(heading, None, std::mem::take(&mut group));
                }
                heading = Some(recipe.category.as_str());
            }
            group.push((
                format!("recipe-{index}"),
                recipe.name.clone(),
                format!(
                    "serves {} · {} step{}",
                    recipe.servings,
                    recipe.steps.len(),
                    if recipe.steps.len() == 1 { "" } else { "s" }
                ),
                Glyph::Reader,
            ));
        }
        if let Some(heading) = heading {
            screen = screen.section_rows(heading, None, group);
        }
        Self::page_controls(screen, page, count).build()
    }

    fn browse_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let mut pages = Vec::new();
        let mut page = Vec::new();
        // At most twelve recipes: measure the exact grouped layout so that a
        // continuation page can repeat its category without stealing a row.
        for index in self.browse_order() {
            page.push(index);
            if page.len() > 1
                && !self
                    .browse_slice(&page, 0, 2)
                    .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                    .issues
                    .is_empty()
            {
                page.pop();
                pages.push(std::mem::take(&mut page));
                page.push(index);
            }
        }
        if !page.is_empty() {
            pages.push(page);
        }
        pages
    }

    fn browse_screen(&self, context: &Context) -> Screen {
        if self.recipes.is_empty() {
            return self
                .browse_prefix()
                .empty_state("Nothing from Mealie yet.")
                .primary_button("sync", "Sync Mealie")
                .build();
        }
        let pages = self.browse_pages(context);
        let page = self.browse_page.min(pages.len().saturating_sub(1));
        self.browse_slice(&pages[page], page, pages.len())
    }

    fn ingredients_prefix(&self) -> ScreenBuilder {
        let screen = ScreenBuilder::new("kitchencard")
            .top_bar("Ingredients")
            .tabs(1, [("cook", "Steps"), ("ingredients", "Ingredients")])
            .secondary(format!(
                "Serves {} · tap a line to check it off",
                self.servings
            ));
        self.timer_banner(screen)
    }

    /// The ingredient lines as a checklist. Each line carries whether it is
    /// ticked and the runtime draws the box, so a ticked line is the same
    /// size as an open one and nothing on the page moves when it is ticked.
    /// There is no selection bar: ticking off is the whole job here, and
    /// nothing is done to the ticked lines all at once.
    fn ingredient_rows(&self) -> Vec<(String, String, String, Glyph, RowState)> {
        self.recipe().map_or_else(Vec::new, |recipe| {
            recipe
                .ingredients
                .iter()
                .enumerate()
                .map(|(index, ingredient)| {
                    let amount = amounts::amount(ingredient, self.servings, recipe.servings);
                    let detail = if amount.is_empty() || amount == ingredient.label() {
                        String::new()
                    } else {
                        amount
                    };
                    (
                        format!("ingredient-{index}"),
                        ingredient.label().to_owned(),
                        detail,
                        // The box takes the lead's place, so this mark is
                        // never drawn; it is what the row would lead with
                        // anywhere it is not a checklist.
                        Glyph::Circle,
                        if self.checked.contains(&index) {
                            RowState::Checked
                        } else {
                            RowState::Unchecked
                        },
                    )
                })
                .collect()
        })
    }

    fn ingredients_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.ingredient_rows();
        let borrowed = rows
            .iter()
            .map(|(_, title, detail, _, _)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        context.paginate_rows_under(
            &borrowed,
            false,
            Position::AtTheFoot,
            &self.ingredients_prefix().build(),
        )
    }

    fn ingredients_screen(&self, context: &Context) -> Screen {
        if self.recipe().is_none() {
            return ScreenBuilder::new("kitchencard")
                .top_bar("Ingredients")
                .empty_state("Pick a recipe first.")
                .primary_button("browse", "Pick a recipe")
                .build();
        }
        let screen = self.ingredients_prefix();
        let rows = self.ingredient_rows();
        if rows.is_empty() {
            return screen
                .empty_state("No ingredient list came with this recipe.")
                .build();
        }
        let pages = self.ingredients_pages(context);
        let page = self.ingredients_page.min(pages.len().saturating_sub(1));
        Self::page_controls(
            screen.checked_rows(pages[page].iter().map(|&index| rows[index].clone())),
            page,
            pages.len(),
        )
        .build()
    }

    /// The first-run screen: the one computer command, Check again, and
    /// typing the address on the reader instead.
    ///
    /// Once a check has found nothing, Check again is the action of the
    /// banner that says so, and the button under the command goes, so it is
    /// offered once. Where the panel still has no room, held sideways at the
    /// larger text sizes, the book mark goes first and then the heading is
    /// set as a section over the full width; the words and both controls
    /// stay at every size.
    fn connect_screen(&self, context: &Context) -> Screen {
        const TEXT: &str =
            "Run this on your computer. It asks for your Mealie address and API token.";
        let checked = self
            .note
            .as_ref()
            .is_some_and(|(_, note)| note == NO_ACCOUNT_YET);
        let build = |shape: Shape| {
            let mut screen = Self::banner(
                ScreenBuilder::new("kitchencard").top_bar("Kitchen Card"),
                self.note.as_ref(),
            );
            if checked {
                screen = screen.banner_action("check-again", "Check again");
            }
            screen = match shape {
                Shape::Marked => screen.splash(Some(Glyph::Book), "Connect Mealie", TEXT),
                Shape::Plain => screen.splash(None, "Connect Mealie", TEXT),
                Shape::Compact => screen.section("Connect Mealie").secondary(TEXT),
            }
            .command("kobo connect kitchencard");
            if !checked {
                screen = screen.primary_button("check-again", "Check again");
            }
            screen.button("settings", "Type the address here").build()
        };
        let metrics = context.metrics();
        [Shape::Marked, Shape::Plain]
            .into_iter()
            .map(build)
            .find(|screen| {
                !screen
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
            })
            .unwrap_or_else(|| build(Shape::Compact))
    }

    /// Tonight's recipe: what it is, how many it serves, and the way into
    /// cooking it.
    ///
    /// The servings are a stepper, one number with a minus and a plus,
    /// because two buttons spelling "− serving" and "+ serving" beside the
    /// number said it three times and wrapped onto two lines each at the
    /// larger text sizes. Where the panel still cannot hold the page, at the
    /// larger sizes, Start cooking and Pick recipe move into the bar at the
    /// foot, and held sideways the recipe's counts and description then move
    /// to a second page the owner turns to; nothing is dropped.
    fn tonight_screen(&self, context: &Context, recipe: &Recipe) -> Screen {
        let cook = if self.step > 0 && self.step < recipe.steps.len() {
            format!("Continue from step {}", self.step + 1)
        } else {
            "Start cooking".to_owned()
        };
        let build = |shape: TonightShape, page: usize| {
            let first = shape != TonightShape::Paged || page == 0;
            let second = shape != TonightShape::Paged || page == 1;
            let screen = ScreenBuilder::new("kitchencard")
                .top_bar("Kitchen Card")
                .top_bar_action("sync", "Sync")
                .top_bar_glyph("settings", "Settings", Glyph::Settings);
            // The second page carries the recipe's name as a running head, as
            // a book's later pages do, so the counts and the description keep
            // the room the title took on the first; the note was read there.
            let mut screen = if first {
                Self::banner(
                    screen.heading(&recipe.name).secondary(&recipe.category),
                    self.note.as_ref(),
                )
            } else {
                screen.section(&recipe.name)
            };
            if first {
                screen = screen
                    .stepper(
                        format!("Serves {}", self.servings),
                        "less",
                        Glyph::Minus,
                        "more",
                        Glyph::Plus,
                    )
                    .stepper_ends(self.servings > 1, self.servings < MAX_SERVINGS);
            }
            if second {
                screen = screen.facts([
                    ("Steps", recipe.steps.len().to_string()),
                    ("Ingredients", recipe.ingredients.len().to_string()),
                ]);
                if !recipe.description.is_empty() {
                    screen = screen.text(&recipe.description);
                }
            }
            if recipe.steps.is_empty() && first {
                screen = screen.secondary("No steps came with this recipe.");
            }
            if shape == TonightShape::Paged {
                screen = screen
                    .page_turns("tonight-previous", "tonight-next")
                    .page_position(u16::try_from(page + 1).unwrap_or(u16::MAX), 2);
            }
            if shape == TonightShape::Stacked {
                screen = if recipe.steps.is_empty() {
                    screen.disabled_button("cook", "Start cooking")
                } else {
                    screen.primary_button("cook", &cook)
                };
                return screen.button("browse", "Pick recipe").build();
            }
            // In the bar a recipe without steps has nothing to start, and the
            // line above already says so.
            if recipe.steps.is_empty() {
                screen.bottom_action("browse", "Pick recipe").build()
            } else {
                screen
                    .action_bar([("cook", cook.as_str()), ("browse", "Pick recipe")])
                    .build()
            }
        };
        let metrics = context.metrics();
        let fits = |screen: &Screen| {
            !screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        [TonightShape::Stacked, TonightShape::Barred]
            .into_iter()
            .map(|shape| build(shape, 0))
            .find(fits)
            .unwrap_or_else(|| build(TonightShape::Paged, self.tonight_page.min(1)))
    }

    #[allow(clippy::too_many_lines)] // Each arm is a distinct, small screen.
    fn screen(&self, context: &Context) -> Screen {
        if let Some(setting) = self.editing {
            let prompt = match setting {
                Setting::Server => "Mealie HTTPS address",
            };
            return ScreenBuilder::new("kitchencard")
                .top_bar("Kitchen Card settings")
                .text_entry(&self.entry, prompt, "Save")
                .build();
        }
        let view = self.view.unwrap_or(View::Tonight);
        match view {
            View::Tonight if !self.ready() => self.connect_screen(context),
            View::Tonight if self.recipes.is_empty() => {
                let mut screen = Self::banner(
                    ScreenBuilder::new("kitchencard")
                        .top_bar("Kitchen Card")
                        .top_bar_glyph("settings", "Settings", Glyph::Settings),
                    self.note.as_ref(),
                );
                if self.offline && self.note.is_none() {
                    screen = screen.banner(BannerLevel::Info, "Off the air.");
                }
                screen
                    .splash(
                        Some(Glyph::Book),
                        "No recipes yet",
                        "Sync Mealie to bring tonight's dinner here.",
                    )
                    .primary_button("sync", "Sync Mealie")
                    .build()
            }
            View::Tonight => {
                // With nothing chosen for tonight, the recipes are the home
                // page. A page between them saying how many were waiting
                // and offering to show them was one more tap to the list.
                let Some(recipe) = self.recipe() else {
                    return self.browse_screen(context);
                };
                self.tonight_screen(context, recipe)
            }
            View::Browse => self.browse_screen(context),
            View::Cook => {
                let Some(recipe) = self.recipe() else {
                    return ScreenBuilder::new("kitchencard")
                        .top_bar("Kitchen Card")
                        .splash(
                            Some(Glyph::Book),
                            "Nothing on the go",
                            "Pick a recipe before cooking.",
                        )
                        .primary_button("browse", "Pick a recipe")
                        .build();
                };
                if recipe.steps.is_empty() {
                    return ScreenBuilder::new("kitchencard")
                        .top_bar("Cooking")
                        .empty_state("No steps came with this recipe.")
                        .button("tonight", "Back")
                        .build();
                }
                let step = self.step.min(recipe.steps.len() - 1);
                // The step is what a cook reads from across the counter, so
                // it is set large. A step too long for the page at that size
                // keeps body type rather than losing its end or its buttons.
                let large = self.cook_screen(recipe, step, true);
                let metrics = context.metrics();
                let chrome = kobo_sdk::Chrome::for_screen(
                    &large,
                    false,
                    kobo_sdk::Chrome::measuring(true).status,
                );
                let fits = !large
                    .diagnostics(&metrics, &chrome)
                    .issues
                    .iter()
                    .any(|issue| issue.severity == kobo_sdk::DiagnosticSeverity::Error);
                if fits {
                    large
                } else {
                    self.cook_screen(recipe, step, false)
                }
            }
            View::Ingredients => self.ingredients_screen(context),
            View::Finished => {
                let name = self
                    .recipe()
                    .map_or("Dinner", |recipe| recipe.name.as_str());
                let mut screen = ScreenBuilder::new("kitchencard")
                    .top_bar("Kitchen Card")
                    .splash(Some(Glyph::Check), "Dinner's ready", name);
                if let Some(recipe) = self.recipe() {
                    screen = screen.facts([
                        ("Serves", self.servings.to_string()),
                        ("Steps", recipe.steps.len().to_string()),
                    ]);
                }
                screen.button("again", "Cook again").build()
            }
            // The address is all there is to type here. The API token is
            // long and secret, so it comes from the computer with the same
            // command the first screen gives, and nothing on this screen
            // names a second way of doing it.
            // A problem is said here too, since an address refused on this
            // page otherwise just disappears from the field.
            View::Settings => Self::banner(
                ScreenBuilder::new("kitchencard").top_bar("Kitchen Card settings"),
                self.note.as_ref().filter(|(problem, _)| *problem),
            )
            .field("server", &self.server, "https://mealie.example")
            .field_label("Mealie address")
            .field_kind(FieldKind::Address)
            .section("API token")
            .secondary("Sent from your computer. Run this there, then sync here.")
            .command("kobo connect kitchencard")
            .build(),
        }
    }
}

impl Kitchen {
    fn cook_screen(&self, recipe: &Recipe, step: usize, large: bool) -> Screen {
        let last = step + 1 == recipe.steps.len();
        let mut screen = ScreenBuilder::new("kitchencard")
            .top_bar(format!("Cooking · {} of {}", step + 1, recipe.steps.len()))
            .tabs(0, [("cook", "Steps"), ("ingredients", "Ingredients")])
            .secondary(&recipe.name);
        screen = self.timer_banner(screen);
        screen = if large {
            screen.heading_at_level(2, &recipe.steps[step])
        } else {
            screen.text(&recipe.steps[step])
        };
        if self.timer.is_none() {
            if let Some(minutes) = step_timer(&recipe.steps[step]) {
                screen = if last {
                    screen.button("timer", format!("Start {minutes} min timer"))
                } else {
                    screen.primary_button("timer", format!("Start {minutes} min timer"))
                };
            }
        }
        if self.timer.is_some() {
            screen = screen.button("stop-timer", "Stop the timer");
        }
        // The right side of the page is the next step, as it is the next page
        // of a book. A Next step button said the same under every step and
        // took the room a long step needs.
        if last {
            screen = screen.primary_button("finish", "Finish");
        }
        screen
            .page_turns("previous", "next")
            .reading_menu("tonight")
            .build()
    }
}

/// The first duration a step names, in minutes: "20 minutes", "5 min",
/// "30-35 minutes" (the start of a range).
fn step_timer(step: &str) -> Option<u32> {
    let words: Vec<&str> = step
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        let lower = word.to_lowercase();
        if lower.starts_with("min") {
            for candidate in words[..index].iter().rev().take(2).rev() {
                if let Ok(minutes) = candidate.parse::<u32>() {
                    if minutes > 0 {
                        return Some(minutes);
                    }
                }
            }
            return None;
        }
    }
    None
}

impl KoboApp for Kitchen {
    fn on_foreground(&mut self, context: &mut Context) {
        self.reload_config(context);
        self.resume_timer(context);
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.reload_config(context);
        self.resume_timer(context);
    }

    fn on_start(&mut self, context: &mut Context) {
        "mealie".clone_into(&mut self.credential);
        self.servings = 2;
        context.store().load(CONFIG);
        context.store().load(TONIGHT);
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.key == key)
        {
            let event = snapshot.stored(context, &result);
            self.cache_event(context, event);
        } else {
            self.on_store(context, result);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.key == key)
        {
            let event = snapshot.stored(context, &result);
            self.cache_event(context, event);
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.owns_file(name))
        {
            let event = snapshot.shelf(context, &result);
            self.cache_event(context, event);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
                let before = (self.server.clone(), self.credential.clone());
                if let Some(value) = &value {
                    let text = String::from_utf8_lossy(value);
                    let mut parts = text.lines();
                    parts
                        .next()
                        .unwrap_or_default()
                        .clone_into(&mut self.server);
                    parts
                        .next()
                        .unwrap_or("mealie")
                        .clone_into(&mut self.credential);
                }
                if self.snapshot.is_none()
                    || before != (self.server.clone(), self.credential.clone())
                {
                    if self.snapshot.is_some() {
                        self.recipes.clear();
                        self.recipes_origin = None;
                    }
                    self.open_cache(context);
                }
                if std::mem::take(&mut self.checking) && !self.ready() {
                    self.problem(NO_ACCOUNT_YET);
                } else if self.ready() {
                    self.clear_no_account_note();
                }
            }
            if key == TONIGHT {
                if let Some(value) = &value {
                    let text = String::from_utf8_lossy(value);
                    let mut parts = text.split('|');
                    let slug = parts.next().unwrap_or_default();
                    if !slug.is_empty() {
                        self.tonight = Some(slug.to_owned());
                        self.servings = parts
                            .next()
                            .and_then(|servings| servings.parse().ok())
                            .unwrap_or(2);
                        self.step = parts.next().and_then(|step| step.parse().ok()).unwrap_or(0);
                        self.checked = parts
                            .next()
                            .unwrap_or_default()
                            .split(',')
                            .filter_map(|index| index.parse().ok())
                            .collect();
                    }
                }
            }
            self.show(context);
        }
    }

    #[allow(clippy::too_many_lines)] // Actions are flat by design; each arm is one behavior.
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // "Timer stopped." answers the tap that stopped it and goes with the
        // next one; "Time is up" stays until the cook moves on.
        if self
            .timer_note
            .as_ref()
            .is_some_and(|(problem, _)| !problem)
        {
            self.timer_note = None;
        }
        if let Some(setting) = self.editing {
            match self.entry.handle(action) {
                Some(Typing::Submitted(value)) => {
                    if !value.is_empty() {
                        match setting {
                            Setting::Server => {
                                if value.starts_with("https://") {
                                    self.server = value;
                                    self.clear_no_account_note();
                                    self.recipes.clear();
                                    self.recipes_origin = None;
                                    self.tonight = None;
                                    self.persist_config(context);
                                    self.persist_tonight(context);
                                    self.open_cache(context);
                                } else {
                                    self.problem(
                                        "The address must start with https://. Kitchen Card does not use plain http.",
                                    );
                                }
                            }
                        }
                    }
                    self.editing = None;
                }
                Some(Typing::Cancelled) => self.editing = None,
                Some(_) => {}
                None => {
                    if action == ActionId::BACK {
                        self.entry.close();
                        self.editing = None;
                    }
                }
            }
        } else if action == action_id("check-again") {
            self.note = None;
            self.checking = true;
            self.reload_config(context);
        } else if action == action_id("settings") {
            self.view = Some(View::Settings);
        } else if action == action_id("server") {
            self.entry = TextEntry::new().address();
            self.entry.open_with(if self.server.is_empty() {
                "https://"
            } else {
                &self.server
            });
            self.editing = Some(Setting::Server);
        } else if action == action_id("back") || action == ActionId::BACK {
            if self.view == Some(View::Ingredients) {
                self.view = Some(View::Cook);
            } else {
                if matches!(self.view, Some(View::Cook | View::Finished)) {
                    self.stop_timer(context);
                }
                self.view = Some(View::Tonight);
            }
        } else if action == action_id("retry-save") {
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.retry(context);
            }
            self.flush_cache(context);
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("browse") {
            self.browse_page = 0;
            self.view = Some(View::Browse);
        } else if action == action_id("tonight") {
            if self.view == Some(View::Cook) || self.view == Some(View::Finished) {
                self.stop_timer(context);
            }
            self.view = Some(View::Tonight);
        } else if action == action_id("cook") {
            // The step is kept, so a cook who stepped away, or whose reader
            // restarted, carries on where they were. Finishing starts the
            // next cook at the top.
            self.view = Some(View::Cook);
            if self.timer.is_none() {
                self.timer_note = None;
            }
            context.device().keep_awake(Duration::from_secs(3600));
            self.persist_tonight(context);
        } else if action == action_id("ingredients") {
            self.view = Some(View::Ingredients);
        } else if action == action_id("list-previous") || action == action_id("list-next") {
            let browsing = self.browsing();
            let (page, count) = match self.view {
                _ if browsing => (self.browse_page, self.browse_pages(context).len()),
                Some(View::Ingredients) => {
                    (self.ingredients_page, self.ingredients_pages(context).len())
                }
                _ => return,
            };
            let last = count.saturating_sub(1);
            let page = page.min(last);
            let next = if action == action_id("list-next") {
                page.saturating_add(1).min(last)
            } else {
                page.saturating_sub(1)
            };
            if browsing {
                self.browse_page = next;
            } else {
                self.ingredients_page = next;
            }
        } else if action == action_id("tonight-next") {
            self.tonight_page = 1;
        } else if action == action_id("tonight-previous") {
            self.tonight_page = 0;
        } else if action == action_id("more") && self.servings < MAX_SERVINGS {
            self.servings += 1;
            self.persist_tonight(context);
        } else if action == action_id("less") && self.servings > 1 {
            self.servings -= 1;
            self.persist_tonight(context);
        } else if action == action_id("next") {
            if let Some(recipe) = self.recipe() {
                if self.step + 1 < recipe.steps.len() {
                    self.step += 1;
                    self.persist_tonight(context);
                } else {
                    self.finish(context);
                }
            }
        } else if action == action_id("previous") && self.step > 0 {
            self.step -= 1;
            self.persist_tonight(context);
        } else if action == action_id("finish") {
            self.finish(context);
        } else if action == action_id("again") {
            self.step = 0;
            self.checked.clear();
            self.view = Some(View::Cook);
            self.persist_tonight(context);
        } else if action == action_id("timer") {
            if let Some(recipe) = self.recipe() {
                if let Some(minutes) = recipe.steps.get(self.step).and_then(|s| step_timer(s)) {
                    self.start_timer(context, minutes, SystemTime::now());
                }
            }
        } else if action == action_id("stop-timer") {
            self.stop_timer(context);
            self.timer_note = Some((false, "Timer stopped.".to_owned()));
        } else if let Some(index) =
            (0..self.recipes.len()).find(|index| action == action_id(&format!("recipe-{index}")))
        {
            let slug = self.recipes[index].slug.clone();
            self.select(context, &slug);
        } else if self.view == Some(View::Ingredients) {
            if let Some(index) = (0..self.recipe().map_or(0, |recipe| recipe.ingredients.len()))
                .find(|index| action == action_id(&format!("ingredient-{index}")))
            {
                if let Some(position) = self.checked.iter().position(|kept| *kept == index) {
                    self.checked.remove(position);
                } else {
                    self.checked.push(index);
                }
                self.persist_tonight(context);
            }
        }
        self.show(context);
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        if self.timer.is_some() {
            self.check_timer(context, SystemTime::now());
            self.show(context);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if let (DeviceRequest::ScheduleWake { .. }, DeviceResult::Denied(_)) = (request, result) {
            if let Some(timer) = &mut self.timer {
                timer.wake_refused = true;
                self.show(context);
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if let Some(timer) = self.timer.as_mut().filter(|timer| timer.tick == Some(task)) {
            timer.tick = None;
            // A tick cut short by the app going to the back is armed again
            // when it comes to the front.
            if matches!(outcome, TaskOutcome::Completed(_)) {
                self.check_timer(context, SystemTime::now());
                self.show(context);
            }
            return;
        }
        let Some((id, kind)) = self.task else {
            return;
        };
        if id != task {
            return;
        }
        self.task = None;
        let origin = (self.server.clone(), self.credential.clone());
        if self
            .task_origin
            .take()
            .is_some_and(|requested| requested != origin)
        {
            return;
        }
        match (kind, outcome) {
            (PendingTask::List, TaskOutcome::Completed(bytes)) => {
                self.offline = false;
                match mealie::parse_list(&bytes) {
                    Some(stubs) if stubs.is_empty() => {
                        self.recipes.clear();
                        self.reconcile(context);
                        self.keep_recipes(context);
                        self.info(
                            "Mealie has no recipes yet. Add some on your computer, then sync.",
                        );
                    }
                    Some(mut stubs) => {
                        let overflow = stubs.len().saturating_sub(mealie::MAX_RECIPES);
                        stubs.truncate(mealie::MAX_RECIPES);
                        if overflow > 0 {
                            self.info(format!(
                                "Keeping the {} newest; Mealie listed {} more.",
                                stubs.len(),
                                overflow
                            ));
                        }
                        self.importing = Some((stubs, 0));
                        self.imported.clear();
                        self.dropped = 0;
                        self.fetch_detail(context);
                    }
                    None => self.problem("Mealie's answer was not a recipe list."),
                }
            }
            (PendingTask::Detail(index), TaskOutcome::Completed(bytes)) => {
                match mealie::parse_detail(&bytes) {
                    Some(recipe) => self.imported.push(recipe),
                    None => self.dropped += 1,
                }
                if let Some((_, next)) = &mut self.importing {
                    *next = index + 1;
                }
                self.fetch_detail(context);
            }
            (_, TaskOutcome::Cancelled) => {}
            (PendingTask::List, TaskOutcome::Failed(error)) => self.fail(error),
            (PendingTask::Detail(_), TaskOutcome::Failed(error)) => {
                if self.imported.is_empty() {
                    self.importing = None;
                    self.fail(error);
                } else {
                    let kept = self.imported.len();
                    self.problem(format!(
                        "Mealie stopped answering after {kept} recipes; kept what arrived."
                    ));
                    self.finish_import(context);
                }
            }
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("kitchencard", Kitchen::default()).map_or_else(
        |error| {
            eprintln!("kitchencard: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn recipe() -> Recipe {
        mealie::parse_detail(
            br#"{
                "slug":"lemon-chickpeas","name":"Lemon chickpeas",
                "description":"Bright, fast, and good with rice.",
                "recipeServings":2.0,
                "recipeCategory":[{"name":"Weeknight"}],
                "recipeIngredient":[
                    {"quantity":2.0,"unit":{"name":"tins"},"food":{"name":"chickpeas"},"note":"drained","display":"2 tins chickpeas"},
                    {"quantity":0.5,"unit":null,"food":{"name":"lemon"},"note":"","display":"1/2 lemon"}
                ],
                "recipeInstructions":[
                    {"title":"","text":"Warm the olive oil in a broad pan."},
                    {"title":"","text":"Add chickpeas and cook for 10 minutes until their edges colour."},
                    {"title":"","text":"Rest for 5 minutes, then serve."}
                ]
            }"#,
        )
        .unwrap()
    }

    fn cooking() -> Kitchen {
        let mut app = Kitchen {
            server: "https://mealie.example".to_owned(),
            credential: "mealie".to_owned(),
            recipes: vec![recipe()],
            ..Kitchen::default()
        };
        app.tonight = Some("lemon-chickpeas".to_owned());
        app.servings = 2;
        app
    }

    #[test]
    fn step_timers_are_found_in_plain_words() {
        assert_eq!(step_timer("Simmer for 20 minutes."), Some(20));
        assert_eq!(step_timer("rest 5 min"), Some(5));
        assert_eq!(step_timer("Bake 30-35 minutes until golden."), Some(30));
        assert_eq!(step_timer("Warm the oil."), None);
        assert_eq!(step_timer("Add 2 tins of tomatoes."), None);
    }

    #[test]
    fn step_navigation_stays_in_range() {
        let mut app = cooking();
        app.step = app.recipe().unwrap().steps.len() - 1;
        assert!(app.step + 1 >= app.recipe().unwrap().steps.len());
    }

    #[test]
    fn the_address_is_typed_on_the_runtime_with_its_own_keys() {
        let mut runner = kobo_sdk::AppRunner::new(Kitchen {
            view: Some(View::Settings),
            ..Kitchen::default()
        });
        let asked = |commands: &[kobo_sdk::Command]| {
            commands.iter().find_map(|command| match command {
                kobo_sdk::Command::Device(kobo_sdk::DeviceRequest::TypeText {
                    kind,
                    initial,
                    ..
                }) => Some((*kind, initial.clone())),
                _ => None,
            })
        };
        let commands = runner.action(action_id("server"));
        assert_eq!(
            asked(&commands),
            Some((FieldKind::Address, "https://".to_owned()))
        );
        runner.type_text("https://mealie.example");
        assert_eq!(runner.app().server, "https://mealie.example");
        assert!(runner.app().editing.is_none());
    }

    fn drawn(app: &Kitchen) -> String {
        format!("{:?}", app.screen(&Context::default()))
    }

    #[test]
    fn settings_send_the_token_with_the_same_command_as_the_first_screen() {
        let app = Kitchen {
            view: Some(View::Settings),
            ..cooking()
        };
        let settings = drawn(&app);
        assert!(settings.contains("kobo connect kitchencard"), "{settings}");
        assert!(!settings.contains("secret set"), "{settings}");
        assert!(!settings.contains("Credential"), "{settings}");
    }

    fn timing(minutes: u32) -> (Kitchen, Context, SystemTime) {
        let mut app = Kitchen {
            view: Some(View::Cook),
            step: 1,
            ..cooking()
        };
        let mut context = Context::default();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        app.start_timer(&mut context, minutes, start);
        (app, context, start)
    }

    #[test]
    fn a_timer_asks_for_a_wake_and_ticks_on_the_next_minute_boundary() {
        let (app, context, start) = timing(20);
        let asked: Vec<_> = context
            .commands()
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::Device(DeviceRequest::ScheduleWake { seconds }) => {
                    Some(*seconds)
                }
                _ => None,
            })
            .collect();
        assert_eq!(asked, [1200]);
        let timer = app.timer.as_ref().unwrap();
        assert!(timer.tick.is_some());
        assert_eq!(timer.next_tick(start), 60);
        assert_eq!(timer.next_tick(start + Duration::from_secs(90)), 30);
        assert_eq!(timer.banner(start), "Timer: 20 min left, set on step 2.");
    }

    #[test]
    fn a_timer_keeps_counting_until_its_time_and_then_says_time_is_up() {
        let (mut app, mut context, start) = timing(20);
        app.timer.as_mut().unwrap().tick = None;
        app.check_timer(&mut context, start + Duration::from_secs(5 * 60));
        let timer = app.timer.as_ref().expect("still running at five minutes");
        assert!(timer.tick.is_some(), "the next tick is armed");
        app.check_timer(&mut context, start + Duration::from_secs(20 * 60));
        assert!(app.timer.is_none());
        assert_eq!(
            app.timer_note,
            Some((true, "Time is up: the 20 min timer from step 2.".to_owned()))
        );
        assert!(drawn(&app).contains("Time is up"), "{}", drawn(&app));
    }

    #[test]
    fn a_timer_that_ran_out_while_the_reader_slept_says_how_long_ago() {
        let (mut app, mut context, start) = timing(5);
        app.check_timer(&mut context, start + Duration::from_secs(15 * 60));
        assert_eq!(
            app.timer_note,
            Some((
                true,
                "Time was up 10 min ago: the 5 min timer from step 2.".to_owned()
            ))
        );
    }

    #[test]
    fn a_refused_wake_keeps_the_timer_and_says_the_app_must_stay_open() {
        let mut runner = kobo_sdk::AppRunner::new(Kitchen {
            view: Some(View::Cook),
            step: 1,
            ..cooking()
        });
        runner.action(action_id("timer"));
        runner.device_result(DeviceResult::Denied(kobo_sdk::DenyReason::Unsupported));
        let app = runner.app();
        assert!(app.timer.as_ref().is_some_and(|timer| timer.wake_refused));
        assert!(
            drawn(app).contains("this reader cannot wake itself for it"),
            "{}",
            drawn(app)
        );
    }

    #[test]
    fn a_cook_who_steps_away_continues_from_the_saved_step_and_finishing_starts_over() {
        let mut runner = kobo_sdk::AppRunner::new(Kitchen {
            view: Some(View::Tonight),
            step: 2,
            ..cooking()
        });
        assert!(drawn(runner.app()).contains("Continue from step 3"));
        runner.action(action_id("cook"));
        assert_eq!(runner.app().step, 2);
        runner.action(action_id("finish"));
        assert_eq!(runner.app().step, 0);
        runner.action(action_id("tonight"));
        let tonight = drawn(runner.app());
        assert!(tonight.contains("Start cooking"), "{tonight}");
        assert!(!tonight.contains("Continue from"), "{tonight}");
    }

    #[test]
    fn a_plain_http_address_is_refused_with_the_reason_on_the_settings_page() {
        let mut runner = kobo_sdk::AppRunner::new(Kitchen {
            view: Some(View::Settings),
            credential: "mealie".to_owned(),
            ..Kitchen::default()
        });
        runner.action(action_id("server"));
        runner.type_text("http://mealie.invalid");
        assert!(runner.app().server.is_empty());
        let settings = drawn(runner.app());
        assert!(settings.contains("does not use plain http"), "{settings}");
    }

    #[test]
    fn the_note_that_no_account_arrived_goes_once_an_address_is_saved() {
        let mut runner = kobo_sdk::AppRunner::new(Kitchen {
            view: Some(View::Settings),
            credential: "mealie".to_owned(),
            ..Kitchen::default()
        });
        runner.app_mut().problem(NO_ACCOUNT_YET);
        runner.action(action_id("server"));
        runner.type_text("https://mealie.example");
        assert!(runner.app().note.is_none());
    }

    #[test]
    fn with_nothing_chosen_for_tonight_the_recipes_are_the_home_page() {
        let app = Kitchen {
            tonight: None,
            ..cooking()
        };
        let home = drawn(&app);
        assert!(!home.contains("Pick tonight"), "{home}");
        assert!(home.contains(&app.recipes[0].name), "{home}");
        assert!(home.contains("Settings"), "{home}");
    }

    #[test]
    fn every_screen_lays_out_clean_on_clara() {
        let chrome = Chrome::default();
        for (view, app) in [
            (None, Kitchen::default()),
            (
                None,
                Kitchen {
                    server: "https://mealie.example".to_owned(),
                    credential: "mealie".to_owned(),
                    ..Kitchen::default()
                },
            ),
            (Some(View::Tonight), cooking()),
            (Some(View::Browse), cooking()),
            (Some(View::Cook), cooking()),
            (
                Some(View::Cook),
                Kitchen {
                    timer: Some(Timer {
                        step: 1,
                        minutes: 10,
                        ends: SystemTime::now() + Duration::from_secs(600),
                        tick: None,
                        wake_refused: true,
                    }),
                    step: 1,
                    ..cooking()
                },
            ),
            (Some(View::Ingredients), cooking()),
            (
                Some(View::Ingredients),
                Kitchen {
                    checked: vec![0],
                    servings: 4,
                    ..cooking()
                },
            ),
            (Some(View::Finished), cooking()),
            (Some(View::Settings), cooking()),
        ] {
            let mut app = app;
            if let Some(view) = view {
                app.view = Some(view);
            }
            assert!(
                app.screen(&Context::default())
                    .diagnostics(&CLARA_BW_METRICS, &chrome)
                    .issues
                    .is_empty(),
                "view {view:?} has layout issues"
            );
        }
    }

    #[test]
    fn check_off_and_servings_scale_the_ingredient_lines() {
        let mut app = cooking();
        app.view = Some(View::Ingredients);
        app.servings = 4;
        app.checked = vec![0];
        let layout = app
            .screen(&Context::default())
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let text = format!("{layout:?}");
        assert!(text.contains("4 tins"), "scaled amount on screen: {text}");
        assert!(
            text.contains("text_lines: [\"1\"]"),
            "scaled whole amount on screen: {text}"
        );
    }
}

#[cfg(test)]
mod ui_review_tests;
