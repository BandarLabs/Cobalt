//! The original starter deck offered the first time the app opens.
//!
//! Built at runtime with the same verified writer the host converter uses, so
//! the sample passes every admission check a staged collection passes. The
//! cards are original general knowledge, with one Japanese greeting and one
//! longer answer so the first session already shows the typeface subset and
//! page turning.

use kobo_flashcards_format::{
    encode, BundleManifest, Card, Deck, DeckConfiguration, DeckQueue, Note, NoteType, ReviewQueue,
    Source, CONVERTER_REVISION,
};
use std::collections::BTreeMap;

const CARDS: [(&str, &str); 6] = [
    (
        "What does a compass needle point toward?",
        "Magnetic north.",
    ),
    (
        "What is the name for water turning into vapour?",
        "Evaporation.",
    ),
    ("How many sides does a hexagon have?", "Six."),
    (
        "こんにちは",
        "\"Hello\" - a daytime greeting, romanized \"konnichiwa\".",
    ),
    ("Which planet is closest to the Sun?", "Mercury."),
    (
        "Name the four largest moons of Jupiter.",
        "Io, Europa, Ganymede and Callisto, the Galilean moons. Ganymede is larger than the \
         planet Mercury, Io is the most volcanically active body known, Europa hides a deep \
         ocean under its ice, and ancient Callisto is among the most heavily cratered worlds \
         in the solar system.",
    ),
];

fn source() -> Source {
    Source {
        package_kind: "sample".to_owned(),
        collection_member: "sample".to_owned(),
        collection_schema: 0,
        normalized_schema: 0,
        collection_id: 1,
        collection_created: 0,
        collection_modified: 1,
        schema_modified: 1,
        dirty: 0,
        user_sequence: 0,
        last_sync: 0,
        note_count: CARDS.len(),
        card_count: CARDS.len(),
        converter_revision: CONVERTER_REVISION.to_owned(),
        original_config_json: "{}".to_owned(),
        original_models_json: "{}".to_owned(),
        original_decks_json: "{}".to_owned(),
        original_deck_configurations_json: "{}".to_owned(),
        original_tags_json: "{}".to_owned(),
        normalized_config: Vec::new(),
        normalized_tags: Vec::new(),
    }
}

fn card(id: i64, front: &str, back: &str) -> Card {
    Card {
        id,
        note_id: id,
        deck_id: 1,
        ordinal: 0,
        user_sequence: 0,
        queue: 0,
        card_type: 0,
        due: id,
        interval: 0,
        ease_factor: 0,
        repetitions: 0,
        lapses: 0,
        remaining_steps: 0,
        original_due: 0,
        original_deck_id: 0,
        flags: 0,
        data: String::new(),
        modified: 1,
        template_name: "Recognition".to_owned(),
        front: front.to_owned(),
        back: back.to_owned(),
        front_spans: Vec::new(),
        back_spans: Vec::new(),
        tags: Vec::new(),
        question_media_names: Vec::new(),
        answer_media_names: Vec::new(),
        media_names: Vec::new(),
        attachments: Vec::new(),
        diagnostics: Vec::new(),
    }
}

/// The verified sample bundle, ready to write to the shelf under the one name
/// the application reads.
#[must_use]
pub fn collection() -> Vec<u8> {
    let mut manifest = BundleManifest::empty(source());
    manifest.notetypes.push(NoteType {
        id: 1,
        name: "Basic".to_owned(),
        original_json: "{}".to_owned(),
    });
    manifest.deck_configurations.push(DeckConfiguration {
        id: 1,
        name: "Default".to_owned(),
        original_json: "{}".to_owned(),
    });
    manifest.decks.push(Deck {
        id: 1,
        name: "Getting started".to_owned(),
        configuration_id: Some(1),
        original_json: "{}".to_owned(),
    });
    for (index, (front, back)) in CARDS.iter().enumerate() {
        let id = i64::try_from(index + 1).unwrap_or(1);
        manifest.notes.push(Note {
            id,
            guid: format!("sample-{id}"),
            notetype_id: 1,
            modified: 1,
            user_sequence: 0,
            tags: Vec::new(),
            fields: vec![(*front).to_owned(), (*back).to_owned()],
            sort_field: (*front).to_owned(),
            checksum: id,
            flags: 0,
            data: String::new(),
        });
        manifest.cards.push(card(id, front, back));
    }
    let card_ids = manifest
        .cards
        .iter()
        .map(|card| card.id)
        .collect::<Vec<_>>();
    manifest.review_queue = ReviewQueue {
        card_ids: card_ids.clone(),
        new_count: card_ids.len(),
        learning_count: 0,
        review_count: 0,
        decks: vec![DeckQueue {
            source_index: 0,
            root_deck_id: 1,
            card_ids,
            new_count: CARDS.len(),
            learning_count: 0,
            review_count: 0,
        }],
    };
    encode(manifest, BTreeMap::new()).expect("the sample bundle is valid by construction")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_sample_is_a_verified_reviewable_bundle() {
        let bytes = super::collection();
        let bundle = kobo_flashcards_format::decode(&bytes).expect("sample decodes");
        let manifest = bundle.manifest();
        assert_eq!(manifest.cards.len(), super::CARDS.len());
        assert_eq!(manifest.review_queue.card_ids.len(), super::CARDS.len());
        kobo_flashcards_format::verify_card_images(&bundle, &manifest.review_queue.card_ids)
            .expect("sample media rules hold");
    }
}
