//! Small, evictable covers and read-only summaries of acknowledged reading memory.
use super::{
    legacy_progress_key, progress_key, Context, Glyph, Kept, PictureHandle, StoreResult,
    TilePicture,
};
use kobo_sdk::RowLead;
use std::collections::{BTreeMap, BTreeSet};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 240;
const MAX_PICTURES: usize = 32;
const FIRST_HANDLE: u32 = 100;
const MAGIC: &[u8] = b"PCV1";

#[derive(Clone, Copy)]
enum Position {
    NotStarted,
    Saved(usize),
    Unavailable,
}

#[derive(Default)]
pub(super) struct Previews {
    pictures: BTreeMap<String, (PictureHandle, TilePicture)>,
    visible: BTreeSet<String>,
    attempted: BTreeSet<String>,
    cache_load: Option<(String, String)>,
    positions: BTreeMap<String, Position>,
    position_load: Option<(Kept, String, bool)>,
    pub staged: Option<(String, Vec<u8>)>,
}

fn cache_name(key: &str) -> String {
    // The prefix and 224-bit digest fit the SDK's 64-byte cache-key bound.
    let digest = kobo_net::sha256::hex_digest(format!("panels.cover.v1\0{key}").as_bytes());
    format!("t-{}", &digest[..56])
}

pub(super) fn encode(picture: &kobo_image::Picture) -> Option<Vec<u8>> {
    let picture = picture.fit(WIDTH, HEIGHT).ok()?;
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&u16::try_from(picture.width()).ok()?.to_le_bytes());
    bytes.extend_from_slice(&u16::try_from(picture.height()).ok()?.to_le_bytes());
    bytes.extend_from_slice(picture.grey());
    Some(bytes)
}

fn decode(bytes: &[u8]) -> Option<(u32, u32, &[u8])> {
    if bytes.get(..4)? != MAGIC {
        return None;
    }
    let width = u32::from(u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?));
    let height = u32::from(u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?));
    if !(1..=WIDTH).contains(&width)
        || !(1..=HEIGHT).contains(&height)
        || bytes.len() != 8 + usize::try_from(width * height).ok()?
    {
        return None;
    }
    Some((width, height, &bytes[8..]))
}

pub(super) fn from_comic(bytes: &[u8], comic: &kobo_comic::Comic) -> Option<Vec<u8>> {
    encode(&kobo_comic::page(bytes, comic, comic.metadata.cover.unwrap_or(0)).ok()?)
}

impl Previews {
    pub fn lead(&self, key: &str) -> RowLead {
        self.pictures.get(key).map_or(
            RowLead::Picture(
                TilePicture::new(PictureHandle(u32::MAX), WIDTH, HEIGHT),
                Glyph::Book,
            ),
            |(_, picture)| RowLead::Picture(*picture, Glyph::Book),
        )
    }
    pub fn summary(&self, comic: &Kept) -> String {
        let summary = match self.positions.get(&comic.key) {
            None => format!("Checking position · {} pages", comic.pages),
            Some(Position::NotStarted) => format!("Not started · {} pages", comic.pages),
            Some(Position::Saved(page)) => format!("Saved page {} of {}", page + 1, comic.pages),
            Some(Position::Unavailable) => format!("Position unavailable · {} pages", comic.pages),
        };
        if comic.rtl {
            format!("{summary} · right to left")
        } else {
            summary
        }
    }
    fn put(&mut self, context: &mut Context, key: &str, bytes: &[u8]) {
        let Some((width, height, grey)) = decode(bytes) else {
            return;
        };
        if let Some((handle, _)) = self.pictures.remove(key) {
            context.drop_picture(handle);
        }
        if self.pictures.len() >= MAX_PICTURES {
            if let Some(key) = self.pictures.keys().next().cloned() {
                if let Some((handle, _)) = self.pictures.remove(&key) {
                    context.drop_picture(handle);
                }
            }
        }
        let handle = (FIRST_HANDLE
            ..FIRST_HANDLE + u32::try_from(MAX_PICTURES).expect("small cache"))
            .map(PictureHandle)
            .find(|handle| self.pictures.values().all(|(used, _)| used != handle))
            .expect("free preview handle");
        if let Some(picture) = context.put_picture(handle, width, height, grey.to_vec()) {
            self.pictures.insert(key.into(), (handle, picture));
        }
    }
    pub fn publish(&mut self, context: &mut Context, key: &str) {
        if self.staged.as_ref().is_none_or(|(staged, _)| staged != key) {
            return;
        }
        let (_, bytes) = self.staged.take().expect("matching preview");
        self.put(context, key, &bytes);
        context.store().cache(cache_name(key), bytes);
    }
    pub fn prepare(&mut self, context: &mut Context, visible: &[Kept]) {
        self.visible = visible
            .iter()
            .take(MAX_PICTURES)
            .map(|comic| comic.key.clone())
            .collect();
        self.pictures.retain(|key, (handle, _)| {
            if self.visible.contains(key) {
                true
            } else {
                context.drop_picture(*handle);
                false
            }
        });
        self.attempted.retain(|key| self.visible.contains(key));
        if self.cache_load.is_none() {
            if let Some(comic) = visible.iter().take(MAX_PICTURES).find(|comic| {
                !self.pictures.contains_key(&comic.key) && !self.attempted.contains(&comic.key)
            }) {
                let name = cache_name(&comic.key);
                self.attempted.insert(comic.key.clone());
                self.cache_load = Some((comic.key.clone(), kobo_sdk::cache_key(&name)));
                context.store().load_cached(name);
            }
        }
        if self.position_load.is_none() {
            if let Some(comic) = visible
                .iter()
                .find(|comic| !self.positions.contains_key(&comic.key))
            {
                let key = progress_key(&comic.key);
                context.store().load(&key);
                self.position_load = Some((comic.clone(), key, false));
            }
        }
    }
    pub fn loaded(&mut self, context: &mut Context, key: &str, result: &StoreResult) -> bool {
        if self
            .cache_load
            .as_ref()
            .is_some_and(|(_, pending)| pending == key)
        {
            let (comic, _) = self.cache_load.take().expect("matching cache read");
            if self.visible.contains(&comic) {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    self.put(context, &comic, bytes);
                }
            }
            return true;
        }
        if self
            .position_load
            .as_ref()
            .is_none_or(|(_, pending, _)| pending != key)
        {
            return false;
        }
        let (comic, _, legacy) = self.position_load.take().expect("matching position read");
        if !legacy && matches!(result, StoreResult::Loaded { value: None, .. }) {
            if let Some(key) = legacy_progress_key(&comic.key) {
                context.store().load(&key);
                self.position_load = Some((comic, key, true));
                return true;
            }
        }
        let position = match result {
            StoreResult::Loaded { value, .. } => position(value.as_deref(), comic.pages),
            _ => Position::Unavailable,
        };
        self.positions.insert(comic.key, position);
        true
    }
    pub fn saved_position(&mut self, key: &str, bytes: &[u8], comics: &[Kept]) {
        if let Some(comic) = comics.iter().find(|comic| progress_key(&comic.key) == key) {
            self.positions
                .insert(comic.key.clone(), position(Some(bytes), comic.pages));
        }
    }
    pub fn claim_position_read(&mut self, key: &str) -> bool {
        if self
            .position_load
            .as_ref()
            .is_some_and(|(comic, _, _)| comic.key == key)
        {
            self.position_load = None;
            true
        } else {
            false
        }
    }
}
fn position(bytes: Option<&[u8]>, pages: usize) -> Position {
    match kobo_comic::reader::Memory::saved_page(bytes, pages) {
        Ok(Some(page)) => Position::Saved(page),
        Ok(None) => Position::NotStarted,
        Err(_) => Position::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, StoreRequest};
    fn comic(key: &str) -> Kept {
        Kept {
            key: key.into(),
            title: "Garden".into(),
            pages: 4,
            rtl: false,
        }
    }
    fn cover() -> Vec<u8> {
        let bytes = include_bytes!("../assets/a-small-garden.cbz");
        from_comic(bytes, &kobo_comic::inspect(bytes).unwrap()).unwrap()
    }
    #[test]
    fn corrupt_and_oversized_cached_covers_never_allocate_a_picture() {
        let valid = cover();
        assert!(decode(&valid).is_some());
        let mut oversized = valid.clone();
        oversized[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
        for bytes in [
            &b""[..],
            &b"PCV1"[..],
            &oversized,
            &valid[..valid.len() - 1],
        ] {
            assert!(decode(bytes).is_none());
        }
        assert!(kobo_sdk::is_valid_key(&kobo_sdk::cache_key(cache_name(
            &"x".repeat(64)
        ))));
    }
    #[test]
    fn shelf_loads_only_visible_cache_and_state_records_without_reading_archives() {
        let mut previews = Previews::default();
        let mut context = Context::default();
        let comics = [comic("first"), comic("second")];
        previews.prepare(&mut context, &comics);
        let commands = context.take_commands();
        assert_eq!(commands.len(), 2);
        assert!(commands
            .iter()
            .all(|command| matches!(command, Command::Store(StoreRequest::Load { .. }))));
        previews.prepare(&mut context, &comics);
        assert!(context.take_commands().is_empty());
        // Owner opens the first comic while its position load is outstanding.
        assert!(previews.claim_position_read("first"));
        assert!(!previews.claim_position_read("first"));
    }
    #[test]
    fn late_cache_reply_cannot_reintroduce_an_offscreen_picture_and_eviction_drops_handles() {
        let mut previews = Previews::default();
        let mut context = Context::default();
        previews.prepare(&mut context, &[comic("first")]);
        let key = kobo_sdk::cache_key(cache_name("first"));
        previews.prepare(&mut context, &[comic("second")]);
        assert!(previews.loaded(
            &mut context,
            &key,
            &StoreResult::Loaded {
                key: key.clone(),
                value: Some(cover())
            }
        ));
        assert!(previews.pictures.is_empty());
        previews.staged = Some(("first".into(), cover()));
        previews.publish(&mut context, "first");
        assert!(matches!(previews.lead("first"), RowLead::Picture(_, _)));
        let _ = context.take_commands();
        previews.prepare(&mut context, &[comic("second")]);
        assert!(!previews.pictures.contains_key("first"));
        assert!(context
            .take_commands()
            .iter()
            .any(|command| matches!(command, Command::DropPicture(_))));
    }
    #[test]
    fn missing_and_invalid_positions_have_distinct_summaries_and_legacy_is_read_only() {
        let mut previews = Previews::default();
        let mut context = Context::default();
        let comic = comic("first");
        previews.prepare(&mut context, std::slice::from_ref(&comic));
        let _ = context.take_commands();
        let key = progress_key(&comic.key);
        previews.loaded(
            &mut context,
            &key,
            &StoreResult::Loaded {
                key: key.clone(),
                value: None,
            },
        );
        let legacy = legacy_progress_key(&comic.key).unwrap();
        assert!(context
            .take_commands()
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Load { key }) if key == &legacy)));
        previews.loaded(
            &mut context,
            &legacy,
            &StoreResult::Loaded {
                key: legacy.clone(),
                value: None,
            },
        );
        assert!(previews.summary(&comic).starts_with("Not started"));
        previews.saved_position(&key, b"{}", std::slice::from_ref(&comic));
        assert!(previews.summary(&comic).starts_with("Position unavailable"));
        previews.saved_position(&key, b"2", std::slice::from_ref(&comic));
        assert_eq!(previews.summary(&comic), "Saved page 3 of 4");
        assert!(context.take_commands().is_empty());
    }
}
