//! Original, offline samples. These are fixtures, never provider records.
//! Apps keep them in a separate sample session and discard that session when
//! the owner connects an account. Choosing a sample performs no network or store work.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Collection {
    Notes,
    Reading,
    Cards,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Item {
    /// Stable identity in a sample-only namespace.
    pub id: &'static str,
    pub title: &'static str,
    /// Plain text; for cards, the answer to the title's question.
    pub body: &'static str,
}
impl Collection {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Notes => "Sample notes",
            Self::Reading => "Sample reading",
            Self::Cards => "Sample cards",
        }
    }
    #[must_use]
    pub const fn items(self) -> &'static [Item] {
        match self {
            Self::Notes => NOTES,
            Self::Reading => READING,
            Self::Cards => CARDS,
        }
    }
    /// Tagged export for fixture tools. It cannot be mistaken for an account's
    /// response, and contains no credential, provider URL or remote identifier.
    #[must_use]
    pub fn to_json(self) -> String {
        use kobo_json::{ObjectBuilder as Object, Value};
        Object::new()
            .set("schema", "cobalt.sample-collection")
            .set("version", 1_u32)
            .set("sample", true)
            .set("title", self.name())
            .set(
                "items",
                Value::Array(
                    self.items()
                        .iter()
                        .map(|item| {
                            Object::new()
                                .set("id", item.id)
                                .set("title", item.title)
                                .set("body", item.body)
                                .build()
                        })
                        .collect(),
                ),
            )
            .build()
            .to_json()
    }
}

const NOTES: &[Item] = &[
    Item { id: "sample.note-market", title: "Saturday market", body: "Bring the cloth bags. Look for tomatoes, a loaf of bread and a small bunch of flowers." },
    Item { id: "sample.note-window", title: "Window shelf", body: "Measure the recess before buying a shelf. Leave enough room to open the window and turn the plant pots." },
    Item { id: "sample.note-walk", title: "A walk after lunch", body: "Take the path beside the canal. Stop at the footbridge, then return through the park." },
    Item { id: "sample.note-books", title: "Books to lend", body: "Put the two finished books by the door. Add a note with the page that made me laugh." },
    Item { id: "sample.note-mending", title: "Mending basket", body: "Sew the loose button on the blue shirt. Check the pocket lining before putting the sewing kit away." },
    Item { id: "sample.note-picnic", title: "Picnic list", body: "Cups, a blanket, water, sandwiches and a bag for the rubbish. Check the meeting place before leaving." },
    Item { id: "sample.note-letter", title: "Write a letter", body: "Tell Mira about the new flat, the stubborn cupboard door and the view of the railway from the kitchen." },
    Item { id: "sample.note-desk", title: "Clear the desk", body: "File the receipts. Return the borrowed ruler. Keep one notebook and a pen within reach." },
    Item { id: "sample.note-trip", title: "Before the train", body: "Charge the reader. Pack the ticket, a snack and the address on paper. Leave time to find the platform." },
    Item { id: "sample.note-garden", title: "Garden notebook", body: "Sketch where the afternoon shade falls. Mark the pots that will need moving when the bench arrives." },
    Item { id: "sample.note-evening", title: "A quiet evening", body: "Choose a short chapter, put the phone away and read until the kettle boils." },
    Item { id: "sample.note-recipe", title: "Recipe to ask for", body: "Ask Arun how he makes the lemon dressing. Write down the quantities before trying it again." },
];
const READING: &[Item] = &[
    Item { id: "sample.read-key", title: "The spare key", body: "Every morning, Leena left the spare key under an empty flowerpot. One morning the pot held a seedling. She carried both inside and rang her neighbour.

He had mistaken the empty pot for an invitation to plant something. They found a new home for the key and left the seedling in the sun." },
    Item { id: "sample.read-map", title: "A map in pencil", body: "The map showed a bakery where there was now a bicycle shop. It showed a field where the station stood. Sam folded it along its old creases and went looking for the river.

That, at least, was still where the pencil had put it." },
    Item { id: "sample.read-seat", title: "The window seat", body: "The train was almost empty. Ada chose the window seat and opened a book, but the hills outside kept borrowing her attention.

By the next station she had read three pages and watched an entire shower cross the valley." },
    Item { id: "sample.read-cup", title: "The chipped cup", body: "The cup had a chip too small to notice until someone pointed it out. After that, every guest turned it carefully before drinking.

Its owner marked the opposite side with a tiny blue dot. It became the easiest cup in the cupboard to find by touch." },
    Item { id: "sample.read-bench", title: "Paint on the bench", body: "Someone had painted the park bench green, missing a narrow strip beneath the seat. Two children crouched beside it to inspect the old blue paint.

They decided the bench had once belonged to the sea. Their grandfather said this was as good an explanation as any." },
    Item { id: "sample.read-lamp", title: "The lamp by the door", body: "The hall lamp went out just as the visitors arrived. They stood in the doorway laughing while Ivo found a torch.

Dinner was ready. The coats could wait. He put the torch on the table and brought out the soup." },
    Item { id: "sample.read-note", title: "A note in the margin", body: "On page forty, someone had written, 'Read this aloud.' Noor looked around the empty kitchen and tried it.

The sentence sounded different in the room. She read the next one too, and forgot to turn on the radio." },
    Item { id: "sample.read-rain", title: "Waiting for rain", body: "The washing hung under a sky that could not decide what to do. Every few minutes, Ben stepped outside to look up.

At last he brought it in. The sun came out immediately, as if it had been waiting for him to finish." },
];
const CARDS: &[Item] = &[
    Item {
        id: "sample.card-half",
        title: "What is half of 18?",
        body: "9",
    },
    Item {
        id: "sample.card-area",
        title: "Area of a rectangle?",
        body: "Width multiplied by height.",
    },
    Item {
        id: "sample.card-dozen",
        title: "How many are in a dozen?",
        body: "12",
    },
    Item {
        id: "sample.card-quarter",
        title: "One quarter as a percentage?",
        body: "25%",
    },
    Item {
        id: "sample.card-hour",
        title: "Minutes in two hours?",
        body: "120 minutes.",
    },
    Item {
        id: "sample.card-square",
        title: "What is 7 squared?",
        body: "49",
    },
    Item {
        id: "sample.card-metre",
        title: "Centimetres in a metre?",
        body: "100 centimetres.",
    },
    Item {
        id: "sample.card-triangle",
        title: "Sides of a triangle?",
        body: "3",
    },
    Item {
        id: "sample.card-even",
        title: "What makes an integer even?",
        body: "It is divisible by 2 with no remainder.",
    },
    Item {
        id: "sample.card-fraction",
        title: "One half plus one quarter?",
        body: "Three quarters.",
    },
    Item {
        id: "sample.card-perimeter",
        title: "Perimeter of a square?",
        body: "Four times the side length.",
    },
    Item {
        id: "sample.card-product",
        title: "What is 8 times 6?",
        body: "48",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collections_have_stable_distinct_ids_and_explicit_sample_provenance() {
        let mut ids = std::collections::BTreeSet::new();
        for collection in [Collection::Notes, Collection::Reading, Collection::Cards] {
            assert!(collection.items().len() >= 8);
            for item in collection.items() {
                assert!(item.id.starts_with("sample.") && ids.insert(item.id));
                assert!(!item.title.is_empty() && !item.body.is_empty());
            }
            let encoded = collection.to_json();
            assert!(encoded.len() < 16 * 1024);
            let decoded = kobo_json::parse(&encoded).unwrap();
            assert_eq!(
                decoded.get("sample").and_then(kobo_json::Value::as_bool),
                Some(true)
            );
            assert_eq!(
                decoded
                    .get("items")
                    .and_then(kobo_json::Value::as_array)
                    .unwrap()
                    .len(),
                collection.items().len()
            );
        }
    }
}
