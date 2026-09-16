//! The Mealie boundary. The app names a credential; the runtime owns it.

use kobo_json::Value;

/// The most recipes one sync imports. Twelve covers a week of dinners twice
/// over, and bounding it keeps a shared family account from filling the
/// device with the whole archive.
pub const MAX_RECIPES: usize = 12;

/// A recipe as the list endpoint describes it: enough to name and to fetch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stub {
    pub slug: String,
    pub name: String,
}

/// A recipe with everything cooking needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub slug: String,
    pub name: String,
    pub category: String,
    pub servings: u32,
    pub description: String,
    pub ingredients: Vec<Ingredient>,
    pub steps: Vec<String>,
}

/// One ingredient line, keeping Mealie's own formatting beside the parts.
#[derive(Clone, Debug, PartialEq)]
pub struct Ingredient {
    pub quantity: Option<f64>,
    pub unit: String,
    pub food: String,
    pub note: String,
    /// Mealie's rendered line, kept so an unscaled card reads as written.
    pub display: String,
}

impl Ingredient {
    /// What the line is about: the food, else whatever text came with it.
    #[must_use]
    pub fn label(&self) -> &str {
        if !self.food.is_empty() {
            &self.food
        } else if !self.note.is_empty() {
            &self.note
        } else {
            &self.display
        }
    }
}

pub fn list_url(server: &str) -> String {
    format!(
        "{}/api/recipes?perPage={}&page=1",
        server.trim_end_matches('/'),
        MAX_RECIPES * 2
    )
}

pub fn detail_url(server: &str, slug: &str) -> String {
    format!("{}/api/recipes/{slug}", server.trim_end_matches('/'))
}

/// Parses the list response into stubs. An empty list is a valid answer and
/// stays distinct from a response that was not a recipe list at all.
pub fn parse_list(bytes: &[u8]) -> Option<Vec<Stub>> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let items = value.get("items").and_then(Value::as_array)?;
    let mut seen = std::collections::BTreeSet::new();
    items
        .iter()
        .map(|item| {
            let stub = Stub {
                slug: text(item, "slug", ""),
                name: text(item, "name", ""),
            };
            (!stub.slug.is_empty() && !stub.name.is_empty() && seen.insert(stub.slug.clone()))
                .then_some(stub)
        })
        .collect()
}

/// Parses one detail response. Ingredient and instruction lists may be empty
/// (a recipe under construction is still a recipe) but must be present, so a
/// login page or an error document cannot pass as a recipe.
pub fn parse_detail(bytes: &[u8]) -> Option<Recipe> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let ingredients = value.get("recipeIngredient").and_then(Value::as_array)?;
    let instructions = value.get("recipeInstructions").and_then(Value::as_array)?;
    let slug = text(&value, "slug", "");
    let name = text(&value, "name", "");
    if slug.is_empty() || name.is_empty() {
        return None;
    }
    let category = value
        .get("recipeCategory")
        .and_then(Value::as_array)
        .and_then(|categories| categories.first())
        .map(|category| text(category, "name", ""))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Mealie".to_owned());
    // Servings arrive as a JSON number; a fractional or absent one means the
    // recipe does not say, and two is the honest default.
    let servings = value
        .get("recipeServings")
        .and_then(Value::as_i64)
        .and_then(|servings| u32::try_from(servings).ok())
        .filter(|servings| *servings > 0)
        .unwrap_or(2);
    Some(Recipe {
        slug,
        name,
        category,
        servings,
        description: text(&value, "description", ""),
        ingredients: ingredients.iter().filter_map(parse_ingredient).collect(),
        steps: instructions.iter().filter_map(parse_step).collect(),
    })
}

fn parse_ingredient(value: &Value) -> Option<Ingredient> {
    let ingredient = Ingredient {
        quantity: value
            .get("quantity")
            .and_then(Value::as_f64)
            .filter(|quantity| quantity.is_finite() && *quantity > 0.0),
        unit: value
            .get("unit")
            .map(|unit| text(unit, "name", ""))
            .unwrap_or_default(),
        food: value
            .get("food")
            .map(|food| text(food, "name", ""))
            .unwrap_or_default(),
        note: text(value, "note", ""),
        display: text(value, "display", ""),
    };
    (!ingredient.label().is_empty()).then_some(ingredient)
}

fn parse_step(value: &Value) -> Option<String> {
    let step = text(value, "text", "");
    if step.is_empty() {
        return None;
    }
    let title = text(value, "title", "");
    Some(if title.is_empty() {
        step
    } else {
        format!("{title}: {step}")
    })
}

fn text(value: &Value, key: &str, fallback: &str) -> String {
    let found = value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if found.is_empty() {
        fallback.to_owned()
    } else {
        found.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_built_from_the_configured_server() {
        assert_eq!(
            list_url("https://mealie.example/"),
            "https://mealie.example/api/recipes?perPage=24&page=1"
        );
        assert_eq!(
            detail_url("https://mealie.example/", "lemon-chickpeas"),
            "https://mealie.example/api/recipes/lemon-chickpeas"
        );
    }

    #[test]
    fn a_valid_empty_list_is_not_an_error() {
        assert_eq!(
            parse_list(br#"{"page":1,"per_page":24,"total":0,"items":[]}"#),
            Some(vec![])
        );
        for invalid in [
            b"not JSON".as_slice(),
            b"{}",
            br#"{"items":[{}]}"#,
            br#"{"items":[{"slug":"a","name":"A"},{"slug":"a","name":"A again"}]}"#,
            &[255],
        ] {
            assert!(parse_list(invalid).is_none());
        }
    }

    #[test]
    fn the_list_parses_real_mealie_shape() {
        let stubs = parse_list(
            br#"{"page":1,"total":2,"items":[
                {"id":"1","slug":"lemon-chickpeas","name":"Lemon chickpeas"},
                {"id":"2","slug":"tomato-lentils","name":"Tomato lentils"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(stubs.len(), 2);
        assert_eq!(stubs[0].slug, "lemon-chickpeas");
    }

    #[test]
    fn a_detail_keeps_quantities_units_notes_and_steps() {
        let recipe = parse_detail(
            br#"{
                "slug":"lemon-chickpeas","name":"Lemon chickpeas",
                "description":"Bright and fast.",
                "recipeServings":2.0,
                "recipeCategory":[{"name":"Weeknight"}],
                "recipeIngredient":[
                    {"quantity":2.0,"unit":{"name":"tins"},"food":{"name":"chickpeas"},"note":"drained","display":"2 tins chickpeas"},
                    {"quantity":0.5,"unit":null,"food":{"name":"lemon"},"note":"","display":"1/2 lemon"},
                    {"quantity":null,"unit":null,"food":null,"note":"Salt and pepper","display":"Salt and pepper"}
                ],
                "recipeInstructions":[
                    {"title":"","text":"Warm the oil."},
                    {"title":"Finish","text":"Rest for 5 minutes."}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(recipe.servings, 2);
        assert_eq!(recipe.category, "Weeknight");
        assert_eq!(recipe.ingredients.len(), 3);
        assert_eq!(recipe.ingredients[0].quantity, Some(2.0));
        assert_eq!(recipe.ingredients[0].unit, "tins");
        assert_eq!(recipe.ingredients[2].label(), "Salt and pepper");
        assert_eq!(recipe.steps[1], "Finish: Rest for 5 minutes.");
    }

    #[test]
    fn a_detail_without_the_lists_is_not_a_recipe() {
        assert!(parse_detail(br#"{"slug":"a","name":"A"}"#).is_none());
        assert!(parse_detail(b"<html>login</html>").is_none());
        let sparse = parse_detail(
            br#"{"slug":"a","name":"A","recipeIngredient":[],"recipeInstructions":[]}"#,
        )
        .unwrap();
        assert!(sparse.ingredients.is_empty() && sparse.steps.is_empty());
        assert_eq!(sparse.category, "Mealie");
        assert_eq!(sparse.servings, 2);
    }
}
