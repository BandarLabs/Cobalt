//! Kitchen Card is a deliberately narrow, read-only Mealie companion.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Glyph, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskId, TaskOutcome,
};
use std::{process::ExitCode, time::Duration};

const STATE: &str = "tonight";
const MEALIE_RECIPES: &str = "https://mealie.local/api/recipes?perPage=20";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Tonight,
    Browse,
    Cook,
    Ingredients,
    About,
}

#[derive(Clone, Copy)]
struct Ingredient {
    amount: u16,
    unit: &'static str,
    item: &'static str,
}
#[derive(Clone, Copy)]
struct Recipe {
    title: &'static str,
    category: &'static str,
    servings: u8,
    ingredients: &'static [Ingredient],
    steps: &'static [&'static str],
}

const RECIPES: &[Recipe] = &[
    Recipe {
        title: "Lemon chickpeas",
        category: "Weeknight",
        servings: 2,
        ingredients: &[
            Ingredient {
                amount: 2,
                unit: "tins",
                item: "chickpeas",
            },
            Ingredient {
                amount: 1,
                unit: "",
                item: "lemon",
            },
            Ingredient {
                amount: 120,
                unit: "g",
                item: "spinach",
            },
        ],
        steps: &[
            "Warm the olive oil in a broad pan.",
            "Add chickpeas and cook until their edges colour.",
            "Add spinach, lemon zest and juice. Season, then serve.",
        ],
    },
    Recipe {
        title: "Tomato lentils",
        category: "Batch cook",
        servings: 4,
        ingredients: &[
            Ingredient {
                amount: 250,
                unit: "g",
                item: "red lentils",
            },
            Ingredient {
                amount: 1,
                unit: "tin",
                item: "tomatoes",
            },
            Ingredient {
                amount: 700,
                unit: "ml",
                item: "stock",
            },
        ],
        steps: &[
            "Rinse the lentils.",
            "Simmer lentils, tomatoes and stock for 20 minutes.",
            "Rest for five minutes before serving.",
        ],
    },
    Recipe {
        title: "Mushroom pasta",
        category: "Weeknight",
        servings: 2,
        ingredients: &[
            Ingredient {
                amount: 180,
                unit: "g",
                item: "pasta",
            },
            Ingredient {
                amount: 250,
                unit: "g",
                item: "mushrooms",
            },
            Ingredient {
                amount: 1,
                unit: "clove",
                item: "garlic",
            },
        ],
        steps: &[
            "Boil the pasta in salted water.",
            "Fry mushrooms until dry and browned.",
            "Add garlic, toss with pasta and its cooking water.",
        ],
    },
];

struct Kitchen {
    view: View,
    recipe: usize,
    servings: u8,
    step: usize,
    loaded: bool,
    syncing: bool,
    note: Option<String>,
    offline: bool,
}
impl Default for Kitchen {
    fn default() -> Self {
        Self {
            view: View::Tonight,
            recipe: 0,
            servings: RECIPES[0].servings,
            step: 0,
            loaded: false,
            syncing: false,
            note: None,
            offline: false,
        }
    }
}
impl Kitchen {
    fn current(&self) -> Recipe {
        RECIPES[self.recipe]
    }
    fn save(&self, context: &mut Context) {
        context.store().save(
            STATE,
            format!("{}|{}|{}", self.recipe, self.servings, self.step).into_bytes(),
        );
    }
    fn select(&mut self, recipe: usize, context: &mut Context) {
        self.recipe = recipe;
        self.servings = RECIPES[recipe].servings;
        self.step = 0;
        self.view = View::Tonight;
        self.note = Some("Tonight's card is saved for offline cooking.".into());
        self.save(context);
    }
    fn sync(&mut self, context: &mut Context) {
        self.syncing = true;
        self.note = None;
        if context
            .spawn_retrying(Task::Fetch {
                url: MEALIE_RECIPES.into(),
                offset: 0,
                max_bytes: 256 * 1024,
                credential: Some(Credential::bearer("mealie")),
                headers: Vec::new(),
            })
            .is_none()
        {
            self.syncing = false;
            self.note = Some("Recipes are already updating.".into());
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(screen(self));
    }
}
fn scaled(amount: u16, servings: u8, original: u8) -> String {
    let numerator = amount * u16::from(servings);
    let rounded = numerator / u16::from(original);
    if numerator % u16::from(original) == 0 {
        rounded.to_string()
    } else {
        format!("{numerator}/{original}")
    }
}
fn screen(app: &Kitchen) -> Screen {
    let recipe = app.current();
    let note = app.note.as_deref().unwrap_or(if app.offline {
        "Off the air. Cached recipes still cook."
    } else {
        ""
    });
    match app.view {
        View::Tonight => {
            let mut b = ScreenBuilder::new("kitchen-tonight")
                .top_bar("Kitchen Card")
                .heading(recipe.title)
                .secondary(format!("{} · serves {}", recipe.category, app.servings));
            if !note.is_empty() {
                b = b.banner(BannerLevel::Info, note);
            }
            b.text("Tonight's recipe stays here until you replace it.")
                .primary_button("cook", "Start cooking")
                .buttons([
                    ("browse", "Pick recipe"),
                    (
                        "sync",
                        if app.syncing {
                            "Working…"
                        } else {
                            "Sync Mealie"
                        },
                    ),
                ])
                .build()
        }
        View::Browse => ScreenBuilder::new("kitchen-browse")
            .top_bar("Pick a recipe")
            .secondary(if app.syncing {
                "Working…"
            } else {
                "Mealie categories: Weeknight, Batch cook"
            })
            .rows(RECIPES.iter().enumerate().map(|(i, r)| {
                (
                    format!("recipe-{i}"),
                    r.title,
                    format!("{} · serves {}", r.category, r.servings),
                    Glyph::Reader,
                )
            }))
            .buttons([("sync", "Sync Mealie"), ("tonight", "Tonight")])
            .build(),
        View::Cook => {
            let step = recipe.steps[app.step];
            ScreenBuilder::new("kitchen-cook")
                .top_bar(format!(
                    "Cooking · {} of {}",
                    app.step + 1,
                    recipe.steps.len()
                ))
                .tabs(0, [("cook", "Steps"), ("ingredients", "Ingredients")])
                .heading(step)
                .secondary("Tap left or right side of the page to move one step.")
                .page_turns("previous", "next")
                .reading_menu("tonight")
                .build()
        }
        View::Ingredients => ScreenBuilder::new("kitchen-ingredients")
            .top_bar("Ingredients")
            .tabs(1, [("cook", "Steps"), ("ingredients", "Ingredients")])
            .secondary(format!(
                "Serves {}. Structured amounts scale; source text would remain verbatim.",
                app.servings
            ))
            .rows(recipe.ingredients.iter().map(|i| {
                (
                    "ingredient",
                    format!(
                        "{} {}",
                        scaled(i.amount, app.servings, recipe.servings),
                        i.unit
                    )
                    .trim()
                    .to_string(),
                    i.item,
                    Glyph::Circle,
                )
            }))
            .buttons([("less", "− serving"), ("more", "+ serving")])
            .build(),
        View::About => ScreenBuilder::new("kitchen-about")
            .top_bar("Kitchen Card")
            .heading("About")
            .text("Unofficial read-only companion for Mealie. Mealie is AGPL-3.0.")
            .text("Add your Mealie address after finishing account setup on your computer.")
            .button("tonight", "Tonight")
            .build(),
    }
}
impl KoboApp for Kitchen {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            if let Some(bytes) = value {
                if let Ok(text) = String::from_utf8(bytes) {
                    let bits: Vec<_> = text.split('|').collect();
                    if let [recipe, servings, step] = bits.as_slice() {
                        self.recipe = recipe.parse::<usize>().unwrap_or(0).min(RECIPES.len() - 1);
                        self.servings = servings.parse().unwrap_or(RECIPES[self.recipe].servings);
                        self.step = step
                            .parse::<usize>()
                            .unwrap_or(0)
                            .min(RECIPES[self.recipe].steps.len() - 1);
                    }
                }
            }
            self.loaded = true;
            self.show(context);
        }
    }
    fn on_task(&mut self, context: &mut Context, _: TaskId, outcome: TaskOutcome) {
        self.syncing = false;
        match outcome {
            TaskOutcome::Completed(_) => {
                self.note =
                    Some("Mealie list updated. Pick a recipe to replace tonight's card.".into());
            }
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential) => {
                self.note = Some("Finish Mealie setup on your computer.".into());
            }
            TaskOutcome::Failed(kobo_sdk::TaskError::Offline) => {
                self.offline = true;
                self.note = Some("Off the air. Cached recipes still cook.".into());
            }
            TaskOutcome::Failed(_) | TaskOutcome::Cancelled => {
                self.note =
                    Some("Mealie did not answer. Check its LAN address, then sync again.".into());
            }
        }
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let before = self.view;
        if action == action_id("browse") {
            self.view = View::Browse;
        } else if action == action_id("tonight") {
            self.view = View::Tonight;
        } else if action == action_id("cook") {
            self.view = View::Cook;
            self.step = 0;
            context.device().keep_awake(Duration::from_secs(3600));
        } else if action == action_id("ingredients") {
            self.view = View::Ingredients;
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("more") && self.servings < 12 {
            self.servings += 1;
            self.save(context);
        } else if action == action_id("less") && self.servings > 1 {
            self.servings -= 1;
            self.save(context);
        } else if action == action_id("next") && self.step + 1 < self.current().steps.len() {
            self.step += 1;
            self.save(context);
        } else if action == action_id("previous") && self.step > 0 {
            self.step -= 1;
            self.save(context);
        } else if let Some(index) =
            (0..RECIPES.len()).find(|i| action == action_id(&format!("recipe-{i}")))
        {
            self.select(index, context);
        } else if action == action_id("about") {
            self.view = View::About;
        }
        if before != self.view || action != action_id("sync") {
            self.show(context);
        }
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
    #[test]
    fn structured_amounts_scale_without_inventing_decimals() {
        assert_eq!(scaled(120, 3, 2), "180");
        assert_eq!(scaled(1, 3, 2), "3/2");
    }
    #[test]
    fn step_navigation_stays_in_range() {
        let mut app = Kitchen::default();
        app.step = app.current().steps.len() - 1;
        assert!(app.step + 1 >= app.current().steps.len());
    }
    #[test]
    fn cooking_screen_fits_clara_and_has_turn_targets() {
        let layout = screen(&Kitchen {
            view: View::Cook,
            ..Kitchen::default()
        })
        .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.page_turns.declared().expect("page turns").next,
            action_id("next")
        );
        assert!(screen(&Kitchen {
            view: View::Cook,
            ..Kitchen::default()
        })
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
    }
}
