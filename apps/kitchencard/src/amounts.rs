//! Readable amounts for scaled ingredients.

use crate::mealie::Ingredient;

/// How one ingredient's amount reads at `servings`, given how many the
/// recipe was written for. At the recipe's own servings Mealie's rendered
/// line is kept verbatim; scaling renders a whole number, a common kitchen
/// fraction, or one decimal, followed by the unit and any note.
///
/// A line Mealie has not parsed yet has no quantity, only its text, so the
/// amount it starts with is scaled in place: "1 1/2 kg whole chicken" for
/// four reads "2¼ kg whole chicken" for six. Without that the servings
/// changed and the plain lines, Mealie's default, stayed as written.
#[must_use]
pub fn amount(ingredient: &Ingredient, servings: u32, original: u32) -> String {
    if ingredient.quantity.is_none() && !ingredient.display.is_empty() {
        if let Some(scaled) = scale_text(&ingredient.display, servings, original) {
            // The note of a plain line is usually the line itself, and
            // scaled it is no longer equal to it, so it is not added twice.
            return if ingredient.note.is_empty() || ingredient.note == ingredient.display {
                scaled
            } else {
                format!("{scaled} · {}", ingredient.note)
            };
        }
    }
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

/// What an ingredient row is called at `servings`: its food, else its text
/// with any leading amount scaled the way [`amount`] scales it, so a plain
/// line reads the same in the row as it would beside a parsed one.
#[must_use]
pub fn label(ingredient: &Ingredient, servings: u32, original: u32) -> String {
    let label = ingredient.label();
    if ingredient.quantity.is_none() && ingredient.food.is_empty() {
        if let Some(scaled) = scale_text(label, servings, original) {
            return scaled;
        }
    }
    label.to_owned()
}

/// A line of text with its leading amount scaled from `original` servings to
/// `servings`, or `None` when it is left as written: at the recipe's own
/// servings, or when it does not start with an amount.
fn scale_text(text: &str, servings: u32, original: u32) -> Option<String> {
    if servings == original || original == 0 {
        return None;
    }
    let (quantity, rest) = leading_amount(text)?;
    let scaled = quantity * f64::from(servings) / f64::from(original);
    Some(format!("{}{rest}", fraction(scaled)))
}

/// The amount a line of text starts with, and the text after it. Reads
/// whole numbers, decimals, simple and mixed fractions ("1/3", "1 1/2") and
/// the fraction characters ("½", "1½", "1 ½"). A range ("2-3 cloves",
/// "2 to 3 cloves") is not one amount, and scaling only its first number
/// would say something the recipe does not, so it is left alone.
fn leading_amount(text: &str) -> Option<(f64, &str)> {
    let Some((whole, after_whole)) = number(text) else {
        let (glyph, rest) = vulgar(text)?;
        return amount_ends(rest).then_some((glyph, rest));
    };
    if let Some(after_slash) = after_whole.strip_prefix('/') {
        // A simple fraction: "1/3".
        let (denominator, rest) = number(after_slash)?;
        if denominator == 0.0 || whole.fract() != 0.0 {
            return None;
        }
        let value = whole / denominator;
        return (value > 0.0 && amount_ends(rest)).then_some((value, rest));
    }
    let (mut value, mut rest) = (whole, after_whole);
    if whole.fract() == 0.0 {
        // A mixed number: "1½", "1 ½" or "1 1/2".
        let spaced = rest.strip_prefix(' ').unwrap_or(rest);
        if let Some((glyph, after)) = vulgar(rest).or_else(|| vulgar(spaced)) {
            value += glyph;
            rest = after;
        } else if let Some((numerator, after)) = number(spaced) {
            if let Some((denominator, after)) = after.strip_prefix('/').and_then(number) {
                if numerator.fract() == 0.0 && denominator > numerator && numerator > 0.0 {
                    value += numerator / denominator;
                    rest = after;
                }
            }
        }
    }
    (value > 0.0 && amount_ends(rest)).then_some((value, rest))
}

/// Whether the text after an amount lets it stand as one: the end, a word
/// or a unit run on ("500g"), but not another number or a range.
fn amount_ends(rest: &str) -> bool {
    let Some(first) = rest.chars().next() else {
        return true;
    };
    if !(first.is_whitespace() || first.is_alphabetic()) {
        return false;
    }
    let next = rest.trim_start();
    let word = next.split_whitespace().next().unwrap_or_default();
    !(next.starts_with(['-', '–', '—'])
        || word.eq_ignore_ascii_case("to")
        || word.eq_ignore_ascii_case("or"))
}

/// A run of digits with an optional decimal part, and the text after it.
fn number(text: &str) -> Option<(f64, &str)> {
    let digits = text
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or(text.len(), |(index, _)| index);
    if digits == 0 {
        return None;
    }
    let mut end = digits;
    if let Some(after) = text[digits..].strip_prefix('.') {
        let decimals = after
            .char_indices()
            .find(|(_, character)| !character.is_ascii_digit())
            .map_or(after.len(), |(index, _)| index);
        if decimals > 0 {
            end = digits + 1 + decimals;
        }
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// A fraction character at the start of the text, and the text after it.
fn vulgar(text: &str) -> Option<(f64, &str)> {
    const GLYPHS: [(char, f64); 15] = [
        ('¼', 0.25),
        ('½', 0.5),
        ('¾', 0.75),
        ('⅓', 1.0 / 3.0),
        ('⅔', 2.0 / 3.0),
        ('⅕', 0.2),
        ('⅖', 0.4),
        ('⅗', 0.6),
        ('⅘', 0.8),
        ('⅙', 1.0 / 6.0),
        ('⅚', 5.0 / 6.0),
        ('⅛', 0.125),
        ('⅜', 0.375),
        ('⅝', 0.625),
        ('⅞', 0.875),
    ];
    let first = text.chars().next()?;
    GLYPHS
        .iter()
        .find(|(glyph, _)| *glyph == first)
        .map(|(_, value)| (*value, &text[first.len_utf8()..]))
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

    /// Mealie leaves a line as plain text until its parser runs. Serving six
    /// from a recipe for four used to leave "1 1/2 kg" as written.
    #[test]
    fn a_plain_line_scales_the_amount_it_starts_with() {
        let plain = |text: &str| line(None, "", text, text);
        let chicken = plain("1 1/2 kg whole chicken, patted dry");
        let chicken = Ingredient {
            food: String::new(),
            ..chicken
        };
        assert_eq!(amount(&chicken, 6, 4), "2¼ kg whole chicken, patted dry");
        assert_eq!(label(&chicken, 6, 4), "2¼ kg whole chicken, patted dry");
        assert_eq!(label(&chicken, 4, 4), "1 1/2 kg whole chicken, patted dry");
        assert_eq!(amount(&plain("1/3 cup olive oil"), 6, 4), "½ cup olive oil");
        assert_eq!(amount(&plain("2 lemons, halved"), 6, 4), "3 lemons, halved");
        assert_eq!(amount(&plain("½ lemon"), 8, 4), "1 lemon");
        assert_eq!(amount(&plain("1½ tsp salt"), 4, 2), "3 tsp salt");
        assert_eq!(amount(&plain("1 ½ tsp salt"), 4, 2), "3 tsp salt");
        assert_eq!(amount(&plain("0.5 l stock"), 4, 2), "1 l stock");
        assert_eq!(amount(&plain("500g flour"), 3, 2), "750g flour");
        assert_eq!(amount(&plain("3 eggs"), 1, 2), "1½ eggs");
    }

    #[test]
    fn a_plain_line_without_one_amount_up_front_stays_as_written() {
        let plain = |text: &str| line(None, "", text, text);
        for text in [
            "Salt and pepper",
            "2-3 cloves garlic",
            "2 to 3 sprigs thyme",
            "1 or 2 chillies",
            "1/0 of nothing",
            "3/4/5 mystery",
            "0 eggs",
        ] {
            assert_eq!(amount(&plain(text), 6, 4), text, "{text}");
        }
        let parsed = line(None, "", "2 tins chickpeas", "drained");
        assert_eq!(amount(&parsed, 4, 2), "4 tins chickpeas · drained");
        assert_eq!(label(&parsed, 4, 2), "flour");
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
