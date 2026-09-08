//! App-local publisher font handles mapped into the shared renderer.
use kobo_ui::{DisplayMetrics, FontHandle};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

pub const MAX_APP_FONTS: usize = 16;
static NEXT_FONT: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Default)]
pub struct FontOwner(BTreeMap<FontHandle, FontHandle>);

impl FontOwner {
    #[must_use]
    pub fn resolve(&self, local: FontHandle) -> Option<FontHandle> {
        self.0.get(&local).copied()
    }

    /// Parse before reserving a slot or replacing the current face.
    ///
    /// # Errors
    /// Refuses invalid fonts, exhausted handles, or more than 16 live faces.
    pub fn load(
        &mut self,
        local: FontHandle,
        name: &str,
        bytes: &[u8],
        metrics: DisplayMetrics,
    ) -> Result<(), String> {
        if !self.0.contains_key(&local) && self.0.len() >= MAX_APP_FONTS {
            return Err(format!("{MAX_APP_FONTS} fonts already held"));
        }
        let font = kobo_text::BookFont::from_bytes(bytes, name, metrics)
            .map_err(|error| error.to_string())?;
        let runtime = match self.resolve(local) {
            Some(handle) => handle,
            None => FontHandle(
                NEXT_FONT
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                        next.checked_add(1)
                    })
                    .map_err(|_| "font handles exhausted".to_owned())?,
            ),
        };
        kobo_ui::put_book_typesetter(runtime, Box::new(font));
        self.0.insert(local, runtime);
        Ok(())
    }

    pub fn remove(&mut self, local: FontHandle) {
        if let Some(runtime) = self.0.remove(&local) {
            kobo_ui::drop_book_typesetter(runtime);
        }
    }

    pub fn clear(&mut self) {
        for (_, runtime) in std::mem::take(&mut self.0) {
            kobo_ui::drop_book_typesetter(runtime);
        }
    }
}

impl Drop for FontOwner {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_handles_are_isolated_and_failed_replacements_keep_the_old_font() {
        let mut first = FontOwner::default();
        let mut second = FontOwner::default();
        let local = FontHandle(1);
        for owner in [&mut first, &mut second] {
            owner
                .load(
                    local,
                    "Fixture",
                    kobo_text::TEXT_FONT,
                    kobo_ui::CLARA_BW_METRICS,
                )
                .unwrap();
        }
        let first_handle = first.resolve(local).unwrap();
        let second_handle = second.resolve(local).unwrap();
        assert_ne!(first_handle, second_handle);
        assert!(first
            .load(local, "Broken", b"invalid", kobo_ui::CLARA_BW_METRICS)
            .is_err());
        assert_eq!(first.resolve(local), Some(first_handle));
        first.remove(local);
        assert_eq!(first.resolve(local), None);
        assert_eq!(second.resolve(local), Some(second_handle));
    }

    #[test]
    fn only_valid_fonts_consume_the_bounded_slots() {
        let mut owner = FontOwner::default();
        for number in 0..32 {
            assert!(owner
                .load(FontHandle(number), "Broken", b"", kobo_ui::CLARA_BW_METRICS)
                .is_err());
        }
        for number in 0..16 {
            owner
                .load(
                    FontHandle(number),
                    "Fixture",
                    kobo_text::TEXT_FONT,
                    kobo_ui::CLARA_BW_METRICS,
                )
                .unwrap();
        }
        assert!(owner
            .load(
                FontHandle(16),
                "Fixture",
                kobo_text::TEXT_FONT,
                kobo_ui::CLARA_BW_METRICS
            )
            .is_err());
        owner.remove(FontHandle(0));
        owner
            .load(
                FontHandle(16),
                "Fixture",
                kobo_text::TEXT_FONT,
                kobo_ui::CLARA_BW_METRICS,
            )
            .unwrap();
        owner.clear();
        assert!(owner.0.is_empty());
    }
}
