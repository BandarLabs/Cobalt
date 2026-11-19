//! Readable amounts for scaled ingredients.

use crate::mealie::Ingredient;

/// How one ingredient's amount reads at `servings`, given how many the
/// recipe was written for. At the recipe's own servings Mealie's rendered
/// line is kept verbatim; scaling renders a whole number, a common kitchen
/// fraction, or one decimal, followed by the unit and any note.
#[must_use]
pub fn amount(ingredient: &Ingredient, servings: u32, original: u32) -> String {
    let base = match ingredient.quantity {
        Some(quantity) if original > 0 => {
            if servings == original && !ingredient.display.is_empty() {
                ingredient.display.clone()
            } else {
                let scaled = quantity * f64::from(servings) / f64::from(original);
                let mut rendered = fraction(scaled);
                if !ingredient.unit.is_empty() {
                    rendered.push(' ');
                    rendered.push_str(&ingredient.unit);
                }
                rendered
            }
        }
        _ if !ingredient.display.is_empty() => ingredient.display.clone(),
        _ => String::new(),
    };
    if ingredient.note.is_empty() || base == ingredient.note {
        base
    } else if base.is_empty() {
        ingredient.note.clone()
    } else {
        format!("{base} · {}", ingredient.note)
    }
}

/// A quantity as a whole number, a common fraction (¼ ⅓ ½ ⅔ ¾, mixed when
/// needed), or one decimal. Fractions a cook does not say out loud become a
/// decimal rather than a sevenths.
#[allow(clippy::cast_possible_truncation)] // Guarded finite and small: a recipe amount never nears 2^64.
#[must_use]
pub fn fraction(value: f64) -> String {
    const FRACTIONS: [(f64, &str); 5] = [
        (0.25, "¼"),
        (1.0 / 3.0, "⅓"),
        (0.5, "½"),
        (2.0 / 3.0, "⅔"),
        (0.75, "¾"),
    ];
    if !value.is_finite() || value < 0.0 {
        return "0".to_owned();
    }
    let whole = value.floor();
    let remainder = value - whole;
    for (mark, glyph) in FRACTIONS {
        if (remainder - mark).abs() < 0.02 {
            return if whole < 1.0 {
                glyph.to_owned()
            } else {
                format!("{}{glyph}", u64::try_from(whole as i64).unwrap_or(0))
            };
        }
    }
    if !(0.02..=0.98).contains(&remainder) {
        return format!("{}", u64::try_from(value.round() as i64).unwrap_or(0));
    }
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract().abs() < f64::EPSILON {
        format!("{}", u64::try_from(rounded as i64).unwrap_or(0))
    } else {
        format!("{rounded:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mealie::Ingredient;

    fn line(quantity: Option<f64>, unit: &str, display: &str, note: &str) -> Ingredient {
        Ingredient {
            quantity,
            unit: unit.to_owned(),
            food: "flour".to_owned(),
            note: note.to_owned(),
            display: display.to_owned(),
        }
    }

    #[test]
    fn fractions_read_the_way_a_cook_says_them() {
        assert_eq!(fraction(2.0), "2");
        assert_eq!(fraction(0.5), "½");
        assert_eq!(fraction(1.5), "1½");
        assert_eq!(fraction(0.25), "¼");
        assert_eq!(fraction(2.3333), "2⅓");
        assert_eq!(fraction(0.6666), "⅔");
        assert_eq!(fraction(3.75), "3¾");
        assert_eq!(fraction(0.1), "0.1");
        assert_eq!(fraction(2.97), "3");
        assert_eq!(fraction(f64::NAN), "0");
    }

    #[test]
    fn the_recipes_own_servings_keep_mealies_own_words() {
        let ingredient = line(Some(2.0), "tins", "2 tins chickpeas", "drained");
        assert_eq!(amount(&ingredient, 2, 2), "2 tins chickpeas · drained");
        assert_eq!(amount(&ingredient, 4, 2), "4 tins · drained");
        assert_eq!(amount(&ingredient, 1, 2), "1 tins · drained");
    }

    #[test]
    fn unscaled_lines_fall_back_to_the_display_or_note() {
        assert_eq!(
            amount(&line(None, "", "A knob of butter", ""), 4, 2),
            "A knob of butter"
        );
        assert_eq!(
            amount(&line(None, "", "", "Salt and pepper"), 4, 2),
            "Salt and pepper"
        );
        assert_eq!(amount(&line(Some(0.5), "", "½ lemon", ""), 4, 4), "½ lemon");
    }
}
