//! Article images are read from verified local snapshots before the network.
//!
//! A document parsed from HTML names its pictures without carrying them, so
//! every reader of web articles needs the same three things: a verified local
//! copy consulted before any request, one request in flight at a time so a
//! long article cannot open a dozen sockets at once, and a bounded set of
//! records so an evening of reading does not grow without end. Feeds and the
//! Miniflux reader share this rather than each keeping a copy, because the part
//! that is easy to get wrong is not the fetching. It is deciding which failures
//! may still be retried and which memory may be released.
use crate::BookView;
use kobo_sdk::snapshot::{Snapshot as Cache, SnapshotEvent as Event, DEFAULT_LIMIT as LIMIT};
use kobo_sdk::{Context, StoreResult, Task, TaskId, TaskOutcome};
use std::collections::{BTreeMap, VecDeque};

const MAX_IMAGES: usize = 16;
const MAX_CACHED: usize = 64;
/// Idle shelf copies kept after their records leave memory.
const MAX_DISK_FILES: usize = 128;
/// Idle shelf copies, plus the copies still held in memory.
const MAX_DISK_BYTES: usize = 8 * 1024 * 1024;

struct ShelfCopy {
    key: String,
    slots: [String; 2],
    bytes: usize,
}

/// Saved image identities carry this prefix. It reads as one application's
/// name because Feeds published copies under it before this code was shared,
/// and the name is kept so an upgrade opens the pictures a reader already has.
/// Each application stores into its own directory, so two readers using the
/// same prefix never see each other's files.
const KEY_PREFIX: &str = "rss-image:";

#[derive(Default)]
pub struct Illustrations {
    caches: BTreeMap<String, Cache>,
    names: BTreeMap<String, Vec<String>>,
    queue: VecDeque<String>,
    fetching: Option<(TaskId, String)>,
    /// Published copies whose memory record has been dropped, oldest first.
    disk: VecDeque<ShelfCopy>,
    pub failed: bool,
}

impl Illustrations {
    pub fn can_retry(&self) -> bool {
        self.caches.values().any(Cache::retryable)
    }

    pub fn retry(&mut self, context: &mut Context) {
        if self.can_retry() {
            self.failed = false;
            for cache in self.caches.values_mut() {
                cache.retry(context);
            }
        }
    }

    pub fn close(&mut self, context: &mut Context) {
        if let Some((task, _)) = self.fetching.take() {
            context.cancel(task);
        }
        self.names.clear();
        self.queue.clear();
    }

    pub fn open(&mut self, context: &mut Context, reader: &mut BookView, origin: &str) {
        self.close(context);
        self.failed = false;
        let wanted = reader.missing_pictures();
        if wanted.len() > MAX_IMAGES {
            self.failed = true;
        }
        for name in wanted.into_iter().take(MAX_IMAGES) {
            let Some(url) = crate::picture_url(origin, &name) else {
                self.failed = true;
                continue;
            };
            self.names.entry(url).or_default().push(name);
        }
        let urls: Vec<_> = self.names.keys().cloned().collect();
        for url in urls {
            if let Some(cache) = self.caches.get(&url) {
                if cache.bytes.is_some() {
                    self.provide(context, reader, &url);
                } else if !cache.busy() {
                    self.queue.push_back(url.clone());
                }
            } else if self.make_room(context) {
                let cache = Cache::new(&format!("{KEY_PREFIX}{url}"));
                self.disk.retain(|copy| copy.key != cache.key);
                cache.start(context);
                self.caches.insert(url, cache);
            } else {
                self.failed = true;
            }
        }
        reader.settle_pictures(context);
        self.advance(context);
    }

    fn make_room(&mut self, context: &mut Context) -> bool {
        if self.caches.len() < MAX_CACHED {
            return true;
        }
        let released = self.caches.iter().find_map(|(url, cache)| {
            (!self.names.contains_key(url) && !cache.busy() && !cache.retryable())
                .then(|| url.clone())
        });
        if let Some(url) = released {
            // Keep active and failed writes resident so their
            // acknowledgements and retry candidates are not lost. An idle
            // published copy stays on the shelf until the disk cap, then
            // both slots and the pointer are removed.
            if let Some(cache) = self.caches.remove(&url) {
                if let Some(slots) = cache.published_slots() {
                    self.disk.push_back(ShelfCopy {
                        key: cache.key.clone(),
                        slots,
                        bytes: cache.bytes.as_ref().map_or(0, Vec::len),
                    });
                    self.trim_disk(context);
                }
            }
            true
        } else {
            false
        }
    }

    fn trim_disk(&mut self, context: &mut Context) {
        let resident = self
            .caches
            .values()
            .filter(|cache| cache.published_slots().is_some())
            .count();
        let mut bytes = self.disk.iter().map(|copy| copy.bytes).sum::<usize>()
            + self
                .caches
                .values()
                .filter_map(|cache| {
                    cache
                        .published_slots()
                        .map(|_| cache.bytes.as_ref().map_or(0, Vec::len))
                })
                .sum::<usize>();
        while !self.disk.is_empty()
            && (self.disk.len() + resident > MAX_DISK_FILES || bytes > MAX_DISK_BYTES)
        {
            let Some(old) = self.disk.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(old.bytes);
            for slot in old.slots {
                context.shelf().remove(slot);
            }
            context.store().forget(old.key);
        }
    }

    fn provide(&self, context: &mut Context, reader: &mut BookView, url: &str) {
        if let (Some(names), Some(bytes)) = (
            self.names.get(url),
            self.caches.get(url).and_then(|cache| cache.bytes.as_ref()),
        ) {
            for name in names {
                reader.provide_picture(name, bytes.clone());
            }
            reader.settle_pictures(context);
        }
    }

    fn advance(&mut self, context: &mut Context) {
        if self.fetching.is_some() {
            return;
        }
        if let Some(url) = self.queue.pop_front() {
            if let Some(task) = context.spawn(Task::Fetch {
                url: url.clone(),
                offset: 0,
                max_bytes: u32::try_from(LIMIT).expect("image limit fits protocol"),
                credential: None,
                headers: Vec::new(),
            }) {
                self.fetching = Some((task, url));
            } else {
                self.failed = true;
            }
        }
    }

    pub fn store(
        &mut self,
        context: &mut Context,
        reader: &mut BookView,
        key: &str,
        result: &StoreResult,
        is_file: bool,
    ) -> bool {
        let Some((url, cache)) = self.caches.iter_mut().find(|(_, cache)| {
            if is_file {
                cache.owns_file(key)
            } else {
                cache.key == key
            }
        }) else {
            return false;
        };
        let url = url.clone();
        let event = if is_file {
            cache.shelf(context, result)
        } else {
            cache.stored(context, result)
        };
        if event == Some(Event::Failed) {
            self.failed = true;
        }
        if self.names.contains_key(&url) {
            match event {
                Some(Event::Loaded) if self.caches[&url].bytes.is_none() => {
                    self.queue.push_back(url.clone());
                    self.advance(context);
                }
                Some(Event::Loaded | Event::Saved) => self.provide(context, reader, &url),
                Some(Event::Failed) => self.failed = true,
                None => {}
            }
        }
        true
    }

    /// Takes the answer to an image request, if this is one of ours.
    pub fn task(
        &mut self,
        context: &mut Context,
        reader: &mut BookView,
        task: TaskId,
        outcome: &TaskOutcome,
    ) -> bool {
        let Some((_, url)) = self.fetching.take_if(|(id, _)| *id == task) else {
            return false;
        };
        if let TaskOutcome::Completed(bytes) = outcome {
            if bytes.len() <= LIMIT && kobo_image::decode(bytes).is_ok() {
                if let Some(names) = self.names.get(&url) {
                    for name in names {
                        reader.provide_picture(name, bytes.clone());
                    }
                    reader.settle_pictures(context);
                }
                if !self
                    .caches
                    .get_mut(&url)
                    .is_some_and(|cache| cache.save(context, bytes.clone()))
                {
                    self.failed = true;
                }
            } else {
                self.failed = true;
            }
        } else {
            self.failed = true;
        }
        self.advance(context);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_never_discards_writes_or_images_in_the_current_article() {
        let mut context = Context::default();
        let mut images = Illustrations::default();
        for index in 0..MAX_CACHED {
            let url = format!("https://example.com/{index}.png");
            let mut cache = Cache::new(&url);
            cache.stored(
                &mut context,
                &StoreResult::Loaded {
                    key: cache.key.clone(),
                    value: None,
                },
            );
            if index % 2 == 0 {
                cache.save(&mut context, b"unsaved image".to_vec());
                if index % 4 == 0 {
                    cache.shelf(
                        &mut context,
                        &StoreResult::Denied(kobo_sdk::StoreError::NoRoom),
                    );
                }
            } else {
                images.names.insert(url.clone(), vec![url.clone()]);
            }
            images.caches.insert(url, cache);
        }
        assert!(!images.make_room(&mut context));
        assert_eq!(images.caches.len(), MAX_CACHED);
        images.names.clear();
        assert!(images.make_room(&mut context));
        assert_eq!(images.caches.len(), MAX_CACHED - 1);
        for index in (0..MAX_CACHED).step_by(2) {
            assert!(images
                .caches
                .contains_key(&format!("https://example.com/{index}.png")));
        }
    }

    #[test]
    fn idle_shelf_copies_leave_once_the_byte_cap_is_passed() {
        let mut context = Context::default();
        let mut images = Illustrations::default();
        images.disk.push_back(ShelfCopy {
            key: "rss-image-old".into(),
            slots: ["old.0".into(), "old.1".into()],
            bytes: MAX_DISK_BYTES + 1,
        });
        images.trim_disk(&mut context);
        assert!(images.disk.is_empty());
        let removed = context.commands().iter().any(|command| {
            matches!(
                command,
                kobo_sdk::Command::Store(kobo_sdk::StoreRequest::ShelfRemove { name })
                    if name == "old.0" || name == "old.1"
            )
        });
        assert!(removed, "the idle copy was left on the shelf");
    }
}
