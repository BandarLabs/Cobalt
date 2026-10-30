//! Which cards are due on the reader today, and in what order.
//!
//! The bundle carries the queue the computer saw on the day it was made.
//! That queue is where the reader starts, not where it stops: every card the
//! reader grades gets its own next day from [`crate::scheduler`], a missed
//! card comes back at the end of the same session, and cards Anki had
//! scheduled for later days come due when their day arrives.

use crate::scheduler::{self, Prior, Schedule};
use crate::Grade;
use kobo_flashcards_format::{BundleManifest, Card, ReviewRecord};
use std::collections::BTreeMap;

/// The earliest day a collection's creation date is believed: 1 January
/// 1990. Anki stores review dates as days since the collection was made, and
/// a converter that left that date at zero would otherwise put every review
/// card in the collection due at once.
const EARLIEST_COLLECTION_DAY: i64 = 7_305;

/// One top-level deck, the unit the deck picker offers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    pub root_deck_id: i64,
    pub name: String,
    /// The reader day Anki counts its review dates from, when known.
    day_zero: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct Study {
    today: i64,
    offset_minutes: i16,
    groups: Vec<Group>,
    /// The group of each card, by its index in the manifest.
    card_groups: Vec<Option<usize>>,
    /// Position in the imported queue, for cards that were due on import day.
    queue_rank: BTreeMap<i64, usize>,
    schedules: BTreeMap<i64, Schedule>,
    /// Cards missed today, in the order they were missed.
    again_order: Vec<i64>,
    /// Every card due today, in the order they come up.
    due: Vec<i64>,
}

impl Study {
    pub fn new(manifest: &BundleManifest, today: i64, offset_minutes: i16) -> Self {
        let created = |source: usize| {
            manifest
                .sources
                .get(source)
                .and_then(|source| local_day(source.collection_created, offset_minutes))
                .filter(|day| (EARLIEST_COLLECTION_DAY..=today).contains(day))
        };
        let names: BTreeMap<i64, &str> = manifest
            .decks
            .iter()
            .map(|deck| (deck.id, deck.name.as_str()))
            .collect();
        let name_of = |deck_id: i64| {
            names
                .get(&deck_id)
                .map_or_else(|| "Imported deck".to_owned(), |name| (*name).to_owned())
        };
        let mut groups: Vec<Group> = Vec::new();
        let mut group_of_root: BTreeMap<i64, usize> = BTreeMap::new();
        let mut queued_group = BTreeMap::new();
        for queue in &manifest.review_queue.decks {
            let group = *group_of_root.entry(queue.root_deck_id).or_insert_with(|| {
                groups.push(Group {
                    root_deck_id: queue.root_deck_id,
                    name: name_of(queue.root_deck_id),
                    day_zero: created(queue.source_index),
                });
                groups.len() - 1
            });
            for card_id in &queue.card_ids {
                queued_group.insert(*card_id, group);
            }
        }
        let queue_rank = manifest
            .review_queue
            .card_ids
            .iter()
            .enumerate()
            .map(|(rank, card_id)| (*card_id, rank))
            .collect();
        let roots = root_decks(manifest);
        let home_root = |card: &Card| {
            let home = if card.original_deck_id == 0 {
                card.deck_id
            } else {
                card.original_deck_id
            };
            roots.get(&home).copied().unwrap_or(home)
        };
        // Decks with nothing due on import day follow the imported ones, in
        // name order. Their review dates can only be placed when the bundle
        // came from one collection, because a merged bundle does not say
        // which collection such a deck came from.
        let only_source = (manifest.sources.len() == 1).then_some(0);
        let mut later: Vec<i64> = manifest
            .cards
            .iter()
            .filter(|card| !queued_group.contains_key(&card.id))
            .map(home_root)
            .filter(|root| !group_of_root.contains_key(root))
            .collect();
        later.sort_by(|left, right| name_of(*left).cmp(&name_of(*right)).then(left.cmp(right)));
        later.dedup();
        for root in later {
            group_of_root.insert(root, groups.len());
            groups.push(Group {
                root_deck_id: root,
                name: name_of(root),
                day_zero: only_source.and_then(created),
            });
        }
        let card_groups = manifest
            .cards
            .iter()
            .map(|card| {
                queued_group
                    .get(&card.id)
                    .copied()
                    .or_else(|| group_of_root.get(&home_root(card)).copied())
            })
            .collect();
        let mut study = Self {
            today,
            offset_minutes,
            groups,
            card_groups,
            queue_rank,
            schedules: BTreeMap::new(),
            again_order: Vec::new(),
            due: Vec::new(),
        };
        study.rebuild(manifest);
        study
    }

    /// Replays the review log. A grade counts for a card while the card
    /// still carries the Anki schedule it was given against, so pushing an
    /// unchanged collection again keeps the reader's progress, and a card
    /// studied in Anki since then starts from Anki's word instead.
    pub fn replay(&mut self, manifest: &BundleManifest, records: &[ReviewRecord]) {
        for record in records {
            let Ok(index) = manifest
                .cards
                .binary_search_by_key(&record.card_id, |card| card.id)
            else {
                continue;
            };
            let card = &manifest.cards[index];
            if record.imported_due != card.due
                || record.imported_reps != i64::from(card.repetitions)
            {
                continue;
            }
            let Some(grade) = Grade::from_action(&record.grade) else {
                continue;
            };
            // A grade from before the reader dated them counts as today's,
            // and a clock that went backwards is not allowed to put a grade
            // in the future.
            let day = record.day.unwrap_or(self.today).min(self.today);
            self.apply(index, card, grade, day);
        }
        self.rebuild(manifest);
    }

    /// Grades a card today.
    pub fn grade(&mut self, manifest: &BundleManifest, card_id: i64, grade: Grade) {
        if let Ok(index) = manifest
            .cards
            .binary_search_by_key(&card_id, |card| card.id)
        {
            self.apply(index, &manifest.cards[index], grade, self.today);
            self.rebuild(manifest);
        }
    }

    /// Moves to a new day, if it is one. True when anything changed.
    pub fn set_today(&mut self, manifest: &BundleManifest, today: i64) -> bool {
        if today == self.today {
            return false;
        }
        self.today = today;
        self.rebuild(manifest);
        true
    }

    pub const fn today(&self) -> i64 {
        self.today
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Cards due today, in order, for one group or for all of them.
    pub fn due(&self, manifest: &BundleManifest, group: Option<usize>) -> Vec<i64> {
        self.due
            .iter()
            .copied()
            .filter(|card_id| group.is_none() || self.group_of(manifest, *card_id) == group)
            .collect()
    }

    /// Cards graded today that are not due again today.
    pub fn cleared_today(&self, manifest: &BundleManifest, group: Option<usize>) -> usize {
        self.schedules
            .iter()
            .filter(|(card_id, schedule)| {
                schedule.last_day == self.today
                    && schedule.due_day > self.today
                    && (group.is_none() || self.group_of(manifest, **card_id) == group)
            })
            .count()
    }

    /// The first day after today that a card in the selection comes due.
    pub fn next_due_day(&self, manifest: &BundleManifest, group: Option<usize>) -> Option<i64> {
        manifest
            .cards
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                group.is_none() || self.card_groups.get(*index).copied().flatten() == group
            })
            .filter_map(|(index, card)| match self.schedules.get(&card.id) {
                Some(schedule) => Some(schedule.due_day),
                None if self.queue_rank.contains_key(&card.id) => None,
                None => self.anki_due_day(index, card),
            })
            .filter(|day| *day > self.today)
            .min()
    }

    fn group_of(&self, manifest: &BundleManifest, card_id: i64) -> Option<usize> {
        let index = manifest
            .cards
            .binary_search_by_key(&card_id, |card| card.id)
            .ok()?;
        self.card_groups.get(index).copied().flatten()
    }

    fn apply(&mut self, index: usize, card: &Card, grade: Grade, day: i64) {
        let prior = Prior::of(card, self.anki_due_day(index, card));
        let next = scheduler::answer(self.schedules.get(&card.id), prior, day, grade);
        self.schedules.insert(card.id, next);
        self.again_order.retain(|known| *known != card.id);
        if grade == Grade::Again {
            self.again_order.push(card.id);
        }
    }

    /// The reader day Anki had this card due, for cards with a dated
    /// schedule: reviews and relearning count days from the collection's
    /// creation, learning steps carry a time of day.
    fn anki_due_day(&self, index: usize, card: &Card) -> Option<i64> {
        match card.queue {
            2 | 3 => {
                let group = self.card_groups.get(index).copied().flatten()?;
                let zero = self.groups.get(group)?.day_zero?;
                Some(zero.saturating_add(card.due))
            }
            1 => local_day(card.due, self.offset_minutes),
            _ => None,
        }
    }

    fn rebuild(&mut self, manifest: &BundleManifest) {
        let today = self.today;
        let mut fresh: Vec<((u8, i64, i64), i64)> = Vec::new();
        let mut missed = Vec::new();
        for (index, card) in manifest.cards.iter().enumerate() {
            if self.card_groups.get(index).copied().flatten().is_none() {
                continue;
            }
            match self.schedules.get(&card.id) {
                Some(schedule) if schedule.due_day <= today => {
                    if schedule.again && schedule.last_day == today {
                        missed.push(card.id);
                    } else {
                        fresh.push(((1, schedule.due_day, card.id), card.id));
                    }
                }
                Some(_) => {}
                None => {
                    if let Some(rank) = self.queue_rank.get(&card.id) {
                        let rank = i64::try_from(*rank).unwrap_or(i64::MAX);
                        fresh.push(((0, rank, 0), card.id));
                    } else if let Some(day) = self.anki_due_day(index, card) {
                        if day <= today {
                            fresh.push(((1, day, card.id), card.id));
                        }
                    }
                }
            }
        }
        fresh.sort_unstable();
        self.again_order.retain(|card_id| missed.contains(card_id));
        self.due = fresh.into_iter().map(|(_, card_id)| card_id).collect();
        self.due.extend(self.again_order.iter().copied());
    }
}

/// The top-level deck of every deck, by id. Anki nests decks by name, so a
/// deck's root is the deck named by the first part of its own name.
fn root_decks(manifest: &BundleManifest) -> BTreeMap<i64, i64> {
    let by_name: BTreeMap<&str, i64> = manifest
        .decks
        .iter()
        .rev()
        .map(|deck| (deck.name.as_str(), deck.id))
        .collect();
    manifest
        .decks
        .iter()
        .map(|deck| {
            let top = deck.name.split("::").next().unwrap_or(&deck.name);
            (deck.id, by_name.get(top).copied().unwrap_or(deck.id))
        })
        .collect()
}

/// Days since 1970 on the reader's calendar for a Unix time in seconds.
fn local_day(seconds: i64, offset_minutes: i16) -> Option<i64> {
    (seconds > 0).then(|| {
        seconds
            .saturating_add(i64::from(offset_minutes) * 60)
            .div_euclid(86_400)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_flashcards_format::{Deck, DeckQueue, ReviewQueue, Source, CONVERTER_REVISION};

    const DAY: i64 = 20_000;

    fn source(created_day: i64) -> Source {
        Source {
            package_kind: "apkg".to_owned(),
            collection_member: "collection.anki2".to_owned(),
            collection_schema: 18,
            normalized_schema: 18,
            collection_id: 1,
            collection_created: created_day * 86_400 + 4 * 3_600,
            collection_modified: 1,
            schema_modified: 1,
            dirty: 0,
            user_sequence: 0,
            last_sync: 0,
            note_count: 0,
            card_count: 0,
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

    fn card(id: i64, deck_id: i64, queue: i32, due: i64) -> Card {
        Card {
            id,
            note_id: id,
            deck_id,
            ordinal: 0,
            user_sequence: 0,
            queue,
            card_type: queue.max(0),
            due,
            interval: if queue == 2 { 10 } else { 0 },
            ease_factor: if queue == 2 { 2500 } else { 0 },
            repetitions: if queue == 2 { 3 } else { 0 },
            lapses: 0,
            remaining_steps: 0,
            original_due: 0,
            original_deck_id: 0,
            flags: 0,
            data: String::new(),
            modified: 1,
            template_name: String::new(),
            front: format!("Front {id}"),
            back: format!("Back {id}"),
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

    /// Three new cards due on import day in "Words", and two review cards in
    /// "Verbs" that Anki had due after import day.
    fn manifest(created_day: i64) -> BundleManifest {
        let mut manifest = BundleManifest::empty(source(created_day));
        for (id, name) in [(1, "Words"), (2, "Verbs"), (3, "Verbs::Irregular")] {
            manifest.decks.push(Deck {
                id,
                name: name.to_owned(),
                configuration_id: Some(1),
                original_json: "{}".to_owned(),
            });
        }
        manifest.cards = vec![
            card(1, 1, 0, 1),
            card(2, 1, 0, 2),
            card(3, 1, 0, 3),
            card(4, 2, 2, DAY - created_day + 2),
            card(5, 3, 2, DAY - created_day + 5),
        ];
        manifest.review_queue = ReviewQueue {
            card_ids: vec![1, 2, 3],
            new_count: 3,
            learning_count: 0,
            review_count: 0,
            decks: vec![DeckQueue {
                source_index: 0,
                root_deck_id: 1,
                card_ids: vec![1, 2, 3],
                new_count: 3,
                learning_count: 0,
                review_count: 0,
            }],
        };
        manifest
    }

    #[test]
    fn a_missed_card_comes_back_after_the_rest_of_the_queue() {
        let manifest = manifest(19_000);
        let mut study = Study::new(&manifest, DAY, 0);
        assert_eq!(study.due(&manifest, None), vec![1, 2, 3]);
        study.grade(&manifest, 1, Grade::Again);
        assert_eq!(study.due(&manifest, None), vec![2, 3, 1]);
        study.grade(&manifest, 2, Grade::Again);
        assert_eq!(study.due(&manifest, None), vec![3, 1, 2]);
        study.grade(&manifest, 3, Grade::Good);
        study.grade(&manifest, 1, Grade::Good);
        assert_eq!(study.due(&manifest, None), vec![2]);
        study.grade(&manifest, 2, Grade::Hard);
        assert!(study.due(&manifest, None).is_empty());
        assert_eq!(study.cleared_today(&manifest, None), 3);
    }

    #[test]
    fn graded_cards_come_due_again_on_their_own() {
        let manifest = manifest(19_000);
        let mut study = Study::new(&manifest, DAY, 0);
        for card_id in [1, 2, 3] {
            study.grade(&manifest, card_id, Grade::Good);
        }
        let next = study.next_due_day(&manifest, Some(0)).expect("a next day");
        assert!(next > DAY);
        assert!(study.set_today(&manifest, next));
        assert!(!study.due(&manifest, Some(0)).is_empty());
        assert_eq!(study.cleared_today(&manifest, None), 0);
    }

    #[test]
    fn anki_review_cards_come_due_on_their_own_day_in_their_own_deck() {
        let manifest = manifest(19_000);
        let mut study = Study::new(&manifest, DAY, 0);
        assert_eq!(study.groups().len(), 2);
        assert_eq!(study.groups()[1].name, "Verbs");
        assert!(study.due(&manifest, Some(1)).is_empty());
        assert_eq!(study.next_due_day(&manifest, Some(1)), Some(DAY + 2));
        study.set_today(&manifest, DAY + 5);
        assert_eq!(study.due(&manifest, Some(1)), vec![4, 5]);
    }

    #[test]
    fn a_collection_without_a_creation_date_offers_only_its_imported_queue() {
        let manifest = manifest(0);
        let mut study = Study::new(&manifest, DAY, 0);
        study.set_today(&manifest, DAY + 400);
        assert_eq!(study.due(&manifest, None), vec![1, 2, 3]);
        assert_eq!(study.next_due_day(&manifest, Some(1)), None);
    }

    #[test]
    fn a_logged_grade_survives_a_new_push_of_the_same_schedule_only() {
        let manifest = manifest(19_000);
        let record = |card_id: i64, reps: i64| ReviewRecord {
            bundle_sha256: "a".repeat(64),
            card_id,
            grade: "good".to_owned(),
            imported_due: card_id,
            imported_reps: reps,
            day: Some(DAY - 1),
        };
        let mut study = Study::new(&manifest, DAY, 0);
        study.replay(&manifest, &[record(1, 0), record(2, 4)]);
        // Card 1 was graded yesterday against the schedule it still has, so
        // it is not due; card 2 has been studied in Anki since, so Anki's
        // queue stands.
        assert_eq!(study.due(&manifest, None), vec![2, 3]);
    }

    #[test]
    fn a_grade_dated_in_the_future_counts_as_today() {
        let manifest = manifest(19_000);
        let mut study = Study::new(&manifest, DAY, 0);
        study.replay(
            &manifest,
            &[ReviewRecord {
                bundle_sha256: "a".repeat(64),
                card_id: 3,
                grade: "again".to_owned(),
                imported_due: 3,
                imported_reps: 0,
                day: Some(DAY + 30),
            }],
        );
        assert_eq!(study.due(&manifest, None), vec![1, 2, 3]);
    }
}
