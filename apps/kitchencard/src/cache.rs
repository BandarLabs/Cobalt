//! Versioned local recipe data, kept through restarts and offline days.

use crate::mealie::{Ingredient, Recipe};
use kobo_json::{ObjectBuilder, Value};

pub const LIMIT: usize = 4 * 1024 * 1024;

pub fn encode(recipes: &[Recipe]) -> Option<Vec<u8>> {
    let size = recipes.iter().try_fold(0usize, |size, recipe| {
        size.checked_add(recipe.name.len())?
            .checked_add(recipe.description.len())?
            .checked_add(
                recipe
                    .ingredients
                    .iter()
                    .map(|ingredient| ingredient.display.len() + ingredient.note.len())
                    .sum::<usize>(),
            )?
            .checked_add(recipe.steps.iter().map(String::len).sum::<usize>())
    })?;
    if size > LIMIT {
        return None;
    }
    let items: Vec<Value> = recipes
        .iter()
        .map(|recipe| {
            ObjectBuilder::new()
                .set("slug", recipe.slug.clone())
                .set("name", recipe.name.clone())
                .set("category", recipe.category.clone())
                .set("servings", recipe.servings.to_string())
                .set("description", recipe.description.clone())
                .set(
                    "ingredients",
                    recipe
                        .ingredients
                        .iter()
                        .map(|ingredient| {
                            ObjectBuilder::new()
                                .set(
                                    "quantity",
                                    ingredient
                                        .quantity
                                        .map(|quantity| quantity.to_string())
                                        .unwrap_or_default(),
                                )
                                .set("unit", ingredient.unit.clone())
                                .set("food", ingredient.food.clone())
                                .set("note", ingredient.note.clone())
                                .set("display", ingredient.display.clone())
                                .build()
                        })
                        .collect::<Vec<Value>>(),
                )
                .set(
                    "steps",
                    recipe
                        .steps
                        .iter()
                        .map(|step| Value::from(step.as_str()))
                        .collect::<Vec<Value>>(),
                )
                .build()
        })
        .collect();
    let bytes = ObjectBuilder::new()
        .set("version", "1")
        .set("items", items)
        .build()
        .to_json()
        .into_bytes();
    (bytes.len() <= LIMIT).then_some(bytes)
}

pub fn decode(bytes: &[u8]) -> Option<Vec<Recipe>> {
    if bytes.len() > LIMIT {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if value.get("version")?.as_str()? != "1" {
        return None;
    }
    let mut seen = std::collections::BTreeSet::new();
    value
        .get("items")?
        .as_array()?
        .iter()
        .map(|value| {
            let text = |name| value.get(name).and_then(Value::as_str);
            let slug = text("slug")?.to_owned();
            let name = text("name")?.to_owned();
            if slug.is_empty() || name.is_empty() || !seen.insert(slug.clone()) {
                return None;
            }
            Some(Recipe {
                slug,
                name,
                category: text("category").unwrap_or("Mealie").to_owned(),
                servings: text("servings")?.parse().ok()?,
                description: text("description").unwrap_or_default().to_owned(),
                ingredients: value
                    .get("ingredients")?
                    .as_array()?
                    .iter()
                    .map(|ingredient| {
                        let part = |name| {
                            ingredient
                                .get(name)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        };
                        Some(Ingredient {
                            quantity: {
                                let raw = part("quantity");
                                if raw.is_empty() {
                                    None
                                } else {
                                    raw.parse().ok()
                                }
                            },
                            unit: part("unit"),
                            food: part("food"),
                            note: part("note"),
                            display: part("display"),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?,
                steps: value
                    .get("steps")?
                    .as_array()?
                    .iter()
                    .map(|step| step.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> Recipe {
        Recipe {
            slug: "lemon-chickpeas".to_owned(),
            name: "Lemon chickpeas".to_owned(),
            category: "Weeknight".to_owned(),
            servings: 2,
            description: "Bright and fast.".to_owned(),
            ingredients: vec![
                Ingredient {
                    quantity: Some(2.0),
                    unit: "tins".to_owned(),
                    food: "chickpeas".to_owned(),
                    note: "drained".to_owned(),
                    display: "2 tins chickpeas".to_owned(),
                },
                Ingredient {
                    quantity: None,
                    unit: String::new(),
                    food: String::new(),
                    note: "Salt and pepper".to_owned(),
                    display: "Salt and pepper".to_owned(),
                },
            ],
            steps: vec!["Warm the oil.".to_owned(), "Rest for 5 minutes.".to_owned()],
        }
    }

    #[test]
    fn a_round_trip_keeps_every_field() {
        let recipes = vec![recipe()];
        assert_eq!(decode(&encode(&recipes).unwrap()), Some(recipes));
    }

    #[test]
    fn foreign_or_versioned_wrong_data_is_not_opened() {
        assert!(decode(b"not JSON").is_none());
        assert!(decode(br#"{"version":"2","items":[]}"#).is_none());
        assert!(decode(br#"{"version":"1","items":[{"slug":"a"}]}"#).is_none());
    }
}
