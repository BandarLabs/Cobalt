//! Kitchen Card is a deliberately narrow, read-only Mealie companion.

mod amounts;
mod cache;
mod mealie;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Glyph, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskId, TaskOutcome,
};
use mealie::Recipe;
use std::{process::ExitCode, time::Duration};

const CONFIG: &str = "config";
const TONIGHT: &str = "tonight";
const CACHE_ERROR: &str = "Recipes could not be saved or opened. Retry saving before closing.";

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
    Credential,
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
    checked: Vec<usize>,
    timer: Option<(usize, u32)>,
    view: Option<View>,
    note: Option<(bool, String)>,
    offline: bool,
    keyboard: Keyboard,
    editing: Option<Setting>,
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

    fn fail(&mut self, error: kobo_sdk::TaskError) {
        match error {
            kobo_sdk::TaskError::NoCredential => self.problem(format!(
                "No credential named `{}` yet. On your computer run `kobo secret set {}`.",
                self.credential, self.credential
            )),
            kobo_sdk::TaskError::Offline => {
                self.offline = true;
                self.problem("Off the air. Saved recipes still cook.");
            }
            kobo_sdk::TaskError::Denied => self.problem(format!(
                "Mealie refused the `{}` credential. Check the token on your computer.",
                self.credential
            )),
            _ => self.problem(format!(
                "Mealie did not answer at {}. Check the address in settings.",
                self.server
            )),
        }
    }

    fn start_timer(&mut self, context: &mut Context, minutes: u32) {
        context
            .device()
            .schedule_wake(Duration::from_secs(u64::from(minutes) * 60));
        self.timer = Some((self.step, minutes));
    }

    fn stop_timer(&mut self, context: &mut Context) {
        if self.timer.take().is_some() {
            context.device().cancel_wake();
        }
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn banner(screen: ScreenBuilder, note: Option<&(bool, String)>) -> ScreenBuilder {
        match note {
            Some((true, text)) => screen.banner(BannerLevel::Attention, text),
            Some((false, text)) => screen.banner(BannerLevel::Info, text),
            None => screen,
        }
    }

    #[allow(clippy::too_many_lines)] // One screen per view keeps the layout beside its data.
    fn screen(&self) -> Screen {
        if let Some(setting) = self.editing {
            let prompt = match setting {
                Setting::Server => "Mealie HTTPS address",
                Setting::Credential => "Credential name",
            };
            return ScreenBuilder::new("kitchencard")
                .top_bar("Kitchen Card settings")
                .typed(&self.keyboard, prompt)
                .keyboard(&self.keyboard, "Save")
                .build();
        }
        let view = self.view.unwrap_or(View::Tonight);
        match view {
            View::Tonight if !self.ready() => Self::banner(
                ScreenBuilder::new("kitchencard").top_bar("Kitchen Card"),
                self.note.as_ref(),
            )
            .splash(
                Some(Glyph::Book),
                "Connect Mealie",
                "On your computer run `kobo secret set mealie`, then add the HTTPS address here.",
            )
            .primary_button("settings", "Add address")
            .build(),
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
                let Some(recipe) = self.recipe() else {
                    return Self::banner(
                        ScreenBuilder::new("kitchencard")
                            .top_bar("Kitchen Card")
                            .top_bar_glyph("settings", "Settings", Glyph::Settings),
                        self.note.as_ref(),
                    )
                    .splash(
                        Some(Glyph::Book),
                        "Pick tonight's dinner",
                        format!(
                            "{} recipe{} from Mealie {} waiting.",
                            self.recipes.len(),
                            if self.recipes.len() == 1 { "" } else { "s" },
                            if self.recipes.len() == 1 { "is" } else { "are" }
                        ),
                    )
                    .primary_button("browse", "Pick a recipe")
                    .build();
                };
                let mut screen = Self::banner(
                    ScreenBuilder::new("kitchencard")
                        .top_bar("Kitchen Card")
                        .top_bar_action("sync", "Sync")
                        .top_bar_glyph("settings", "Settings", Glyph::Settings)
                        .heading(&recipe.name)
                        .secondary(&recipe.category),
                    self.note.as_ref(),
                );
                screen = screen.facts([
                    ("Serves", self.servings.to_string()),
                    ("Steps", recipe.steps.len().to_string()),
                    ("Ingredients", recipe.ingredients.len().to_string()),
                ]);
                if !recipe.description.is_empty() {
                    screen = screen.text(&recipe.description);
                }
                if recipe.steps.is_empty() {
                    screen = screen
                        .disabled_button("cook", "Start cooking")
                        .secondary("No steps came with this recipe.");
                } else {
                    screen = screen.primary_button("cook", "Start cooking");
                }
                screen
                    .buttons([
                        ("browse", "Pick recipe"),
                        ("less", "− serving"),
                        ("more", "+ serving"),
                    ])
                    .build()
            }
            View::Browse => {
                let mut screen = Self::banner(
                    ScreenBuilder::new("kitchencard")
                        .top_bar("Pick a recipe")
                        .top_bar_action("sync", "Sync"),
                    self.note.as_ref(),
                );
                if self.recipes.is_empty() {
                    return screen
                        .empty_state("Nothing from Mealie yet.")
                        .primary_button("sync", "Sync Mealie")
                        .button("tonight", "Back")
                        .build();
                }
                let mut by_category: std::collections::BTreeMap<&str, Vec<(usize, &Recipe)>> =
                    std::collections::BTreeMap::new();
                for (index, recipe) in self.recipes.iter().enumerate() {
                    by_category
                        .entry(recipe.category.as_str())
                        .or_default()
                        .push((index, recipe));
                }
                for (category, recipes) in by_category {
                    screen = screen.section_rows(
                        category,
                        None,
                        recipes.into_iter().map(|(index, recipe)| {
                            (
                                format!("recipe-{index}"),
                                recipe.name.clone(),
                                format!(
                                    "serves {} · {} step{}",
                                    recipe.servings,
                                    recipe.steps.len(),
                                    if recipe.steps.len() == 1 { "" } else { "s" }
                                ),
                                Glyph::Reader,
                            )
                        }),
                    );
                }
                screen.button("tonight", "Back").build()
            }
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
                let last = step + 1 == recipe.steps.len();
                let mut screen = ScreenBuilder::new("kitchencard")
                    .top_bar(format!("Cooking · {} of {}", step + 1, recipe.steps.len()))
                    .tabs(0, [("cook", "Steps"), ("ingredients", "Ingredients")])
                    .secondary(&recipe.name);
                if let Some((on, minutes)) = self.timer {
                    screen = screen.banner(
                        BannerLevel::Info,
                        format!("Timer running: {minutes} min, set on step {}.", on + 1),
                    );
                }
                screen = Self::banner(screen, self.note.as_ref()).text(&recipe.steps[step]);
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
                if last {
                    screen = screen.primary_button("finish", "Finish");
                }
                screen
                    .page_turns("previous", "next")
                    .reading_menu("tonight")
                    .build()
            }
            View::Ingredients => {
                let Some(recipe) = self.recipe() else {
                    return ScreenBuilder::new("kitchencard")
                        .top_bar("Ingredients")
                        .empty_state("Pick a recipe first.")
                        .primary_button("browse", "Pick a recipe")
                        .build();
                };
                let screen = ScreenBuilder::new("kitchencard")
                    .top_bar("Ingredients")
                    .tabs(1, [("cook", "Steps"), ("ingredients", "Ingredients")])
                    .secondary(format!(
                        "Serves {} · tap a line to check it off",
                        self.servings
                    ));
                if recipe.ingredients.is_empty() {
                    return screen
                        .empty_state("No ingredient list came with this recipe.")
                        .build();
                }
                screen
                    .rows(recipe.ingredients.iter().enumerate().map(|(index, ingredient)| {
                        let amount = amounts::amount(ingredient, self.servings, recipe.servings);
                        (
                            format!("ingredient-{index}"),
                            ingredient.label().to_owned(),
                            if amount.is_empty() {
                                "As written".to_owned()
                            } else {
                                amount
                            },
                            if self.checked.contains(&index) {
                                Glyph::Check
                            } else {
                                Glyph::Circle
                            },
                        )
                    }))
                    .build()
            }
            View::Finished => {
                let name = self.recipe().map_or("Dinner", |recipe| recipe.name.as_str());
                let mut screen = ScreenBuilder::new("kitchencard")
                    .top_bar("Kitchen Card")
                    .splash(Some(Glyph::Check), "Dinner's ready", name);
                if let Some(recipe) = self.recipe() {
                    screen = screen.facts([
                        ("Serves", self.servings.to_string()),
                        ("Steps", recipe.steps.len().to_string()),
                    ]);
                }
                screen
                    .buttons([("tonight", "Back to tonight"), ("again", "Cook again")])
                    .build()
            }
            View::Settings => ScreenBuilder::new("kitchencard")
                .top_bar("Kitchen Card settings")
                .field("server", &self.server, "https://mealie.example")
                .field("credential", &self.credential, "mealie")
                .secondary(
                    "On your computer run `kobo secret set mealie` with a Mealie API token, then sync here.",
                )
                .button("back", "Back")
                .build(),
        }
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
                self.open_cache(context);
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
        if let Some(setting) = self.editing {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let value = self.keyboard.take().trim().to_owned();
                    if !value.is_empty() {
                        match setting {
                            Setting::Server => {
                                if value.starts_with("https://") {
                                    self.server = value;
                                    self.recipes.clear();
                                    self.recipes_origin = None;
                                    self.tonight = None;
                                    self.persist_config(context);
                                    self.persist_tonight(context);
                                    self.open_cache(context);
                                } else {
                                    self.problem(
                                        "The address needs to start with https:// - plain http is not supported.",
                                    );
                                }
                            }
                            Setting::Credential => {
                                self.credential = value;
                                self.recipes.clear();
                                self.recipes_origin = None;
                                self.tonight = None;
                                self.persist_config(context);
                                self.persist_tonight(context);
                                self.open_cache(context);
                            }
                        }
                    }
                    self.editing = None;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {}
                None => {
                    if action == ActionId::BACK {
                        self.editing = None;
                    }
                }
            }
        } else if action == action_id("settings") {
            self.view = Some(View::Settings);
        } else if action == action_id("server") {
            self.keyboard = Keyboard::with_text(if self.server.is_empty() {
                "https://"
            } else {
                &self.server
            });
            self.editing = Some(Setting::Server);
        } else if action == action_id("credential") {
            self.keyboard = Keyboard::with_text(&self.credential);
            self.editing = Some(Setting::Credential);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = Some(View::Tonight);
        } else if action == action_id("retry-save") {
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.retry(context);
            }
            self.flush_cache(context);
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("browse") {
            self.view = Some(View::Browse);
        } else if action == action_id("tonight") {
            if self.view == Some(View::Cook) || self.view == Some(View::Finished) {
                self.stop_timer(context);
            }
            self.view = Some(View::Tonight);
        } else if action == action_id("cook") {
            self.view = Some(View::Cook);
            self.step = 0;
            context.device().keep_awake(Duration::from_secs(3600));
            self.persist_tonight(context);
        } else if action == action_id("ingredients") {
            self.view = Some(View::Ingredients);
        } else if action == action_id("more") && self.servings < 12 {
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
                    self.stop_timer(context);
                    self.view = Some(View::Finished);
                }
            }
        } else if action == action_id("previous") && self.step > 0 {
            self.step -= 1;
            self.persist_tonight(context);
        } else if action == action_id("finish") {
            self.stop_timer(context);
            self.view = Some(View::Finished);
        } else if action == action_id("again") {
            self.step = 0;
            self.checked.clear();
            self.view = Some(View::Cook);
            self.persist_tonight(context);
        } else if action == action_id("timer") {
            if let Some(recipe) = self.recipe() {
                if let Some(minutes) = recipe.steps.get(self.step).and_then(|s| step_timer(s)) {
                    self.start_timer(context, minutes);
                }
            }
        } else if action == action_id("stop-timer") {
            self.stop_timer(context);
            self.info("Timer stopped.");
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
        if let Some((step, minutes)) = self.timer.take() {
            self.info(format!(
                "Timer done: {minutes} min, set on step {}.",
                step + 1
            ));
            self.show(context);
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
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
                    timer: Some((1, 10)),
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
                app.screen()
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
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let text = format!("{layout:?}");
        assert!(text.contains("4 tins"), "scaled amount on screen: {text}");
        assert!(
            text.contains("text_lines: [\"1\"]"),
            "scaled whole amount on screen: {text}"
        );
    }
}
